# Normalization diagnostics and replay

Deterministic raw-revision normalization in the business worker improves through one loop:
**capture unresolved inputs → log actionable diagnostics → improve the heuristics → replay the
whole corpus**. The [`normalization-heuristics` skill](../.agents/skills/normalization-heuristics/SKILL.md)
runs one turn of that loop on request; any schedule for it lives outside this repository. The crawler
does not normalize; candidates its plausibility checks reject are crawler concerns and are not
recorded here.

## Diagnostics

The pure evaluators return their unchanged decision plus typed issues
([`diagnostics.rs`](../src/product-listing-normalization/src/diagnostics.rs)). Every
independent field is evaluated, so one input reports all of its issues, including soft misses
inside otherwise successful decisions. An envelope failure that prevents field evaluation is
reported once. Absence, intentional `CLEAR`/`UNCHANGED`, no-assertion results, valid
on-request prices and partial shipping assertions are legitimate outcomes, not issues.

Each issue carries a stage, field/input path, stable reason code, scope (`CANDIDATE_DATA` or
`SYSTEM`), caller handling (`REJECTED_INPUT`, `PRESERVED_FIELD`, `UNRESOLVED_FIELD`) and
bounded details. Details never contain input text. Reason codes are persisted; never rename
one.

## Retained corpus

`product_listing_normalization_diagnostic_cases` (business PostgreSQL) holds one row per
versioned entry point (`RAW_VALUES_NORMALIZATION_V1`) and exact input hash, referencing the
immutable raw revision that holds the complete input. Retries and later evaluations refresh
the recorded outcome, issues and build provenance and count occurrences; the evaluator version
is provenance, never identity.

The worker records a case in the same transaction as the revision's completion; a system
failure without a completion is recorded in its own transaction. A recording failure fails
the revision and uses the existing retry handling. Deleting a raw revision deletes its cases,
so every retained reference replays. Seeding recovers history from retained raw revisions.

## CloudWatch events

The worker emits one versioned `product_listing_normalization_unsolved` event per issue after
commit, so events describe committed handling: WARN for candidate-data gaps, ERROR for system
failures. Events carry the case ID, input hash, issue fields,
outcome, source/stream/revision references, versions and the build Git SHA. They support
discovery only; the case table is the replay corpus. Post-commit shipping application
failures are operational errors, logged separately from heuristic misses.

## Replay

`normalization-replay` (`product-listing-normalization-replay`) needs `BUSINESS_DATABASE_URL`
for database modes.

```sh
cargo run --locked -p product-listing-normalization-replay -- --output replay.jsonl
cargo run --locked -p product-listing-normalization-replay -- --reason PRICE_UNKNOWN_CURRENCY
cargo run --locked -p product-listing-normalization-replay -- export --output corpus.jsonl
cargo run --locked -p product-listing-normalization-replay -- --input corpus.jsonl
cargo run --locked -p product-listing-normalization-replay -- seed
```

- Without a selector, every retained case is replayed, including cases fixed earlier; they
  remain regression coverage. Filters (`--listing-source`, `--case`, `--field`, `--reason`)
  only narrow a run.
- A run pages through the corpus bounded at its start; later cases belong to the next run.
- Each case calls the current checkout's production raw-values evaluator on the complete saved
  input and reports every field. Output is JSONL: one `case` record per case, then a `summary` with
  counts by status and by recorded field/reason: `RESOLVED`, `UNRESOLVED`, `INVALID`,
  `INCOMPATIBLE`, `SYSTEM_ERROR` and `UNAVAILABLE_EVIDENCE`.
- Exit status `0` means the snapshot was fully covered, even with unresolved cases; `2` means
  the run is incomplete; `64` is invalid usage.
- Replay never writes canonical state, advances raw heads, redelivers queue messages or
  records new cases. Raising the normalizer version or redelivering raw jobs does not rerun
  completed revisions, and admission never captures an unchanged input again; that is
  [production reapplication](#production-reapplication).
- `seed` evaluates retained UPSERT raw revisions once and records those with issues as
  `EVALUATED_RESOLVED`/`EVALUATED_INVALID` cases.

## Production reapplication

`ops renormalize-raw (--dry-run | --apply)` ([ops](../src/aura-historia-ops/README.md)) normalizes
the latest raw revision of each caught-up stream again with the deployed normalizer. The drain's
completion applies it, with its canonical write and event and its diagnostic case, and records a
completion for the current normalizer version next to the earlier one. The stream head does not
move; a listing the completion creates is bound to it. Crawler shipping is not imported again: it
imports each raw revision once, and that revision's import already happened.

- **Selection.** A stream qualifies while its head has processed its latest revision, that revision
  is an `UPSERT`, and no normalizer of the current or a newer version has completed it, so a
  rerun or an interrupted run never applies a revision twice. A stream with pending revisions
  belongs to the drain. `DELETE` revisions are never reapplied: the listing is already withdrawn,
  and withdrawing again would only undo a later restore.
- **Superseded streams are skipped.** Several streams, such as URL variants of one page, can bind
  one listing. A stream is skipped while another stream bound to the same listing has processed a
  revision captured at the same time or later, because reapplying the older revision would
  restore older state over the newer stream's. The page and the locked stream both check it.
- **Withdrawn listings are skipped.** An upsert restores its listing, so reapplication never touches
  a listing that another path withdrew.
- **Other writers.** Like the source's next change, a reapplied revision overwrites what the
  partner or administrator API changed on a raw-fed listing since.
- **Diagnostics.** A reapplied revision with issues refreshes its diagnostic case and counts as
  one more evaluation of that input in `occurrences`, like any later evaluation.
- **Dry run.** Each stream runs in a transaction that is rolled back: nothing is written or
  logged, but rows stay locked for the stream's few milliseconds. The report counts recorded→new
  outcome transitions, changed listing facts, completion error codes, issue reasons and skips by
  cause.
- **Paging and pacing.** Each stream commits on its own. A page scans a fixed number of streams by
  ID, so its read costs the same however few qualify; the report's `last` resumes a run with
  `--after`, and SIGTERM ends it after the current page. A failed page read, or 50 stream
  failures without a success in between, ends the run with the partial report in the log: its
  `last` continues past the failed streams and `failureStreak` names them, with the `retryAfter`
  position that retries them. With `--apply`, each page waits while the outbox and the job
  queue's ready jobs together hold more than `--max-backlog` entries, logging the count while it
  waits.
- **Effects.** A changed listing appends an ordinary event: projection, watchlist price-change
  notifications and the next periodic match follow, a created listing is enriched like any
  discovery, and a changed title or description is translated and, when the change is
  meaningful, embedded again. The dry run's `changes` sizes that work before applying.

Run it once no worker of an earlier normalizer version runs, dry run first
([runbook](durable-worker-runbook.md#raw-normalization-reapplication)).

## Reviewed fixtures

"Now resolves" is not proof of correctness, and failure logs cannot reveal confidently wrong
successes. Every heuristic change adds reviewed expected outputs to
[`reviewed_cases.jsonl`](../src/product-listing-normalization-replay/tests/fixtures/reviewed_cases.jsonl):
one JSON object per line with `name`, `category` (`REPAIRED`, `SUCCESSFUL` or `NEGATIVE`), an
exported `case` and the `expected` status, decision, issue list and field subset. The fixture
test fails when any of the three categories is missing.
