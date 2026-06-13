# Release and Deployment Notes

Spec: docs/01-cli-commands.md §mux agent deploy, docs/08 §CI gates  
Status: Active  
Linked from: prompts/docs/README.md

## Artifacts

Two binary artifacts are produced per release:

| Binary | Targets | Purpose |
|--------|---------|---------|
| `mux` | Host OS (Linux amd64, macOS amd64/arm64) | Local CLI installed by the developer |
| `mux-agent` | Linux amd64, Linux arm64 | Uploaded to remote hosts via `mux agent deploy` |

`mux-agent` is built as a statically linked musl binary so it runs on any Linux distribution without glibc version constraints:
- `x86_64-unknown-linux-musl` → `mux-agent-amd64`
- `aarch64-unknown-linux-musl` → `mux-agent-arm64`

## Cross-compilation toolchain

Use [`cross`](https://github.com/cross-rs/cross) for musl cross-compilation.

`Cross.toml` at the repo root configures any custom image overrides (currently none required; `cross` default images handle both targets).

### Installing cross

```bash
cargo install cross --git https://github.com/cross-rs/cross
```

### Building mux-agent for both targets

```bash
cross build --release --target x86_64-unknown-linux-musl -p mux-agent
cross build --release --target aarch64-unknown-linux-musl -p mux-agent

# Stage to dist/
mkdir -p dist
cp target/x86_64-unknown-linux-musl/release/mux-agent dist/mux-agent-amd64
cp target/aarch64-unknown-linux-musl/release/mux-agent dist/mux-agent-arm64
chmod +x dist/mux-agent-amd64 dist/mux-agent-arm64
```

## Deploy path lookup

`mux agent deploy <alias>` selects the agent binary in this order:

1. **`MUX_AGENT_BINARY` env var** — explicit path to the binary; used as-is.
2. **Built-in lookup** — `dist/mux-agent-{arch}` where `{arch}` is the value stored in `hosts.arch` after `mux host test` runs (e.g. `amd64` or `arm64`).

Arch values are normalised by `normalize_arch()` in `crates/mux-cli/src/host.rs`:

| Raw `uname -m` | Stored arch |
|----------------|-------------|
| `x86_64` | `amd64` |
| `aarch64` | `arm64` |
| anything else | passed through as-is |

Built-in lookup paths (relative to CWD when `mux` is invoked):

| Arch | Default path |
|------|-------------|
| `amd64` | `dist/mux-agent-amd64` |
| `arm64` | `dist/mux-agent-arm64` |

If the binary is not found at the resolved path, `mux agent deploy` exits with exit code 1 and a human-readable error prefixed `mux: `.

### MUX_AGENT_BINARY override

`MUX_AGENT_BINARY` is intended for:
- CI/CD pipelines that build and stage the binary at a non-default path.
- Testing with a locally-built debug binary.
- Deploying a pinned version to a specific host without updating dist/.

Example:
```bash
MUX_AGENT_BINARY=/tmp/mux-agent-custom mux agent deploy prod
```

## Deploy verification

After upload via SSH, `mux agent deploy` verifies the remote binary:

1. Checks that the remote file size matches the local binary size.
2. Computes SHA-256 of local binary and runs `sha256sum` on the remote; compares.
3. Sets executable bit (`chmod +x <home>/.mux/bin/mux-agent`).
4. Persists version to `agent_versions` table only after successful verification.
5. If agent is already running, attempts graceful `Shutdown` RPC before fallback kill.

If any verification step fails, deploy exits 1 and leaves the remote binary in place (does not delete a partial upload).

## Local mux CLI packaging

For developer install, build and copy the binary:

```bash
cargo build --release -p mux
# Install to ~/.local/bin/ (ensure it's on PATH)
cp target/release/mux ~/.local/bin/mux
```

No system-wide installer is defined in v0.1. Distribution via package managers (Homebrew, apt) is deferred.

## CI release job (GitHub Actions)

The release workflow (`.github/workflows/release.yml`) runs on tag push (`v*`):

```
1. cargo build --release -p mux          (host runner, Linux amd64)
2. cross build --release --target x86_64-unknown-linux-musl -p mux-agent
3. cross build --release --target aarch64-unknown-linux-musl -p mux-agent
4. Stage binaries to dist/ and upload as GitHub release assets
5. Artifact checks: verify each binary is non-zero bytes and executable
```

The unit-test CI (`.github/workflows/ci.yml`) does NOT build mux-agent cross targets — that is release-only to avoid Docker overhead on every PR.

## Constraints and decisions

- musl static linking is required — mux-agent runs on heterogeneous remote hosts with unknown glibc versions.
- `cross` is chosen over a GitHub Actions matrix with native arm64 runners because: (a) musl static linking is the goal, not native-OS testing; (b) `cross` is simpler to run locally; (c) native arm64 runners on GitHub Actions require paid plans.
- `dist/` is gitignored — it is a build artifact directory, not source. The `cross` build step populates it during CI.
- Binary naming: `mux-agent-{arch}` matches the arch string stored in `hosts.arch` (amd64, arm64) so the deploy lookup is a simple path join with no translation layer.
