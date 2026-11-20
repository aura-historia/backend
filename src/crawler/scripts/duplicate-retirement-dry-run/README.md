# Duplicate retirement dry run (#3067)

Read-only measurements to run before the one-off migration that retires product URLs captured
twice under one identity ([#3067](https://github.com/aura-historia/backend/issues/3067)). A
retirement cannot be undone by the crawler, so the migration is written only after these results
are reviewed. Each file opens a `READ ONLY` transaction, which PostgreSQL enforces, and rolls it
back. It creates no functions or temporary objects, so it also runs on a read replica. The rule is
the scraper's: the carrier lookup of `identity_carrier()` in
[`candidate_service.rs`](../../src/scraper/candidate_service.rs) and the same-product rule of
[`same_product.rs`](../../src/scraper/same_product.rs). They need the #3046 migrations
(`last_extracted_source_listing_id`).

Where the dry run differs from a scrape, it is deliberate or noted here:

- It judges every live URL whose page showed an identity another URL carries, including double
  captures from before #3046 whose page identity equals their captured identity; the scraper never
  retires those on its own, which is why the migration exists.
- It leaves out URLs in an identity conflict, which their next scrape decides.
- It compares stored URLs. A legacy row that still has a parameter the crawler no longer stores
  (tracking, session, sort) is compared as stored, so the `SPELLING` count can come out low
  rather than high.
- On a hot standby a long query can be cancelled by recovery conflicts
  (`max_standby_streaming_delay`); run it on the primary's read-only role if that happens.

| File | Answers |
|---|---|
| `1_per_host_counts.sql` | URLs that would retire per host and in total, by evidence (`SPELLING`, `IDENTITY_TOKEN`, `TITLE`), and those left without evidence; the watch-list hosts are labelled |
| `2_watch_list_pairs.sql` | every pair on the hosts #3067 names (`SUSPECT`, `AMBIGUOUS`, `LIST_PAGE_CARRIER`, `QUERY_SPELLING` for antiquariat-neidhardt.de), with URL, carrier, identities, titles and the deciding evidence, for spot-checks against the live pages |
| `3_extra_query_parameter_pairs.sql` | "same path, one URL with extra query parameters" per host and parameter name, with how often the pair shows one identity and one title (Neidhardt's `?cookie=yes` first) |

Run each with a read-only role, from the repository root:

```sh
for f in src/crawler/scripts/duplicate-retirement-dry-run/*.sql; do
  psql -X -q --csv "$CRAWLER_DATABASE_URL" -f "$f" > "$(basename "$f" .sql).csv"
done
```

Then:

1. Attach `1_per_host_counts.csv` to #3067 and compare the total with the 2026-11-18 run
   (13,811: 3,940 by spelling, 9,871 by title only).
2. Open a sample of pairs per watch-list host from `2_watch_list_pairs.csv` in a browser. Decide
   per host whether the pair is one product (retire) or two items sharing a generic title (keep).
   `LIST_PAGE_CARRIER` hosts must show no `TITLE` rows.
3. Use `3_extra_query_parameter_pairs.csv` to decide whether a parameter counts as spelling
   evidence. Accept a name only when `other_page_identity` and `page_identity_unknown` are 0 and every
   pair shares the captured identity. `cookie` on Neidhardt is the case in question; a name such as `id` with other
   identities names another product.

Hand these decisions back. The migration (with `duplicate_of_url`, `duplicate_evidence` and
`duplicate_retired_at` on every row it retires) is written next. Its copy-database dry run, with
production's `statement_timeout` and `lock_timeout`, then measures duration, WAL and the retire
count.
