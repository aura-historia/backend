# LLM Integration

The crawler uses three distinct LLM instances, each with its own system prompt, input/output contract, and retry strategy. All are built via `LLMBuilder` with `resilient=3` and `reasoning=true` unless noted.

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
{ "pattern": "<regex string>" }
```

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=180s`

**Persistence:** Pattern is written to `shops.url_pattern` and used immediately for the remainder of the current crawl run, and in all future runs, to classify `CrawledUrl` instances as `Product`.

---

## 2. Product Schema Generation — `ProductSchemaServiceImpl`

**Purpose:** Given cleaned HTML pages from product URLs, produce one or more `ProductCssSelectorSchema` variants that together cover heterogeneous templates in the same shop.

**When called:**
- On first scrape of a product URL for a given shop (cache miss in `shops_product_schema`).
  - Schema seeding uses multiple pages (`scraper_schema_seed_pages`, default `3`): current page + up to `N-1` additional random same-shop product pages.
  - The seed set is sent in a **single** LLM call. The model may return multiple schemas, where each schema can target a subset of page layouts.
  - Extra seed-page fetches are best-effort and never block schema creation when only the current page is available.
  - This path runs only on schema cache miss, so first scrape can be slower while later scrapes reuse the cached schema.
  - The LLM is guided by detailed field descriptions in the `ProductCssSelectorSchema` struct that flow into the JSON schema sent to the model:
    - **`state`**: Explicitly instructs the LLM to look for semantic availability indicators (schema.org markup, class/text encoding availability, button presence) and **never use price or layout elements** as state selectors. This prevents anti-patterns like extracting CSS class names instead of actual availability values (see issue #867).
    - **`price`**: Guides toward text-content extraction from price-labeled elements, not attributes.
    - **`title`**: Directs to h1/h2 and meta tag elements.
    - **`description`**, **`images`**, **`shops_product_id`**: Similar guidance with HTML/attribute examples and semantic cues.
- On runtime schema miss (append-on-miss flow, issue #801): if no cached schema variant applies during scrape, calls `append_single_schema()` to generate and append a single new schema for that page to the existing set, then retries. This enables heterogeneous shops to accumulate schema variants dynamically without triggering full regeneration.
- On runtime schema miss, regeneration uses an attempt loop (`max_schema_fix_attempts` config slot):
  - each attempt generates one schema from the current page,
  - appends in-memory to cached schemas and re-applies only newly appended candidates for that attempt,
  - persists only when at least one schema applies (deduplicated),
  - discards non-applicable generated schemas and retries.
  - on exhaustion, scraping returns `SchemaRegenerationExhausted`; cron records the error and sets a retry cooldown.
- Normalization does **not** trigger schema regeneration anymore. Normalization errors are propagated directly.
- Every shop-scoped LLM call increments `shops.llm_calls_count` for per-shop observability:
  - URL pattern classification (spider)
  - schema generation/retry (scraper)
- Hard budget guardrail: schema-generation calls are capped by `scraper_max_llm_calls_per_shop` (default `20`).
  - Candidate selection enforces a hard stop for that shop once the cap is reached (`shops.llm_calls_count < cap`).
  - If the cap is reached during an in-flight scrape, scraper returns `LlmBudgetExceeded` and cron writes cooldown metadata (`next_retry_at`) for observability.

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
    "images": ["img.product-image"],
    "date_listed": "time.listed",
    "date_sold": null,
    "default_currency": "EUR"
  }
]
```

`default_currency` is an optional ISO 4217 code the LLM sets when it can determine the shop's currency from full-page context (e.g. a currency shown in the page header or footer). It is `null` when no currency context is visible. The field is used as a fallback by `normalize()` when the extracted price string contains no currency marker.

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=180s`

**Append-and-retry flow (runtime apply miss):**
```
for attempt in 1..=max_schema_fix_attempts:
  increment shops.llm_calls_count
  candidate = append_single_schema(shop_id, html)   // in-memory append
  re-apply only newly appended schema candidates:
    ok -> dedupe, persist candidate set, continue pipeline
    fail -> discard generated schema and retry
if exhausted -> return SchemaRegenerationExhausted
```

**Persistence:** Schema set stored in `shops_product_schema` (keyed by `shop_id`) as a JSON array. During scrape, variants are tried in order until one applies.

---

## 3. Product State Mapping — `ProductStateMappingServiceImpl`

**Purpose:** Classify a raw state string extracted from a product page (e.g. `"En stock"`, `"Sold"`, `"Réservé"`) into a normalized `UrlState`.

**When called:**
- During normalization, if the raw state string is not found in `product_state_mapping` by exact match or by any persisted regex.
- **Not called** when the raw state text exceeds `MAX_STATE_RAW_LEN` (512 bytes) — such inputs are rejected before any DB or LLM call (see State Lookup Hierarchy below).

**Output (plain text, one of two formats):**
```
STATE:<state>
```
or
```
REGEX:<pattern>:<state>
```

Where `<state>` is one of: `AVAILABLE`, `RESERVED`, `SOLD`, `LISTED`, `UNKNOWN`.

The `REGEX` variant is used when the LLM determines the raw value follows a pattern (e.g. multilingual variants of "sold"). The regex is persisted so future raw values matching it are resolved without another LLM call.

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=60s`

**Persistence:** Written to `product_state_mapping` with `mapping_type` of `EXACT` or `REGEX`.

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

`MAX_STATE_RAW_LEN = 512` is defined in `state_mapping_service.rs`. It protects against the PostgreSQL B-tree index key-size cap (~2704 bytes for `TEXT PRIMARY KEY`) and prevents the LLM from being called with garbage CSS-extracted text (e.g. full product title + description when the selector targets the wrong element). Any raw value longer than 512 bytes is almost certainly not a real state string.

This means the LLM is only invoked once per novel raw state string (or pattern). Over time the mapping table grows and LLM calls become rare.

---

## Summary Table

| Instance | Trigger | Output format | Timeout | Cached in |
|---|---|---|---|---|
| URL Classification | Spider: at threshold / end of stream / zero-product reclassify | JSON `{pattern}` | 180s | `shops.url_pattern` |
| Product Schema | Scraper: first scrape per shop / runtime apply miss (append-and-retry) | JSON CSS selectors | 180s | `shops_product_schema` |
| State Mapping | Scraper: novel raw state string (after length guard passes) | Plain text `STATE:` or `REGEX:` | 60s | `product_state_mapping` |
