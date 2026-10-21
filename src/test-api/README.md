# Integration test infrastructure

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
