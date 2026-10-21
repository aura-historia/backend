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

### Isolated test contexts

Opt a migrated test into a scoped context:

```rust,ignore
const DATABASE: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [DATABASE, OpenSearch()], isolated = true)]
async fn should_read_its_own_data(context: TestContext) {
    let pool = get_postgres_client().await;
    let index = context.product_listings_index().unwrap();
    // Construct search/projector adapters through their existing with_index() methods.
}
```

The optional context argument receives the same scope used by fixture setup and
`get_postgres_client()`. The macro catches setup/body/individual teardown panics,
continues reverse teardown, and preserves the original test panic when cleanup also
fails. Unmigrated macro callers retain process-local serialization and acquire the
actual job-wide exclusive reservation. Direct fixture callers must acquire a context
or `reserve_exclusive()` themselves before sharing managed mutable state.

Each context holds one bounded slot and a shared job gate. With Nextest, it validates
`NEXTEST=1`, `NEXTEST_RUN_ID`, `NEXTEST_TEST_GLOBAL_SLOT`, and execution mode
`process-per-test`; configure Nextest test threads no higher than supervisor slots.
Slots start at zero. Competing Nextest invocations under the same supervisor use the
same slot files and wait up to 120 seconds, rather than allocating additional capacity.
Filters/retries require no predetermined test ordering. `exclusive = true` creates a
scoped context holding the exclusive job gate, excluding every ordinary context and
legacy macro caller across all groups and invocations. A group with one thread does
not establish this exclusion. Raw global operations can use `reserve_exclusive()`
and its `scope(future)` method. Role/bootstrap tests requiring absent roles and pristine
`postgres` ownership need a separate supervisor invocation that has not requested a
business baseline; a reservation alone does not restore cluster-global state.

Non-Nextest drivers use `TestContext::acquire_explicit(name, isolation)`, keep the
returned `TestSession`, run work through `session.context().scope(future)`, then await
`session.finish()`. This path uses the same semaphore/file reservations. It deliberately
ignores inherited Nextest slot identity for a separate child batch. A parent waiting
for that batch must release its own lease first. Unmanaged `cargo test` remains
supported with the existing process-owned services and one local context slot.

PostgreSQL prepares an immutable template once per actual migration/setup contract,
under a cross-process template lock. Paths must resolve within the workspace;
missing/empty/escaping declarations fail. The key includes ordered filenames, SQL
contents and setup-script identity. Business prerequisites, extensions and seeded data
belong to the template. Each test receives a fresh generation database before its body,
restoring schema, triggers/constraints and seeds while fencing prior writers. Preparation
and cloning occur in fixture setup; all setup/reset time remains in suite/workflow time
and is reported separately through the existing benchmark recorder. `CONTEXT_ACQUIRE`
includes lease waiting and recovery before fixture setup; `POSTGRES_SLOT_RESET`
includes baseline preparation and cloning; `CONTEXT_CLEANUP` includes owned shutdown
and resource removal after fixture teardown. These phases remain inside full suite time.
A new pool is
created on each calling Tokio runtime and registered for close before database deletion.
Register any manually constructed pool with `register_postgres_pool()`.

OpenSearch contexts create generation-specific indices from production mappings and
reuse the native hybrid RRF pipeline. Pass `product_listings_index()` to every adapter,
write, read and refresh; raw client calls do not rewrite a literal index name. Context
cleanup deletes only owned indices; legacy delete-by-query performs its own refresh
without a second refresh request.

SQS/S3/SES names contain the complete run ID and a fresh test generation, preserving
FIFO suffixes. `queue_name()`, `bucket_name()`, `configuration_set_name()` and
`sender_email()` resolve context identities. Await `queue_name()`, `bucket_name()` and
`configuration_set_name()` before manual create requests; fixtures persist their
index, queue and sender ownership during setup. Labels use lowercase ASCII
letters/digits/internal hyphens; maximum label lengths are 29 for ordinary SQS, 24
for FIFO SQS, 12 for S3, and 13 for SES configuration sets. Worker queue fixtures also
preserve redrive, visibility, encryption and policy metadata. Enable the corresponding
fixture features (`sqs`, `s3`, `ses`, `opensearch`, `postgres`) for every resource a lane
may recover. Unsupported recovery fails before a new test body.

SES uses a unique verified sender. For another owned sender, await
`context.register_sender("secondary")` before passing the returned address to
`verify_sender_identity()`; registration persists recovery ownership before creation.
Capture helpers inspect the default source, and cleanup removes only each owned
sender's returned message IDs. Floci 2.3.0 supports source filtering on GET
and message-ID filtering on DELETE; an email parameter on DELETE is not scoped
cleanup. Unfiltered managed deletion is permitted only inside the legacy exclusive
scope. Test-owned SES configuration sets/identities are removed with the context.

Contexts do not propagate automatically through `tokio::spawn` or OS threads.
Capture resolved resources/pools, explicitly scope a context clone, or use
`context.spawn()` for abort-and-join ownership. Register external worker/server
shutdown through `on_shutdown()`; shutdown must stop and join every delayed writer,
callback or long poll before resources are removed. The API fixture explicitly scopes
its factory on its own runtime and registers server/thread shutdown; `base_url()`
returns the current context's owned URL. Captures, failure injection and caches in
consumer code still need per-context ownership before in-process parallel execution.

For API fake state, a setup fixture before `&API` inserts an `Arc<ApiTestState>` with
`context.insert_api_state(state)`. The factory and test helpers read it through
`context.api_state::<Arc<ApiTestState>>()`. Missing state and duplicate initialization
return errors; insertion after server initialization starts is rejected. The factory
runs once per context/API declaration even when setup callers race. The server stops
and its thread joins before the context drops this owned state.

A worker can capture the scoped resources and remain owned by the context:

```rust,ignore
let pool = context.postgres_pool().await?;
let queue_url = worker_queues.queue_url(); // Resolve before a raw spawned task.
context.spawn(async move {
    run_worker(pool, queue_url).await;
})?;
// An external JoinHandle/server instead registers stop-and-join with on_shutdown().
```

Normal teardown stops/joins owned tasks and servers, closes pools, removes only
registered resources, then releases the reservation. Dropping a session transfers its
reservation and cleanup to a separate runtime thread. A cleanup deadline reports
failure to the caller while delayed joins continue holding the reservation and API
state; resources are removed only after owned shutdown actually completes. Timeout
failures retain recovery metadata. A killed process cannot
run Drop: its persistent slot manifest is recovered before the next holder proceeds.
Cleanup failure remains visible and retains recovery metadata. Corrupt/foreign
ownership fails closed. New generation names protect a reused slot from a late old
request; supervisor GC owns final job cleanup after interruption.

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

For focused context acceptance, build the `isolated_context` test executable and pass
its reported path to the driver:

```sh
cargo test --locked -p test-api --all-features --test isolated_context --no-run
python3 ci/test-context-smoke.py --binary ./target/debug/aura-test-env \
  --fixture-binary <isolated_context-executable> --output /tmp/aura-context-check
```

This runs 1/2/4 slots on actual PostgreSQL/Floci/OpenSearch, competing invocations,
killed-holder recovery, restored seeds/DDL, thread-bound HTTP factories, an untouched
SES sentinel and a filtered real Nextest retry. It records full TRUNCATE versus
recreation measurements for the business baseline. Recreation also restores DDL/seeds
and fences old writers, so a faster TRUNCATE is insufficient for that isolation contract.

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
