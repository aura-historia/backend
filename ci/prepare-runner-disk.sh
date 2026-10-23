#!/usr/bin/env bash
set -euo pipefail
# Image paths: actions/runner-images Ubuntu2404-Readme and build installers.
# No cleanup runs on a developer machine or a self-hosted runner.
mode="${1:-reclaim}"
case "$mode" in
  observe|reclaim) ;;
  *) printf 'Invalid disk preparation mode: %s\n' "$mode" >&2; exit 1 ;;
esac
printf 'disk_preparation_mode=%s\n' "$mode"
python3 -c 'import json,shutil; print(json.dumps({"disk_preparation_before":shutil.disk_usage(".")._asdict()}))'
if [ "$mode" = observe ]; then
  printf 'SDK reclamation skipped; capacity remains guarded during execution.\n'
elif [ "${RUNNER_ENVIRONMENT:-}" = github-hosted ] && [ "${RUNNER_OS:-}" = Linux ]; then
  for sdk in /usr/local/lib/android/sdk /usr/share/dotnet /opt/hostedtoolcache/CodeQL; do
    if [ -d "$sdk" ] && [ ! -L "$sdk" ]; then
      sudo du -sk -- "$sdk"
      sudo rm -rf -- "$sdk"
      printf 'unused_sdk_removed=%s\n' "$sdk"
    else
      printf 'unused_sdk_absent_or_not_a_regular_directory=%s\n' "$sdk"
    fi
  done
else
  printf 'SDK reclamation skipped: requires explicitly GitHub-hosted Linux.\n'
fi
python3 -c 'import json,shutil; print(json.dumps({"disk_preparation_after":shutil.disk_usage(".")._asdict()}))'
