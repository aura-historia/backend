#!/usr/bin/env bash
# Fills a capped tier slice past its MemoryMax and checks that the serving tier keeps
# its memory. See host/README.md.
#
# Usage: sudo host/stress-test.sh [slice]...   (default: aura-batch.slice user.slice)
set -euo pipefail

readonly SERVING=/sys/fs/cgroup/aura-serving.slice
readonly CHUNK_MIB=256

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

[[ ${EUID} -eq 0 ]] || fail "run as root: systemd-run must place the load in a system slice"

# The serving containers, as "<scope> <name>" lines.
serving_containers() {
  docker ps -q | while read -r id; do
    docker inspect "${id}" --format '{{.HostConfig.CgroupParent}} {{.Id}} {{.Name}}'
  done | awk '$1 == "aura-serving.slice" {print "docker-" $2 ".scope", substr($3, 2)}'
}

memory_event() { awk -v key="$2" '$1 == key {print $2}' "$1/memory.events"; }
anon_mib() { awk '$1 == "anon" {print int($2 / 1048576)}' "$1/memory.stat"; }

snapshot() {
  serving_containers | while read -r scope name; do
    local cgroup=${SERVING}/${scope}
    echo "${name} oom_kill=$(memory_event "${cgroup}" oom_kill) anon_mib=$(anon_mib "${cgroup}") started=$(docker inspect "${name}" --format '{{.State.StartedAt}}')"
  done
}

stress_slice() {
  local slice=$1 max limit_mib
  max="$(cat "/sys/fs/cgroup/${slice}/memory.max")"
  # Without a cap the load would take memory from the serving tier instead of proving
  # the cap; refuse rather than test that.
  [[ ${max} != max ]] || fail "${slice} has no MemoryMax; run 'install.sh cgroups' first"
  limit_mib=$(( max / 1048576 + 2048 ))
  log "${slice}: memory.max $(( max / 1048576 )) MiB; allocating until the kernel stops it"

  local before after oom_before
  before="$(snapshot)"
  oom_before="$(memory_event "/sys/fs/cgroup/${slice}" oom_kill)"
  # The loader stops itself at memory.max + 2 GiB, so a cap that does not hold ends the
  # test with exit 3 instead of exhausting the host.
  set +e
  systemd-run --quiet --wait --collect --slice="${slice}" --unit="aura-stress-$$" \
    python3 -I -c '
import sys
limit_mib, chunk_mib = int(sys.argv[1]), int(sys.argv[2])
chunks = []
while len(chunks) * chunk_mib < limit_mib:
    chunks.append(bytearray(b"\x01") * (chunk_mib << 20))
sys.exit(3)
' "${limit_mib}" "${CHUNK_MIB}"
  local status=$?
  set -e
  after="$(snapshot)"

  log "${slice}: loader exit ${status}, slice oom_kill ${oom_before} -> $(memory_event "/sys/fs/cgroup/${slice}" oom_kill)"
  [[ ${status} -ne 3 ]] || fail "${slice}: the loader passed memory.max + 2 GiB; the cap does not hold"
  printf 'serving before:\n%s\nserving after:\n%s\n' "${before}" "${after}"
  [[ "$(awk '{print $1, $2, $4}' <<<"${before}")" == "$(awk '{print $1, $2, $4}' <<<"${after}")" ]] \
    || fail "a serving container was OOM-killed or restarted during the ${slice} test"
}

slices=("$@")
[[ ${#slices[@]} -gt 0 ]] || slices=(aura-batch.slice user.slice)
[[ -d ${SERVING} ]] || fail "aura-serving.slice is not active"
for slice in "${slices[@]}"; do
  stress_slice "${slice}"
done
log "serving tier kept its memory in every test"
