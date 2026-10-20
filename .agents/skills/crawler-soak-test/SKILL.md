---
name: crawler-soak-test
description: "Run the local crawler dev binary against a batch of real small/medium antique shops for a timed round, collect evidence from the crawler_demo database, the JSON log and the review console, and write ranked reliability findings with code references and ticket coverage. Invoke only on request; it spends LLM budget and fetches live shops."
disable-model-invocation: true
---

# Crawler soak test

A soak round runs `cargo run -p crawler --bin dev` against 6–10 real shops for 60–90 minutes and
turns what happens into a findings note built from
[`references/findings-template.md`](references/findings-template.md). The output is evidence
and ranked findings, not fixes. Do not change runtime code during a round; to verify a code
change against known state use the `crawler-dev-verify` skill instead.

Shop scope: **small/medium non-JS single-dealer antique shops, no auction houses, no marketplaces.**
Large catalogues count only where they expose a general bug. Prefer shops not used in earlier rounds
(earlier findings notes under `docs/crawler/` list them) unless re-testing a known finding.

The skill ships no helper scripts on purpose: write the preflight checks, status queries and
log filters for the platform you are on (curl, `psql` through `docker exec`, Python for the JSON
log and captures) in the scratchpad, never in the repo. The evidence table at the end says what
each check must answer.

## Preconditions

- Docker Desktop running. The dev binary runs `docker compose up -d` itself; `crawler-postgres-1`
  serves `crawler_demo` on `localhost:5432` (user/password `postgres`).
- `.env` provides `VERTEX_AI_PROJECT_ID` and `VERTEX_AI_LOCATION`; Application Default
  Credentials are valid (`gcloud auth application-default login`). Never print `.env` values.
- No `dev.exe` running. While it runs, cargo cannot rebuild crawler bins; for tests in parallel use
  `--target-dir target/scratch` from PowerShell.
- Check `CRAWLER_SCHEMA_LLM_REVIEW_MODE` and `CRAWLER_REVIEW_REQUIRED` in `.env` before the
  round: an invalid review mode silently falls back to auto-approve (#2126). Note the effective
  mode in the findings; do not fix it mid-round.

## Procedure

1. **Pick and preflight shops.** Collect 6–10 candidate domains. For each, fetch the root with
   curl and the crawler user agent and record: root status and redirect target, apex/www split,
   robots `Disallow` and `Crawl-delay`, same-host link count, a JS-only heuristic (few links and
   no product markup in the HTML), and whether `sitemap.xml` exists. Drop JS-rendered shops,
   auction houses and marketplaces; keep one deliberately awkward host (slow, HTTP-only, apex/www
   split) per round if it exposes a general bug class. Record the preflight line for every shop,
   including the dropped ones.
2. **Register the round.** Replace the `(index, name, slug)` tuple array in
   `demo_listing_sources()` and the domain list zipped in `main()` in
   [`src/crawler/src/bin/dev.rs`](../../../src/crawler/src/bin/dev.rs) with the round's shops.
   Use a start index above every index used so far in this database: the ID suffix is
   `...639f{index:02x}`, and the committed code only formats one hex digit (`7{index}`), so change
   the format string to `{index:02x}` too. These edits are temporary test state: never commit them;
   revert with `git checkout -- src/crawler/src/bin/dev.rs` at the end. Make sure the checkout is
   on an up-to-date `develop` first (`git fetch` and compare with `origin/develop`), so the
   findings describe current code.
3. **Start the round.** Build with `cargo build --locked -p crawler --bin dev` and run the binary
   from the repo root with `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS=3` (use `1` to reproduce the
   default head-of-line stall) and `CRAWLER_LOG_LEVEL=info` (`debug` for per-URL outcomes; about
   14 KB per URL); stdout to `target/soak/<date>/round-<n>.jsonl`, stderr to a separate file. Run
   it in a terminal tab or in the background and note the start time in UTC.
   Do not wait for cooldowns or recrawl timers when the round needs a domain to run now: set
   `next_crawl_at = now()` (spider) or `next_scrape_at = now()` (scraper circuit) for that domain
   in `listing_source_domains`, and list every such reset in the note's local state.
4. **Observe.** Every 10–15 minutes take a database snapshot (per-domain spider state and
   timers, URL counts by class and disposition, scraped and failed URLs with error kinds, pending
   reviews, LLM budget per source) and a log summary (records by level and message, pass totals
   and durations, LLM retries and timeouts, spider outcomes per domain, heartbeat share). Keep a
   timeline of anomalies with timestamps: stalled passes, circuits, 429/403, zero-product crawls,
   regeneration loops. Live-fetch pages with curl when a selector or pattern looks wrong.
   **Review every pending schema before approving it**, as a reviewer would in production:
   `GET /api/reviews/{id}` for the candidate and its validation summary,
   `GET /api/reviews/{id}/matrix?refresh=true` to re-run the deterministic gate on the live seed
   pages, then check each field against a live page: `state` must resolve to an availability (not a
   category or page title), `price` must parse, `source_listing_id` must be stable across the
   source's schemas, `images` must be product images, and selectors should be label-anchored rather
   than positional. Fix a bad field with `POST /api/reviews/{id}/schema-field`
   (`{"schema_index":n,"field":"price","rule":{...}}`) or replace the whole candidate with
   `POST /api/reviews/{id}/candidate` (`{"schemas":[...]}`), re-run the matrix, and only then
   `POST /api/reviews/{id}/approve` with `{"notes":"soak: reviewed, <what you changed>"}`. Reject
   or mark `needs-repair` when no selector fix makes the page extract correctly, and record every
   decision with its time.
   Each wrong candidate is itself a finding: note the field, the wrong selector and the live value.
   Rules that bite when perfecting a schema:
   - **`additional_selectors` aggregate, they are not fallbacks** (`rule.rs` `apply`). With
     `cardinality: all` every extra selector appends its matches, so a "fallback" on
     `description` or `images` duplicates content. Use one selector that fits every variant
     (`container > *`, or a comma group whose parts never co-occur) and check the aggregated
     value in the capture, not just that the field applies.
   - A present optional rule (description, price, estimates, raw attributes) that matches
     nothing leaves the field empty since #2116; pages without a description or price no longer
     need a separate schema. Split variants only when the state or identity rule differs. Read
     `optional_field_misses` in the matrix summary: a miss on a page that does carry the text is
     a selector defect (often a child combinator where the markup nests deeper), not a pass.
   - Images may legitimately be absent: anchor the image selector on a gallery container
     (`.stage img.main`), because a container that exists without image evidence yields an empty
     list instead of a failure. Never fall back to `og:image`; it is often a site default.
   - Identity must exist on every variant (available, sold, POA). A missing identity silently
     becomes a hash. Prefer IDs every template carries (form container post id, canonical link).
   - Prefer the schema.org `availability` link over button text for `state`; normalization
     resolves its last path segment.
   Test the candidate on a sold page, a price-on-application page, a page without a description
   and a page without photos before approving, not only on the stored seeds; save those pages
   with curl and inspect the markup. A fresh-generation review that appears minutes after an
   approval means one of your rules failed on a real page: fix the reviewed schema in that
   review's candidate and drop the LLM's new schema unless it is better. After approving, read
   the next captures in `scraped_raw_observations.json` (identity, description items, distinct
   images) to confirm the schema does what the matrix suggested.
   The console's field editor only edits core fields of a schema, and editing an approved review
   rewrites the live list from that review's candidate. Fix raw attributes or add schemas while
   the review is still pending (`POST …/candidate`); a change after approval needs the live
   `listing_source_product_schemas` row, and the note must say so.
5. **Stop** after 60–90 minutes, or earlier if every domain is terminal, with Ctrl+C in the tab.
   Take a final database snapshot and log summary. The scraper pass is a barrier, so a pass that
   is still running at stop time is itself a finding.
6. **Audit the code** for each anomaly before writing it up: find the exact file and line that
   produces the behaviour, and state the fix direction in one or two lines. Check existing issues
   with `gh issue list --search "<keyword>"` so every finding carries ticket coverage
   (covered / partly / not ticketed).
7. **Write the findings note** at `docs/crawler/.crawler-soak-findings-<YYYY-MM-DD>.md` using
   [`references/findings-template.md`](references/findings-template.md). Rank findings by impact on
   small/medium shops. Report only what is new or regressed against earlier notes; cite the earlier
   note for known items instead of restating them. Leading-dot filenames are deliberately temporary:
   the note is deleted once its findings are issues.
8. **Clean up.** Revert `dev.rs`, stop the dev process, and list the local state left behind in the
   note (demo sources still in `crawler_demo`, reviews you approved, the size of
   `scraped_raw_observations.json`). Reset the database with the `db-reset` script under
   `src/crawler/scripts/linux/` or `src/crawler/scripts/windows/` only when the next round needs
   a clean slate.

## Evidence sources

| Source | Where | What it answers |
|---|---|---|
| Crawler DB | `docker exec crawler-postgres-1 psql -U postgres -d crawler_demo`: `listing_source_domains` (pattern state, crawl outcome and timers, scrape failure streak), `listing_source_urls` (class, disposition, `last_scraped`, failure fields, identity), `crawler_reviews` and `crawler_review_pages`, `listing_sources.llm_calls_count`, `listing_source_product_schemas` | spider state per domain, URL classes, scrape outcomes, retry timers, reviews, LLM budget |
| JSON log | `target/soak/<date>/round-<n>.jsonl`, one record per line with `timestamp`, `level`, `fields.message`, `spans[*].url` and `spans[*].domain` | pass totals and durations (`Scraper scheduler pass complete`, `Spider scheduler pass complete`), LLM retries/timeouts, crawl diagnostics, circuits, every `WARN` and `ERROR` kind |
| Review console | http://127.0.0.1:7878 (`GET /api/reviews`, `GET /api/listing_sources`, `GET /api/reviews/{id}`, `GET /api/reviews/{id}/matrix?refresh=true`, `POST /api/reviews/{id}/approve\|reject\|needs-repair\|candidate\|schema-field`) | schema candidates, deterministic gate, blocked URLs, review decisions |
| Raw captures | `scraped_raw_observations.json` in the binary's working directory (a JSON array; `sourceRecordKey`, `sourcePayload`, `rawValues`) | what was actually extracted |
| Live pages | curl with the crawler user agent (`CRAWLER_USER_AGENT`) | whether a selector or pattern is right |

Mutations on the console need an `Origin: http://127.0.0.1:7878` header on the request.

## Rules

- Spend LLM budget deliberately: a round of 10 shops costs roughly 20–60 schema and classification
  calls. Do not loop on a shop that keeps timing out; record it and move on.
- Be polite to shops: one round per shop, no manual hammering, stop a domain that returns 429.
- Never log or paste credentials, `.env` values or raw provider payloads into the note.
- Findings cite code as `path:line` and evidence as a DB count, a log line or a live fetch.
  "Looks wrong" without one of those is a hypothesis, labelled as such.
- Do not turn findings into issues or fixes inside the soak run; that is a separate task.
