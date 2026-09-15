#!/usr/bin/env bash
# CI-only preparation. Does not create/start/remove containers or publish images.
set -euo pipefail
umask 077

if (( $# != 0 )); then
  echo "::error::PostgreSQL image preparation accepts no arguments" >&2
  exit 2
fi
: "${GITHUB_ENV:?GITHUB_ENV must name the job environment file}"

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
image="$(python3 - "$repo_root/src/test-api/postgres/image-ref.txt" <<'PY_IMAGE'
from pathlib import Path
import sys

try:
    lines = Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
except (OSError, UnicodeError):
    raise SystemExit("PostgreSQL fixture reference could not be read")
if len(lines) != 1:
    raise SystemExit("PostgreSQL fixture reference must be one nonempty line")
value = lines[0]
if (not value or not value.isascii() or value.startswith("-")
        or "://" in value
        or any(ch.isspace() or ord(ch) < 33 or ord(ch) == 127 for ch in value)):
    raise SystemExit("Invalid PostgreSQL fixture reference")
print(value)
PY_IMAGE
)"

# Keep the approved registry-login configuration, but force the fixture daemon.
unset DOCKER_HOST DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH DOCKER_TLS
unset DOCKER_API_VERSION
endpoint='unix:///var/run/docker.sock'
diagnostics="$(mktemp -d "${RUNNER_TEMP:-/tmp}/aura-ci-postgres.XXXXXX")"

if timeout --signal=TERM --kill-after=10s 300s \
    docker --host "$endpoint" pull "$image" \
    >"$diagnostics/pull.log" 2>&1; then
  :
else
  status=$?
  printf '::error::PostgreSQL image pull failed (exit=%s); diagnostics retained privately in %s\n' \
    "$status" "$diagnostics" >&2
  exit "$status"
fi

if image_id="$(timeout --signal=TERM --kill-after=5s 30s \
    docker --host "$endpoint" image inspect --format '{{.Id}}' "$image" \
    2>"$diagnostics/inspect.log")"; then
  :
else
  status=$?
  printf '::error::PostgreSQL image is not inspectable after pull (exit=%s); diagnostics retained privately in %s\n' \
    "$status" "$diagnostics" >&2
  exit "$status"
fi

if [[ ! "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "::error::PostgreSQL image inspection returned an invalid local image ID" >&2
  exit 1
fi

printf 'AURA_TEST_POSTGRES_IMAGE=%s\n' "$image_id" >> "$GITHUB_ENV"
printf 'Prepared PostgreSQL fixture image %s\n' "$image_id"
# This directory was created exclusively by this invocation; it contains only its logs.
rm -rf -- "$diagnostics"
