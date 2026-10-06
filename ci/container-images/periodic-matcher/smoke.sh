#!/usr/bin/env bash
set -euo pipefail

image_ref="${1:?usage: smoke.sh IMAGE_REF}"
expected_ca="$(sha256sum infra/assets/rds-ca-layer/aura-historia/rds-ca/global-bundle.pem | cut -d ' ' -f 1)"
container="matcher-smoke-${RANDOM}-${RANDOM}"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT

inspect="$(docker image inspect "$image_ref")"
jq -e '
  length == 1 and .[0].Os == "linux" and .[0].Architecture == "amd64" and
  .[0].Config.Labels["org.opencontainers.image.title"] == "search-filter-periodic-match" and
  .[0].Config.User == "10001:10001" and
  (.[0].Config.ExposedPorts // {} | length == 0) and
  (.[0].Config.Volumes // {} | keys == ["/tmp"]) and
  .[0].Config.Entrypoint == ["/usr/local/bin/search-filter-periodic-match"] and
  (.[0].Config.Cmd // []) == [] and
  (.[0].Config.Env // [] | all(.[]; (
    startswith("POSTGRES_USERNAME=") or startswith("POSTGRES_PASSWORD=") or
    startswith("OPENSEARCH_USERNAME=") or startswith("OPENSEARCH_PASSWORD=") or
    startswith("CLOUDFLARE_API_TOKEN=") or
    startswith("CLOUDFLARE_API_TOKEN_SSM_PARAMETER=")
  ) | not))
' <<< "$inspect" >/dev/null

ca_sha="$(docker run --rm --network none --read-only --entrypoint /usr/bin/sha256sum \
  "$image_ref" /opt/aura-historia/rds-ca/global-bundle.pem | cut -d ' ' -f 1)"
test "$ca_sha" = "$expected_ca" || { echo 'Image public RDS CA does not match the repository asset.' >&2; exit 1; }
if docker run --rm --network none --read-only --user 0:0 --entrypoint /usr/bin/touch \
  "$image_ref" /root/should-not-be-writable >/dev/null 2>&1; then
  echo 'Image root filesystem is writable despite --read-only.' >&2
  exit 1
fi

status=0
timeout 20s docker run --rm --name "$container" --network none --read-only "$image_ref" \
  > smoke-no-args.log 2>&1 || status=$?
test "$status" -eq 1 && grep -q 'startup_failed' smoke-no-args.log || {
  echo 'No-argument execution did not reach the expected bounded startup failure.' >&2; exit 1;
}

status=0
timeout 20s docker run --rm --name "$container" --network none --read-only "$image_ref" --unexpected \
  > smoke-args.log 2>&1 || status=$?
test "$status" -eq 1 && grep -q 'accepts no arguments' smoke-args.log || {
  echo 'The executable did not reject an unexpected argument before startup.' >&2; exit 1;
}
