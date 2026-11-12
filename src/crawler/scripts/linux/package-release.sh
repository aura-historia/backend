#!/usr/bin/env bash
# Packages the crawler's release binaries and example systemd units as
# crawler-<sha>.tar.gz plus crawler-<sha>.tar.gz.sha256, which `deploy.sh` installs on the host.
# Usage: package-release.sh <full lowercase source sha> <cargo release target dir> <output dir>
# Contract: docs/crawler/operations.md#build-and-release
set -euo pipefail

readonly BINARIES=(server review-mcp review-agent-runner)
readonly UNITS=(crawler-server.service review-agent-runner.service)

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CRAWLER_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"

[[ $# -eq 3 ]] || { echo "usage: ${0##*/} <sha> <release target dir> <output dir>" >&2; exit 2; }
sha="$1"
release_dir="$2"
out_dir="$3"
[[ "$sha" =~ ^[a-f0-9]{40}$ ]] || { echo "the source must be a full lowercase SHA" >&2; exit 2; }

name="crawler-${sha}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
root="${stage}/${name}"
mkdir -p "${root}/systemd" "$out_dir"

for binary in "${BINARIES[@]}"; do
  install -m 0755 "${release_dir}/${binary}" "${root}/${binary}"
done
for unit in "${UNITS[@]}"; do
  install -m 0644 "${SCRIPT_DIR}/${unit}" "${root}/systemd/${unit}"
done

# The crawler-local migrations this build applies at startup: `deploy.sh` compares them with the
# running release's to decide whether a rollback or an upgrade needs a database restore or backup.
(cd "${CRAWLER_DIR}/migrations" && find . -maxdepth 1 -type f -name '*.sql' -printf '%f\n' | LC_ALL=C sort) \
  > "${root}/MIGRATIONS"
printf '%s\n' "$sha" > "${root}/SOURCE_SHA"

# Reproducible archive: the same inputs give the same bytes.
tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
  -C "$stage" -czf "${out_dir}/${name}.tar.gz" "$name"
(cd "$out_dir" && sha256sum "${name}.tar.gz" > "${name}.tar.gz.sha256")
echo "packaged ${out_dir}/${name}.tar.gz"
