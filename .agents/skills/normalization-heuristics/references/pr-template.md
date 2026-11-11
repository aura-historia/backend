# Pull request template

Title: `feat(normalization): extend <fields> heuristics from the <YYYY-MM-DD> corpus`

Write the body from evidence. Replace every `<…>`; delete sections that do not apply. Never paste
listing text beyond the short labels, amounts and phrases that the rules match.

```markdown
## Summary

I replayed the full production normalization diagnostic corpus: <N> cases from <S> sources,
recorded <first> to <last>, <M> of them under the current v<V> heuristics. I exported it with the
`normalization-heuristics` skill (read-only Fargate task, transfer over ECS Exec, SHA-256
verified) and replayed it offline with `normalization-replay --input`. The temporary AWS
resources were deleted afterwards.

### <Field> (`<file>.rs`)
- **<Rule>**: `<observed label>` → `<VALUE>` (<cases> cases, <dealer or "several dealers">).
  <Why it is generic; the evidence for dealer-dependent labels.>

`PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION` goes from <V> to <V+1>. It is provenance only.

## Corpus replay (<N> cases, current `develop` → this PR)

<output of `analyze.py compare BASELINE AFTER`>

**Regression check, case by case against the baseline replay:**
- <no worse status / every flagged item and why it is intended>
- <re-attributed reasons, if any, and why the case still abstains>
- I checked every newly resolved value by hand.

### Deliberately not changed
- **<label or class>** (<cases>): <reason, for example no sold evidence, prose, crawler artefact>

### Crawler follow-ups (not normalization)
- #<issue>: <field> holds <artefact class> (<hosts>, <cases>)
- <artefact> stopped after <fix PR> (no issue)

### Operator follow-ups (configuration, not code)
- <host>: <needed fallback currency>, <only if fallbackCurrency is wired; see the policy>

## Test plan
- [x] `cargo test -p product-listing-normalization`
- [x] `cargo test -p product-listing-normalization-replay` (<k> new reviewed fixtures)
- [x] `cargo test -p crawler --lib`, `-p product-listing-service`, `-p product-service`
- [x] `cargo test -p product-listing-postgres --all-features --test listing_delivery` (shipping changes)
- [x] `cargo clippy` with `-D warnings` on the affected crates, `cargo fmt`, `ci/check-test-layout.py`
- [x] Offline replay of the full production corpus, with a case-by-case diff

Not deployed. The new evaluator has not run against production.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```
