#!/usr/bin/env bash
# Trusted control-plane commands; invoked inside a container without AWS credentials.
set -euo pipefail
cd /candidate
export CARGO_TARGET_DIR=/output/target
export CARGO_HOME=/output/cargo
export RUSTUP_HOME=/output/rustup
export RUSTUP_TOOLCHAIN=1.98.0
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=315532800
export AWS_EC2_METADATA_DISABLED=true
mkdir -p /output/lambdas /output/templates /output/assemblies
capture_memory() {
  for counter in /sys/fs/cgroup/memory.peak /sys/fs/cgroup/memory/memory.max_usage_in_bytes; do
    if [ -r "$counter" ]; then
      cat "$counter" > "/output/${1}.memory-peak-bytes"
      return
    fi
  done
}
trap 'capture_memory "${1:-compile}"' EXIT
if [ "${1:-}" = prepare ]; then
  # Dependencies and tool installers run without publication credentials as well.
  # Keep the image's toolchain and the added target across disposable containers.
  if [ ! -d "$RUSTUP_HOME" ]; then
    cp -R /usr/local/rustup "$RUSTUP_HOME"
  fi
  rustup target add x86_64-unknown-linux-musl
  cargo install cargo-lambda --version 1.9.0 --locked
  python3 -m venv /output/tools
  /output/tools/bin/pip install ziglang==0.14.1
  zig_binary="$(/output/tools/bin/python -c 'import pathlib,ziglang; print(pathlib.Path(ziglang.__file__).parent / "zig")')"
  ln -sf "$zig_binary" /output/tools/bin/zig
  curl --fail --location --silent --show-error https://nodejs.org/dist/v26.0.0/node-v26.0.0-linux-x64.tar.xz -o /output/node.tar.xz
  curl --fail --location --silent --show-error https://nodejs.org/dist/v26.0.0/SHASUMS256.txt -o /output/node-shasums
  (cd /output && sed -n 's#  node-v26.0.0-linux-x64.tar.xz$#  node.tar.xz#p' node-shasums | sha256sum --check -)
  tar --no-same-owner -xf /output/node.tar.xz -C /output
  export PATH=/output/node-v26.0.0-linux-x64/bin:/output/cargo/bin:/output/tools/bin:$PATH
  cargo fetch --locked
  npm --prefix infra ci --ignore-scripts
  npm --prefix mjml ci --ignore-scripts
  exit 0
fi
export PATH=/output/node-v26.0.0-linux-x64/bin:/output/cargo/bin:/output/tools/bin:$PATH
# Preserve the Lambda profile used by the previous proven publisher.
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
rustc --version | head -1 | grep -F '1.98.0'
cargo lambda --version | grep -E '^cargo-lambda 1\.9\.0($| )'
node --version | grep -Fx 'v26.0.0'
while IFS= read -r binary; do
  cargo lambda build --locked --offline --release --target x86_64-unknown-linux-musl --output-format zip --package "$binary" --bin "$binary"
  cp "/output/target/lambda/$binary/bootstrap.zip" "/output/lambdas/$binary.zip"
done < /control/lambdas.txt
while IFS= read -r template; do
  output="/output/templates/${template%.mjml}.html"
  mkdir -p "$(dirname "$output")"
  mjml/node_modules/.bin/mjml "$template" -o "$output"
done < /control/templates.txt
