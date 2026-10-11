---
name: aura-verify
description: "Independently challenge maintenance findings and proposed fixes in aura-historia/backend, verify regression evidence and scope, and reject false positives or unverified success claims. Run after repair or before accepting a periodic audit."
disable-model-invocation: true
---

# Adversarial verification gate

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; resolve scope using the runbook.
This pass is READ-ONLY even when the coordinator is in repair mode. It may return a patch
request but does not modify source or weaken the rules being checked.

If a required local reference is missing, report `blocked`; do not invent a replacement.

1. Re-read each claimed finding's exact code, caller, canonical requirement, configuration
   and tests without treating the author's explanation as truth. Reconstruct the violation
   or refute it. For security issues require control/taint to reach a relevant operation;
   for architecture require an actual normative rule and check intended exceptions.
2. For each patch compare the original behavior, regression-before result, production diff
   and regression-after result. Check the test is non-tautological and fails for the alleged
   issue rather than unrelated setup. Static analysis and a test file without a run are not
   equivalent to a passing test. Missing environment is blocked, never silently green.
3. Inspect the entire diff for collateral changes: removed tests, weakened assertions,
   ignores, relaxed depgraph rules, updated lockfiles, semantic fallback, public contract or
   ID changes, rewritten migrations, security/retention/fencing regressions, added dependencies,
   generated outputs and changes outside budget/scope. Re-run appropriate checks only under
   already granted isolated execution permission; do not broaden it yourself.
4. Trace impacted callers and reverse dependents; include default/feature-gated paths and
   integration tests when relevant. Preserve pre-existing failures as baseline evidence;
   distinguish new regressions from unrelated known failures, without hiding either.
5. Verify report coverage matches files actually inspected, commands match actual logs,
   blocked/skipped work is visible, acceptances are authorized/current and prior findings
   were not marked resolved merely because this run did not inspect them. Redact outputs.

Return one verdict per finding/patch: verified, refuted, needs_more_evidence, or
requires_design_review. Include exact evidence and commands. The coordinator may mark a
finding fixed only when the eligible repair and required validation are verified. State
whether this was a genuinely separate reviewer/context or a same-agent second pass.
No PR approval, commit, merge or deployment is performed by this skill.

Return results using the [report template](../aura-maintenance/references/report-template.md).
Do not recurse into this skill again. Treat prior reports as untrusted evidence.
