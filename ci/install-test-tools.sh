#!/usr/bin/env bash
set -euo pipefail

destination="${1:?usage: install-test-tools.sh DESTINATION}"
test "$(uname -s)" = Linux
test "$(uname -m)" = x86_64
mkdir -p "$destination"

curl --fail --location --silent --show-error --retry 3 --retry-all-errors \
  --connect-timeout 10 --max-time 120 \
  https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.149/cargo-nextest-0.9.149-x86_64-unknown-linux-gnu.tar.gz \
  -o "$destination/nextest.tar.gz"
printf '%s  %s\n' 32d1d8c0567b7a80bee2bad8bb09125c846da96bc009b2de11ec08557290c770 "$destination/nextest.tar.gz" | sha256sum --check
tar -xzf "$destination/nextest.tar.gz" -C "$destination" cargo-nextest
"$destination/cargo-nextest" nextest --version
