#!/usr/bin/env bash
set -euo pipefail

destination="${1:?usage: install-test-tools.sh DESTINATION}"
test "$(uname -s)" = Linux
test "$(uname -m)" = x86_64
mkdir -p "$destination"

curl --fail --location --silent --show-error --retry 3 \
  https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.149/cargo-nextest-0.9.149-x86_64-unknown-linux-gnu.tar.gz \
  -o "$destination/nextest.tar.gz"
printf '%s  %s\n' 32d1d8c0567b7a80bee2bad8bb09125c846da96bc009b2de11ec08557290c770 "$destination/nextest.tar.gz" | sha256sum --check
tar -xzf "$destination/nextest.tar.gz" -C "$destination" cargo-nextest

curl --fail --location --silent --show-error --retry 3 \
  https://github.com/mozilla/grcov/releases/download/v0.10.7/grcov-x86_64-unknown-linux-gnu.tar.bz2 \
  -o "$destination/grcov.tar.bz2"
printf '%s  %s\n' f9d0de258c737659fbf2c88e861784f392a784b52425b23f901d5e0cecdfd581 "$destination/grcov.tar.bz2" | sha256sum --check
tar -xjf "$destination/grcov.tar.bz2" -C "$destination" grcov
"$destination/cargo-nextest" nextest --version
"$destination/grcov" --version
