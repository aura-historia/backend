#!/usr/bin/env bash
set -euo pipefail

profiles="${1:?profile directory required}"
binaries="${2:?binary directory required}"
report="${3:?LCOV output required}"

if profile_path="$(find "$profiles" -type f -name '*.profraw' -print -quit)"; then
  if [ -z "$profile_path" ]; then
    echo "No LLVM coverage profiles were written." >&2
    exit 1
  fi
else
  status=$?
  echo "Failed to search for LLVM coverage profiles (find exited with $status)." >&2
  exit "$status"
fi
mkdir -p "$(dirname "$report")"
grcov "$profiles" --binary-path "$binaries" -s . -t lcov --branch --ignore-not-existing --ignore '/*' --ignore '*/target/*' --ignore '*/.cargo/*' -o "$report"
test -s "$report"
