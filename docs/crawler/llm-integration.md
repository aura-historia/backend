# LLM Integration

The crawler has four LLM use cases, each with its own system prompt, input/output contract, and retry strategy. All are
built via `LLMBuilder` with `resilient=3` and `reasoning=true` unless noted.

---

## 1. URL Classification — `UrlClassificationServiceImpl`

**Purpose:** Given a sample of URLs crawled from a shop, produce a regex that matches product page URLs.

**When called:**

- During a spider run, once `classify_threshold` (default 200) URLs have been buffered.
- Also at end-of-stream if the threshold was never reached.
- Also after full crawl if the persisted pattern matched zero product URLs (reclassify).

**Input:** Up to `max_sample_urls` (default 500) raw URL strings, deduplicated and filtered.

**Output (JSON):**

```json
{
  "pattern": "<regex string>"
}
```

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=180s`

**Persistence:** Pattern is written to `shops.url_pattern` and used immediately for the remainder of the current crawl
run, and in all future runs, to classify `CrawledUrl` instances as `Product`.

---

## 2. Product Schema Generation — `ProductSchemaServiceImpl`

**Purpose:** Given cleaned HTML pages from product URLs, produce one or more `ProductCssSelectorSchema` variants that
together cover heterogeneous templates in the same shop.

**When called:**

- On first scrape of a product URL for a given shop (cache miss in `shops_product_schema`).
    - Schema seeding uses multiple pages (`scraper_schema_seed_pages`, default `3`): current page + up to `N-1`
      additional random same-shop product pages.
    - The seed set is sent in a **single** LLM call. The model may return multiple schemas, where each schema can target
      a subset of page layouts.
    - Extra seed-page fetches are best-effort and never block schema creation when only the current page is available.
    - This path runs only on schema cache miss, so first scrape can be slower while later scrapes reuse the cached
      schema.
    - The LLM is guided by detailed field descriptions in the `ProductCssSelectorSchema` struct that flow into the JSON
      schema sent to the model:
        - **`shops_product_id`**: Prefer stable product-specific identifiers from semantic nodes or canonical data
          attributes; avoid layout/widget ids and generic container labels.
        - **`title`**: Prefer the visible product heading (`h1` first, then strong semantic alternatives); avoid site
          headers, breadcrumbs, category labels, and repeated teaser titles.
        - **`description`**: Prefer the main product-description/body area and allow fragmented extraction; avoid
          shipping info, legal copy, navigation, and recommendation sections.
        - **`price`**: Prefer visible text from the actual product price element; avoid attributes, wrapper containers,
          struck-through comparison prices, and unrelated prices.
        - **`price_estimate_min` / `price_estimate_max`**: Use only for explicitly shown estimate bounds; avoid deriving
          them from a single sale price, bid price, or unrelated range/filter widgets.
        - **`state`**: Explicitly instructs the LLM to prioritize state sources in this order: clear explicit state text
          first, product-specific add-to-cart/buy button text second, and other product-specific availability buttons
          third. It should avoid generic class names or whole script blobs, and it must **never use price or layout
          elements** as state selectors. This prevents anti-patterns like extracting CSS class names instead of actual
          availability values (see issue #867).
        - **`images`**: Prefer canonical product media URLs from `src`, `srcset`, href-like media links, or
          gallery-specific attributes; avoid logos, icons, placeholders, and unrelated thumbnails.
        - **`auction_start` / `auction_end`**: Prefer machine-readable datetime-bearing nodes such as `time[datetime]`,
          structured data, or clearly labeled auction metadata; avoid generic date text unless it is clearly the auction
          timestamp.
        - **`default_currency`**: Treated as full-page fallback context, not a selector rule; set it only when currency
          can be inferred from surrounding page context.
- On runtime schema miss (append-on-miss flow, issue #801): if no cached schema variant applies during scrape, calls
  `append_single_schema()` to generate and append a single new schema for that page to the existing set, then retries.
  This enables heterogeneous shops to accumulate schema variants dynamically without triggering full regeneration.
- On runtime schema miss, regeneration uses an attempt loop (`max_schema_fix_attempts` config slot):
    - attempt 1 generates from the current page HTML only,
    - attempts 2..N include the previously failed generated schema and its extraction error as repair context,
    - appends in-memory to cached schemas and re-applies only candidates not already known to fail in the current retry
      loop,
    - persists only when at least one schema applies (deduplicated),
    - discards non-applicable generated schemas and retries.
    - on exhaustion, scraping returns `SchemaRegenerationExhausted`; cron records the error and sets a retry cooldown.
- Normalization can trigger schema regeneration only for schema-fixable errors (title empty/unknown language, price
  parse/currency issues, `StateTextTooLong`).
    - Non-fixable normalization errors (e.g. state mapping DB failures, invalid image URL, datetime parse issues) are
      propagated directly.
- Every shop-scoped LLM call increments `shops.llm_calls_count` for per-shop observability:
    - URL pattern classification (spider)
    - schema generation/retry (scraper)
    - schema self-evaluation (scraper review gate)
        - state-mapping LLM fallback (scraper normalization)
- Hard budget guardrail: **all crawler LLM call types share a single combined cap** `scraper_max_llm_calls_per_shop` (
  default `20`).
    - Candidate selection enforces a hard stop for that shop once the cap is reached (`shops.llm_calls_count < cap`).
    - If the cap is reached during an in-flight scrape, scraper returns `LlmBudgetExceeded` and cron writes cooldown
      metadata (`next_retry_at`) for observability.

**HTML pre-processing (before sending to LLM):**

- `<script>`, `<style>`, `<nav>`, `<footer>`, `<header>`, `<form>` elements stripped.
- Noisy attributes (`class`, `id`, `style`, `data-*`, `aria-*`) removed via `kuchiki`.
- This dramatically reduces token usage.

**Output (JSON):** An array of `ProductCssSelectorSchema` objects:

```json
[
  {
    "title": "h1.product-title",
    "price": "span.price",
    "state": "div.availability",
    "images": [
      "img.product-image"
    ],
    "date_listed": "time.listed",
    "date_sold": null,
    "default_currency": "EUR"
  }
]
```

`default_currency` is an optional ISO 4217 code the LLM sets when it can determine the shop's currency from full-page
context (e.g. a currency shown in the page header or footer). It is `null` when no currency context is visible. The
field is used as a fallback by `normalize()` when the extracted price string contains no currency marker.

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=180s`

**Append-and-retry flow (runtime apply miss):**

```
for attempt in 1..=max_schema_fix_attempts:
  increment shops.llm_calls_count
  candidate = append_single_schema(domain, html, failed_schema?, last_error?)
    // attempt 1: failed_schema/last_error are None (fresh generation)
    // attempt 2+: failed_schema/last_error come from previous failed generated schema
  re-apply only schemas not already known to fail in this loop:
    ok -> dedupe, persist candidate set, continue pipeline
    fail -> discard generated schema and retry
if exhausted -> return SchemaRegenerationExhausted
```

**Persistence:** Schema set stored in `shops_product_schema` (keyed by `shop_id`) as a JSON array. During scrape,
variants are tried in order until one applies.

---

## 3. Product Schema Evaluation - `ProductSchemaServiceImpl`

**Purpose:** Judge generated `ProductCssSelectorSchema` candidates before unattended persistence. The evaluator is
judge-only: it does not repair schemas or produce replacement selectors.

**When called:** After schema generation on initial schema creation, append repair, or normalization repair when
`CRAWLER_SCHEMA_LLM_REVIEW_MODE` is `report_only` or `auto_approve_high_confidence`.

**Evidence contract:** The prompt receives:

- the generated schemas,
- cleaned sampled product-page HTML,
- the deterministic schema-application matrix with extracted raw values and selector match counts,
- `deterministic_approval_ok`, which is true only when every sampled page has an applicable schema with required raw
  values (`shops_product_id`, `title`, `state`, and at least one image).

The evaluator must return `NEEDS_HUMAN_REVIEW` unless the evidence is clear. Auto-approval requires both deterministic
coverage and an LLM verdict of `APPROVE` with `HIGH` confidence.

**Output (JSON):**

```json
{
  "decision": "APPROVE",
  "confidence": "HIGH",
  "approved_by_llm": true,
  "summary": "Schemas cover the sampled product pages and extract product-specific required fields.",
  "risks": [],
  "page_findings": [
    {"role": "PRIMARY", "schema_index": 0, "finding": "Required fields extracted from product-specific nodes."}
  ]
}
```

Malformed JSON, LLM errors, low confidence, rejection, unavailable evaluator configuration, or exhausted budget all fall
back to a normal pending `PRODUCT_SCHEMA` human review. The evaluator payload is stored under
`crawler_reviews.validation_summary.auto_schema_evaluation` and shown in the Crawler Review Console.

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=180s`

---

## 4. Product State Mapping — `ProductStateMappingServiceImpl`

**Purpose:** Classify a raw state string extracted from a product page (e.g. `"En stock"`, `"Sold"`, `"Réservé"`) into a
normalized `UrlState`.

**When called:**

- During normalization, if the raw state string is not found in `product_state_mapping` by exact match or by any
  persisted regex.
- **Not called** when the raw state text exceeds `MAX_STATE_RAW_LEN` (512 bytes) — such inputs are rejected before any
  DB or LLM call (see State Lookup Hierarchy below).

**Output (plain text, one of two formats):**

```
STATE:<state>
```

or

```
REGEX:<pattern>:<state>
```

Where `<state>` is one of: `AVAILABLE`, `RESERVED`, `SOLD`, `LISTED`, `UNKNOWN`.

The `REGEX` variant is used when the LLM determines the raw value follows a pattern (e.g. multilingual variants of "
sold"). The regex is persisted so future raw values matching it are resolved without another LLM call.

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=60s`

**Return contract:** `get_state_mapping()` returns `(ProductStateMappingRecord, bool)` where the `bool` is `true` only
when the LLM fallback (step 3) was invoked. The caller (`normalize()` in `product_normalization_service.rs`) propagates
this as the `u32` component of its own `(NormalizedProduct, u32)` return value. `scraper_service.rs` charges
`consume_llm_budget_n_or_err(shop_id, url, n)` post-hoc with that count; `n = 0` is a no-op so DB-hit paths incur zero
budget overhead.

**Persistence:** Written to `product_state_mapping` with `mapping_type` of `VALUE` or `REGEX`.

---

## State Lookup Hierarchy (4-tier)

```
0. Length guard: len(trim+lowercase(raw)) > MAX_STATE_RAW_LEN (512 bytes)?
     └── warn + return RawStateTooLong → NormalizationError::StateTextTooLong
         → propagated as normalization failure (no schema regeneration)
1. Exact match on product_state_mapping.raw
2. Regex scan: iterate all REGEX rows, test pattern against raw value
3. LLM call → persist result → return
```

`MAX_STATE_RAW_LEN = 512` is defined in `state_mapping_service.rs`. It protects against the PostgreSQL B-tree index
key-size cap (~2704 bytes for `TEXT PRIMARY KEY`) and prevents the LLM from being called with garbage CSS-extracted
text (e.g. full product title + description when the selector targets the wrong element). Any raw value longer than 512
bytes is almost certainly not a real state string.

This means the LLM is only invoked once per novel raw state string (or pattern). Over time the mapping table grows and
LLM calls become rare.

---

## Summary Table

| Instance           | Trigger                                                                | Output format                   | Timeout | Cached in               | Budget-tracked                     |
|--------------------|------------------------------------------------------------------------|---------------------------------|---------|-------------------------|------------------------------------|
| URL Classification | Spider: at threshold / end of stream / zero-product reclassify         | JSON `{pattern}`                | 180s    | `shops.url_pattern`     | Yes                                |
| Product Schema     | Scraper: first scrape per shop / runtime apply miss (append-and-retry) | JSON CSS selectors              | 180s    | `shops_product_schema`  | Yes                                |
| Schema Evaluation  | Scraper: after generated schemas, before review/persistence            | JSON verdict/confidence         | 180s    | `crawler_reviews`       | Yes                                |
| State Mapping      | Scraper: novel raw state string (after length guard passes)            | Plain text `STATE:` or `REGEX:` | 60s     | `product_state_mapping` | Yes (via `normalize` return count) |
