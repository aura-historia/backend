# CI inventory and measurements

[`integrate.yml`](../.github/workflows/integrate.yml) runs four independent API,
crawler, integration and remaining test jobs with Nextest and four slots, plus
workspace lint. Every relevant pull request and `develop` push runs all lanes.
The final quality job requires every test job and lint to succeed, including API
acceptance tests. No affected-package selector precedes tests.

Required CI and the matched lane/legacy controls use `coverage: none`, preserving
the full test and lint scope while omitting LLVM instrumentation, capture, export
and Sonar analysis. Test jobs skip SDK reclamation; workspace lint retains its
existing checks, toolchain, cache and preparation policy.

Each ordinary test job resets its separate `target/no-coverage` directory and
then restores dependencies through the existing pinned cache action. Its validated
`compilation_cache: dependencies|cold` input defaults to `dependencies`; ordinary
native callers select it explicitly and legacy test controls select `cold`, which
disables restore and save. Saved targets exclude workspace libraries and test
executables. Registry/Git dependency data and compiled external dependencies can
be reused; every workspace suite still compiles, discovers and executes normally.
Lane measurements own their separate empty/reused target policy.

Cache keys retain lane, coverage, compiler/environment and Cargo input separation,
plus a bounded hash of execution policy files. Preserve the automatic environment
hash, including installed compiler inventory. A reset before restoration does
not establish a cold build: count it cold only when logs show no restored target,
including fallback restores. A false exact-hit result alone can mean a miss,
fallback or cache error. Retain resolved keys, compressed size and restore/save
duration; upload/post-save time belongs in workflow completion. Cache capacity,
eviction and the resulting time savings require observations.

The shared action retains explicit optional `coverage: llvm` compatibility and
its existing LLVM artifact gates; its coverage default remains `llvm` so callers
must select the adopted policy explicitly.

## Performance evidence

The benchmark corpus at `b9637f605`, preserved at optimized source
[`080e04c18`](https://github.com/aura-historia/backend/commit/080e04c18e6418cf880da49dcc3e4b6c5025cc67),
contains 112 packages, 288 targets, 5,742 declared cases (5,724 active and 18
ignored) and 12 actual doctests. Two optimized cold observations
([18m03s](https://github.com/aura-historia/backend/actions/runs/39840169811),
[17m35s](https://github.com/aura-historia/backend/actions/runs/39840203207)) passed
the full corpus, lint and their actual quality gates. Their median time from
workflow creation to successful quality was 17m49s. Two equivalent full legacy
observations
([20m26s](https://github.com/aura-historia/backend/actions/runs/39832788590),
[18m54s](https://github.com/aura-historia/backend/actions/runs/39835851229)) had a
19m40s median. The observed 1m51s (9.4%) reduction passes the recorded median gate;
the aspiration to finish under 15 minutes remains unmet. Both comparison cohorts
contain two observations; the bounded legacy cohort was fixed before reading its second
outcome.

Product source, product tests, Cargo/fixture inputs, active compiler and profiles
match the frozen benchmark source; reviewed CI orchestration differs. Actual
checkout trees, case/ignored/doctest identities and outcomes, feature selections,
fixture identities and cleanup were audited. These timings belong to that corpus.
Upstream develop adds 169 and removes 35 crawler cases (net +134); combined source
requires its own applicable CI proof.

All four native caches were real misses without fallback in each cold observation.
Successful saves totalled approximately 3.93 GB compressed per observation;
Integration post-save took 19.772s and 19.489s, included in quality latency.
The [separate cache observation on identical source](https://github.com/aura-historia/backend/actions/runs/39843281077)
is excluded from the cold median. Assess reuse and its cost from that run's actual
restore/save records before attributing a benefit. Same-job reuse of a complete
target directory does not predict dependency-cache savings.

The earlier native cohort
([1](https://github.com/aura-historia/backend/actions/runs/39810496299),
[2](https://github.com/aura-historia/backend/actions/runs/39815814790),
[3](https://github.com/aura-historia/backend/actions/runs/39815802654)) had a 22m02s
median and failed the same gate before optimization. The
[release-contended full control](https://github.com/aura-historia/backend/actions/runs/39823462369)
passed in 36m06s but was excluded before its outcome because an unrelated release
occupied 17 of 20 assigned runner slots. Its queue delay cannot establish an
architecture speedup.

The historical small-source observations also use the previous cold native policy:

| Small source change | Ordinary four-lane latency | Affected legacy latency | Legacy scope: packages / active cases / doctests | Occupied runner seconds: ordinary / legacy |
| --- | --- | --- | --- | --- |
| Leaf | [20m58s](https://github.com/aura-historia/backend/actions/runs/39817420015) | [8m02s](https://github.com/aura-historia/backend/actions/runs/39827788662) | 1 / 7 / 0 | 3,641 / 709 |
| API | [20m28s](https://github.com/aura-historia/backend/actions/runs/39817437881) | [17m23s](https://github.com/aura-historia/backend/actions/runs/39828063290) | 1 / 941 / 0 | 3,713 / 1,385 |
| Shared fixture | [22m33s](https://github.com/aura-historia/backend/actions/runs/39817920654) | [18m50s](https://github.com/aura-historia/backend/actions/runs/39829846446) | 37 / 3,065 / 1 | 3,747 / 10,543 |

The full-workspace figures above measure Integrate workflow creation through its
successful quality gate. The small scenario latencies span the earliest applicable
workflow creation through the last applicable successful check, including lint,
artifact upload and the gate. Their ordinary envelopes include applicable
Container checks and runner time; affected legacy includes its selector, lint,
selected packages and gate. Occupied runner time sums setup and poststeps while
excluding queues. Keep it separate from latency and billing cost; incomplete job
clocks cannot establish an aggregate or an aggregate percentage reduction.

Comparisons must verify actual checkout trees and source deltas, product source,
product tests, Cargo inputs, compiler flags and preparation policy, and record
workflow/adapter revisions separately.
The small ordinary heads use base `b9637f605`; affected control heads use base
`5fb1053d5`, containing the
[singleton-artifact adapter fix](https://github.com/aura-historia/backend/commit/5fb1053d5757c982015893ab93cbcf138f2c7450).
The exact comment patch, product source, product tests, fixtures, Cargo inputs
and build configuration match per scenario; five reviewed measurement adapter
files differ. All three corrected affected controls passed their actual selector,
lint, package and strict execution-artifact gates.
Each row records one cold observation per side of a reviewed comment-only source
change. It measures compiler invalidation; feature costs and warm behavior remain
unmeasured. Ordinary CI executes the full workspace; affected legacy executes its
selected subset above. CPU, image and queue conditions can differ. The original
[leaf](https://github.com/aura-historia/backend/actions/runs/39818361734) and
[API](https://github.com/aura-historia/backend/actions/runs/39818693260) controls
failed their artifact gate after their selected tests passed. Those failures are
retained and contribute no successful latency.

Four lanes remain selected: the
[three-lane](https://github.com/aura-historia/backend/actions/runs/39810662128) and
[four-lane](https://github.com/aura-historia/backend/actions/runs/39810670043)
cold critical-path ranking reversed on repeat, and three lanes had less disk
margin without a repeatable critical-path win. Stock libtest showed no demonstrated
material advantage. Hosted CPU/image variation limits causal attribution; an
in-job warm pass is reuse evidence, not an independent repetition. Diagnostic
per-pass maxima and two-pass workflow envelopes are separate from ordinary CI
creation-to-last-applicable-check latency. Concurrent job times must not be summed
as latency. Native samples also retain their observed lint cache state and
concurrent workflow activity.

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
its own Cargo target directory, separated from lint and instrumented caches.

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

[`test-lane`](../.github/actions/test-lane/action.yml) validates `coverage: llvm|none`
before execution and installs checksum-verified, pinned Nextest. Both modes use
the same dated nightly, bundled LLVM tools, dev/test debug level one and existing
build/test concurrency. The `none` mode clears coverage flags, encoded flag
overrides and profile/capture variables, and selects normal rustdoc. The driver
keeps its ownership checks, compiled case lists, actual doctests, exceptional
phases and JUnit without collecting coverage metadata. Lint and release builds
keep the stable repository toolchain.
Native no-coverage selections owning `INFRASTRUCTURE` use the supervisor compiled
by lane discovery, which requires one runnable executable at the requested path
before starting fixtures. Other selections, legacy controls and LLVM execution
retain an explicit supervisor build when required.
The job supervisor owns service cleanup, including cancellation; no job prunes
shared Docker resources. Failure retains test/compile diagnostics and any coverage
produced without allowing a failed lane to pass the final gate.
Execution diagnostics upload before optional coverage export, and final diagnostics
retain their workflow attempt, including that attempt's LCOV when enabled. An
LLVM rerun replaces the stable coverage artifact; the final gate still requires
each lane's current result to succeed.
Each lane's `job.json` records the actual checkout SHA/tree, workflow SHA/ref,
CPU, memory, runner image/version, selected compilation-cache policy and configured
cache suffix/exact-hit result.
It also records the coverage mode, target path, encoded flags, whether cache
restoration was attempted, its result and whether the target was reset before
restoration. Resolved/fallback cache keys and restore/save diagnostics remain in
the cache action log. Missing metadata is reported as null; a runner label alone
does not prove identical hardware.
After execution, `fixture-images.json` records available fixture image IDs and
repository digests through bounded, best-effort inspection of the reviewed
references; unavailable identities are null and policy/read failures retain a
diagnostic before the execution artifact uploads.
Supervisor builds and tests both use debug level one, avoiding a
second dependency build caused only by different dev/test debug settings.
Native lanes, legacy controls and layout measurements set the lazy PostgreSQL
fixture override to the managed supervisor's immutable image before execution
and image prefetch, so these paths use the same PostgreSQL image policy.

No-coverage test/control jobs and the small legacy selector skip SDK deletion;
preparation retains real initial/final disk capacity. Optional LLVM jobs retain
the existing Android SDK, .NET and CodeQL reclamation only on explicitly
GitHub-hosted Linux runners. Local and self-hosted execution performs no SDK cleanup.
The two optimized cold Integration observations sampled minimum free disk of
21.93–23.12 GB without SDK reclamation or a guard breach; the three-/four-lane
diagnostics sampled 17.03/20.48 GB respectively.
The [one-job layout](https://github.com/aura-historia/backend/actions/runs/39804553558)
crossed the three-GiB floor during a narrowed dependency build and was cancelled
before completing the corpus. Owned fixture cleanup completed. The layout is
rejected under this policy, without falling back to SDK deletion or retry. Samples
are observations, not guaranteed instantaneous minima or future capacity guarantees.
Disk observations stream and retain free bytes, owned Cargo/profile
sizes and hosted Docker storage summaries. A best-effort three-GiB floor cancels
through the same process owner, preserving time and space for cleanup/diagnostics;
it cannot guarantee capacity against a sudden large allocation. Directory/Docker
size sampling is less frequent and bounded, and stops below the floor; missing
observations remain null.
The same periodic record includes nullable host memory total/available KiB.

The following coverage contracts apply only to optional LLVM execution.
[`export-coverage.py`](../ci/export-coverage.py) selects Nextest test executable
paths, Cargo `compiler-artifact.executable` paths (including child binaries and
the supervisor), and Cargo-recorded repository proc-macro shared libraries.
Every Cargo-backed execution phase records its own successful build metadata,
including narrower exceptional selections and doctest dependency builds. The
driver's `coverage-artifacts.json` lists every required phase stream; missing or
incomplete streams fail export. A rerun invalidates its previous capture before
compiling. Raw phase stdout bytes are retained and replayed after the command;
invalid text is replaced only for console display. Stderr remains live, and
stdout replay failures preserve an existing child failure or cancellation.
Cargo-recorded non-test executables are copied
before another phase can replace their stable paths. Metadata and the object
manifest retain the original and captured paths and content hash; required export
uses the copies. Identical executable bytes share one retained copy under the
invocation's coverage directory, outside Cargo caches and ordinary artifact uploads.
These copies add local disk usage; benchmark diagnostics retain them with the raw
profiles for replay. The free-space floor covers their filesystem; target/profile
directory size samples exclude the copies.
Nextest's `non-test-binaries` runtime/archive metadata can also list Rust metadata
and archives as `dylib`; Cargo owns executable and proc-macro object selection.
Rustdoc retains
objects separately per crate so merged doctests cannot overwrite another crate's
executable. The driver requires every lane-owned doctest target to complete.
LLVM-enabled lanes and diagnostics use fresh phase profile
directories with binary/module identity and LLVM's `%b-%4m`
[online merge pool](https://clang.llvm.org/docs/SourceBasedCodeCoverage.html#running-the-instrumented-program).
Up to four locked files per build ID/module signature limit profile multiplication
from test processes within each phase; they do not bound total workspace bytes.
Compilation and discovery profiles preserve macro-expansion coverage separately from test execution;
they cannot replace required execution profiles. Cached builds contribute only the
compiler work actually performed. Object paths, profiles and compiler versions
are retained with test diagnostics. Every required export relays raw stderr to the
job log; each successful export also saves it in the object manifest's
`export_diagnostics`, keyed by LLVM format (`lcov` or `text` for JSON).
The optional mismatch report is off by default. An explicit
`--mismatch-diagnostics report` runs one diagnostic
`llvm-cov report --dump --num-threads=1` after both strict exports succeed, if either
warns about mismatched data. It uses the same objects and merged profile, discards stdout,
and retains raw stderr and exit status in `mismatch_diagnostics` and the job log.
Diagnostic failures do not replace export outcomes. Mismatch counts describe skipped
mapping records; raw function names and expected hashes require corroboration before
source attribution. Attribution applies only to the observed source/profile vector;
different cold/warm compilation can produce different unused copies.
Each Nextest invocation preserves its JUnit
report before the next phase, including failures; slowest cases and stage timings
remain alongside the phase logs. Cargo control outcomes remain in those logs.

Coverage preserves the Sonar source/exclusion policy. LLVM entry counters are
normalized to repository source paths; branch records use block zero and branch ID
`2 * condition_start_column + outcome` (`0` for true, `1` for false), preserving
condition identity when other conditions are absent from a lane build. Function
names and entry counts come from LLVM JSON because the pinned LLVM LCOV writer
crashes on functions with branch mappings but no code regions. Ordinary function
locations use the first code region; a branch-only function uses its earliest
recorded condition as a source anchor when all branch mappings belong to its owned
primary file. A record with no code or branch mapping is omitted from LCOV only
when its entry count is zero; diagnostics retain its exact name, primary filename,
count and reason. Nonzero unmapped records and ambiguous anchors fail export.
Repeated records for the same source condition (including identical macro-generated
evaluation paths within a function) are unioned; a shared start with a different
end or region kind fails. Zero branch outcomes remain zero, and summaries are recomputed from the merged
entries. Ambiguous branch locations and invalid source lines fail export.
[`merge-coverage.py`](../ci/merge-coverage.py) requires the exact nonempty, valid
lane reports before emitting one deterministic source record per file. Missing
lanes, failed tests or invalid coverage fail optional LLVM-enabled controls.
Historical reports had no branch records, so they provide no numeric
branch baseline. Optional LLVM action/helper invocations use the same pinned
nightly coverage configuration. Historical stable/
grcov measurements retain their original source/configuration and cannot establish
equivalence with this instrumentation. The original package baseline collector
remains available for historical diagnostics.

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
`aura_integration_test` in legacy mode expands to a serial test; isolated and
exclusive modes do not add that serial wrapper. Process globals/serial locks do
not coordinate separate Nextest processes. Scheduling changes must review the
fixture boundary before enabling concurrency.

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

## Adding tests and local feedback

Keep tests with their owning crate and follow the
[test placement rules](arch.md#20-testing-strategy). Start with the smallest useful
package/target selection; a fixture-free library example is:

```sh
cargo test --locked --package money --all-features --lib
```

For managed consumer tests, use the [supervisor commands](../src/test-api/README.md#job-supervisor)
and [isolated context contract](../src/test-api/README.md#isolated-test-contexts),
with Nextest threads no higher than supervisor slots. Keep pristine and unmanaged
controls in their [exceptional phases](../src/test-api/README.md#exceptional-execution-phases).
The lane driver reproduces complete lane execution; its `--list-only` mode checks
compiled ownership and case parity before starting fixtures.

For a new or changed target, review its source-classification evidence and the
[package ownership rules](#inventory-and-parity-contract), then update the manifest
through the reviewed inventory flow. New cases in existing targets remain visible
through compiled discovery. Preserve ignored child controls and doctest eligibility;
do not make an inventory failure pass by disabling a target. Register manual
resources before creation and delayed workers with their real stop/join operation.

## Diagnosing failures

Start with the failed lane's attempt-scoped `tests-*-execution-attempt-*` artifact
for `job.json`, `discovery/compile.log`, case lists, phase/JUnit logs and disk samples.
The final `tests-*-attempt-*` artifact adds available LCOV and its object manifest.
For LLVM-enabled controls, check required export errors before optional diagnostics;
their gates also require every exact coverage artifact. Required CI retains test
diagnostics and requires successful lint and all test lanes.

To inspect a concrete mismatch on a local managed lane, retain its original raw
profiles, objects and build metadata, then rerun the exporter from the same checkout
with the pinned toolchain and its `llvm-tools-preview` component installed. Set
`coverage_dir` and `lane_dir` to that invocation's absolute output directories:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-10-20 python3 ci/export-coverage.py \
  --profiles "$coverage_dir/profiles" \
  --nextest "$lane_dir/discovery/nextest.json" \
  --cargo-artifact-manifest "$lane_dir/discovery/coverage-artifacts.json" \
  --cargo-artifacts "$lane_dir/supervisor.jsonl" \
  --doctests "$coverage_dir/doctests" --output "$coverage_dir/diagnostic.lcov" \
  --mismatch-diagnostics report
```

Include the original phase manifest and any separately recorded Cargo metadata
streams; omit the supervisor stream when that invocation did not build it.
Retain the captured non-test executables with the other local objects for replay.
Ordinary hosted artifacts contain provenance and
normalized coverage, rather than replayable profiles or binaries. Reproduce the
lane locally when those inputs are unavailable. A skipped or successful diagnostic
report does not prove that a coverage delta is harmless: compare actual case,
feature and owned source/line/branch coverage before attributing a change.

For interrupted fixtures, use the supervisor's
[owned-resource GC](../src/test-api/README.md#job-supervisor) with the original
state directory. Retained state or a missing cleanup-complete event requires
investigation; a timed-out job does not prove cleanup. Never prune shared Docker
resources. The [existing smoke drivers](../src/test-api/README.md#smoke-verification)
check recovery and exact owned-resource absence when that behavior needs diagnosis.

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
collector with explicit `--coverage none`, retaining case lists, JUnit, logs,
runner hardware/image, flags and image identities. Supervisor compilation is a
separate measured stage only when it requires an explicit build. Native
no-coverage selections owning `INFRASTRUCTURE` compile and validate that executable
in the discovery stage. Startup/cleanup remain within test time. Successful
Nextest output is retained for fixture timing observations.
Compile and execution discovery metadata remain separate. Coverage is explicitly
null and no profiles, captures or exports are produced in this mode. Optional
local `--coverage llvm` invocations start with fresh profiles; warm target reuse
can skip macro expansion and cannot reproduce cold compilation counters. No
profiles are replayed from the cold run.

Before this workflow is registered on the default branch, add `ci/lanes-four`,
`ci/lanes-three` or `ci/lanes-one` to a same-repository pull request. The guarded
label event checks out its actual head SHA and screens only that layout with
Nextest/four slots/FULL; its run number identifies the repetition. A `-libtest`
suffix selects the stock harness for a compatibility comparison under the same policy.
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
through lint, artifact upload and execution-inventory validation. Compare equivalent
complete workflow observations with the previous per-package fanout before
claiming a complete CI speedup.

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
lazy process fixtures and a separate cold Cargo target with cache disabled. Complete controls add
the original root/proc-macro omissions to the existing `test-api` job explicitly,
whose all-features graph already includes their fixture/macro dependencies. These
targets share its execution inventory. Affected controls retain the
actual original selection. Both preserve the required pristine/unmanaged phases
and validate compiled case/ignored lists. The final gate requires actual selector,
lint and every selected matrix job success. Both controls also require the exact
selected, attempt-scoped execution artifact set, matching package/target/doctest
ownership and complete compiled case lists; missing or foreign owners fail.

This control deliberately uses the same verified prebuilt tools, pinned runner
family, four test threads, independent lint and no-coverage policy as new CI.
It bounds fanout to twenty concurrent jobs. Case listing, explicit exceptional
phases and full-control root/proc-macro additions are parity adjustments. Historical
`cargo install grcov`, unpinned actions, runner allocation and global volume prune
are not reproduced; their costs are not measured by this control. Record actual
cache restores, hardware and all these adjustments with each comparison. A partial
affected run does not establish complete-corpus parity or equivalent full-workspace
speed. Keep literal historical evidence separate from the same-tool control.
All matched controls/layouts use the same no-coverage preparation policy, disk
guard and aligned dev/test debug settings. Include preparation cost and actual
capacity/cache observations; a capacity failure rejects the observed layout
without falling back to SDK deletion or retry. Owned fixture cleanup still applies.

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
The retained standalone legacy helper [`collect-coverage.sh`](../ci/collect-coverage.sh)
bounds grcov to one parser thread and one Rayon worker and emits GNU time resource
logs; the frozen baseline invokes grcov directly in [`bench-ci.py`](../ci/bench-ci.py)
and does not use that helper or its worker bounds.

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
the same case sets and, when explicitly measuring coverage, coverage identity totals,
then compare repeated cold and
warm stage distributions separately. Do not compare a cached compile against an
uncached one or sum concurrent matrix job durations as workflow wall time.
These optional package scenarios do not represent the full affected-workspace
fanout of a fixture change. End-to-end claims require equivalent complete
workflow comparisons, as recorded in [performance evidence](#performance-evidence).

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
global-reset fixtures safe to run concurrently. Callers select the harness and
concurrency within the fixture isolation contract. Supervisor diagnostics use their own
`AURA_TEST_ENV` record prefix and do not alter the benchmark timing schema.
