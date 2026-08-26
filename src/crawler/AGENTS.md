# DOX

## Purpose

- Own `crawler` crate.
- Hold crawler design, operator flow, and durable crate contract.

## Core Design

- Crawler be async, Postgres-backed, LLM-assisted ingest system for antique shop sites.
- Root modules: `llm_runtime`, `local_db`, `logging`, `network`, `review`, `scraper`, `service`, `spider`, `vertex_ai`.
- Main neighbors: `application`, `large-language-model`, `localization`, `money`, `platform-postgres`, `product-listing-core`/`product-listing-service`/`product-listing-postgres`, `shop-core`/`shop-service`/`shop-postgres`.
- Main binaries: `server`, `demo`, `demo-spider`, `demo-scraper`, `fetch-fixture`.
- `service::cron` drive three parallel loops: shop sync, spider, scraper.
- Spider and scraper cron use global slot schedulers. Refill only schedulable work; scraper fetch picks random eligible domains, takes up to 100 due URLs per domain by default, and excludes domains already seen in the pass.
- Shop sync reads published Shop summaries through `shop-service` and `shop-postgres` from authoritative business Postgres, then stores crawler scope locally.
- Spider crawl shop domains, discover URLs, infer or refresh shop product regex, and batch-upsert URL metadata.
- Spider HTTP asks for `gzip, br, deflate` only; avoid zstd decode noise from bad origins.
- Scraper consume product URLs, fetch HTML with short inline retry backoff capped at 2s, detect stored soft-404 removed templates, reuse cached CSS selector schemas, normalize products, and push results onward. `Retry-After` headers must not sleep domain workers; failed URLs use `shop_urls.next_retry_at` after final fetch failure.
- Scraper applies all cached schemas to one parsed page, prepares and validates candidate-local data including images, ranks by usable completeness, then normalizes richest to least rich. Candidate-data failures reject only that schema; external/system failures abort and never trigger fresh generation. Fresh generation starts only after cached candidates exhaust, and cached schemas are never modified or generation inputs.
- Scraper listing handoff uses one bounded in-memory channel and one collector per scheduler pass. Producers await capacity. Partial batches flush at size, maximum age, or channel close. The collector never overlaps flushes. Each batch coalesces duplicate `ProductListingKey` values and executes unique canonical ProductListing upserts with bounded concurrency below the authoritative business Postgres pool size. URLs are marked scraped only for matching successful input positions. Structured logs expose enqueue wait, queue depth, oldest item age, upsert latency, persistence failures, and local mark failures.
- Scraper description text without own language signal inherits title language only when language was detected from the title itself.
- `review` own human-review rail and optional LLM-judge rail for URL patterns and schemas.
- Postgres be crawler source of truth. Main durable tables be `shops`, `shop_domains`, `shop_urls`, `shops_product_schema`, `shops_removed_page_schema`, `crawler_reviews`, `crawler_review_pages`, `listing_availability_mapping`.
- Main handoff be DB-backed: shop sync feeds spider; spider feeds scraper through `shop_urls`; scraper calls the canonical `product-listing-service` upsert use case against authoritative business Postgres. Crawler uses source Shop ID as ProductListing seller ID; raw marketplace seller names are not canonical seller identities.
- Locking be two-layer: process-local locks stop duplicate in one process, DB lock/cooldown metadata stop bad overlap and hot-loop retries across runs after final fetch failure.
- LLM use stay bounded and explicit: URL regex inference, product schema generation, HTML-only fresh page classification, schema evaluation, and listing-availability mapping fallback. Services stay generic over `large-language-model::LargeLanguageModel`; provider/model selection stays in executable wiring. `vertex_ai` wires Vertex AI Gemini with Google Application Default Credentials, while `llm_runtime` owns crawler retry, concurrency, and pacing.
- Product normalization completes deterministic preparation before listing-availability mapping. A deterministic candidate-data failure makes no mapping DB/LLM call and consumes zero mapping LLM budget. Mapping returns `Availability`, durable `NoAssertion`, or non-durable `Ignore`; only verified removal evidence changes crawler-local presence. `Ignore` never clears aggregate availability or withdraws a listing.
- Crawler LLM budgets be explicit: product schema generation/fresh generation and URL classification use 180 seconds; listing-availability mapping uses 60 seconds. Provider retry be bounded to 3 attempts with rate-limit, outage, transient, and timeout classes. Structured-response correction be bounded to 3 fresh attempts, so one logical call can make at most 9 provider calls. The crawler LLM governor reserves future request-start slots atomically. Reservation is serialized, but waiting for a reserved slot does not hold the start-gate mutex. Provider retry sleeps still release the request permit.
- Shop-level LLM spend be budgeted through `shops.llm_calls_count`.
- Review and schema cache be safety rail: generated artifacts can be audited, approved, or superseded.
- Schema generation and fresh single-page generation must use YAML-grounded selectors only. Prefer `null` over guessed optional-field selectors. State selector prompt must choose only availability/cart action nodes and exclude price text.
- Schema prompt DSL strips script/style and layout noise, including header/footer/nav custom elements.
- Product schemas may generate configured raw attribute selectors for review/demo/file inspection only. Missing raw attribute selector matches are skipped; extracted raw values are not DB or product-command data. New raw attribute keys need schema regeneration for existing cached shop schemas.
- Initial multi-page generation accepts product schema responses only. Fresh single-page generation accepts product, removed, and not-product classifications. Removed needs verified selector-bound text or regex evidence, stores shop-scoped `shops_removed_page_schema`, and marks URL `WITHDRAWN`. Not-product needs verified reason and only changes that URL class to `other`; never update shop URL pattern from one page.
- Fresh schema generation creates a brand-new schema from the current page; it never localizes, selector-patches, or mutates a cached schema. Freshly generated schemas are only persisted after they apply and normalize successfully.
- Cached schema scoring lives in `scraper::scraper_service::extraction::schema_candidates`. Each populated prepared logical field counts once; normalized-away values score zero. `default_currency` and URL-hash fallback IDs do not score. Stored order only breaks score ties.
- Local dev support live here too: `docker-compose.yml`, `scripts/linux/`, `scripts/windows/`, `migrations/`, and test fixtures under `tests/`.
- `fetch-fixture` writes fetched HTML to `tests/fixtures/html`.
- Demo product file snapshots are display-only, never command replay input; availability patch state uses tagged `SET`, `CLEAR`, or `UNCHANGED` output.
- `demo` and `server` auto-run crawler-local migrations on startup. Migrations be authoritative crawler DB contract.
- `server` needs `BUSINESS_DATABASE_URL` for Shop reads and ProductListing writes. LLM-enabled binaries need `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION`, and Google Application Default Credentials (for example `GOOGLE_APPLICATION_CREDENTIALS` locally). `VERTEX_AI_MODEL` selects schema generation/repair; `CRAWLER_VERTEX_AI_CHEAP_MODEL` and operation-specific overrides select low-risk models. `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` and `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` bound all crawler LLM calls. Crawler-local state and business writes use separate Postgres transactions; a product commit followed by a local mark failure remains retryable. Server product-push tuning is held in `CrawlerCronConfig`: `push_batch_size`, `push_queue_capacity`, `push_max_batch_age`, `push_max_concurrency`, and `business_db_max_connections`. These are code-level settings, not environment variables.

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
- Crawler truth live in Postgres. OpenSearch be a read-side neighbor, not crawler truth.
- Review rail be safety feature, not garnish. Keep audit fields and approval modes meaningful.
- URL classification should stay mostly deterministic after regex inference. Do not turn every page decision into fresh LLM call.
- Schema repair should grow cache carefully. Bad generated schema should die fast, not poison shop cache.
- Listing-availability mapping should prefer exact or regex reuse before LLM fallback.
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
