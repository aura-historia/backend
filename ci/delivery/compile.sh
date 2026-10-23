#!/usr/bin/env bash
# Trusted control-plane commands; invoked inside a container without AWS credentials.
set -euo pipefail
cd /candidate
export CARGO_TARGET_DIR=/output/target
export CARGO_HOME=/output/cargo
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=315532800
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export AWS_EC2_METADATA_DISABLED=true
mkdir -p /output/lambdas /output/templates /output/assemblies
if [ "${1:-}" = prepare ]; then
  # Dependencies and tool installers run without publication credentials as well.
  cargo install cargo-lambda --version 1.9.0 --locked
  python3 -m venv /output/tools
  /output/tools/bin/pip install ziglang==0.14.1
  curl --fail --location --silent --show-error https://nodejs.org/dist/v26.0.0/node-v26.0.0-linux-x64.tar.xz -o /output/node.tar.xz
  curl --fail --location --silent --show-error https://nodejs.org/dist/v26.0.0/SHASUMS256.txt -o /output/node-shasums
  (cd /output && sed -n 's#  node-v26.0.0-linux-x64.tar.xz$#  node.tar.xz#p' node-shasums | sha256sum --check -)
  tar -xf /output/node.tar.xz -C /output
  export PATH=/output/node-v26.0.0-linux-x64/bin:/output/cargo/bin:/output/tools/bin:$PATH
  cargo fetch --locked
  npm --prefix infra ci --ignore-scripts
  npm --prefix mjml ci --ignore-scripts
  exit 0
fi
export PATH=/output/node-v26.0.0-linux-x64/bin:/output/cargo/bin:/output/tools/bin:$PATH
rustc --version | head -1 | grep -F '1.98.0'
cargo lambda --version | grep -E '^cargo-lambda 1\.9\.0($| )'
node --version | grep -Fx 'v26.0.0'
while IFS= read -r binary; do
  cargo lambda build --locked --offline --release --target x86_64-unknown-linux-musl --output-format zip --bin "$binary"
  cp "/output/target/lambda/$binary/bootstrap.zip" "/output/lambdas/$binary.zip"
done < /control/lambdas.txt
while IFS= read -r template; do
  output="/output/templates/${template%.mjml}.html"
  mkdir -p "$(dirname "$output")"
  mjml/node_modules/.bin/mjml "$template" -o "$output"
done < /control/templates.txt
