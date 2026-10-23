#!/usr/bin/env bash
set -euo pipefail
# Swatinem/rust-cache hashes every installed toolchain into its key. Removing the
# runner image's unused toolchains keeps keys stable across image updates.
# No toolchain is removed on a developer machine or a self-hosted runner.
if [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; then
  printf 'Toolchain retention skipped: requires a GitHub-hosted runner.\n'
  exit 0
fi
active="$(rustup show active-toolchain)"
active="${active%% *}"
rustup default "$active"
mapfile -t installed < <(rustup toolchain list --quiet)
for toolchain in "${installed[@]}"; do
  if [ "$toolchain" != "$active" ]; then
    rustup toolchain uninstall "$toolchain"
  fi
done
remaining="$(rustup toolchain list --quiet)"
if [ "$remaining" != "$active" ]; then
  printf 'Expected only %s, found:\n%s\n' "$active" "$remaining" >&2
  exit 1
fi
printf 'retained_toolchain=%s\n' "$active"
