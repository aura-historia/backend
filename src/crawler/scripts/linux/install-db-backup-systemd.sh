#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
crawler_dir="$(cd -- "${script_dir}/../.." && pwd)"
template_dir="${script_dir}/systemd"
systemd_dir="${SYSTEMD_DIR:-/etc/systemd/system}"
backup_retention_days="${BACKUP_RETENTION_DAYS:-14}"
unit_name="crawler-db-backup"

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "Run this with sudo." >&2
  exit 1
fi

if [[ ! -f "${template_dir}/${unit_name}.service.in" ]]; then
  echo "Missing template: ${template_dir}/${unit_name}.service.in" >&2
  exit 1
fi

if [[ ! -f "${template_dir}/${unit_name}.timer.in" ]]; then
  echo "Missing template: ${template_dir}/${unit_name}.timer.in" >&2
  exit 1
fi

render_template() {
  local input_file="$1"
  local output_file="$2"

  sed \
    -e "s#__CRAWLER_DIR__#${crawler_dir}#g" \
    -e "s#__BACKUP_RETENTION_DAYS__#${backup_retention_days}#g" \
    "${input_file}" > "${output_file}"
}

render_template "${template_dir}/${unit_name}.service.in" "${systemd_dir}/${unit_name}.service"
render_template "${template_dir}/${unit_name}.timer.in" "${systemd_dir}/${unit_name}.timer"

systemctl daemon-reload
systemctl enable --now "${unit_name}.timer"

systemctl list-timers --all "${unit_name}.timer"

echo "Installed ${unit_name}.timer"
echo "Backup retention: ${backup_retention_days} day(s)"
echo "Edit ${systemd_dir}/${unit_name}.timer to change cadence."
