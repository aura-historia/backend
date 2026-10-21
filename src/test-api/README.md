# Integration test infrastructure

## Job supervisor

Build the Rust CLI and run one service lifecycle around the complete test command:

```sh
cargo build --locked -p test-api --features supervisor --bin aura-test-env
./target/debug/aura-test-env run --profile postgres --slots 4 -- <test-command>
./target/debug/aura-test-env run --profile api --slots 4 -- <test-command>
```

Profiles are `postgres` (PostgreSQL only), `aws` (Floci only), `opensearch` (Floci and
OpenSearch), and `api` (all three). Managed images are pinned by digest in
[`environment/services.rs`](src/environment/services.rs): PostgreSQL 16, Floci 2.3.0,
and real OpenSearch 3.5.0. OpenSearch runs directly on the run's Docker network;
Floci has memory storage and no Docker socket. PostgreSQL retains logical WAL and
the generated CA/SANs used by verified TLS. The supervisor installs the workspace
OpenSearch mapping and native RRF hybrid pipeline before launching the command.
Fixture migration directories and setup scripts retain their existing semantics;
server readiness does not pre-apply the business schema to every profile.

The supervisor targets Linux, including `/proc` for stale process recovery.
Docker must be available at the local `/var/run/docker.sock`. Each invocation owns
UUID-named, labelled containers, volumes and a bridge network, with Docker-assigned
ports published only on `127.0.0.1`. The supervisor inherits command stdout/stderr,
forwards SIGINT/SIGTERM to its child process group, terminates remaining descendants,
and cleans its resources after success, failure or cancellation. Its exit code is
the child's exit code, or `128 + signal`; infrastructure/cleanup failure after a
successful command returns 1. A service exit, unhealthy state or changed start
identity fails the invocation; restarting requires a new invocation.

`AURA_TEST_ENV` JSON diagnostics on stderr report run ID, service start counts,
bootstrap time and cleanup time. They are independent of optional `AURA_CI_TIMING`
benchmark records. The private run directory defaults to
`$TMPDIR/aura-test-env-v1-<uid>` and can be selected with `--state-dir ABSOLUTE`.
A held OS file lock protects each live invocation from GC. Normal runs first collect
stale invocations; operators can retry explicitly after a hard kill or Docker outage:

```sh
./target/debug/aura-test-env gc
```

GC validates owner manifests and removes only exact run/UID-labelled resources. It
verifies the managed invocation environment of remaining process-group members
before signalling a stale group. SIGKILL prevents synchronous supervisor cleanup;
GC completes that recovery. Docker failures retain the run directory for retry.
No global prune is used. These controls govern fixture ownership; they are not a
network or process sandbox for an arbitrary supplied command.

### Managed child contract, version 1

Both `AURA_TEST_ENV_MANAGED=1` and an absolute `AURA_TEST_ENV_CONFIG` are required.
Partial or malformed configuration fails before unmanaged startup. The JSON file
is atomically published as `<private-state>/<run_id>/environment.json`; its parent
is owned by the current UID with mode 0700. The validated fields are:

| Field | Contract |
| --- | --- |
| `schema_version` | Integer `1`; unknown versions/fields fail |
| `run_id` | 32 lowercase hexadecimal characters, matching the directory |
| `profile` | `POSTGRES`, `AWS`, `OPENSEARCH`, or `API` |
| `slots` | Integer 1–1024, invocation capacity for downstream test leases |
| `postgres` | Null or `{port, root_certificate}`; localhost database/user/password `postgres`, verified TLS required, CA within the run directory |
| `aws_endpoint` | Null or loopback HTTP origin with explicit port |
| `opensearch_endpoint` | Null or loopback HTTP origin with explicit port |
| `aws_services` | Exact sorted profile topology: `eventbridge`, `s3`, `sesv2`, `sqs`; OpenSearch profiles also contain `opensearch` |

Endpoints accept `127.0.0.1` only, without userinfo, query, fragment or non-root path.
Topology is immutable; fixtures requesting unsupported services or extra Floci
environment fail. Child processes cache validated configuration and local clients,
attach without testcontainers, and register no container/CA cleanup. Per-call
PostgreSQL pools remain owned by the test's Tokio runtime.

The command receives fixed fake AWS credentials, `eu-central-1`, local endpoint
configuration, disabled IMDS, and empty run-owned AWS config/credential files.
Inherited AWS credential sources, profiles and service endpoint overrides are
removed. Never supply live credentials or external endpoints to test commands.

Infrastructure sharing does not yet provide per-test state isolation: existing
fixtures truncate shared tables/indices and clear captured SES messages. Run
mutating legacy suites sequentially; process-local `serial_test` does not coordinate
Nextest processes. Slot leases and scoped test state are owned by the follow-on
TestContext work. Independent attach-only probes are safe concurrently.

### Smoke verification

```sh
cargo test --locked -p test-api --all-features --test managed_environment -- --nocapture
# Lifecycle-only driver when a fixture test binary is not needed:
python3 ci/test-env-smoke.py --binary ./target/debug/aura-test-env
```

The smoke suite uses real services: PostgreSQL-only, AWS-only, native OpenSearch,
304 attach processes with Docker absent from their PATH, two simultaneous full
supervisors, interruption, nonzero exit/abort, descendants, cancellation during
bootstrap, a forced service restart, partial Docker failure, active-run GC and
hard-kill recovery. The Rust entry also exercises business migrations/extensions,
replication prerequisites and verified TLS through managed fixture attachment.
The driver verifies exact run resources and state are removed.

## Unmanaged local fixtures

AWS fixtures start a pinned [Floci](https://github.com/floci-io/floci) container automatically.
Image versions live in the [AWS fixture](src/aws.rs) and [OpenSearch bootstrap](src/opensearch.rs).
Tests need a local Docker daemon, the Docker CLI, and access to `/var/run/docker.sock`.
No emulator account or license token is required. The SDK uses fixed test credentials and
`eu-central-1`; PostgreSQL runs independently in its existing real database fixture.

Each test executable owns one emulator. Declare the complete AWS service topology on its first
test; later tests may request subsets with compatible environment settings. Floci enables its
services together, and the topology check catches inconsistent fixture declarations.
PostgreSQL-only tests start no AWS emulator.

OpenSearch uses a real, version-pinned engine launched by Floci. The fixture waits for readiness,
uses its published host port, and installs the workspace mappings and hybrid search pipeline.
Containers and volumes carry a process-specific namespace and are removed on normal exit or
interruption. The [lifecycle test](tests/aws.rs) exercises concurrent process isolation and cleanup.
SES fixtures verify senders and expose captured messages without configuring an SMTP relay.

Run the fixture suite with:

```sh
cargo test --locked -p test-api --all-features
```

## Comparing emulator performance

From each checkout, run the same package through this revision's
[benchmark runner](../../ci/benchmark-integration.sh), using its absolute path for older checkouts.
For example:

```sh
ci/benchmark-integration.sh aura-historia-api /tmp/aura-api-benchmark
```

The runner builds before timing, executes all test targets with one test thread, and writes build
and test logs, environment metadata, and wall time. Pre-pull the images used by both checkouts
before measuring; run them sequentially on the same machine. Compare identical passing test
counts, keep startup and cleanup inside the timed run, and record the revisions, image versions,
host load, and results with the issue. A failing or filtered suite is not a performance result.
