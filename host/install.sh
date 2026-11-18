#!/usr/bin/env bash
# Installs the host's cgroup tiers and network baseline. See host/README.md.
#
# Usage: sudo host/install.sh <step>...
#   cgroups   install the tier slices, cap user.slice, move the crawler units to the
#             batch tier, default Docker containers to user.slice, remove the
#             OpenSearch CPU reservation
#   ssh       accept SSH keys only (refuses while a recent SSH user has no key)
#   firewall  ufw: deny inbound except SSH and 443; the same for Docker-published ports
#   check     print the effective state without changing anything
#
# Every step is idempotent; rerun it after a partial failure.
set -euo pipefail

HOST_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly HOST_DIR
readonly SYSTEMD_DIR=/etc/systemd/system
readonly TIER_SLICES=(aura_serving.slice aura_pipeline.slice aura_batch.slice)
readonly BATCH_UNITS=(crawler-server.service review-agent-runner.service crawler-backup.service crawler-autodeploy.service)
readonly RETIRED_SLICE=auraopensearch.slice
readonly RESERVATION_UNITS=(init.scope system.slice user.slice machine.slice)
readonly DOCKER_DAEMON_CONFIG=/etc/docker/daemon.json
readonly DOCKER_DEFAULT_PARENT=user.slice
readonly SSH_DROP_IN=/etc/ssh/sshd_config.d/10-aura-keys-only.conf
readonly UFW_BLOCK_BEGIN='# BEGIN aura-historia docker ingress'
readonly UFW_BLOCK_END='# END aura-historia docker ingress'

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

require_root() { [[ ${EUID} -eq 0 ]] || fail "run as root (sudo $0 $*)"; }

require_cgroup_v2() {
  [[ "$(stat -fc %T /sys/fs/cgroup)" == cgroup2fs ]] || fail "cgroup v2 is required"
  [[ "$(docker info --format '{{.CgroupDriver}}')" == systemd ]] || fail "Docker must use the systemd cgroup driver"
}

cgroup_populated() {
  local events=/sys/fs/cgroup/$1/cgroup.events
  [[ -f ${events} ]] && grep -qx 'populated 1' "${events}"
}

# Removes a drop-in only when it sets AllowedCPUs and nothing else, so an unrelated
# operator setting in the same directory is never deleted.
remove_cpu_drop_in() {
  local file=$1
  [[ -f ${file} ]] || return 0
  if grep -v -E '^\s*(#|$|\[(Slice|Scope)\]|AllowedCPUs=)' "${file}" | grep -q .; then
    fail "${file} sets more than AllowedCPUs; review it by hand"
  fi
  rm -- "${file}"
  rmdir --ignore-fail-on-non-empty -- "$(dirname -- "${file}")"
  log "removed ${file}"
}

install_cgroups() {
  require_cgroup_v2
  for slice in "${TIER_SLICES[@]}"; do
    install -m 0644 "${HOST_DIR}/systemd/${slice}" "${SYSTEMD_DIR}/${slice}"
  done
  install -D -m 0644 "${HOST_DIR}/systemd/user-slice-tier.conf" "${SYSTEMD_DIR}/user.slice.d/50-aura-tier.conf"
  for unit in "${BATCH_UNITS[@]}"; do
    install -D -m 0644 "${HOST_DIR}/systemd/batch-member.conf" "${SYSTEMD_DIR}/${unit}.d/50-aura-batch.conf"
  done

  remove_cpu_drop_in "${SYSTEMD_DIR}/init.scope.d/60-aura-opensearch-cpus.conf"
  for unit in "${RESERVATION_UNITS[@]}"; do
    remove_cpu_drop_in "/etc/systemd/system.control/${unit}.d/50-AllowedCPUs.conf"
  done

  systemctl daemon-reload
  # Neither daemon-reload nor an empty AllowedCPUs= rewrites a running unit's cpuset,
  # so widen it to every online CPU until the next boot, which starts without a mask.
  for unit in "${RESERVATION_UNITS[@]}"; do
    systemctl set-property --runtime "${unit}" AllowedCPUs="$(cat /sys/devices/system/cpu/online)" \
      || log "could not widen AllowedCPUs on ${unit}"
  done
  systemctl enable --now "${TIER_SLICES[@]}"
  log "tier slices active; crawler units join aura_batch.slice at their next start"
  default_docker_parent

  retire_opensearch_slice
  verify_all_cpus_allowed
}

# PID 1, Docker and this shell stand for init.scope, system.slice and user.slice.
verify_all_cpus_allowed() {
  local online pid allowed
  online="$(cat /sys/devices/system/cpu/online)"
  for pid in 1 "$(pgrep -o dockerd)" $$; do
    allowed="$(awk '/^Cpus_allowed_list/ {print $2}' "/proc/${pid}/status")"
    [[ ${allowed} == "${online}" ]] || fail "PID ${pid} may still use only CPUs ${allowed} of ${online}; reboot to apply the removal"
  done
  log "host processes may use every CPU (${online})"
}

# A container that names no tier would otherwise run uncapped in system.slice; ad-hoc
# containers such as test fixtures belong to the sessions that start them.
default_docker_parent() {
  local current merged
  current="$(cat "${DOCKER_DAEMON_CONFIG}" 2>/dev/null || echo '{}')"
  [[ "$(jq -r '."cgroup-parent" // empty' <<<"${current}")" == "${DOCKER_DEFAULT_PARENT}" ]] && return 0
  merged="$(jq --arg parent "${DOCKER_DEFAULT_PARENT}" '. + {"cgroup-parent": $parent}' <<<"${current}")"
  install -D -m 0644 /dev/stdin "${DOCKER_DAEMON_CONFIG}" <<<"${merged}"
  log "Docker defaults containers to ${DOCKER_DEFAULT_PARENT}; it applies after 'systemctl restart docker' (host/README.md)"
}

retire_opensearch_slice() {
  if ! systemctl cat "${RETIRED_SLICE}" >/dev/null 2>&1; then
    return 0
  fi
  if cgroup_populated "${RETIRED_SLICE}"; then
    log "${RETIRED_SLICE} still has processes: recreate OpenSearch in aura_serving.slice (host/README.md), then rerun 'cgroups'"
    return 0
  fi
  systemctl disable --now "${RETIRED_SLICE}"
  rm -f -- "${SYSTEMD_DIR}/${RETIRED_SLICE}"
  systemctl daemon-reload
  log "retired ${RETIRED_SLICE}"
}

# Terminal logins in both wtmp files, plus every password login sshd logged in the
# journal (which also covers scp, rsync and other sessions without a terminal).
recent_ssh_users() {
  {
    last -w --nohostname 2>/dev/null
    last -w --nohostname -f /var/log/wtmp.1 2>/dev/null
  } | awk '$2 ~ /^pts\// {print $1}'
  journalctl --quiet --no-pager -o cat -t sshd -t sshd-session --since -60d \
    --grep 'Accepted (password|keyboard-interactive)' 2>/dev/null |
    sed -nE 's/.*Accepted [a-z/-]+ for (invalid user )?([^ ]+) from.*/\2/p'
}

# Every such account must hold a key, or turning off passwords locks it out.
ssh_users_without_keys() {
  local user home
  recent_ssh_users | sort -u | while read -r user; do
    home="$(getent passwd "${user}" | cut -d: -f6)" || continue
    [[ -s ${home}/.ssh/authorized_keys || -s ${home}/.ssh/authorized_keys2 ]] || echo "${user}"
  done
}

install_ssh() {
  local missing
  missing="$(ssh_users_without_keys)"
  [[ -z ${missing} ]] || fail "these recent SSH users have no ~/.ssh/authorized_keys: ${missing//$'\n'/ }. Install their keys and log in with them first."
  install -D -m 0644 "${HOST_DIR}/ssh/10-aura-keys-only.conf" "${SSH_DROP_IN}"
  if ! sshd -t; then
    rm -f -- "${SSH_DROP_IN}"
    fail "sshd rejected the configuration; drop-in removed"
  fi
  systemctl try-reload-or-restart ssh.service
  log "SSH accepts keys only; keep this session open and test a new key login before closing it"
}

# These awk programs read to the end: under pipefail, exiting early would kill the
# writer with SIGPIPE and fail the whole step.
ssh_port() { sshd -T | awk '$1 == "port" && !seen++ {print $2}'; }

external_interface() { ip -o route show default | awk 'NR == 1 {print $5}'; }

install_ufw_docker_block() {
  local interface=$1 rules
  for rules in /etc/ufw/after.rules /etc/ufw/after6.rules; do
    sed -i "/^${UFW_BLOCK_BEGIN}\$/,/^${UFW_BLOCK_END}\$/d" "${rules}"
    sed "s/@EXTERNAL_INTERFACE@/${interface}/g" "${HOST_DIR}/firewall/docker-user.rules" >>"${rules}"
  done
}

install_firewall() {
  local port interface stale
  port="$(ssh_port)"
  interface="$(external_interface)"
  [[ -n ${port} ]] || fail "cannot read the SSH port from sshd -T"
  [[ -n ${interface} ]] || fail "no default route; cannot name the external interface"
  stale="$(stale_ufw_rules "${port}")"
  [[ -z ${stale} ]] || fail "ufw holds rules this step does not own; delete them (ufw delete ...) or keep them by hand, then rerun:"$'\n'"${stale}"

  # ufw allow is idempotent; allow SSH before anything can enable the default deny.
  ufw allow "${port}/tcp" comment 'SSH'
  ufw allow 443/tcp comment 'HTTPS'
  ufw allow 443/udp comment 'HTTP/3'
  ufw default deny incoming
  ufw default allow outgoing
  install_ufw_docker_block "${interface}"
  ufw --force enable
  ufw reload
  log "firewall: inbound ${port}/tcp and 443 only, including Docker-published ports on ${interface}"
}

# Rules an earlier operator added would turn on with ufw, unseen; refuse them first.
stale_ufw_rules() {
  local port=$1
  ufw show added | grep '^ufw ' | grep -v -E "^ufw allow (${port}/tcp|443/tcp|443/udp)( comment .*)?$" || true
}

print_check() {
  echo "## CPUs (online $(cat /sys/devices/system/cpu/online))"
  for pid in 1 "$(pgrep -o dockerd || true)" "$(pgrep -o -x sshd || true)" $$; do
    [[ -n ${pid} ]] && printf '%-8s %-50s %s\n' "${pid}" "$(cut -d: -f3 "/proc/${pid}/cgroup")" "$(awk '/Cpus_allowed_list/ {print $2}' "/proc/${pid}/status")"
  done
  echo "## Slices"
  for unit in "${TIER_SLICES[@]}" user.slice "${RETIRED_SLICE}"; do
    systemctl show "${unit}" -p Id -p ActiveState -p CPUWeight -p IOWeight -p MemoryLow -p MemoryMax -p MemoryCurrent | paste -sd' '
  done
  echo "## Batch units"
  for unit in "${BATCH_UNITS[@]}"; do
    systemctl show "${unit}" -p Id -p Slice -p ActiveState -p ControlGroup | paste -sd' '
  done
  echo "## Containers"
  docker ps --format '{{.Names}}' | while read -r name; do
    docker inspect "${name}" --format '{{.Name}} parent={{.HostConfig.CgroupParent}} mem_limit={{.HostConfig.Memory}} reservation={{.HostConfig.MemoryReservation}} oom_score_adj={{.HostConfig.OomScoreAdj}}'
  done
  echo "## SSH"
  sshd -T | grep -E '^(passwordauthentication|kbdinteractiveauthentication|permitrootlogin|authenticationmethods|pubkeyauthentication) '
  echo "## Firewall"
  ufw status verbose
  iptables -S DOCKER-USER || true
  echo "## Listening on non-loopback addresses"
  ss -Htlnu | awk '$5 !~ /^(127\.|\[::1\]|\[::ffff:127\.)/ {print $1, $5}' | sort -u
}

main() {
  [[ $# -gt 0 ]] || { sed -n '2,11p' "$0"; exit 2; }
  require_root "$@"
  for step in "$@"; do
    case ${step} in
      cgroups) install_cgroups ;;
      ssh) install_ssh ;;
      firewall) install_firewall ;;
      check) print_check ;;
      *) fail "unknown step ${step}" ;;
    esac
  done
}

main "$@"
