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

**Purpose:** Given a cleaned HTML page from a product URL, produce a `ProductCssSelectorSchema` — a set of CSS selectors for extracting title, price, state, images, and dates.

**When called:**
- On first scrape of a product URL for a given shop (cache miss in `shops_product_schema`).
- On schema failure: if applying the schema throws an error (e.g. selector no longer valid), `fix_product_schema` is called with the broken schema + error message. The dispatcher (`cron.rs`) guarantees at most one in-flight scrape per domain at a time, so no per-domain mutex is required.
- On normalization failure: if `ProductNormalizationService::normalize` returns a schema-fixable error (e.g. `StateTextTooLong`, `PriceParseError`, `TitleEmpty`), the same `fix_product_schema` path is triggered via `normalize_with_retry` using a synthetic `ApplySchemaError` hint derived from the normalization error.
  - Note: `PriceUnknownCurrency` is **not** treated as schema-fixable. When a raw price string contains no currency marker the scraper first attempts to resolve the currency from the shop URL's TLD (e.g. `.de` → EUR). If that also fails the price cannot be parsed, but changing the CSS selector cannot fix this — the LLM fix loop would never terminate. The price is simply left unparseable for that product.
- Fix attempts are tracked per domain across batches via `schema_fix_attempts: HashMap<String, u32>`. The counter counts *consecutive* failed LLM-fix attempts (where the LLM returned a schema that still failed to apply). Once a domain reaches `max_schema_fix_attempts` consecutive failed attempts the domain is skipped with `SchemaFixAttemptsExhausted` — no further LLM calls are made. The counter resets after **every** successful scrape for that domain (with or without a fix), so it represents failures since the last clean scrape, not total lifetime failures. This prevents premature budget exhaustion on domains whose pages have heterogeneous layouts.

**HTML pre-processing (before sending to LLM):**
- `<script>`, `<style>`, `<nav>`, `<footer>`, `<header>`, `<form>` elements stripped.
- Noisy attributes (`class`, `id`, `style`, `data-*`, `aria-*`) removed via `kuchiki`.
- This dramatically reduces token usage.

**Output (JSON):** A `ProductCssSelectorSchema` struct, serialized:
```json
{
  "title": "h1.product-title",
  "price": "span.price",
  "state": "div.availability",
  "images": ["img.product-image"],
  "date_listed": "time.listed",
  "date_sold": null
}
```

**LLM config:** `resilient=3`, `reasoning=true`, `timeout=180s`

**Fix flow (straight to LLM — no re-fetch, no per-domain mutex):**
```
is_fix_budget_exhausted(domain)?  → bail with SchemaFixAttemptsExhausted
increment_fix_attempts(domain)
llm.fix_product_schema(failed_schema, apply_error, html)
re-apply fixed schema:
  ok → persist (save_product_schema) and return (raw, schema_was_fixed=true)
  still fails → return SchemaFixApplyFailed (not persisted)
```

**Persistence:** Schema stored in `shops_product_schema` (keyed by `shop_id`). Shared across all product URLs of the same shop.

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
         → triggers schema-fix flow B in ScraperServiceImpl (state selector is wrong)
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
| Product Schema | Scraper: first scrape per shop / schema-apply failure / normalization-triggered fix | JSON CSS selectors | 180s | `shops_product_schema` |
| State Mapping | Scraper: novel raw state string (after length guard passes) | Plain text `STATE:` or `REGEX:` | 60s | `product_state_mapping` |
