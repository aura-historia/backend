#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 PACKAGE OUTPUT_PREFIX (run from the checkout to benchmark)" >&2
  exit 2
fi

benchmark_package=$1
benchmark_output=$2
mkdir -p "$(dirname "$benchmark_output")"
export CARGO_PROFILE_TEST_DEBUG=${CARGO_PROFILE_TEST_DEBUG:-0}
export CARGO_INCREMENTAL=0

{
  git rev-parse HEAD
  git status --porcelain
  rustc --version
  docker --version
  uname -srm
  nproc
} > "${benchmark_output}.environment.txt"

# Build before timing. Each test executable starts and cleans up its own fixtures.
cargo test --locked -p "$benchmark_package" --tests --all-features --no-run \
  > "${benchmark_output}.build.log" 2>&1
/usr/bin/time -f 'elapsed_seconds=%e' -o "${benchmark_output}.time.txt" \
  cargo test --locked -p "$benchmark_package" --tests --all-features -- --test-threads=1 \
  > "${benchmark_output}.tests.log" 2>&1
cat "${benchmark_output}.time.txt"
