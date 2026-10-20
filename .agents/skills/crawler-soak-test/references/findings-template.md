# Crawler soak findings (<YYYY-MM-DD>, local `dev`, not a durable doc)

Run: <HH:MM>–<HH:MM> UTC, <n> rounds, <n> shops. LLM concurrency <n>, crawler log level <info|debug>, effective schema review mode <mode>.
Evidence comes from the `crawler_demo` DB, `target/soak/<date>/round-*.jsonl`, the review console, live page fetches and a code audit.
Scope for ranking: **small/medium non-JS single-dealer shops, no auction houses.**
Earlier notes: <link to previous findings note>; items already reported there are referenced, not repeated.
Delete this file once the findings are turned into issues.

## Shops

| Round | Shop (platform) | Preflight | Outcome |
|---|---|---|---|
| 1 | example.com (WooCommerce) | 200, robots ok, 240 links, sitemap | 412 pages, 180 product URLs, schema auto-approved, 95 captured |

Dropped at preflight: <domain (reason)>, ...

## Timeline of anomalies

| Time (UTC) | Domain | What happened | Evidence |
|---|---|---|---|
| 13:59 | all | scraper pass still running at 14:32 | `Scraper scheduler pass complete` duration 1980 s |

## Findings, ranked for small/medium shops

### 1. <title> — <not ticketed | partly #NNNN | #NNNN>

- What happened: <shop, numbers, timestamps>.
- Cause: <`path/file.rs:line`, one or two lines>.
- Impact on small/medium shops: <why it ranks here>.
- Fix direction: <one or two lines; no implementation>.

### 2. ...

## Regressions and confirmations against earlier notes

- <earlier finding>: still present / fixed / changed, with evidence.

## Observability gaps seen this run

- <question an operator had that the INFO log could not answer, and what was missing>.

## Ticket coverage

- Covered or partly covered: #...
- Not ticketed: <one line per finding>

## Local state left behind

- `src/crawler/src/bin/dev.rs`: reverted / still holds the round list (uncommitted).
- Reviews approved by hand: <ids, times>.
- `crawler_demo` holds <n> demo sources; `scraped_raw_observations.json` is <size>.
- The `dev` process is stopped; the console at :7878 is down.
