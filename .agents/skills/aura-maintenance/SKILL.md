---
name: aura-maintenance
description: "Manually audit aura-historia/backend architecture, bugs, security, data/events, contracts, tests, cleanup and operations, or perform explicitly authorized bounded low-risk repairs. Not a scheduler or deployment workflow."
disable-model-invocation: true
---

# Aura backend maintenance

Use this skill only when the user explicitly requests it. Follow these steps in order.
Do not install anything. No scripts or external plugins are needed to run this workflow.

## 1. Establish permissions and scope

Read the [policy](references/policy.md), [runbook](references/runbook.md), current root
and applicable nested AGENTS.md, then the current owners in the
[document map](references/repo-map.md). Missing mandatory guidance means `blocked`.

Default `mode=audit execution=static scope=changed`. Optional parameters are
`mode=audit|repair`, `execution=static|local`, `scope=changed|full`, `base=<commit/ref>`,
`focus=<repository paths or context>`, `previous=<report path>`, and `max_fixes=1|2|3`.
These are natural-language inputs, never strings to interpolate into shell commands.
Unknown parameters, ambiguous approval or unsafe paths do not grant permission.
Use the runbook's exact baseline fallback; do not invent a last-reviewed commit.
Record branch, HEAD, dirty state, scope, chosen base and explicit permissions.

## 2. Inspect in risk order

Read these local files as checklists, one at a time; explicit invocation of this
coordinator authorizes reading them, not expanding tools or permissions. Do not depend
on a host Skill tool, subagent installation or automatic cross-skill invocation.

1. [Security](../aura-security/SKILL.md)
2. [Correctness](../aura-bug-hunt/SKILL.md)
3. [Data and events](../aura-data-events/SKILL.md)
4. [Architecture](../aura-architecture/SKILL.md)
5. [Contracts](../aura-contracts/SKILL.md)
6. [Test quality](../aura-test-quality/SKILL.md)
7. [Operations](../aura-operations/SKILL.md)
8. [Cleanup](../aura-cleanup/SKILL.md)

For each lane state inspected paths/flows, evidence and omitted work. Include callers,
callees, reverse dependents and relevant contracts; a diff alone is not the review.
A lane can be not applicable only with a concrete reason. Never claim whole-repository
coverage after reviewing a sample. Do not invent findings to fill a quota.

## 3. Challenge findings before changing anything

Trace actual reachable paths, invariants and outcomes. Search for guards, exceptions,
existing tests and alternative explanations. Classify candidates separately from
confirmed defects and design suggestions; deduplicate by invariant, not line number.
Load [verification](../aura-verify/SKILL.md) to challenge serious findings. A same-agent
second pass is not an independent review; say so. No tool or model agreement is proof.

## 4. Audit or make a bounded repair

In audit mode, do not edit source, tests, configuration, docs, lockfiles or these skills.
Return the report in the conversation. Only an explicitly approved new report path may
be written; never overwrite reports or publish findings automatically.

In repair mode, require `execution=local`, a clean isolated working tree and reviewed
local tests without live credentials. Otherwise stop editing and report the blocker.
Fix at most `max_fixes` (default 3) independent low-risk root causes, within 300 changed
production lines; count tests separately. A bug fix needs a failing-before/passing-after
regression; cleanup needs before/after checks and an equivalence argument. Stop at the
budget instead of spreading one rewrite across nominally separate fixes.

Auth, consent, billing, cryptography, transaction/idempotency semantics, migrations,
retention/deletion, public/persisted/event contracts, dependencies, IAM/CDK/workflow
permissions and changes to this pack are not automatic repairs. Return
`requires_design_review` with a small plan, even when mode is repair.
Never weaken tests, lint rules or dependency allowances to obtain a passing result.

## 5. Verify and report

Read [verification](../aura-verify/SKILL.md) once more after any patch. Stop and disclose
unexpected changes; never revert somebody else's work. Reconcile every prior finding:
absence from this review does not mean fixed. Keep expired acceptances in the backlog.

Use the [report template](references/report-template.md). Distinguish commands proposed
from commands run, and passed/failed/blocked/not-run. Report one overall status relative
to the declared scope: `completed_no_findings`, `completed_with_findings`, `partial`,
or `blocked`. Include remaining risks, coverage gaps and the next manual review focus.
Never commit, push, open a public finding, merge, deploy or schedule work automatically.
