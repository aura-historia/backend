# Normalization diagnostics and replay

Deterministic normalization improves through one loop: **capture unresolved inputs → log
actionable diagnostics → improve the heuristics → replay the whole corpus**. The improving
agent and its schedule live outside this repository.

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
versioned entry point and exact input hash. Retries and later evaluations refresh the recorded
outcome, issues and build provenance and count occurrences; the evaluator version is
provenance, never identity.

| Entry point | Evidence | Recorded |
| --- | --- | --- |
| `RAW_VALUES_NORMALIZATION_V1` | Reference to the immutable raw revision | In the completion transaction; system failures in their own transaction |
| `CRAWLER_PREPARATION_V1` | Snapshot of the extracted product handed to preparation (selected images included), effective URL, original source-record key and fallback currency | Before a discarded candidate is dropped |

Deleting a raw revision deletes its cases, so every retained reference replays. A recording
failure fails the revision or scrape attempt and uses the existing retry handling. Crawler
inputs discarded before this table existed are unrecoverable; seeding recovers raw-revision
history only.

## CloudWatch events

The owning boundary emits one versioned `product_listing_normalization_unsolved` event per
issue: WARN for candidate-data gaps, ERROR for system failures. The worker logs after commit,
so events describe committed handling. Events carry the case ID, input hash, issue fields,
outcome, source/stream/revision references, versions and the build Git SHA. They support
discovery only; the case table is the replay corpus. Post-commit shipping application
failures are operational errors, logged separately from heuristic misses.

## Replay

`normalization-replay` (crawler crate) needs `BUSINESS_DATABASE_URL` for database modes.

```sh
cargo run --locked -p crawler --bin normalization-replay -- --output replay.jsonl
cargo run --locked -p crawler --bin normalization-replay -- --reason PRICE_UNKNOWN_CURRENCY
cargo run --locked -p crawler --bin normalization-replay -- export --output corpus.jsonl
cargo run --locked -p crawler --bin normalization-replay -- --input corpus.jsonl
cargo run --locked -p crawler --bin normalization-replay -- seed
```

- Without a selector, every retained case is replayed, including cases fixed earlier; they
  remain regression coverage. Filters (`--entry-point`, `--listing-source`, `--case`,
  `--field`, `--reason`) only narrow a run.
- A run pages through the corpus bounded at its start; later cases belong to the next run.
- Each case calls the current checkout's production evaluator on the complete saved input and
  reports every field. Output is JSONL: one `case` record per case, then a `summary` with
  counts by status and by recorded field/reason: `RESOLVED`, `UNRESOLVED`, `INVALID`,
  `INCOMPATIBLE`, `SYSTEM_ERROR` and `UNAVAILABLE_EVIDENCE`.
- Exit status `0` means the snapshot was fully covered, even with unresolved cases; `2` means
  the run is incomplete; `64` is invalid usage.
- Replay never writes canonical state, advances raw heads, redelivers queue messages or
  records new cases. Raising the normalizer version or redelivering raw jobs does not rerun
  completed revisions; production reapplication is a separate, owned operation.
- `seed` evaluates retained UPSERT raw revisions once and records those with issues as
  `EVALUATED_RESOLVED`/`EVALUATED_INVALID` cases.

## Reviewed fixtures

"Now resolves" is not proof of correctness, and failure logs cannot reveal confidently wrong
successes. Every heuristic change adds reviewed expected outputs to
[`reviewed_cases.jsonl`](../src/crawler/tests/fixtures/normalization_replay/reviewed_cases.jsonl):
one JSON object per line with `name`, `category` (`REPAIRED`, `SUCCESSFUL` or `NEGATIVE`), an
exported `case` and the `expected` status, decision, issue list and field subset. The fixture
test fails when any of the three categories is missing.
