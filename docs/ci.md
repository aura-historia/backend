# CI inventory and measurements

[`integrate.yml`](../.github/workflows/integrate.yml) owns mandatory quality gates and
the current package test matrix. The inventory and benchmark tools observe that
contract; their lane assignments do not select tests or change coverage policy.

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
The current workflow runs package-directory `cargo test --all-features`; compiled
workspace discovery is explicitly marked `WORKSPACE_ALL_FEATURES`. A selection
redesign must preserve the intended configurations and compare actual case sets.

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
not aggregate simultaneous process memory. Logical target size is sampled at
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

The current determinator excludes the root and nested `test-api-macros` packages.
The inventory includes both. Current package `cargo test` invocations include
doctests; nextest alone would omit them. Current coverage upload files are
concatenated for Sonar, so repeated dependency source records can occur; the
baseline collector exposes sums and identity unions without changing that merge.

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
