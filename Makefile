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

# Integration tests (placeholder until mux-3bv scaffolds the test environment)
test-integration:
	@echo "Integration test environment not yet configured — see mux-3bv"

# All tests (unit + integration)
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
