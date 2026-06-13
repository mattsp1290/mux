#!/usr/bin/env bash
# Full CI gate: fmt-check → clippy → unit tests.
# Exit 1 on first failure. Mirrors the `make check` target.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

echo "==> cargo fmt --check"
cargo fmt --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace

echo "All checks passed."
