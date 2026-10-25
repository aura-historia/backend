#!/usr/bin/env bash
# Trusted control-plane commands; invoked inside a container without AWS credentials.
set -euo pipefail
cd /candidate
export CARGO_TARGET_DIR=/output/target
export CARGO_HOME=/cache/cargo
export RUSTUP_TOOLCHAIN=1.98.0
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=315532800
export AWS_EC2_METADATA_DISABLED=true
export npm_config_cache=/output/npm-cache
export ZIG_GLOBAL_CACHE_DIR=/output/zig-global-cache
export XDG_CACHE_HOME=/output/tool-cache
mkdir -p /output/lambdas /output/templates /output/assemblies
capture_resources() {
  for counter in /sys/fs/cgroup/memory.peak /sys/fs/cgroup/memory/memory.max_usage_in_bytes; do
    if [ -r "$counter" ]; then
      cat "$counter" > "/output/${1}.memory-peak-bytes"
      break
    fi
  done
  if [ -r /sys/fs/cgroup/cpu.stat ]; then
    cat /sys/fs/cgroup/cpu.stat > "/output/${1}.cpu-stat"
  fi
}
trap 'capture_resources "${1:-compile}"' EXIT
if [ "${1:-}" = prepare ]; then
  cargo fetch --locked
  npm --prefix infra ci --ignore-scripts
  npm --prefix mjml ci --ignore-scripts
  exit 0
fi
# Preserve the Lambda profile used by the previous proven publisher.
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export RUSTC_WRAPPER=/control/cache-rustc.sh
export RUSTC_WORKSPACE_WRAPPER=/control/cache-rustc.sh
export SCCACHE_DIR=/cache/sccache
export SCCACHE_CACHE_SIZE=10G
export SCCACHE_IDLE_TIMEOUT=0
rustc --version | head -1 | grep -F '1.98.0'
cargo lambda --version | grep -E '^cargo-lambda 1\.9\.0($| )'
node --version | grep -Fx 'v26.0.0'
sccache --version | grep -Fx 'sccache 0.19.0'
sccache --start-server
sccache --zero-stats
capture_compile() {
  capture_resources compile
  sccache --show-stats --stats-format json > /output/compiler-cache.json
  sccache --stop-server > /dev/null
}
trap capture_compile EXIT
arguments=()
while IFS= read -r binary; do
  arguments+=(--package "$binary" --bin "$binary")
done < /control/lambdas.txt
/usr/bin/time -v -o /output/compile.resources.txt \
  cargo lambda build --locked --offline --release --target x86_64-unknown-linux-musl --output-format zip "${arguments[@]}"
while IFS= read -r binary; do
  cp "/output/target/lambda/$binary/bootstrap.zip" "/output/lambdas/$binary.zip"
done < /control/lambdas.txt
while IFS= read -r template; do
  output="/output/templates/${template%.mjml}.html"
  mkdir -p "$(dirname "$output")"
  mjml/node_modules/.bin/mjml "$template" -o "$output"
done < /control/templates.txt
