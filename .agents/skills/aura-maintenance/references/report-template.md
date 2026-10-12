# Maintenance report

Use this structure in the conversation; do not create a report file without approval.
Replace explanatory placeholders with evidence or an explicit unknown/not-run statement.
This is a format guide, not a pre-completed report.

## Run

Record UTC time, repository, branch/HEAD, dirty state, declared scope/focus, resolved base
and selection method, mode/execution permissions, current documents read and reviewer type.
Choose status: `completed_no_findings`, `completed_with_findings`, `partial`, `blocked`.
Explain the status against the declared scope, not against an imagined entire audit.

## Coverage

| Lane/context | Paths and flows actually inspected | Evidence/checks | Not inspected / blockers |
| --- | --- | --- | --- |

Account for all requested lanes. Distinguish exhaustive inventory from sampled deep review.

## Findings

For each: fingerprint; category; P0-P3 severity; high/medium/low confidence with rationale;
state; verification verdict; exact file/lines and symbol; normative source/section;
reachable input/state path; expected vs actual; impact/preconditions; refutation attempt;
minimal reproduction or decisive static proof; proposed smallest remedy; risks and tests.
Do not include secret values or sensitive raw payloads. Put unproven candidates separately.

## Prior findings and acceptances

| Prior fingerprint | Rechecked? | Current state | Evidence or reason still unresolved | Owner/review date if approved |
| --- | --- | --- | --- | --- |

Retain out-of-scope items; absence is not fixed. Disclose expired or missing acceptances.

## Changes and verification

No changes in audit mode. For repair: root causes fixed, production additions/deletions,
test lines, before/after regression results, equivalence arguments and complete diff review.
Verifier verdict per patch: verified/refuted/needs_more_evidence/requires_design_review.
Say whether reviewer was truly independent or a same-agent second pass.

| Exact command / static check | Tool version | passed/failed/blocked/not_run | Observed evidence / reason |
| --- | --- | --- | --- |

## Remaining work

Unresolved risks, environmental unknowns, omitted lanes, next manual focus and exact resume
point. Do not publish automatically or mark an unexecuted test/client as passed.
