# CI inventory and measurements

[`integrate.yml`](../.github/workflows/integrate.yml) runs independent API, crawler,
integration and remaining test jobs, plus workspace lint. Every relevant pull
request and `develop` push runs all lanes. The final Sonar job requires every test
job and lint to succeed, validates the exact lane coverage artifacts, and waits for
the existing Sonar quality gate. No affected-package selector precedes tests.

## Inventory and parity contract

Run from the checkout with its pinned Rust toolchain and cargo-nextest installed:

```sh
python3 ci/inventory.py --output /tmp/ci-inventory --features
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 \
  python3 ci/inventory.py --output /tmp/ci-inventory --features --discover
```

The first command uses `cargo metadata --locked` to enumerate every workspace
package and Cargo target, including the root, nested proc macro, integration
binaries, lib/bin/example/bench targets, required features and doctest eligibility.
It compares the live set with [`test-lanes.json`](../ci/test-lanes.json). A removed,
added, reassigned, regated or disabled target fails the check. After reviewing an
intentional delta, accept it with `--write-manifest` and commit the manifest change
alongside the source change. Never use that flag in a parity gate. There is no
fallback lane for an unrecognized target in a gate run.

`--lane API|CRAWLER|INFRASTRUCTURE|REMAINING` also writes `lane-selection.json`
with package names, Cargo `--package` arguments, all owned target IDs and their
doctest subset. It validates the complete manifest before selecting a lane. API
and crawler retain exclusive package ownership; any other package with an
infrastructure-classified target belongs wholly to `INFRASTRUCTURE`. This keeps
mixed packages in one job while preserving each target's source classification
as review evidence. Empty lanes and duplicated or foreign target identities fail
instead of producing empty Cargo arguments that could select the root package.
The manifest's target annotations remain source-classification evidence; the
package projection owns execution. The runner family is explicit; each job retains
its own Cargo target directory and coverage cache, separated from lint.

[`run-test-lane.py`](../ci/run-test-lane.py) is the shared sequential lane driver.
It accepts repeated `--lane` selections
and requires explicit `--harness nextest|libtest`, `--slots`, `--profile`,
`--supervisor`, `--state-dir` and `--output` arguments. It refreshes and validates
compiled Nextest/libtest case parity before running the selected packages. A
pristine PostgreSQL bootstrap runs in a fresh environment; ordinary managed tests
and explicit doctests share the main environment; fixture lifecycle controls run
afterward with managed/Nextest identity cleared. `--list-only` compiles and verifies
ownership and case/ignored parity without starting services or running test bodies.
Discovery output retains selectors, compiled case lists and compiler diagnostics.
Coverage variables are inherited.
The fixture guide owns the [exact exceptional cases](../src/test-api/README.md#exceptional-execution-phases).
Stock libtest partitions crawler library `runtime::`/`logging::` cases using full
discovered names and executes that same set sequentially; `llm_runtime::` stays
ordinary. Harness, slots and supervisor profile are explicit inputs; the workflow
owns their selected values. Managed lanes enable the complete `test-api` recovery
feature union through dependency-qualified features without executing fixture test
targets in a second lane. Package-local and lane-selected feature graphs and case
sets must be compared when changing this configuration.

[`test-lane`](../.github/actions/test-lane/action.yml) configures coverage before
cache restoration and installs checksum-verified, pinned Nextest. Coverage jobs use
a dated nightly and its bundled LLVM tools; lint, compatibility and release builds
keep the stable repository toolchain.
The job supervisor owns service cleanup, including cancellation; no job prunes
shared Docker resources. Failure retains test/compile diagnostics and any coverage
produced without allowing a failed lane to pass the final gate.
Execution diagnostics upload before coverage export, and final diagnostics retain
their workflow attempt, including that attempt's LCOV when available. A rerun
replaces the stable required coverage artifact only; the final
gate still requires each lane's current result to succeed.
Each lane's `job.json` records the actual checkout SHA/tree, workflow SHA/ref,
CPU, memory, runner image/version and configured cache suffix/exact-hit result.
Resolved/fallback cache keys remain in the cache action log. Missing metadata is
reported as null; a runner label alone does not prove identical hardware.
Instrumented supervisor builds and tests both use debug level one, avoiding a
second dependency build caused only by different dev/test debug settings.

Test jobs reclaim only the unused Android SDK, .NET and CodeQL installations on
explicitly GitHub-hosted Linux runners. Local and self-hosted execution performs
no SDK cleanup. Disk observations stream and retain free bytes, owned Cargo/profile
sizes and hosted Docker storage summaries. A best-effort three-GiB floor cancels
through the same process owner, preserving time and space for cleanup/diagnostics;
it cannot guarantee capacity against a sudden large allocation. Directory/Docker
size sampling is less frequent and bounded, and stops below the floor; missing
observations remain null.
The same periodic record includes nullable host memory total/available KiB.

[`export-coverage.py`](../ci/export-coverage.py) uses compiled Nextest/Cargo
object metadata, including child binaries, the supervisor and repository
proc-macro shared libraries. Rustdoc retains
objects separately per crate so merged doctests cannot overwrite another crate's
executable. The driver requires every lane-owned doctest target to complete.
Fresh phase profile directories retain process/module identity. Compilation and
discovery profiles preserve macro-expansion coverage separately from test execution;
they cannot replace required execution profiles. Cached builds contribute only the
compiler work actually performed. Object paths, profiles and compiler versions
are retained with test diagnostics. Each Nextest invocation preserves its JUnit
report before the next phase, including failures; slowest cases and stage timings
remain alongside the phase logs. Cargo control outcomes remain in those logs.

Coverage preserves the Sonar source/exclusion policy. LLVM entry counters are
normalized to repository source paths; branch records use block zero and branch ID
`2 * condition_start_column + outcome` (`0` for true, `1` for false), preserving
condition identity when other conditions are absent from a lane build. Duplicate
source/line/branch records are unioned,
zero branch outcomes remain zero, and summaries are recomputed from the merged
entries. Ambiguous branch locations and invalid source lines fail export.
[`merge-coverage.py`](../ci/merge-coverage.py) requires the exact nonempty, valid
lane reports before emitting one deterministic source record per file. Missing
lanes, failed tests or invalid coverage fail mandatory CI before the existing
Sonar gate. Historical reports had no branch records, so they provide no numeric
branch baseline. Required lanes, the legacy scheduling control and layout
diagnostics use the same pinned nightly coverage configuration. Historical stable/
grcov measurements retain their original source/configuration and cannot establish
equivalence with this instrumentation. The original package baseline collector
remains available for historical diagnostics.

Current lanes, legacy controls and lane diagnostics use LLVM's `%b-%4m`
[online merge pool](https://clang.llvm.org/docs/SourceBasedCodeCoverage.html#running-the-instrumented-program):
up to four locked profile files per build ID/module signature within each fresh
phase or diagnostic output namespace. This limits file multiplication from separate
test processes; it does not bound the workspace's total profile bytes.

`--discover` refreshes `cargo nextest list --locked --workspace --all-features
--all-targets`, compares every case and ignored flag with each binary's libtest
`--list` and `--ignored --list`, then separately lists workspace doctests with
`cargo test --locked --workspace --all-features --doc -- --list`. Test bodies are
not executed by these listing commands. Custom harnesses require an explicit
adapter/review; an unsupported target is never treated as a zero-test binary.
Compilation and listing must succeed before counts are authoritative. A source-only
inventory uses null counts and `NOT_DISCOVERED`, rather than estimates.
Both complete and ignored doctest listings must announce every eligible target.
Ambiguous crate names fail until package-specific discovery can distinguish them.

Case names come from compiled harnesses, so parameterized cases remain distinct.
Ignored cases are retained. Platform-dependent and feature-dependent source
attributes are review evidence; compiled counts apply only to the recorded host,
toolchain and feature configuration. Other platforms need their own discovery.
Source subtree scans include sibling modules, inactive cfg branches and comments,
and can overclassify a target's fixture needs. Each package retains its own source
evidence and excludes subtrees owned by nested workspace members. Scans record
file/line evidence for the integration macro, direct serialization, testcontainers,
Postgres, persistent globals, external effects and cluster-wide DDL/replication.
These signals cannot prove absence of indirect or dynamically generated effects. In particular,
`aura_integration_test` expands to a serial test, and process globals/serial locks
do not coordinate separate nextest processes. Follow-on scheduling must review
the fixture boundary before enabling concurrency.

`--features` records `cargo tree --locked --all-features --edges normal,build,dev`
from the workspace and each package directory. The output lists features added by
workspace unification for every dependency in each package graph. Cargo metadata
alone is insufficient: it resolves the workspace even when invoked in a package
directory. The feature comparison unions host/build contexts by package/version;
it is a resolver view, not proof that differently compiled suites are equivalent.
The previous workflow ran package-directory `cargo test --all-features`; compiled
workspace discovery is explicitly marked `WORKSPACE_ALL_FEATURES`. Lane selection
must preserve the intended configurations and compare actual case sets.

Versioned artifact contracts live in [`ci/schemas`](../ci/schemas). Package/target
identity is `package::cargo-kind::target-name`; Cargo's canonical names and feature
identifiers are retained. `API`, `CRAWLER`, `INFRASTRUCTURE` and `REMAINING` describe
runner/fixture review ownership. All current runner assignments are Ubuntu.

## Managed fixture execution

The [fixture README](../src/test-api/README.md#isolated-test-contexts) owns context,
resource and shutdown requirements. Each ordinary selected suite runs under one job
supervisor with Nextest concurrency bounded by its slots. Compile mixed lanes with
the fixture recovery feature union; keep all targets discoverable and doctests in
their separate Cargo invocation.

The [exceptional phase contract](../src/test-api/README.md#exceptional-execution-phases)
owns exact pristine-bootstrap and unmanaged lifecycle selections. Exclude those cases
from ordinary managed commands, then execute their owning phases. Job-wide exclusive
leases coordinate global mutation across every group and invocation; a one-thread
group alone cannot provide that guarantee. Preserve ignored child probes and their
parent-driven executions in case accounting.

## Repeatable measurements

[`benchmark-lanes.yml`](../.github/workflows/benchmark-lanes.yml) is an opt-in
comparison of four lanes, three lanes (integration plus remaining), or one job.
Dispatch each layout at the same immutable source SHA, harness and slot count.
Start with one cold/warm diagnostic pair per layout, then repeat close contenders
or unstable configurations. Every layout executes the full owned
workspace, including exceptional phases and doctests. Each test job starts with
an empty Cargo target, then immediately repeats with that target; registry, Docker
images and OS caches are not claimed cold. Lint runs independently.
[`bench-test-lane.py`](../ci/bench-test-lane.py) reuses the baseline stage/RSS/disk
collector and current coverage export, retaining case lists, raw profiles, logs,
runner hardware/image, flags and image identities. It reports supervisor compilation
separately and includes startup/cleanup within test time. Successful Nextest output
is retained for fixture timing observations.
Compile and execution discovery metadata remain separate. Each invocation starts
with fresh profiles; in-job warm target reuse can skip macro expansion and therefore
cannot reproduce cold compilation counters. No profiles are replayed from the cold
run. Ordinary hosted dependency caches exclude workspace outputs by default.

Before this workflow is registered on the default branch, add `ci/lanes-four`,
`ci/lanes-three` or `ci/lanes-one` to a same-repository pull request. The guarded
label event checks out its actual head SHA and screens only that layout with
Nextest/four slots/FULL; its run number identifies the repetition. A `-libtest`
suffix selects the stock harness for an instrumented compatibility comparison.
Dispatch inputs
remain explicit for other configurations. Source SHA and workflow SHA are validated
and recorded separately. Labels do not start measurements for fork pull requests.

Small-change dispatches require a distinct `prime_revision` and scenario
`LEAF`, `API` or `FIXTURE`. Labels ending in `-leaf`, `-api` or `-fixture` instead
use the actual pull request base/head pair. The cold invocation runs the base;
the warm invocation checks out the head and reuses that exact target. Both run
the complete owned workspace. The recorded patch, source identities and cache
states must substantiate the scenario; labels do not manufacture source changes.
Use reviewed comment-only experiment heads when isolating compiler invalidation
without changing product cases. Same-revision FULL pairs retain unchanged-source
warm behavior. Base priming and head execution are different observations, not two
repetitions at the head. Collect the completed workflow envelope with
`collect-ci-run.py`; do not sum concurrent jobs. This opt-in workflow measures
through lint, artifact upload and coverage validation. Sonar scan/gate duration and
the previous per-package fanout need their own same-source workflow evidence before
claiming a complete CI speedup or selecting the final lane count.

The unused `ci-determinator` package and its three selector self-tests are retired.
Legacy replay must build the original tool and retain its selection/build scheduling
cost: source is available at `5c9bccac4e5c3d1e34ee021b7bbadcf78db592d4`.
[`compare-legacy-ci.yml`](../.github/workflows/compare-legacy-ci.yml) is an optional
same-tool control. `ci/legacy-full` on a same-repository PR builds that original
selector, compares against the recorded baseline, then runs its real package
fanout. `ci/legacy-affected` uses the actual PR base instead. Explicit dispatch
inputs are also available. The isolated selector workspace retains its original
source/manifests and locked versions; only unreachable lock nodes are pruned.
Its real release build/cache/selection precede test scheduling. Each selected
package uses libtest, package-local all-features, original `--nocapture` output,
lazy process fixtures and a separate Cargo target/cache. Complete controls add
the original root/proc-macro omissions to the existing `test-api` job explicitly,
whose all-features graph already includes their fixture/macro dependencies. These
targets share its nonempty coverage artifact; no empty root artifact or invented
counter is required. Affected controls retain the
actual original selection. Both preserve the required pristine/unmanaged phases
and validate compiled case/ignored lists and exact coverage artifacts.

This control deliberately uses the same verified prebuilt tools, pinned runner
family, four test threads, independent lint and waited Sonar gate as new CI.
It bounds fanout to twenty concurrent jobs. Case listing, explicit exceptional
phases and complete-coverage validation are parity adjustments. Historical
`cargo install grcov`, unpinned actions, runner allocation and global volume prune
are not reproduced; their costs are not measured by this control. Record actual
cache restores, hardware and all these adjustments with each comparison. A partial
affected run does not establish complete-corpus parity or equivalent full-workspace
speed. Keep literal historical evidence separate from the same-tool control.
All current controls/layouts use the same hosted SDK reclamation, disk guard and
aligned dev/test debug settings; include their cost and observed cache state in
the comparison.

[`benchmark-ci.yml`](../.github/workflows/benchmark-ci.yml) is deliberately opt-in.
Dispatch it with a full immutable SHA, or apply `ci/benchmark` to a same-repository
PR to bootstrap its first run before default-branch registration. It checks out
the PR's actual head SHA, rather than the synthetic merge ref. It runs two
independent repetitions for each scenario:

| Scenario | Package | Represents |
| --- | --- | --- |
| `LEAF` | `money` | Small leaf change |
| `API` | `aura-historia-api` | API package and its acceptance fixtures |
| `FIXTURE` | `test-api` | Shared fixture package change |

Each job measures an empty Cargo target directory, then immediately reuses that
directory for a warm measurement at the same revision and build flags. Both runs
use complete package `cargo test --locked --all-features -- --nocapture`, including
default Cargo test targets and doctests. Compilation/linking is measured first with
`--no-run`; the test stage can still include rustdoc compilation and Cargo's
up-to-date checks. Tests retain their normal fixture lifecycle and concurrency.

`COLD_TARGET` refers only to Cargo build outputs. Registry downloads, Docker images
and OS caches are not claimed cold. Fixture images are prefetched consistently
and their available repository digests/image IDs are recorded before and after.
References derive from fixture source; the requested OpenSearch engine determines
the corresponding image tag through a reviewed [Floci release mapping](https://github.com/floci-io/floci/blob/2.3.0/src/main/java/io/github/hectorvent/floci/services/opensearch/OpenSearchVersions.java).
An unreviewed Floci/engine combination fails closed. Missing image identity remains null. The image
snapshot is a cache observation, not proof that every listed image was used.
The runner image/version, CPU/memory, exact tool versions, build flags, SHA, dirty
state and run/job IDs are part of each record. GitHub's workflow ref and SHA also
identify the workflow definition when dispatching a different measured source SHA.
grcov is pinned and its downloaded archive checksum is verified; the test toolchain
comes from `rust-toolchain.toml`.
This historical [collector](../ci/collect-coverage.sh) uses one grcov parser thread
and one Rayon worker, which control separate worker pools. Its export logs record
GNU time's elapsed time and peak RSS.

To reproduce a pair locally, choose a new target directory and two new output
directories, retain the same checkout and flags, and install the workflow's grcov:

```sh
ci/bench-ci.sh --package money --scenario LEAF --repeat 1 \
  --cache COLD_TARGET --target-dir /tmp/ci-leaf-target --output /tmp/ci-leaf-cold
ci/bench-ci.sh --package money --scenario LEAF --repeat 1 \
  --cache WARM_TARGET --target-dir /tmp/ci-leaf-target --output /tmp/ci-leaf-warm
```

Repeat each pair with an independent empty target directory. Never delete shared
Docker resources to prepare a run. Use local isolated test fixtures and fake
credentials. The collector overrides the standard AWS credentials with test
values; it does not need deployed services. `--without-coverage` produces an
explicit uninstrumented diagnostic and cannot substitute for a coverage baseline.

The collector preserves command logs and separate compile/link, test/fixture and
coverage timings. GNU `time` reports maximum RSS for a command and its children,
not aggregate simultaneous process memory; interrupted commands can lack it.
Logical target size is sampled at
most every ten seconds and before/after each stage; brief peaks, profile output
and Docker storage are outside that sampled size. Sampling itself adds filesystem
work, so use the same collection policy for both sides of a comparison.

`AURA_CI_BENCHMARK_TIMING=1` enables best-effort, prefixed JSON observations on
stderr at existing fixture readiness/reset and macro setup/body/teardown
boundaries. This bypasses the fixture's info-level tracing subscriber without
changing it. Default test runs emit no new timing records. Both comparison sides
must contain and enable the same instrumentation. It adds timing/logging overhead;
phase records are measurements of the instrumented run. Setup includes nested
Postgres/OpenSearch startup/migrations; teardown includes legacy resets.
`CONTEXT_ACQUIRE`, `POSTGRES_SLOT_RESET` and `CONTEXT_CLEANUP` expose lease
waiting/recovery, baseline preparation/cloning, and owned shutdown/resource removal;
see the [context lifecycle contract](../src/test-api/README.md#isolated-test-contexts).
All remain inside full suite/workflow time. These durations overlap and must not be summed as wall time. Body records exclude macro setup and
teardown, but include helpers called by the test. Failed/cancelled phases can lack
a completion record. Ordinary libtest tests have no separate body measurement.
Normal timing records are written as complete lines in one buffer; recognized
records with malformed payloads fail collection instead of silently losing samples.

Coverage artifacts retain raw profiles and LCOV. The JSON reports both LCOV record
sums and a file/line/function/branch identity union. The union does not reconcile
different compiler branch IDs and is not a Sonar coverage total. Missing metrics
are null or explicitly unobserved; they are never replaced with zero.
Collected LCOV must have complete source records and grcov summaries matching the
emitted entries; truncated or inconsistent output fails the measurement.

After a workflow finishes, collect its full scheduling/job/step envelope:

```sh
python3 ci/collect-ci-run.py RUN_ID --output /tmp/baseline/workflow.json
gh run download RUN_ID --pattern 'baseline-*' --dir /tmp/baseline/artifacts
```

The completed-run collector records the actual run attempt. Workflow duration uses
the attempt endpoint's completed `updated_at` minus `created_at`, including
scheduling and finalization. The run endpoint retains the first creation time after
a rerun and cannot measure a later attempt. Job scheduling delay is measured only
when the API provides a job creation timestamp; workflow creation is never
substituted. Per-package local
measurement duration excludes scheduling and artifact upload. Keep the workflow
envelope with all six job artifacts to make the baseline independently auditable.
Missing or reversed API timestamp intervals remain null with raw timestamps
preserved. Reruns can reuse successful jobs from an earlier attempt and are
diagnostic snapshots, not fresh full-suite performance repetitions. Use fresh run
IDs for controlled repetitions.

For a same-revision old/new comparison, pin both implementations to the same source
SHA/test corpus, toolchain, flags, runner/image policy and instrumentation; collect
the same case sets and coverage identity totals, then compare repeated cold and
warm stage distributions separately. Do not compare a cached compile against an
uncached one or sum concurrent matrix job durations as workflow wall time.
These three package scenarios do not represent the full affected-workspace fanout
of a fixture change. An equivalent complete old/new workflow comparison remains
required before claiming end-to-end CI improvements.

## Existing omissions and baseline evidence

The previous determinator excluded the root and nested `test-api-macros` packages.
Lane ownership includes both. Package `cargo test` invocations include doctests;
the driver preserves them separately because Nextest alone omits them. The old
workflow concatenated repeated dependency source records for Sonar.
The baseline collector retains those original sums and identity unions; the native
coverage path described above emits a normalized union.

Historical [run 2579](https://github.com/aura-historia/backend/actions/runs/39579788255)
spanned 1,580 seconds from creation to the last job completion and had 110 test
jobs. Its API acceptance binary ran 370 tests
in 513.63 seconds. The actual checkout was merge commit
`cd92443039846187a96ae0113d7b991aeeb4127d`, distinct from the run's reported PR head.
Its logs record 5,543 passed and five ignored cases, grcov 0.10.7, mixed runner
image versions, 94 exact Rust-cache restores and 16 restore misses. Retained LCOV
contains repeated sources and no branch records despite `--branch`. Fixture phase
timings, peak RSS and disk were not recorded and cannot be reconstructed.
Historical and current source revisions differ; their case counts do not establish
coverage parity or a speedup. Preserve original run metadata, logs, artifacts and
their hashes alongside the controlled measurements; do not treat this historical
run as a repeated cold/warm pair.
The checked-in [reference observations](../ci/baselines/reference-2579.json)
retain the selected package stage data, coverage totals, missing values and source
artifact hashes without embedding the original job logs in Git.

## Job infrastructure supervisor

The [fixture developer guide](../src/test-api/README.md#job-supervisor) owns the
versioned managed environment contract, profiles, credential policy, ownership,
signal/GC recovery and real-service smoke command. This infrastructure can be used
with any child runner. Its slot count describes capacity; it does not make legacy
global-reset fixtures safe to run concurrently. CI runner selection and state
isolation are separate follow-on changes. Supervisor diagnostics use their own
`AURA_TEST_ENV` record prefix and do not alter the benchmark timing schema.
