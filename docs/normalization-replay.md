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
cargo run --locked -p product-listing-normalization-replay -- boilerplate candidates --output candidates.jsonl
cargo run --locked -p product-listing-normalization-replay -- boilerplate review --input reviewed.jsonl
```

- Without a selector, every retained case is replayed, including cases fixed earlier; they
  remain regression coverage. Filters (`--listing-source`, `--case`, `--field`, `--reason`)
  only narrow a run.
- A run pages through the corpus bounded at its start; later cases belong to the next run.
- Each case calls the current checkout's production raw-values evaluator on the complete saved
  input, with its source's current reviewed [description boilerplate](#description-boilerplate),
  and reports every field. Exported cases carry that set as `descriptionBoilerplate`, so offline
  replay matches; an export without it strips nothing. Output is JSONL: one `case` record per case, then a `summary` with
  counts by status and by recorded field/reason: `RESOLVED`, `UNRESOLVED`, `INVALID`,
  `INCOMPATIBLE`, `SYSTEM_ERROR` and `UNAVAILABLE_EVIDENCE`.
- Exit status `0` means the snapshot was fully covered, even with unresolved cases; `2` means
  the run is incomplete; `64` is invalid usage.
- Replay never writes canonical state, advances raw heads, redelivers queue messages or
  records new cases. Raising the normalizer version or redelivering raw jobs does not rerun
  completed revisions; production reapplication is a separate, owned operation.
- `seed` evaluates retained UPSERT raw revisions once and records those with issues as
  `EVALUATED_RESOLVED`/`EVALUATED_INVALID` cases.

## Description boilerplate

`boilerplate candidates` reads the latest raw description of every crawled listing, one listing
source at a time (`--listing-source` narrows it), and counts the listings that contain each sentence. An
unreviewed sentence in at least `--min-listings` listings (default 5) and `--min-share` of the
source's listings (default 0.2) is written as a `candidate` record with its normalized `sentence`,
an `example` as written, `listings` and `share`. A `source` record follows each source with its
description bytes, the `repeatedShare` of repeated sentences and the `strippedShare` its current
`STRIP` set removes; replaying over current data before and after a review measures the drop.
Candidate records hold listing text: write them to a private working file, never to logs, the
repository or GitHub.

A reviewer copies the candidates they decide and adds `"decision": "STRIP"` for shop
boilerplate or `"decision": "KEEP"` for an item fact. `boilerplate review` validates the whole
file first, rejects any line without a decision and records it in one transaction; a later
decision for the same source and sentence replaces the earlier one. The worker reads the new
set for revisions normalized afterwards ([contract](product-listing.md#description-boilerplate)).

## Reviewed fixtures

"Now resolves" is not proof of correctness, and failure logs cannot reveal confidently wrong
successes. Every heuristic change adds reviewed expected outputs to
[`reviewed_cases.jsonl`](../src/product-listing-normalization-replay/tests/fixtures/reviewed_cases.jsonl):
one JSON object per line with `name`, `category` (`REPAIRED`, `SUCCESSFUL` or `NEGATIVE`), an
exported `case` and the `expected` status, decision, issue list and field subset. The fixture
test fails when any of the three categories is missing.
