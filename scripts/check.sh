#!/usr/bin/env bash
# Full CI gate — delegates to `make check` so there is one source of truth.
# Exit 1 on first failure.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" \
  || { echo "error: not inside a git repository" >&2; exit 1; }
cd "$ROOT"

exec make check
