---
name: add-scraper-integration-test
description: Use when adding a scraper HTML integration fixture, especially when using the fetch-fixture binary and tests/fixtures/fixtures.json.
---

# Add Scraper Integration Test

Use the existing fixture-driven test. Keep one schema cache file per shop so
fixtures exercise every cached schema, not only the schema that produced the
expected output. Do not add a bespoke Rust test unless the behavior cannot be
expressed by the fixture harness.

## Workflow

1. Read `src/crawler/AGENTS.md` and the scraper fixture test files.
2. Fetch each page with the project binary:

```text
cargo run -p crawler --bin fetch-fixture -- "URL" STATE
```

The binary writes to:

```text
src/crawler/tests/fixtures/html/<shop>_<state>.html
```

3. Inspect the fetched HTML and choose stable, page-specific selectors.
   Filename must be exactly `<shop>_<state>.html`. Use a distinct state label
   when the fixture pages need distinct files, for example
   `georgianantiques_sale.html` and `georgianantiques_priceless.html`.
4. Add or update `src/crawler/tests/fixtures/schemas/<shop>.json`.
   Store the shop's cached schemas as a JSON array in stored-order.
5. Add one object per page to `src/crawler/tests/fixtures/fixtures.json`.
6. Include:
   - `html`
   - `schemas_file`
   - `schema_index` for the expected schema in that shop cache
   - `raw_state`
   - `state_record`
   - expected `raw` extraction
   - expected `normalized` product
7. The fixture extraction test applies every schema in `schemas_file` to the
   page, then asserts the expected `schema_index` result. Keep schemas that
   fail on a layout variant in the shop file when they represent real cache
   history.
8. Keep listed, sold, removed, and layout variants as separate fixture cases
   when their fields or state selectors differ.
9. Use `default_currency` when the page has a known shop currency.
10. Set image selectors to the product gallery only. Exclude logos, related
   products, icons, and thumbnails.

## Expected Data

- `raw` must match direct extraction from the expected schema exactly.
- `schema_index` must point to the schema that should win for that page.
- `normalized` must match the product normalization output.
- `price` uses minor units in normalized JSON, for example `79500` for GBP
  795.00.
- Set both price amount and currency, or set both to `null`.
- Use RFC3339 strings for auction dates.
- `state_record` and normalized `state` use supported uppercase values such as
  `LISTED`, `AVAILABLE`, or `SOLD`.

## Validate

Run the fixture integration test:

```text
cargo test -p crawler --test scraper_parsing_pipeline --all-features -- --nocapture
```

Then run formatting and the crawler checks:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- \
  -D warnings \
  -D clippy::result-large-err
cargo test -p crawler --lib --all-features
```

Do not commit fetched HTML or fixture JSON until the focused test passes.
