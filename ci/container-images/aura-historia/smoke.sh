#!/usr/bin/env bash
set -euo pipefail

image_ref="${1:?usage: smoke.sh IMAGE_REF}"

inspect="$(docker image inspect "$image_ref")"
jq -e '
  length == 1 and .[0].Os == "linux" and .[0].Architecture == "amd64" and
  .[0].Config.Labels["org.opencontainers.image.title"] == "aura-historia" and
  .[0].Config.User == "10001:10001" and
  (.[0].Config.Volumes // {} | keys == ["/tmp"]) and
  (.[0].Config.Entrypoint // []) == [] and
  .[0].Config.Cmd == ["api"] and
  (.[0].Config.Env // [] | all(.[]; (
    startswith("POSTGRES_USERNAME=") or startswith("POSTGRES_PASSWORD=") or
    startswith("OPENSEARCH_USERNAME=") or startswith("OPENSEARCH_PASSWORD=") or
    startswith("CLOUDFLARE_API_TOKEN=") or startswith("ANTHROPIC_API_KEY=") or
    startswith("STRIPE_API_KEY=") or startswith("GOOGLE_APPLICATION_CREDENTIALS=")
  ) | not))
' <<< "$inspect" >/dev/null

if docker run --rm --network none --read-only --user 0:0 --entrypoint /usr/bin/touch \
  "$image_ref" /root/should-not-be-writable >/dev/null 2>&1; then
  echo 'Image root filesystem is writable despite --read-only.' >&2
  exit 1
fi

# Every entrypoint exists and fails closed without configuration instead of starting.
for entrypoint in api mcp worker; do
  status=0
  timeout 20s docker run --rm --network none --read-only "$image_ref" "$entrypoint" \
    > "smoke-$entrypoint.log" 2>&1 || status=$?
  test "$status" -ne 0 && test "$status" -ne 124 || {
    echo "The $entrypoint entrypoint did not fail closed without configuration." >&2; exit 1;
  }
done

status=0
timeout 20s docker run --rm --network none --read-only "$image_ref" ops \
  > smoke-ops.log 2>&1 || status=$?
test "$status" -eq 2 && grep -q 'usage: ops <command>' smoke-ops.log || {
  echo 'ops did not print its usage for a missing command.' >&2; exit 1;
}
