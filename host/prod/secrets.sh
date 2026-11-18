#!/usr/bin/env bash
# The prod project's host configuration and secret files (host/prod/README.md#secrets).
# Run as root.
#
# Usage: sudo host/prod/secrets.sh <command>
#   init   create every file the project reads from host/prod/templates: PostgreSQL role
#          passwords and the pgBackRest passphrase generated, other values left for the
#          operator. Never overwrites an existing file.
#   check  fail on a missing file, a wrong owner or mode, or an empty required value
#   copy   upload an encrypted copy of the configuration directory to the backup bucket
set -euo pipefail

PROD_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROD_DIR
readonly TEMPLATE_DIR="${PROD_DIR}/templates"
readonly CONFIG_DIR=/etc/aura-historia/prod
readonly RECIPIENT="${CONFIG_DIR}/backup-recipient.asc"
readonly PUBLIC_FILES=(opensearch-root-ca.pem google-credentials.json)
readonly COPY_KEY=secrets/aura-prod-config.tar.gz.gpg

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Paths of every template relative to the template directory, e.g. secrets/aws.env.
templates() { (cd "${TEMPLATE_DIR}" && find . -name '*.env' -printf '%P\n' | sort); }

# The value of KEY in an env file; empty when unset. Values are never printed by this script.
value_of() { sed -n "s/^$1=//p" "$2" | tail -n 1; }

init() {
  install -d -o root -g root -m 0700 "${CONFIG_DIR}" "${CONFIG_DIR}/secrets"
  local relative target group content holders existing
  declare -A generated=()
  # One value per group, shared by every file that names it, such as the runtime password in
  # postgres-runtime.env and postgres-bootstrap.env. A group is generated only while none of
  # its files exists, so the copies always agree.
  for group in $(grep -rhoE '@generate:[a-z-]+@' "${TEMPLATE_DIR}" | sort -u); do
    holders="$(grep -rl -- "${group}" "${TEMPLATE_DIR}" | sed "s|^${TEMPLATE_DIR}/||")"
    existing=0
    for relative in ${holders}; do
      [[ -e ${CONFIG_DIR}/${relative} ]] && existing=$((existing + 1))
    done
    if [[ ${existing} -eq 0 ]]; then
      generated[${group}]="$(openssl rand -hex 32)"
    elif [[ ${existing} -ne $(wc -w <<<"${holders}") ]]; then
      fail "only some of ${holders//$'\n'/, } exist under ${CONFIG_DIR}; restore the rest from the encrypted copy instead of generating ${group}"
    fi
  done
  while read -r relative; do
    target="${CONFIG_DIR}/${relative}"
    [[ -e ${target} ]] && continue
    content="$(cat "${TEMPLATE_DIR}/${relative}")"
    for group in "${!generated[@]}"; do
      content="${content//${group}/${generated[${group}]}}"
    done
    install -o root -g root -m 0600 /dev/stdin "${target}" <<<"${content}"
    log "created ${target}"
  done < <(templates)
  log "fill in the empty values, install ${PUBLIC_FILES[*]} (host/prod/README.md#secrets), then run 'check'"
}

check() {
  local relative target key problems=0
  for relative in $(templates) "${PUBLIC_FILES[@]}"; do
    target="${CONFIG_DIR}/${relative}"
    if [[ ! -f ${target} ]]; then
      echo "missing: ${target}"; problems=$((problems + 1)); continue
    fi
    if [[ ${relative} == *.env && "$(stat -c '%U:%G %a' "${target}")" != "root:root 600" ]]; then
      echo "not root:root 0600: ${target}"; problems=$((problems + 1))
    fi
    [[ ${relative} == *.env ]] || continue
    # Required keys are the template's uncommented ones; commented keys are optional.
    for key in $(sed -nE 's/^([A-Z0-9_]+)=.*/\1/p' "${TEMPLATE_DIR}/${relative}"); do
      if [[ -z "$(value_of "${key}" "${target}")" ]]; then
        echo "empty: ${key} in ${target}"; problems=$((problems + 1))
      fi
    done
    if grep -q '@generate:' "${target}"; then
      echo "ungenerated placeholder in ${target}"; problems=$((problems + 1))
    fi
  done
  for relative in "${PUBLIC_FILES[@]}"; do
    target="${CONFIG_DIR}/${relative}"
    [[ ! -f ${target} || "$(stat -c '%a' "${target}")" == 644 ]] || {
      echo "not mode 0644 (the processes run as uid 10001): ${target}"; problems=$((problems + 1));
    }
  done
  [[ ${problems} -eq 0 ]] || fail "${problems} problem(s) in ${CONFIG_DIR}"
  log "${CONFIG_DIR} is complete"
}

# The archive is encrypted to an operator's OpenPGP public key whose private key never touches
# the host; the bucket keeps earlier versions of the object.
copy() {
  [[ -s ${RECIPIENT} ]] || fail "install the operators' OpenPGP public key as ${RECIPIENT} first"
  local bucket gnupg_home
  bucket="$(value_of AURA_BACKUP_BUCKET "${CONFIG_DIR}/compose.env")"
  [[ -n ${bucket} ]] || fail "AURA_BACKUP_BUCKET is empty in ${CONFIG_DIR}/compose.env"
  gnupg_home="$(mktemp -d)"
  trap 'rm -rf -- "${gnupg_home}"' EXIT
  tar -C "$(dirname -- "${CONFIG_DIR}")" -czf - "$(basename -- "${CONFIG_DIR}")" |
    GNUPGHOME="${gnupg_home}" gpg --batch --no-tty --quiet --trust-model always \
      --recipient-file "${RECIPIENT}" --encrypt --output - |
    AWS_ACCESS_KEY_ID="$(value_of AWS_ACCESS_KEY_ID "${CONFIG_DIR}/secrets/aws.env")" \
    AWS_SECRET_ACCESS_KEY="$(value_of AWS_SECRET_ACCESS_KEY "${CONFIG_DIR}/secrets/aws.env")" \
    AWS_REGION=eu-central-1 AWS_EC2_METADATA_DISABLED=true \
      aws s3 cp --only-show-errors - "s3://${bucket}/${COPY_KEY}"
  log "uploaded the encrypted configuration to s3://${bucket}/${COPY_KEY}"
}

main() {
  [[ ${EUID} -eq 0 ]] || fail "run as root (sudo $0 $*)"
  case ${1:-} in
    init) init ;;
    check) check ;;
    copy) copy ;;
    *) sed -n '2,11p' "$0"; exit 2 ;;
  esac
}

main "$@"
