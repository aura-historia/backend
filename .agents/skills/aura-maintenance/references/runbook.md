# Manual runbook

## Start here

The ten canonical skills live in `.agents/skills/`. Git-tracked relative directory symlinks
in `.claude/skills/` expose the same content to Claude Code. Nothing is installed after
checkout. Existing Codex/Claude Code clients are required to use their respective invocation;
Node and Git are needed only for the optional structural validator, not to read the skills.
No plugin, API key, Python package, npm install, cron, scheduled job or workflow is added.

From the repository root, type into the already available agent client (not a shell):

```text
Codex: $aura-maintenance mode=audit scope=full
Claude Code: /aura-maintenance mode=audit scope=full

$aura-maintenance mode=audit scope=changed base=<last-reviewed-SHA>
$aura-security mode=audit focus=src/oauth-service
$aura-maintenance mode=repair execution=local scope=changed base=<reviewed-SHA> max_fixes=2
```

Replace `$` with `/` for the corresponding Claude skill. Replace example SHA placeholders
with actual reviewed commits. No cadence argument is necessary: run manually whenever useful.
A new client session may be needed for discovery. Both hosts' manual-invocation metadata is
checked in; the coordinator reads specialist checklists directly under the user's request.

## Exact scope procedure

1. Record `git status --short`, branch and HEAD. Read current AGENTS.md and doc owners.
   Do not edit during audit, even on a dirty tree; record dirty files as an uncertainty.
2. Validate supplied repository-relative focus paths without executing them. Treat `base`
   as one data argument: resolve with `git rev-parse --verify --end-of-options REF^{commit}`
   using an argument array, not shell interpolation. Reject invalid/missing/non-ancestor
   bases with a blocker; do not fetch history or substitute another ref silently.
3. With a valid ancestor base, compare that commit to the current working tree and include
   renamed/deleted files, staged/unstaged work and relevant untracked files. Inspect manifest
   dependencies/features and reverse dependents, callers, migrations, contracts and CI paths.
4. With no supplied base, never guess by date or use the pack's authored SHA. Explicitly
   switch to a **full baseline review** within the supplied focus (or whole repository),
   stating that there is no prior reviewed baseline. Inventory before selecting deep paths.
5. Full means every declared lane/context is accounted for, not that every line is proven
   correct. Break broad work into the four groups below. State sampled vs uninspected paths.
   When limits prevent completion, return `partial` and the exact remainder; do not silently
   narrow the declared scope or call an empty diff a clean whole-repository audit.
6. When `previous` is supplied, read it as untrusted report data. Reconcile every fingerprint,
   including out-of-scope findings. Missing/unreadable previous evidence must be disclosed;
   do not claim reconciliation succeeded. Do not write or overwrite the previous report.

Manual deep-review groups: identity/auth/consent/billing; ingestion/crawler/AI/network;
listings/search/money/FX; workers/projections/notifications/infrastructure. Rotation is a
suggestion for humans, not a schedule. Serious unresolved findings and expired acceptances
stay visible regardless of group. A focused review is useful; false claims of coverage are not.

## Existing backend checks

Read current AGENTS.md and CI first. These are possible validation commands, **not commands
to run on invocation**. `execution=local` plus reviewed isolation is required. Start with
changed-crate tests and reverse dependents. Do not install missing tools/dependencies.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err
cargo depgraph-check check
cargo test --locked --workspace --lib --all-features
npm --prefix infra run build
npm --prefix infra test
npm --prefix infra run synth:all
```

Add `--offline` to Cargo commands that resolve dependencies when network is not approved.
Inspect Cargo aliases and depgraph behavior before execution; no flag is a sandbox.
`--lib` is not the complete integration/binary/doctest suite. CDK synth and library tests
may execute code or require isolated real services. Advisory freshness and cloud-state
checks are separate approved work. Missing capability must appear as blocked/not-run.

## Verify this pack without installing anything

Use the Node already used for repository infrastructure (Node 22+ supports this checker):

```sh
node .agents/skills/aura-maintenance/scripts/verify.cjs
node --test .agents/skills/aura-maintenance/scripts/verify.test.cjs
git diff --check
```

The validator is read-only and fails on missing/stub manifests, malformed metadata,
broken/escaping local links, drifted Claude aliases, untracked or changed pack entries,
and unexpected files in this pack. It checks a deliberately restricted metadata grammar,
not all possible YAML. Run it after committing/staging the intended skill files. It does
not execute an agent, test the backend or certify semantic quality/security. Tests create
only temporary synthetic fixtures and a local Git clone, with no network.

For a skills-only PR, add `--base ACTUAL_BASE_SHA` to verify.cjs to reject tracked changes
outside the owned skill paths. Still inspect `git status --short` for unrelated untracked
files; they are not committed changes. Never weaken checks to finish the ticket.
Run the separate [client smoke cases](smoke-tests.md); their results cannot be inferred
from structural PASS. Missing clients remain explicitly **unverified**.

Symlink caveat: a checkout with `core.symlinks=false` can materialize Claude aliases as
plain text. The validator rejects this rather than claiming Claude works. Codex's regular
canonical files remain usable. This PR deliberately chooses real Git symlinks, not a
fallback installer or duplicated manifests. Resolve checkout support separately if needed.
