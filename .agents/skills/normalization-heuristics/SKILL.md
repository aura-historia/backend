---
name: normalization-heuristics
description: "Export the production normalization diagnostic corpus (product_listing_normalization_diagnostic_cases) read-only, replay it offline, extend the deterministic availability/price/shipping/text heuristics for every unresolved reason the evidence supports, prove there are no regressions case by case, open a GitHub issue for every crawler extraction error, and open a pull request. Invoke only on request; it reads production data and opens issues and a PR."
disable-model-invocation: true
---

# Improve normalization heuristics from the production corpus

This runs one turn of the loop in [normalization diagnostics and replay](../../../docs/normalization-replay.md):
**export the unresolved inputs → replay → extend the heuristics → replay the whole corpus → PR.**
The outcome is one pull request against `develop` that resolves every recorded issue the evidence
supports. It names everything else it leaves unresolved, and why. Every crawler error found
along the way gets a GitHub issue. Previous turns: #2840, #2858, #2875.

Read [the decision policy](references/policy.md) before changing any rule. It holds what
was learned about which labels, formats and vetoes are safe to resolve.

Optional inputs (natural language, never interpolated into commands):
- `work=<dir>`: reuse an export from earlier today instead of exporting again.
- `focus=<FIELD or REASON>`: start with that bucket. All buckets are still triaged.

## Rules

- **Read-only production access.** Only [`scripts/prod_corpus.py`](scripts/prod_corpus.py) touches
  AWS. It runs SQL inside a read-only transaction and refuses writes, transaction control and psql
  meta-commands. Never run SQL against production any other way, and never write to it.
- **Listing content never reaches logs, the repository or GitHub.** Only aggregates, counts, hashes, UUIDs,
  dealer hosts and slugs may pass through CloudWatch (`query`, `size`, `sources`). The corpus moves
  only over ECS Exec into a `0600` file. The S3 gateway endpoint allows no uploads; never route
  around it (for example through the public subnets) and never widen its policy.
- **Never read a secret value.** Credentials are injected into the task as ECS secrets.
- Keep all working data under `/tmp/aura-normalization/<YYYY-MM-DD>/` (created `0700`), never in
  the repository. Reviewed fixtures are minimized synthetic cases, never prod payloads.
- Run `teardown` and confirm `status` is empty before you open the PR, and after any failure.

## Preconditions

1. AWS CLI credentials for the production account: `aws sts get-caller-identity` succeeds. If they
   have expired, ask the user to refresh them; do not work around it.
2. The Session Manager plugin is on `PATH` (`session-manager-plugin --version`). If it is missing,
   install it user-locally. This needs no sudo:

   ```sh
   d=$(mktemp -d) && curl -sSfL -o "$d/smp.deb" \
     https://s3.amazonaws.com/session-manager-downloads/plugin/latest/ubuntu_64bit/session-manager-plugin.deb
   mkdir -p ~/.local/opt/ssmplugin ~/.local/bin && dpkg-deb -x "$d/smp.deb" ~/.local/opt/ssmplugin
   ln -sf ~/.local/opt/ssmplugin/usr/local/sessionmanagerplugin/bin/session-manager-plugin ~/.local/bin/
   ```

3. `openssl` is available. It filters the committed RDS CA bundle to the region's roots.
4. The working tree is clean. Branch from a freshly fetched `origin/develop`:
   `git switch -c feat/normalization-heuristics-<YYYY-MM-DD> origin/develop`.

Below, `S=.agents/skills/normalization-heuristics/scripts`, `W=/tmp/aura-normalization/<YYYY-MM-DD>`
and `R=target/release/normalization-replay`.

## Procedure

### 1. Baseline evaluator

Before you edit anything, build the replay binary from the branch point and keep a copy:

```sh
cargo build --locked --release -p product-listing-normalization-replay
mkdir -p "$W" && chmod 700 "$W" && cp "$R" "$W/baseline-replay"
```

### 2. Export the corpus

Skip this step when `work=` points at an existing export with `corpus.jsonl` and `sources.tsv`.

```sh
python3 -I $S/prod_corpus.py status                # must be empty; otherwise run teardown first
python3 -I $S/prod_corpus.py setup   --work "$W"
python3 -I $S/prod_corpus.py size    --work "$W"   # volume by evaluator version and reason
python3 -I $S/prod_corpus.py export  --work "$W"   # about 3 minutes for 38k cases
python3 -I $S/prod_corpus.py sources --work "$W"   # listing source id -> slug, host
```

The roles stay until step 9, so step 4 can run evidence queries. Each query is a reviewed SQL file
run with `prod_corpus.py query --work "$W" FILE.sql`, and must select only aggregates, hosts or
IDs.

### 3. Baseline replay

```sh
"$W/baseline-replay" --input "$W/corpus.jsonl" --output "$W/baseline.jsonl"
python3 -I $S/analyze.py summary "$W/baseline.jsonl"
```

Exit status 0 means that the snapshot was fully covered. `RESOLVED` counts in the summary belong
only to cases that an *earlier* evaluator version recorded; a case recorded by the current version
always reproduces its own issue.

### 4. Triage every unresolved reason

Work through **every** `FIELD/REASON` in the summary that has `UNRESOLVED` or `INVALID` cases,
largest first, and give each a disposition for the PR (implemented, needs evidence,
crawler (an issue in step 9), configuration, prose, not expressible). For each one:

```sh
python3 -I $S/analyze.py tally --work "$W" "$W/baseline.jsonl" AVAILABILITY AVAILABILITY_UNRECOGNIZED --limit 120
python3 -I $S/analyze.py labels --work "$W" www.dealer.example     # every label one dealer uses
```

Classify each distinct value with [the policy](references/policy.md). Gather per-dealer evidence
with an aggregate query before you resolve any dealer-dependent label. For shipping, measure
whether the vetoes hide statements before you touch a rule (the policy describes how), and
remove the temporary instrumentation.

### 5. Implement

Follow the existing structure of `src/product-listing-normalization/src/{availability,price,shipping,text,language}.rs`.
Add no new abstractions and change no persisted reason code. Every rule gets unit tests in the same
file's `mod tests`: positive cases written as dealers write them, and the near misses the rule must
reject. Raise `PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION` once.

### 6. Replay, diff and review

```sh
cargo build --locked --release -p product-listing-normalization-replay
"$R" --input "$W/corpus.jsonl" --output "$W/after.jsonl"
python3 -I $S/analyze.py diff "$W/baseline.jsonl" "$W/after.jsonl"     # exit 1 = review required
python3 -I $S/analyze.py compare "$W/baseline.jsonl" "$W/after.jsonl"  # table for the PR
```

`diff` writes example case IDs to `after.review.json`. Explain every flagged item or fix it. Hand-check
every newly resolved value: map each changed field back to its raw value in `corpus.jsonl`. Remove
any change that resolves no case. Repeat steps 4 to 6 until no bucket has an evidence-backed
improvement left.

### 7. Reviewed fixtures

Write a spec of minimized synthetic cases. Each has invented title and description, the
observed raw value, and the fields to pin:

```json
[{"name": "purchase-action-availability", "category": "REPAIRED", "title": "Antique English oak chair",
  "description": "A restored English oak chair from a private collection.",
  "availability": "Bestellen", "fallbackCurrency": "EUR", "fields": ["availability"]}]
```

Optional keys: `price`, `priceEstimateMin`, `priceEstimateMax`, `priceFormat`, `attributes`. Check
each spec against both evaluators, then append it:

```sh
python3 -I $S/analyze.py fixtures spec.json --replay-bin "$W/baseline-replay"   # REPAIRED must fail here
python3 -I $S/analyze.py fixtures spec.json --replay-bin "$R" \
  --append src/product-listing-normalization-replay/tests/fixtures/reviewed_cases.jsonl
```

Every case must carry only its intended issue. If an unintended `TITLE_UNKNOWN_LANGUAGE` appears,
lengthen the invented text. Cover each change with `REPAIRED`, and each boundary with `NEGATIVE`.

### 8. Validate

```sh
cargo fmt --all -- --check && python3 ci/check-test-layout.py
cargo clippy --locked -p product-listing-normalization -p product-listing-normalization-replay \
  --all-targets --all-features -- -D warnings -D clippy::result-large-err
cargo test --locked -p product-listing-normalization -p product-listing-normalization-replay -p product-service --all-features
cargo test --locked -p crawler --lib --all-features
cargo test --locked -p product-listing-service --all-features
cargo test --locked -p product-listing-postgres --all-features --test listing_delivery   # shipping changes; needs Docker
python3 -m unittest discover -s $S                                                        # if the skill changed
```

If a durable contract changes, update its owning document. For example, the shipping diagnostic
boundary is in `docs/shipping.md`.

### 9. Open an issue for every crawler error

Every value classified as a crawler error in step 4 gets a GitHub issue. This applies even when no
PR follows. Open one issue per error class: the same field with the same kind of wrong
content, such as a URL in `state` or a status label in `price`. List every affected source in it.
Before you write an issue:

1. **Confirm the error is current.** Raw revision IDs are UUIDv7, so the corpus dates each capture:

   ```sh
   python3 -I $S/analyze.py captured --work "$W" www.dealer.example AVAILABILITY --match '^javascript:'
   ```

   An artefact whose last capture predates the deploy of a crawler fix needs no issue. Name it in the
   PR instead. For example, mojibake prices stopped with #2826.
2. **Find duplicates.** Run `gh issue list --state open --search "<host>"` for each host and search
   for the artefact class. If an open issue already covers it, comment with the new counts instead.
3. **Explain why the crawler accepted it.** Look up the field's check in
   `src/crawler/src/scraper/plausibility.rs`. The check may be missing, `ReportOnly`, or only
   `LowersRank`. If no check covers the class, the issue also asks for one.

Each issue has the labels `crawler`, `scraper` and `bug`. It states the field, the artefact class,
and a table of host, case count and last capture. It ends with the expected fix: re-review that
source's schema, a new or stricter plausibility check, or both. The repository is public. An issue
may show hosts, counts, dates and the short wrong value itself (`.`, an image path, a button label).
Never put a listing title, description, listing URL or ID in it. Link the issues from the PR's
crawler follow-ups.

### 10. Clean up, commit, open the PR

```sh
python3 -I $S/prod_corpus.py teardown --work "$W" && python3 -I $S/prod_corpus.py status
```

Commit with a conventional message and push the branch. Open the PR against `develop` with
[the template](references/pr-template.md). If no change is backed by evidence, open no PR; report the
dispositions and the crawler issues instead.

### 11. Report to the user

Give the PR link, the crawler issue links and the before/after statuses. Name the largest remaining
buckets and who owns them (crawler, configuration, product decision). Say that the corpus stays at
`$W/corpus.jsonl` (`0600`) until they delete it.
