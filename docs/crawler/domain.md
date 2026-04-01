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
- After a successful scrape that resolves availability: transitions to `AVAILABLE`, `RESERVED`, `SOLD`, or `LISTED` (if state extraction fails).
- `REMOVED`: set when the HTTP fetch returns a non-200 / page no longer exists.
- Scraper re-visits URLs in states `UNKNOWN`, `LISTED`, `AVAILABLE`, `RESERVED` (i.e. not terminal states `SOLD` / `REMOVED`).

---

## Product Normalization Pipeline

After raw HTML extraction via CSS selectors, `ProductNormalizationService` transforms each field:

### State
Three-tier lookup (see [LLM Integration — State Lookup Hierarchy](./llm-integration.md#state-lookup-hierarchy-3-tier)):
1. Exact match in `product_state_mapping`
2. Regex scan over persisted patterns
3. LLM call → persist → return

### Title
- Language detected using `lingua` (language detection library).
- Stored as `Localized<Title>` — a title tagged with its ISO 639-1 language code.
- Allows multilingual shops to have per-language titles without overwriting each other.

### Price
- Multi-locale currency parsing: handles formats like `1.200,50 €`, `$1,200.50`, `1 200 CHF`.
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

The spider sets `main_hash` (SHA-256 of the full page HTML) on each `shop_urls` row during crawl.

The scraper sets `last_scraped_hash` after a successful scrape.

Before fetching HTML, the scraper compares `current_hash == last_scraped_hash`. If equal, the page hasn't changed since the last scrape — the scraper marks the URL as visited and skips the fetch entirely. This is the primary mechanism for avoiding redundant work on re-crawled shops.
