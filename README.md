# mux

tmux + persistence — clean-room Rust reimplementation.

## Quick Start

`mux` lets you create persistent tmux sessions on remote hosts over SSH, then attach and detach from them as if you never left.

**Prerequisites:** `ssh-agent` running with your key loaded, tmux ≥ 3.0 on each remote host.

```sh
# One-time setup
mux init
mux host add myserver alice@192.168.1.10
mux host test myserver          # connects, probes, and stores the host key (TOFU)
mux agent deploy myserver       # uploads the mux-agent binary

# Create a session from a GitHub repo
mux create alice/myproject --host myserver

# List and attach
mux list
mux attach myproject
```

For the full command reference, TOFU details, troubleshooting, and environment variables, see the **[User Guide](docs/guide.md)**.

---

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
cargo fmt --all --check

# Lint (warnings are errors; --workspace + --all-targets covers every crate)
cargo clippy --workspace --all-targets -- -D warnings

# Unit tests
cargo test --workspace

# Build both binaries
cargo build --bins

# Release build
cargo build --release --bins
```

See `Makefile` for all available targets (`make fmt`, `make lint`, `make test`, `make build`, etc.).
