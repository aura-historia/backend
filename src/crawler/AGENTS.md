# DOX

## Purpose

- Own `crawler` crate.
- Hold crawler design, operator flow, and durable crate contract.

## Core Design

- Crawler be async, Postgres-backed, LLM-assisted ingest system for antique shop sites.
- Root modules: `google_llm`, `local_db`, `logging`, `network`, `review`, `scraper`, `service`, `spider`.
- Main neighbors: `common`, `product`, `shop`.
- Main binaries: `server`, `demo`, `demo-spider`, `demo-scraper`, `fetch-fixture`.
- `service::cron` drive three parallel loops: shop sync, spider, scraper.
- Spider and scraper cron use global slot schedulers. Refill only schedulable work; scraper fetch picks random eligible domains, takes up to 100 due URLs per domain by default, and excludes domains already seen in the pass.
- Shop sync load active shops and domains from upstream shop search into local Postgres.
- Spider crawl shop domains, discover URLs, infer or refresh shop product regex, and batch-upsert URL metadata.
- Spider HTTP asks for `gzip, br, deflate` only; avoid zstd decode noise from bad origins.
- Scraper consume product URLs, fetch HTML with short inline retry backoff capped at 2s, detect stored soft-404 removed templates, reuse or grow CSS selector schemas, normalize products, and push results onward. `Retry-After` headers must not sleep domain workers; failed URLs use `shop_urls.next_retry_at` after final fetch failure.
- Scraper description text without own language signal inherits title language only when language was detected from the title itself.
- `review` own human-review rail and optional LLM-judge rail for URL patterns and schemas.
- Postgres be crawler source of truth. Main durable tables be `shops`, `shop_domains`, `shop_urls`, `shops_product_schema`, `shops_removed_page_schema`, `crawler_reviews`, `crawler_review_pages`, `product_state_mapping`.
- Main handoff be DB-backed: shop sync feeds spider; spider feeds scraper through `shop_urls`; scraper feeds backend product push.
- Locking be two-layer: process-local locks stop duplicate in one process, DB lock/cooldown metadata stop bad overlap and hot-loop retries across runs after final fetch failure.
- LLM use stay bounded and explicit: URL regex inference, product schema generation, HTML-only append-repair page classification, schema evaluation, state mapping fallback.
- Shop-level LLM spend be budgeted through `shops.llm_calls_count`.
- Review and schema cache be safety rail: generated artifacts can be audited, approved, repaired, or superseded.
- Schema generation and append repair must use YAML-grounded selectors only. Prefer `null` over guessed optional-field selectors. State selector prompt must choose only availability/cart action nodes and exclude price text.
- Schema prompt DSL strips script/style and layout noise, including header/footer/nav custom elements.
- Product schemas may generate configured raw attribute selectors for review/demo/file inspection only. Missing raw attribute selector matches are skipped; extracted raw values are not DB or product-command data. New raw attribute keys need schema regeneration for existing cached shop schemas.
- Initial schema generation accepts product schema responses only. Append repair accepts product, removed, and not-product classifications.
- Append repair classifies failed pages as product, removed, or not-product. Removed needs verified selector-bound text or regex evidence, stores shop-scoped `shops_removed_page_schema`, and marks URL `REMOVED`. Not-product needs verified reason and only changes that URL class to `other`; never update shop URL pattern from one page.
- Local dev support live here too: `docker-compose.yml`, `scripts/linux/`, `scripts/windows/`, `migrations/`, and test fixtures under `tests/`.
- `fetch-fixture` writes fetched HTML to `tests/fixtures/html`.
- `demo` and `server` auto-run migrations on startup. Migrations be authoritative DB contract.

## Ownership

- This doc rule `src/crawler/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.
- No crawler doc rail in `docs/`. Crawler truth live here.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, binaries, loop flow, DB contract, review flow, env vars, scripts, or tests change.
- Update migrations in same change as durable Postgres contract change. Never patch live schema by hand and forget migration.
- If new table, index, retry field, review field, or query contract appear, document it here and cover it with tests.
- If loop cadence, candidate rules, lock semantics, retry semantics, or LLM budget semantics change, document it here in same change.
- Local DB scripts live in `scripts/linux/` and `scripts/windows/`. Keep both sides honest when workflow change.
- `server` and `demo` auto-run migrations. Keep that startup contract stable unless strong reason.
- Review mode env behavior be durable contract. Changes there need doc and test thought.
- Keep no semantic duplicate drift across code comments, tests, and this file. One crawler truth.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep trait seams clean at external edges: shop source, fetcher, LLM, review, product push.
- Keep cron orchestration thin. Put real rule in spider, scraper, review, or service modules.
- Keep retry, cooldown, lock, and budget semantics explicit. Hidden side effect bad.
- Prefer append-or-upsert flows over destructive rewrite when preserving crawler history matters.
- Crawler truth live in Postgres. OpenSearch and DynamoDB be neighbors, not crawler truth.
- Review rail be safety feature, not garnish. Keep audit fields and approval modes meaningful.
- URL classification should stay mostly deterministic after regex inference. Do not turn every page decision into fresh LLM call.
- Schema repair should grow cache carefully. Bad generated schema should die fast, not poison shop cache.
- State mapping should prefer exact or regex reuse before LLM fallback.
- Price normalization de-dupes repeated visible/accessibility price text only when candidates agree or one clean decimal form beats malformed visual cents.
- Keep spider per-site concurrency bounded. `spider::Website` default concurrency can explode HTTP/2 stream churn; crawler pins a conservative per-site limit and scraper-owned reqwest clients stay HTTP/1-only.
- Filter non-actionable `html5ever::tree_builder` warnings at crawler entrypoints.
- Avoid code duplication between `demo` and `server` when shared builder or service can hold it.
- Testcontainers tests be preferred proof for DB behavior. Keep fixtures focused and stable.

## Verification

- `cargo check -p crawler`
- `cargo test -p crawler --all-features`
- `cargo test -p crawler --tests`
- For DB/dev-flow change: check `docker-compose.yml`, `migrations/`, both script folders, and affected tests together.
- For LLM/review change: check candidate selection, budget accounting, and review persistence tests.

## Child DOX Index

- None.
