# mux

tmux + persistence — clean-room Rust reimplementation.

## Workspace layout

```
crates/
  mux/          # mux CLI binary
  mux-agent/    # mux-agent remote daemon binary
  mux-cli/      # CLI subcommand implementations
  mux-core/     # Shared domain types and validation
  mux-state/    # SQLite local state layer
  mux-ssh/      # SSH trust, TOFU, and transport
  mux-rpc/      # RPC client/server bindings
  mux-tmux/     # tmux adapter (direct argv, no shell)
proto/          # RPC protocol definitions
```

## Development commands

Run the full CI gate (fmt-check + clippy + unit tests) with either:

```sh
make check
# or
./scripts/check.sh
```

Individual commands:

```sh
# Format check (no-op on CI; run `cargo fmt --all` to fix locally)
cargo fmt --check

# Lint (warnings are errors; --all-targets includes tests and examples)
cargo clippy --all-targets -- -D warnings

# Unit tests
cargo test --workspace

# Build both binaries
cargo build --bins

# Release build
cargo build --release --bins
```

See `Makefile` for all available targets (`make fmt`, `make lint`, `make test`, `make build`, etc.).
