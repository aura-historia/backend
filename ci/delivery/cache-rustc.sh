#!/usr/bin/env bash
set -euo pipefail
# Cargo nests RUSTC_WORKSPACE_WRAPPER inside RUSTC_WRAPPER for workspace crates.
# Recompile application code; only external dependencies enter the compiler cache.
if [ "$1" = /control/cache-rustc.sh ]; then
  shift
  exec "$@"
fi
exec sccache "$@"
