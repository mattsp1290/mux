# SSH Stack Validation

Spec: docs/04-ssh-trust-and-transport.md  
Status: Active  
Linked from: prompts/docs/README.md

## What this document covers

The mux client performs all SSH operations using a Rust SSH library. This document
validates the choice and lists required capabilities that the chosen library must satisfy.

## Requirements derived from the spec

From docs/04, the SSH stack must support:

### Authentication

| Requirement | Notes |
|-------------|-------|
| `ssh-agent` socket access via `SSH_AUTH_SOCK` | Key auto-discovery: iterate agent-offered keys |
| No password authentication | Only agent-forwarded keys are used |
| Agent forwarding for remote sessions | Required by `mux create` (clone step) |

### TOFU / host-key verification

| Requirement | Notes |
|-------------|-------|
| Collect server host key before completing handshake | Required for TOFU first-contact flow |
| Intercept key-mismatch events programmatically | Must abort on mismatch without connecting |
| Supply a custom known-hosts file or in-memory check | Checked against `known_host_fingerprints` table |
| Algorithms: `ssh-ed25519`, `ecdsa-sha2-nistp256`, `rsa-sha2-512`, `rsa-sha2-256` | Plus verbatim storage of unknown algorithms |

### Transport channels

| Requirement | Notes |
|-------------|-------|
| Unix domain socket forwarding (`SocketPath` / streamlocal) | Preferred transport for agent RPC |
| Direct TCP forwarding | Fallback transport for agent RPC |
| Non-interactive (no PTY) command execution | For TOFU probe, transport probe, workdir ops |
| PTY allocation (`-t` flag equivalent) | For `tmux attach-session` in `mux attach` |

### MOTD noise handling

The library must allow raw stdout/stderr capture from remote commands so the caller
can strip MOTD/banner noise before the first sentinel-matching line.

## Candidate: `russh`

`russh` (https://crates.io/crates/russh) is the primary candidate:
- Supports `ssh-agent` integration via `russh-keys` + `SSH_AUTH_SOCK`.
- Programmatic host-key callbacks (reject or accept per key).
- Direct-tcpip channel (TCP forwarding).
- Exec channels (non-interactive commands).
- PTY request on exec channels.
- Streamlocal (Unix socket) forwarding: check `russh` release notes for `direct-streamlocal` support — this is the key capability to verify.

**Streamlocal gap**: If `russh` does not support `direct-streamlocal` in the workspace-pinned version, the TCP fallback path must be the default and streamlocal must be disabled. Update `docs/04 §Transport selection` if this is the case and document the version at which it becomes available.

## Candidate: `ssh2` (libssh2 bindings)

`ssh2` (https://crates.io/crates/ssh2) is the fallback:
- Mature, widely used, C library under the hood.
- Agent forwarding: supported.
- Programmatic host-key checking: `session.host_key()` returns raw key bytes.
- Direct-tcpip: `session.channel_direct_tcpip()`.
- Streamlocal: `session.channel_direct_streamlocal()` — verify availability.

Downside: FFI overhead, C linkage complicates cross-compilation for arm64 agent deployment.

## Validation checklist

Before marking `mux-a2b` closed:

- [ ] Confirm chosen library version is pinned in `Cargo.toml`.
- [ ] Streamlocal forwarding compiles and connects on target platform (Linux x86_64 and arm64).
- [ ] Host-key callback fires before connection completes — verify with a test host with wrong fingerprint.
- [ ] Agent key enumeration works against a real `ssh-agent` socket.
- [ ] Exec channel captures stdout/stderr independently.
- [ ] PTY allocation works for `tmux attach-session`.

## Exclusions

- No raw `openssh` process spawning for anything except `mux attach` (which uses `exec ssh`).
- `mux attach` explicitly uses `exec ssh` per docs/07 §Attach flow — the SSH library is not used there.
