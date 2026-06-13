# 08 — Errors, Observability, and Tests

## Error handling

### Error prefix

All user-facing errors are prefixed `mux: ` (lowercase, colon, space).

### Error categories

Errors are categorised at the command boundary:

| Category | Exit code | Description |
|---|---|---|
| User input | 1 | Invalid arguments, unknown alias, bad format |
| Host error | 1 | TOFU mismatch, connection refused, timeout |
| Remote error | 1 | RPC failure, agent error, SSH exec failure |
| Internal | 2 | Unexpected panics, DB corruption, impossible state |

### Human-readable hints

Errors that require user action must include a hint:
- `ssh_agent_not_forwarded`: "Run `ssh-add` to load your key into ssh-agent."
- `host_key_mismatch`: "Use `mux host trust <alias>` to review and rotate the key."
- `workdir_pre_existing`: "Remove the existing directory or use a different host."

### create flow observability

Metrics emitted during `mux create` (as tracing spans/events):
- `create_duration`: total time for the create flow (ms).
- `git_clone_duration`: time spent in git clone (ms).
- `error_count` by `host` and `category`.

## Logging

- `tracing` crate; `tracing-subscriber` with `EnvFilter`.
- Default level: `info` when `RUST_LOG` is unset.
- Format: single-line human-readable for TTY; JSON for non-TTY (future).
- Agent log written to `<home>/.mux/agent.log`.

## Test strategy

### Unit tests

- Location: `#[cfg(test)]` modules within each crate.
- Coverage requirements: every error enum variant must have at least one test that
  triggers it. Every public function with documented edge cases must have tests for
  those cases.
- No network, no filesystem (use `tempfile` for FS-required tests), no real SSH.

### Integration tests

- Location: `tests/` at workspace root or per-crate.
- Test environment: controlled SSH/tmux hosts (see mux-3bv for environment plan).
- Required scenarios:
  - `mux init`: MUX_HOME, default `~/.mux`, private permissions, repeated runs.
  - Host: add/list/remove/test/trust round-trip.
  - Create: success path, workdir pre-existing, git clone failure, ssh-agent missing.
  - List: import, orphan, resurrection, unreachable host.
  - Status: UUID lookup, shortname, unreachable host.
  - Kill: ownership, mismatch refusal, no-op, dead mark.
  - Attach: dead session rejected, argv pinning.
  - Agent: deploy, logs, stop, lifecycle races.

### CI gates (separate steps)

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace` (unit tests)
4. Integration tests (when environment is available)
5. Artifact checks (release binaries)

### Test utilities

Workspace dev-dependencies available for all crates:
- `assert_cmd` — CLI invocation assertions.
- `predicates` — output matching predicates.
- `tempfile` — temporary directories and files.

## Observability targets

- `mux create`: span covering the full create transaction.
- Agent startup: span for readiness poll.
- SSH connection: span per connection attempt.
- RPC calls: span per call with method + duration.
