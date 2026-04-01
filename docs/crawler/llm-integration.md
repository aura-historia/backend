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
- On schema failure: if applying the schema throws an error (e.g. selector no longer valid), `fix_product_schema` is called — up to 3 retries — with the broken schema + error message to produce a corrected one.

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

**Fix flow (manual retry, not via resilient builder):**
```
for attempt in 0..3:
    result = llm.fix_product_schema(schema, error)
    if ok: persist and return
    else: continue
return Err
```

**Persistence:** Schema stored in `shops_product_schema` (keyed by `shop_id`). Shared across all product URLs of the same shop.

---

## 3. Product State Mapping — `ProductStateMappingServiceImpl`

**Purpose:** Classify a raw state string extracted from a product page (e.g. `"En stock"`, `"Sold"`, `"Réservé"`) into a normalized `UrlState`.

**When called:**
- During normalization, if the raw state string is not found in `product_state_mapping` by exact match or by any persisted regex.

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

## State Lookup Hierarchy (3-tier)

```
1. Exact match on product_state_mapping.raw
2. Regex scan: iterate all REGEX rows, test pattern against raw value
3. LLM call → persist result → return
```

This means the LLM is only invoked once per novel raw state string (or pattern). Over time the mapping table grows and LLM calls become rare.

---

## Summary Table

| Instance | Trigger | Output format | Timeout | Cached in |
|---|---|---|---|---|
| URL Classification | Spider: at threshold / end of stream / zero-product reclassify | JSON `{pattern}` | 180s | `shops.url_pattern` |
| Product Schema | Scraper: first scrape per shop / schema fix | JSON CSS selectors | 180s | `shops_product_schema` |
| State Mapping | Scraper: novel raw state string | Plain text `STATE:` or `REGEX:` | 60s | `product_state_mapping` |
