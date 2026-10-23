# Integration test fixtures

## Run locally

Tests need Linux with PID descriptor support, a Docker daemon at
`/var/run/docker.sock`, and the Docker CLI. Fixtures start real PostgreSQL,
Floci and OpenSearch as needed, using fake credentials and local endpoints.
No emulator account, license token or deployed service is required.

Start with a consumer package:

```sh
cargo test --locked -p product-listing-postgres --all-features --test public_catalogue_overview
```

Plain Cargo tests use process-owned fixtures and serialize access where needed.
For parallel Nextest with shared services:

```sh
cargo build --locked -p test-api --all-features --bin aura-test-env
./target/debug/aura-test-env run --profile postgres --slots 4 -- \
  cargo nextest run --locked -p crawler --all-features --test-threads 4
```

Profiles: `postgres` (PostgreSQL), `aws` (Floci), `opensearch` (Floci and
OpenSearch), `api` (all three). Use the [CI driver](../../docs/ci.md) for complete
lanes and the pristine migration phase. Run doctests separately with
`cargo test --locked -p <package> --all-features --doc`.

This crate's `tests/` contains ignored fixture diagnostics. Run a selected
diagnostic outside any managed supervisor:

```sh
cargo test --locked -p test-api --all-features --test postgres -- --ignored --test-threads=1
```

## Supervisor ownership

One supervisor owns the complete command's service lifecycle. It creates
UUID-named, labelled containers, volumes and a network; ports bind only to
`127.0.0.1`. Images are pinned in
[`environment/services.rs`](src/environment/services.rs). PostgreSQL uses
verified TLS and logical WAL. Managed OpenSearch installs the workspace mapping
and hybrid pipeline before tests start.

Children attach to validated job configuration. Both
`AURA_TEST_ENV_MANAGED=1` and absolute `AURA_TEST_ENV_CONFIG` must be present
together; partial or invalid configuration fails before fixture startup.
The supervisor supplies fake credentials, local endpoints and disabled IMDS.

SIGINT/SIGTERM are forwarded, owned descendants are stopped across sessions, and
only invocation-owned resources are removed. Command failures are preserved;
service failure or cleanup failure also fails the invocation. Diagnostics appear
as `AURA_TEST_ENV` JSON on stderr.

Private state defaults to `$TMPDIR/aura-test-env-v1-<uid>`; select another path
with `--state-dir ABSOLUTE`. A lock protects live runs. Startup collects stale
runs. After a hard kill or Docker failure, recover against the same state directory:

```sh
./target/debug/aura-test-env gc
```

Recovery validates ownership and signals through PID descriptors to prevent PID
reuse. Failed cleanup retains recovery state. No global Docker prune is used.

## Isolated tests

Declare actual services and request a scoped context:

```rust,ignore
const DATABASE: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [DATABASE, OpenSearch()], isolated = true)]
async fn should_read_its_own_data(context: TestContext) {
    let pool = get_postgres_client().await;
    let index = context.product_listings_index().unwrap();
    // Pass the scoped pool/index to the adapters under test.
}
```

The optional argument receives the context used by setup and teardown. The macro
preserves test panics while continuing reverse teardown. Legacy macro callers
retain serialization and the same exclusive reservation.

Keep Nextest threads at or below supervisor slots. Cross-process leases bound
concurrency; `exclusive = true` excludes every other context for global operations.
A one-thread Nextest group does not establish that exclusion. Absent-role and
pristine-cluster tests require a fresh supervisor.

PostgreSQL clones a fresh database from an immutable migration/setup template per
test generation. Migration paths stay within the workspace. Register manually
created pools with `register_postgres_pool()` so they close before database removal.

Pass `product_listings_index()` to every OpenSearch adapter, write, read and refresh.
Resolve context-owned SQS/S3/SES names before creation; register extra senders with
`register_sender()`. Register manually created resources before creating them and
enable all fixture features required for recovery.

Contexts do not propagate through raw Tokio tasks or OS threads. Capture resolved
resources, scope a context clone, or use `context.spawn()`. `spawn_owned()` retains
the real worker join and preserves its result, cancellation and panic.

Register external workers/servers with `on_shutdown()`. Stop and join delayed
writers, callbacks and long polls before releasing resources. Teardown joins work,
closes pools, removes resources, then releases the lease. Dropped sessions transfer
cleanup to a dedicated runtime; deadline failure retains the reservation until
owned work stops.

Insert API fake state through `insert_api_state()` before server creation and read
it through `api_state()`. The factory runs once per context and the server joins
before state is released. Client pools belong to the runtime that uses them.

Killed tests leave recovery manifests for the next slot holder. Corrupt or foreign
ownership fails closed; fresh generation names fence late requests from old tests.
