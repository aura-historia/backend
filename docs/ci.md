# CI and local tests

## Local feedback

Start with the package or test you changed:

```sh
cargo test --locked -p product-listing-postgres --all-features --test public_catalogue_overview
cargo test --locked -p product-listing-core --all-features
```

Use the repository toolchain. Integration and some library tests need Linux,
Docker and the Docker CLI; fixtures start local PostgreSQL, Floci and OpenSearch
with fake credentials. No deployed service or AWS login is required.
The [fixture guide](../src/test-api/README.md) explains ownership and cleanup.

Broader validation: `cargo test --locked --workspace --all-features` includes
library, binary, integration and documentation tests. Nextest needs a separate
`cargo test --locked --workspace --all-features --doc` invocation for doctests.

Fixture diagnostics under `src/test-api/tests/` are ignored by default. Run a
selected diagnostic outside a managed supervisor:

```sh
cargo test --locked -p test-api --all-features --test postgres -- --ignored --test-threads=1
```

## Required CI

[Integrate](../.github/workflows/integrate.yml) runs independent lint and four
test lanes on relevant PRs and `develop` pushes. Nextest uses four slots.
The final quality job requires lint and every lane to succeed.

| Lane | Packages | Service profile |
| --- | --- | --- |
| API | API package, including acceptance tests | `api` |
| CRAWLER | Crawler | `postgres` |
| INFRASTRUCTURE | Other packages using integration fixtures | `api` |
| REMAINING | Packages without shared infrastructure requirements | `none` |

[The manifest](../ci/test-lanes.json) assigns every workspace package exactly
once. Add new packages to the appropriate array; new targets in an assigned
package are included automatically. Missing, duplicated and unknown packages
fail validation. Inspect ownership with:

```sh
python3 ci/inventory.py --output /tmp/aura-inventory --lane API
```

The [driver](../ci/run-test-lane.py) compiles selected tests, validates ownership,
runs them under one supervisor and runs their doctests. Migration bootstrap runs
first in a separate pristine PostgreSQL environment because its absent-role
assertions need an untouched cluster.

To reproduce a lane with the CI compiler and build settings:

```sh
rustup toolchain install nightly-2026-10-20 --profile minimal
export RUSTUP_TOOLCHAIN=nightly-2026-10-20
export CARGO_BUILD_JOBS=4
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=1
export CARGO_PROFILE_TEST_DEBUG=1
export CARGO_TARGET_DIR="$PWD/target/no-coverage"
bash ci/install-test-tools.sh "$PWD/target/ci-tools"
export PATH="$PWD/target/ci-tools:$PATH"
cargo build --locked -p test-api --all-features --bin aura-test-env
python3 ci/run-test-lane.py --lane API --slots 4 --profile api \
  --features test-api/supervisor,test-api/s3,test-api/ses \
  --features test-api/aura-historia-api,test-api/api-gateway,test-api/eventbridge \
  --supervisor "$CARGO_TARGET_DIR/debug/aura-test-env" \
  --state-dir /tmp/aura-test-env-local --output /tmp/aura-api-results
```

Select the lane/profile from the table. The remaining lane needs no services.
Give concurrent runs separate output directories.

## Cache and failures

Required tests use no LLVM instrumentation or Sonar analysis. Each job resets
`target/no-coverage`, then restores compatible external dependencies through
the pinned cache action. Workspace crates and test executables are rebuilt.
Keys separate lanes, compiler/environment, Cargo inputs and execution policy.

Only successful `develop` push runs publish the lint and test-lane caches.
Pull requests restore the newest compatible `develop` entry, which GitHub
exposes to every branch, and save nothing. Work unique to a PR, such as a
changed `Cargo.lock`, lane driver or compiler, is therefore rebuilt on each
push until it reaches `develop`; a miss costs time only, and PRs still run
every test. Jobs remove runner-image toolchains they do not compile with
before restoring, because the cache action hashes every installed toolchain.
Inspect restore logs to distinguish an exact hit, a fallback to an older
`develop` entry and a miss.

The disk guard stops execution below three GiB free. Tests keep installed SDKs;
lint retains its existing disk preparation. Cleanup removes only owned resources
and preserves command failure/cancellation status.

Artifacts retain command output, discovery and JUnit. Start with the failing
command and supervisor `AURA_TEST_ENV` diagnostics. After a hard kill, run
`aura-test-env gc` against the same state directory; see the fixture guide.

Historical measurements and their limits are recorded in
[PR #2422](https://github.com/aura-historia/backend/pull/2422).
Further cache improvements are tracked in
[#2425](https://github.com/aura-historia/backend/issues/2425).
