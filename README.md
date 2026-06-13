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

```sh
# Format check
cargo fmt --check

# Lint (warnings are errors)
cargo clippy -- -D warnings

# Unit tests
cargo test

# Build both binaries
cargo build --bins

# Release build
cargo build --release --bins
```
