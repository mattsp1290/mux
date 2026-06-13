.PHONY: fmt fmt-check lint test test-unit test-integration build build-release all check

# Format all crates in-place
fmt:
	cargo fmt --all

# Check formatting without modifying files (CI gate)
fmt-check:
	cargo fmt --all --check

# Lint: warnings are errors; --workspace is explicit so adding a root [package]
# to Cargo.toml later doesn't silently narrow coverage
lint:
	cargo clippy --workspace --all-targets -- -D warnings

# Unit tests only
test-unit:
	cargo test --workspace

# Integration tests — requires Docker and the mux binary to be built first.
# Usage:
#   make build && make test-integration          # uses target/debug/mux
#   MUX_BIN=/path/to/mux make test-integration  # explicit binary path
test-integration:
	cargo test -p mux-integration-tests --test integration --features integration-tests -- --test-threads=1

# All tests (unit + integration)
# Note: test-integration requires Docker; use test-unit for Docker-free runs.
test: test-unit test-integration

# Debug build of both binaries
build:
	cargo build --bins

# Release build of both binaries
build-release:
	cargo build --release --bins

# Full CI gate: fmt-check + lint + test
check: fmt-check lint test

all: check build
