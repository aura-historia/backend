#!/usr/bin/env bash
set -euo pipefail

# Only an explicit 404 permits publication; permission/network failures must stop it.
error_file="$(mktemp)"
trap 'rm -f "$error_file"' EXIT
if aws s3api head-object --bucket "$1" --key "$2" >/dev/null 2>"$error_file"; then
  echo true
elif grep -Eq '^(aws: \[ERROR\]: )?An error occurred \((404|NoSuchKey|NotFound)\) when calling the HeadObject operation:' "$error_file"; then
  echo false
else
  cat "$error_file" >&2
  exit 1
fi
