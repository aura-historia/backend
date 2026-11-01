# Real-world matcher challenge set

This corpus contains **88 search/listing judgments using 69 real museum objects**:
48 expected matches and 40 expected non-matches across eight intents. All 69 JPEGs
and the catalogue facts used to construct the listings are stored here. Validation
and the review page make no network requests. Source URLs in the provenance
snapshots are citations, not dependencies.

The objects and catalogue facts come from the Cleveland Museum of Art's CC0 open
access collection. Images and metadata retain their CC0 dedication; see
[CC0-1.0.txt](CC0-1.0.txt) and each record's credit line in `sources/`. Test code
follows the repository license. Each asset has a SHA-256 digest in
[manifest.json](manifest.json).

## What this measures

The cases cover plain-query alternatives and object-versus-depiction ambiguity;
oil paintings versus prints, drawings and photographs; subject exclusions;
porcelain versus stoneware; manufacture/origin/date requirements; and multi-picture
lots with an exclusion in different image positions. Text and single-image cases
use actual catalogue titles and factual descriptions. Four-picture lots are
explicitly constructed groups of real objects, with one stored photograph per
work; they are not historical auction lots.

**Expected-match labels are agent-authored and have not been human-reviewed.**
They were fixed before the production run. Labels and their rationales are
reviewable in the manifest and the offline gallery. This is a purposive challenge
set, not a random sample of dealer listings or an estimate of the production error
rate. Some objects recur under different intents or in constructed lots; 88
judgments are not 88 independent objects. No model-quality release gate is passed
by this dataset until its labels have been reviewed.

## Offline use

Only Python 3.10+ and its standard library are required:

```sh
python3 src/search-filter-service/benchmarks/real-world-v1/run.py validate
python3 src/search-filter-service/benchmarks/real-world-v1/run.py review
```

Open [review.html](review.html) directly in a browser. By default, `review` uses
the recorded production baseline. The page loads only local
images and can filter cases by intent or result. Text-only cases show a local
photograph for human review, but that photograph is not submitted to the matcher.
To regenerate a scored review, pass `review --report /path/to/report.json`.

## Opt-in deployed run

The live driver uses the **currently scheduled production task definition**. It
does not rebuild the binary, change the schedule or modify deployed prompts,
thresholds, batching, network rules, IAM, or bucket policies. It requires an empty
active catalogue to bound fixture exposure and uses one isolated temporary source.
It creates eight notifications-disabled saved filters, excludes other groups at
filter creation, and verifies indexed freshness before starting the matcher.

Use an authenticated AWS CLI and a current administrator bearer in a mode-0600
file outside the repository. Delegated API tokens need users, search-filters,
product-listings, listing-sources, partnerships, partnership-applications and
notifications read/write scopes. The driver uses the supplied credential; it does
not mint, revoke, print or copy it into the repository.

```sh
python3 src/search-filter-service/benchmarks/real-world-v1/run.py live \
  --allow-production \
  --token-file /tmp/matcher-admin-token \
  --state-dir /tmp/matcher-corpus-private-state \
  --output /tmp/matcher-corpus-results.json
```

The state directory must be new and outside the repository. It holds recovery
identifiers and S3 object-version references, not the bearer or signed image URLs.
Images are uploaded from the pinned local bytes to a temporary prefix in the
existing endpoint-permitted artifact bucket. Short-lived signed reads use its
IPv4-only S3 hostname, controlling for the separately tracked image DNS bug
[#2659](https://github.com/aura-historia/backend/issues/2659). Therefore these results
measure judgment with delivered evidence, not the reliability of merchant image
hosting. No image is downloaded from a source website during a live replay.

The runner temporarily resolves the account to Ultimate so source restrictions and
enhanced intents can be exercised. Plain cases still submit zero images and no
enhanced intent. This is not a Free/Pro entitlement-policy test. Invocation-only
limits set projection lag to zero after convergence, concurrency to two, attempts
to one and the matching deadline to ten minutes. Technical failures remain
unresolved instead of becoming negative labels. The conservative preflight budget
must stay below the manifest's 1,500-neuron ceiling; account-wide quota and upstream
embedding consumption are not measured by that estimate.

Cleanup runs on success or failure: filters and associated test notifications are
deleted, listings withdrawn, grants removed, ingestion disabled, original account
tier restored, and exact temporary S3 object versions deleted. Historical source,
party, application, listing and matching-run rows remain by application contract.
If cleanup is interrupted, use the retained private state and a valid bearer:

```sh
python3 src/search-filter-service/benchmarks/real-world-v1/run.py cleanup \
  --state-dir /tmp/matcher-corpus-private-state \
  --token-file /tmp/matcher-admin-token
```

## Reading results

The report keeps persisted matches, scores and requested/sent image counts per
case. Classifier precision/recall include only decisions with all intended evidence
delivered. Not-retrieved cases, missing scores and degraded image evidence are
reported separately; inspect these before interpreting aggregate quality.
`observed_persisted_matches` additionally counts retrieval exclusions as observed
pipeline outcomes, without treating them as model decisions.
Reported neuron use is calculated from invocation input-token counts and the
manifest's pinned pricing conversion, not an account invoice or remaining quota.

## Recorded production baseline

[The recorded report](results-prod-2026-10-31.json) used deployed revision
`8614fdac7dec923134ea399864def4e86ebaf8d1` on 2026-10-31. All eight filters completed;
87 candidates received model decisions and one negative case was excluded during
lexical retrieval. All 108 requested images were delivered. There were no stale
candidates, provider failures or scan/candidate caps.

Against the provisional labels: **42 true positives, 38 true negatives, one false
positive and six false negatives** among model decisions; precision 97.7%, recall
87.5%. The unscored negative is one further correct pipeline exclusion.

| Intent | Model decisions | False matches | Missed matches |
| --- | ---: | ---: | ---: |
| Plain clocks | 8 | 0 | 0 |
| Plain lighting | 7 | 1 | 0 |
| River oil paintings | 12 | 0 | 1 |
| Food still lifes | 12 | 0 | 0 |
| Meissen vessels | 12 | 0 | 0 |
| French clocks | 12 | 0 | 0 |
| Chinese blue-and-white porcelain | 12 | 0 | 0 |
| Four-picture lots | 12 | 0 | 5 |

The lighting false positive is a candlestick design drawing; its negative label
assumes a search for physical lighting rather than depictions/designs. The single
river-painting miss is *Haverstraw Bay*, a Hudson River scene. Five of the six
all-water picture lots were missed; all six mixed lots were rejected. The lot cases
share a small pool of images, so their errors are correlated and do not establish
a general four-image failure rate. Review these cases in the gallery before
changing labels, instructions, model or threshold.

The run consumed 134,143 recorded input tokens, approximately 463.5 neurons at the
pinned conversion. Cleanup completed: 88 listings withdrawn, eight filters removed,
69 temporary image versions deleted, grants removed and Free tier restored. The
caller-owned test credential was separately revoked after final verification.

The owning operational contract remains the
[saved-search quality gate](../../../../docs/durable-worker-runbook.md#saved-search-batching-quality-gate).
