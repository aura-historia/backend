#!/usr/bin/env bash
set -euo pipefail

: "${COMMIT_SHA:?COMMIT_SHA must be bound to the checkout}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"

if [[ ! "$COMMIT_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error::Invalid checkout SHA" >&2
  exit 1
fi

umask 077
helper_iid="$RUNNER_TEMP/c2-opensearch-helper.iid"
helper_meta="$RUNNER_TEMP/c2-opensearch-helper.json"
witness_iid="$RUNNER_TEMP/c2-opensearch-witness.iid"
witness_meta="$RUNNER_TEMP/c2-opensearch-witness.json"
engine_ref_file="$RUNNER_TEMP/c2-opensearch-engine.ref"
engine_meta="$RUNNER_TEMP/c2-opensearch-engine.json"
rm -f "$helper_iid" "$helper_meta" "$witness_iid" "$witness_meta" "$engine_ref_file" "$engine_meta"

DOCKER_BUILDKIT=1 docker build \
  --platform linux/amd64 \
  --file deploy/images/Dockerfile \
  --target opensearch-test-helper \
  --build-arg COMMIT_SHA="$COMMIT_SHA" \
  --iidfile "$helper_iid" \
  .

helper_id="$(cat "$helper_iid")"
if [[ ! "$helper_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "::error::Helper build did not produce an immutable local image ID" >&2
  exit 1
fi
test "$(docker image inspect --format '{{.Id}}' "$helper_id")" = "$helper_id"
docker image inspect --format '{{json .}}' "$helper_id" > "$helper_meta"
python3 - "$helper_meta" "$helper_id" <<'PY'
import json
import re
import sys

metadata = json.load(open(sys.argv[1], encoding="utf-8"))
identifier = sys.argv[2]
config = metadata.get("Config") or {}
if (
    metadata.get("Id") != identifier
    or metadata.get("Os") != "linux"
    or metadata.get("Architecture") != "amd64"
    or config.get("User") != "10001:10001"
    or config.get("Entrypoint") != ["/usr/bin/python3"]
    or config.get("Volumes")
    or config.get("Healthcheck")
):
    raise SystemExit("helper image contract failed")
for item in config.get("Env") or []:
    key = item.split("=", 1)[0]
    if key.startswith(("AWS_", "GOOGLE_", "GCP_", "AZURE_")) or any(
        marker in key for marker in ("PASSWORD", "TOKEN", "CREDENTIAL", "PROXY")
    ):
        raise SystemExit("helper image contains ambient credential configuration")
if not re.fullmatch(r"sha256:[0-9a-f]{64}", identifier):
    raise SystemExit("helper image ID format failed")
PY

# The witness is a CI-only test image. The non-forwarding unit stub deliberately
# omits GITHUB_ACTIONS and continues to exercise the helper/engine contract.
if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
DOCKER_BUILDKIT=1 docker build \
  --platform linux/amd64 \
  --file deploy/images/Dockerfile \
  --target opensearch-tls-witness \
  --build-arg COMMIT_SHA="$COMMIT_SHA" \
  --iidfile "$witness_iid" \
  .

witness_id="$(cat "$witness_iid")"
if [[ ! "$witness_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "::error::Witness build did not produce an immutable local image ID" >&2
  exit 1
fi
test "$(docker image inspect --format '{{.Id}}' "$witness_id")" = "$witness_id"
docker image inspect --format '{{json .}}' "$witness_id" > "$witness_meta"
python3 - "$witness_meta" "$witness_id" <<'PY'
import json
import re
import sys

metadata = json.load(open(sys.argv[1], encoding="utf-8"))
identifier = sys.argv[2]
config = metadata.get("Config") or {}
if (
    metadata.get("Id") != identifier
    or metadata.get("Os") != "linux"
    or metadata.get("Architecture") != "amd64"
    or config.get("User") != "10001:10001"
    or config.get("Entrypoint") != ["/usr/local/bin/opensearch-tls-witness"]
    or config.get("Volumes")
    or config.get("Healthcheck")
):
    raise SystemExit("witness image contract failed")
for item in config.get("Env") or []:
    key = item.split("=", 1)[0]
    if key.startswith(("AWS_", "GOOGLE_", "GCP_", "AZURE_")) or any(
        marker in key for marker in ("PASSWORD", "TOKEN", "CREDENTIAL", "PROXY")
    ):
        raise SystemExit("witness image contains ambient credential configuration")
if not re.fullmatch(r"sha256:[0-9a-f]{64}", identifier):
    raise SystemExit("witness image ID format failed")
PY
fi

python3 - "deploy/compose/opensearch/image.ref" "$engine_ref_file" <<'PY'
from pathlib import Path
import re
import sys

raw = Path(sys.argv[1]).read_bytes()
try:
    text = raw.decode("ascii")
except UnicodeDecodeError:
    raise SystemExit("OpenSearch image pin is not ASCII")
match = re.fullmatch(
    r"(opensearchproject/opensearch:[0-9]+\.[0-9]+\.[0-9]+@sha256:[0-9a-f]{64})\n?",
    text,
)
if match is None:
    raise SystemExit("OpenSearch image pin is not a stable immutable reference")
Path(sys.argv[2]).write_text(match.group(1), encoding="ascii")
PY
engine_ref="$(cat "$engine_ref_file")"
docker pull --platform linux/amd64 "$engine_ref"
engine_id="$(docker image inspect --format '{{.Id}}' "$engine_ref")"
if [[ ! "$engine_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "::error::OpenSearch pull did not produce an immutable local image ID" >&2
  exit 1
fi
test "$(docker image inspect --format '{{.Id}}' "$engine_ref")" = "$engine_id"
docker image inspect --format '{{json .}}' "$engine_ref" > "$engine_meta"
python3 - "$engine_meta" "$engine_ref" "$engine_id" <<'PY'
import json
import re
import sys

metadata = json.load(open(sys.argv[1], encoding="utf-8"))
reference = sys.argv[2]
identifier = sys.argv[3]
tagged_repository, digest = reference.rsplit("@", 1)
repository, _version = tagged_repository.rsplit(":", 1)
if (
    metadata.get("Id") != identifier
    or metadata.get("Os") != "linux"
    or metadata.get("Architecture") != "amd64"
    or repository + "@" + digest not in (metadata.get("RepoDigests") or [])
    or not re.fullmatch(r"sha256:[0-9a-f]{64}", identifier)
):
    raise SystemExit("selected OpenSearch registry/local identity failed")
PY

printf 'C2_HELPER_IMAGE=%s\n' "$helper_id" >> "$GITHUB_ENV"
if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
  printf 'C2_WITNESS_IMAGE=%s\n' "$witness_id" >> "$GITHUB_ENV"
fi
printf 'C2_OPENSEARCH_IMAGE=%s\n' "$engine_id" >> "$GITHUB_ENV"
