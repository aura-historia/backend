---
name: crawler-dev-verify
description: "After every change to crawler runtime code (src/crawler: scraper, spider, review, schema, scheduling), build and run the local dev binary against the crawler_demo state to confirm the change does what the task says and that nothing else regressed, before declaring the change done. Unit tests are not a substitute: they do not fetch live shops, apply cached schemas to real pages or drive the scheduler."
---

# Crawler dev verification

Every crawler change ends with a dev-binary run, not only with `cargo test`. The run must
reproduce the situation the task describes (the pages, shops or schedule state that misbehaved)
and show, with log lines, database counts and captures, that the new behaviour appears and that
no new failure kind appears. Report what reproduced and what did not; never claim a change works
because the build is green.

A verification run exercises one change for 15–40 minutes on known state; it is not a soak
round, which observes unchanged code for 60–90 minutes on new shops. The skill ships no helper
scripts on purpose: write the one-off queries and log filters for the platform you are on
(`psql` through `docker exec`, Python for the JSON files, curl for live pages) in the scratchpad,
never in the repo. The evidence sources are:

| Source | Where | What it answers |
|---|---|---|
| Crawler DB | `docker exec crawler-postgres-1 psql -U postgres -d crawler_demo` | `listing_source_domains` (crawl and scrape timers, failure streak), `listing_source_urls` (class, disposition, `last_scraped`, failure fields, `last_source_listing_id`), `listing_source_product_schemas` (live schemas), `crawler_reviews` and `crawler_review_pages` (status, reason, seed pages), `listing_sources.llm_calls_count` |
| JSON log | `target/soak/<date>/verify-<n>.jsonl`, one record per line with `timestamp`, `level`, `fields.message`, `spans[*].url` | pass totals (`Scraper scheduler pass complete`), every `WARN` and `ERROR` kind, the change's own diagnostic, LLM retries |
| Review console | `http://127.0.0.1:7878`: `GET /api/reviews`, `GET /api/reviews/{id}`, `GET /api/reviews/{id}/matrix?refresh=true`, `POST /api/reviews/{id}/approve\|reject\|needs-repair` with an `Origin: http://127.0.0.1:7878` header (an approval carries the `expected_candidate_version` it was based on) | candidates, the deterministic gate on live seed pages, review decisions |
| Raw captures | `scraped_raw_observations.json` in the binary's working directory (a JSON array; `sourceRecordKey`, `sourcePayload`, `rawValues`) | what was extracted per page |
| Live pages | curl with the crawler user agent (`AuraHistoriaBot/1.0 (+https://aura-historia.com)`) | whether a selector is right for a page variant |

## Scope rule

Crawler work changes code under `src/crawler` only. Never edit another crate, even when a fix
would be simpler there (normalization, money, listing-source, test-api). When the crawler change
needs behaviour from another crate, stop, describe the needed change with `path:line`, and leave
it to the user as a ticket or a separate decision.

## Preconditions

- Docker Desktop running (`docker ps` lists `crawler-postgres-1`); the binary runs
  `docker compose up -d` itself. `crawler_demo` keeps the state of earlier rounds: listing
  sources, URLs, live schemas, reviews. That state is the test bed; do not reset it.
- `.env` in the binary's working directory. In a worktree copy it from the repo root (`.env` is
  gitignored). Never print its values.
- No `dev.exe` running (`Get-Process dev`). While it runs, cargo cannot rebuild crawler bins;
  tests in parallel need `--target-dir target/scratch` from PowerShell.
- `cargo fmt`, `cargo clippy` and the focused tests are already green.

## Procedure

1. **Decide what must be visible.** Write down, before starting, the log message, database
   change or capture field that proves the change, and the failure that the change removes.
   Example (#2116): the per-field miss diagnostic appears, no `fresh_schema_generation` review
   is created, the page is captured with the field empty.
2. **Register the shops.** The committed `dev.rs` lists three demo shops that are not in the
   database. Replace them temporarily with the shops whose state you need, using the IDs already
   in `listing_sources` (`...639f{index:02x}`): edit the `(index, name, slug)` tuple array, the
   domain list zipped in `main()`, and the ID format line, which must read `{index:02x}`. Mark
   the edit `TEMPORARY`, never commit it, and revert with
   `git checkout -- src/crawler/src/bin/dev.rs` at the end. Registration disables listing
   sources that are not in the list, so include every source the run needs.
3. **Prepare the state and back it up first.** Copy every row you will change to the scratchpad
   (`psql -At` with an explicit separator) and write the restore SQL before the run:
   - Live schemas (`listing_source_product_schemas`) when the change concerns extraction: keep
     only the schema that exposes the behaviour, e.g. drop a "no-description" variant to force
     the optional-field path.
   - URL eligibility (`candidate_service.rs`, `eligible_urls`): a product URL is scraped when
     `last_scraped` is null or older than one day, `next_retry_at` is null or past, the source
     has no `PENDING_REVIEW` product-schema review, and the domain has no open circuit. To
     rescrape a sample set `last_scraped`, `last_scraped_hash`,
     `last_scraped_schema_fingerprint` and `last_captured_raw_input_sha256` to null; the last
     one forces a new capture even when the extracted input is unchanged.
   - Pick the sample from the previous captures in the repo-root `scraped_raw_observations.json`
     (index by `sourceRecordKey`) and from `crawler_review_pages` of the reviews the task cites,
     so the exact pages that misbehaved are in the run. 150–400 URLs is enough.
   - Keep the run focused: defer other domains' pending URLs with `next_retry_at` (the domain
     `next_scrape_at` only gates domains with a failure streak), and push `next_crawl_at` out so
     the spider stays quiet unless the change is a spider change.
4. **Build and start.** `cargo build --locked -p crawler --bin dev`, then run
   `target/debug/dev.exe` from the worktree root with `CRAWLER_LOG_LEVEL=debug`
   (the per-URL and per-field diagnostics are debug level), `CRAWLER_AUTHORING_MODE=review` and
   `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS=3`; stdout to
   `target/soak/<date>/verify-<n>.jsonl`, stderr to a separate file. Note the UTC start time.
   The scraper runs 50 URLs per domain per pass and the pass is a barrier, so a 360-URL sample
   over three domains takes about five passes of four minutes in a debug build.
5. **Watch.** Tail the log for `Scraper scheduler pass complete`, review creation, schema
   generation, `"level":"ERROR"` and every `WARN` kind; count the change's own diagnostic.
   Check the database every pass: scraped and pending per domain, `crawler_reviews` created
   since the start, `llm_calls_count`. Stop as soon as the sample is exhausted or a pass repeats
   the same skipped URLs; a pass that only skips is itself a finding.
6. **Judge with evidence.**
   - The proof from step 1 is in the log or the captures, with counts.
   - No new `ERROR`, no new `WARN` kind, no fetch failure, no review and no LLM call that the
     change does not predict.
   - Diff the new captures against the previous ones per URL (identity, title, state, price,
     description count, image count). Identity must never change; count changes must be
     explained (duplicate-selector cleanup, items sold since). Do the diff with Python reading
     both files as UTF-8; a PowerShell `ConvertFrom-Json` index mangles non-ASCII and shows
     false title differences. Confirm a suspected encoding change at byte level with `xxd`.
   - Spot-check every soft miss or skipped page against the live page with curl and the crawler
     user agent: a field that is on the page but not extracted is a selector defect to report,
     not a pass.
7. **Restore and revert.** Run the restore SQL, verify schema counts, timers and review counts
   match the backup, stop the process, revert `dev.rs`, and list what the run left behind: new
   capture file in the worktree, updated `last_scraped` on the sample, log files.
8. **Report** in the task summary: passes and totals, what reproduced and what did not, each new
   finding with `path:line` and a sample URL, and which findings are outside the change. Leave the
   evidence paths in the shared notes folder when the task came from another session.

## Agent mode

Verify review-agent behaviour (#2565) with `dev` in `CRAWLER_AUTHORING_MODE=agent`. It is opt-in:
the crawler runs as in `manual`, makes no LLM call and needs no Vertex AI setting.

1. **Start.** Run `dev` as in step 4 with `CRAWLER_AUTHORING_MODE=agent` and without
   `CRAWLER_REVIEW_AUTH_TOKEN`, so the browser console at `http://127.0.0.1:7878` stays the human's
   tokenless loopback console. Keep the scheduler quiet as in step 3 unless the change is a
   pipeline change.
2. **Token file.** The start writes `review-agent-tokens.json` to the working directory (mode
   `0600`, `{"author": "...", "reviewer": "..."}`, git-ignored) and logs `Generated the review
   agents' tokens of this start ...` with its `path`. Check the mode (`stat -c %a`) and that
   neither token occurs in the JSON log or stderr (`grep -cF` with each token). Never print the
   tokens: read them into shell variables, for example
   `AUTHOR=$(python3 -c 'import json; print(json.load(open("review-agent-tokens.json"))["author"])')`.
3. **Act as an agent through `review-mcp`.** The agents reach the console only through the review
   MCP server (`docs/crawler/operations.md#review-mcp-server`), so drive it as their MCP client
   does. Build it with `cargo build --locked -p crawler --bin review-mcp` and start one process per
   role from the worktree root, `target/debug/review-mcp --role author` and `--role reviewer`, with
   `CRAWLER_REVIEW_CONSOLE_URL` unset (the dev console) and `CRAWLER_REVIEW_AGENT_TOKEN_FILE` unset or
   pointing at the token file above; stderr to a file of its own. Drive each over stdio with a
   small scripted client in the scratchpad (newline-delimited JSON-RPC: `initialize`,
   `notifications/initialized`, `tools/list`, `tools/call`), never with the `claude` CLI, so the run
   spends no LLM budget and no login. Expect 12 author and 7 reviewer tools, JSON-RPC error
   `-32602` for the other role's tool, `isError` results whose text is a JSON object with `kind`
   (`URL_OUTSIDE_REVIEW_SOURCE` for a URL off the review's ListingSource, before any request;
   `FORBIDDEN` with `reason`; `CONFLICT` with `failed_checks` when the approval guard refuses, for a
   URL pattern `TOO_FEW_LABELS`, `PRODUCT_LABEL_NOT_MATCHED`, `NOT_PRODUCT_LABEL_MATCHED`), and
   answers below 48 KiB. The author reads (`get_review`, `inspect_page`), tests
   (`evaluate_candidate`), stages (`update_candidate`, `update_schema_field`, `add_review_page`) and
   labels (`label_sample_urls`, `PRODUCT` or `NOT_PRODUCT` with a `reason`); the reviewer refreshes a
   product-schema review's live matrix (`refresh_matrix`) and approves with the `candidate_version`
   it read, or rejects with `notes`. Until a pattern is approved the spider stores every crawled
   URL as `other`; check `listing_source_urls.url_class`. The event trail (`get_review`, `events`)
   records `AGENT_AUTHOR` and `AGENT_REVIEWER`. Neither token may occur in `review-mcp`'s stderr or
   in any tool answer. For a console route no tool covers, send `Authorization: Bearer $AUTHOR` or
   `$REVIEWER` with curl; no `Origin` header is needed, and the other role's routes answer `403`
   `AUTHOR_CANNOT_DECIDE` or `REVIEWER_CANNOT_CHANGE_CANDIDATES`.
4. **Rotation and kill switch.** A restart writes new tokens and the old ones answer `401`; a
   running `review-mcp` reads the new token on that `401` and retries once, so its next tool call
   succeeds without a restart. A restart with `CRAWLER_AUTHORING_MODE=manual` removes the file, and
   `review-mcp` answers `UNAUTHORIZED`. Delete nothing else by hand.

## Rules

- Only `src/crawler` changes; see the scope rule above.
- Spend LLM budget deliberately: a verification run should need zero or a handful of calls; an
  unexpected generation is a finding.
- Be polite to shops: one pass over a sample, no re-fetch loops, stop a domain that returns 429.
- Never log or paste `.env` values, credentials or raw provider payloads.
- Findings that are not caused by the change go into tickets or the hand-over note, not into
  the change.
