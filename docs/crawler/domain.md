# Domain Concepts

---

## URL Classification

Every URL discovered during a spider run is classified before being stored. Classification is purely heuristic — no LLM involved.

### Blacklist (filtered out entirely)
URLs containing any of these substrings are dropped before classification:
`cart`, `wishlist`, `?replytocom=`, `/wp-admin/`, `.jpg`, `.pdf`, `.png`

### Classification rules (evaluated in order)

| Class | Condition |
|---|---|
| `Product` | Matches the shop's `url_pattern` regex (if known) |
| `Imprint` | Path contains: `impressum`, `mentions-legales`, `mentions_legales`, `legal-notice` |
| `Category` | Path contains: `category`, `categories`, `collections`, `collection`, `/shop/`, `/store/`, `produits`, `produkte` |
| `Info` | Path contains: `about`, `contact`, `faq`, `datenschutz`, `privacy`, `cgv`, `agb`, `shipping`, `livraison` |
| `Other` | Everything else |

`Product` classification via regex is only possible after `UrlClassificationServiceImpl` has run (at `classify_threshold` URLs). Before that, new URLs default to `Other` unless heuristics match them as `Imprint`/`Category`/`Info`.

### URL Normalisation
Before classification and dedup, each URL is normalized:
- Hash fragment (`#...`) stripped.
- Trailing slash removed (except for the root path `/`).
- Deduplication via Bloom filter (capacity 100k, FP rate 0.001) — URLs already seen in the current crawl run are dropped.

---

## UrlState Lifecycle

`UrlState` tracks the known availability of a product URL over time.

```
UNKNOWN
  └─► LISTED       (URL discovered, not yet scraped with content)
        └─► AVAILABLE   (product in stock)
        └─► RESERVED    (product reserved / held)
              └─► SOLD       (purchase confirmed)
              └─► REMOVED    (page gone / 404)
        └─► SOLD
        └─► REMOVED
```

- Initial state when a URL is first inserted by the spider: `UNKNOWN`.
- After a successful scrape, the crawler writes the normalized availability back into `shop_urls.state`. This can become `AVAILABLE`, `RESERVED`, `SOLD`, `LISTED`, `REMOVED`, or remain `UNKNOWN`.
- The same normalized product state is also propagated separately into the product backend via the product upsert command path.
- `REMOVED`: set when the HTTP fetch returns a non-200 / page no longer exists.
- Scraper re-visits URLs in states `UNKNOWN`, `LISTED`, `AVAILABLE`, `RESERVED` and excludes terminal states `SOLD` / `REMOVED`.

---

## Product Normalization Pipeline

After raw HTML extraction via CSS selectors, `ProductNormalizationService` transforms each field:

### State
Four-tier lookup (see [LLM Integration — State Lookup Hierarchy](./llm-integration.md#state-lookup-hierarchy-4-tier)):
0. **Length guard**: if `len(trim+lowercase(raw)) > 512` bytes, return `NormalizationError::StateTextTooLong` — which the scraper routes into the schema-fix path (the `state` CSS selector is extracting the wrong element).
1. Exact match in `product_state_mapping`
2. Regex scan over persisted patterns
3. LLM call → persist → return

`StateTextTooLong` (and other normalization errors that indicate a wrong selector — bad price, empty title, etc.) feed back into the schema-fix flow in `ScraperServiceImpl` rather than being terminal failures. This means normalization errors can trigger an LLM schema correction, just like an `apply()` failure does.

`NormalizationError::ShopsProductIdEmpty` and `PriceUnknownCurrency` are **not** routed into the schema-fix loop — see the Price and Shops Product ID subsections above.

### Shops Product ID

- Extracted from the page by the CSS selector schema field `shops_product_id`.
- If the extracted value is blank after trimming, the full product page URL is used as a stable fallback identifier (infallible — normalization never fails on this field).
- The fallback means `NormalizationError::ShopsProductIdEmpty` is never produced by the main pipeline and never triggers the schema-fix loop.

### Title
- Language detected using `lingua` (language detection library).
- Stored as `Localized<Title>` — a title tagged with its ISO 639-1 language code.
- Allows multilingual shops to have per-language titles without overwriting each other.

### Price
- Multi-locale currency parsing: handles formats like `1.200,50 €`, `$1,200.50`, `1 200 CHF`.
- **Fallback currency from TLD**: if the extracted price string contains no currency symbol or ISO code (e.g. bare `"18,00"` or `"1590"` on a German site), the currency is inferred from the shop URL's TLD — `.de`/`.at`/`.fr`/`.es`/`.it`/`.nl`/`.be`/`.pt`/`.fi`/`.ie`/`.lu` → EUR; `.co.uk`/`.uk` → GBP; `.us` → USD; `.au`/`.com.au` → AUD; `.ca` → CAD; `.nz`/`.co.nz` → NZD. Generic TLDs (`.com`, `.net`, `.org`, …) have no fallback.
- `PriceUnknownCurrency` (bare price with unrecognised TLD) is **not** routed into the LLM schema-fix loop — the selector is correct; changing it cannot help.
- Extracts both `price_value` (numeric) and `price_currency` (ISO 4217 code).
- Stored separately on `shop_urls`.

### Images
- Relative URLs resolved to absolute using the product page's base URL.
- Multiple images collected per product (schema supports a list of selectors).

### Dates
- `date_listed` and `date_sold` parsed from ISO 8601 / RFC 3339 strings if present in the HTML.
- Stored as UTC timestamps.

---

## Change Detection

The scraper stores `last_scraped_hash` after a successful scrape.

On the next scrape run, it fetches HTML and computes an in-memory hash:
- SHA-256 of the `<main>...</main>` fragment when present,
- SHA-256 of the full HTML document when no `<main>` tag exists.

If the computed hash equals `last_scraped_hash` **and** a `<main>` tag was found, the scraper skips extraction/normalization and only refreshes scrape bookkeeping. Pages without a `<main>` tag are always re-extracted. The hash is always stored after a successful scrape.
