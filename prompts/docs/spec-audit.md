# mux Spec Audit

**Date:** 2026-06-13  
**Auditor:** ralph (iteration 54)  
**Method:** Requirement-by-requirement review of docs/00–08; evidence gathered by reading codebase, searching for implementations and tests, and verifying CLI entry-point wiring.

---

## Executive Summary

The mux codebase is architecturally sound and well-tested at the unit level. All domain types, validation, local state, RPC protocol, tmux adapter, TOFU trust, and transport probing are fully implemented and tested. The clean-room guidelines are respected throughout.

The single critical gap is the **production SSH executor**: the `SshHost`, `RemoteExec`, and `DeployHost` traits have no concrete implementation for real SSH connections. This blocks CLI entry points for `host test`, `host trust`, `agent deploy/logs/stop`, `mux create`, `mux attach`, `mux list`, `mux status`, and `mux kill` — all return `bail!("SSH execution not yet implemented")` in `lib.rs`. The domain logic for each command is fully implemented and tested via `MockSshHost` / `MockDeployHost`, and requires only wiring once the executor lands.

Four smaller gaps are documented below.

---

## docs/00 — Clean-Room Guidelines

| Requirement | Status | Evidence |
|---|---|---|
| No original-source inspection | ✓ CONFIRMED | Spec-only authorship; no original tmux/mosh/ssh source cited |
| Cite spec sections before implementation | ✓ CONFIRMED | Module-level comments throughout cite doc numbers |
| All public API contracts match spec exactly | ✓ CONFIRMED | Type definitions and RPC schema match docs/01–05 exactly |

No gaps.

---

## docs/01 — CLI Commands

### Global Behaviour

| Requirement | Status | Evidence |
|---|---|---|
| Binary name: `mux` | ✓ | `crates/mux/Cargo.toml` |
| `MUX_HOME` env var → `~/.mux` default | ✓ | `crates/mux-cli/src/mux_home.rs:13–40` |
| `mux: ` prefix on all errors | ✓ | `crates/mux/src/main.rs:24–43` (prefix applied at exit) |
| Exit codes 0/1/2 | ✓ | `crates/mux-core/src/error.rs:224–257` (exit_code()) |
| Completions: bash/zsh/fish/powershell/elvish | ✓ | `crates/mux-cli/src/lib.rs:138–142` (clap_complete) |
| Completions work before MUX_HOME resolution | ✓ | `crates/mux/src/main.rs:13–22` (early dispatch) |

### `mux init`

| Requirement | Status | Evidence |
|---|---|---|
| Creates `$MUX_HOME` at mode 0700 | ✓ | `crates/mux-state/src/store.rs:75–80` |
| Creates `mux.db` at mode 0600 | ✓ | `crates/mux-state/src/store.rs:75–80` |
| Runs migrations | ✓ | `crates/mux-state/src/migrations.rs:30–79` |
| Idempotent | ✓ | `tests/integration/init.rs:40–51` + unit tests |
| No v0.1 config file | ✓ | No config creation code found |

### `mux host add/list/remove`

| Requirement | Status | Evidence |
|---|---|---|
| Validates alias per docs/02 rules | ✓ | `crates/mux-core/src/types.rs:19–32` (HostAlias::from_str) |
| Validates `user@addr` format | ✓ | `crates/mux-cli/src/host.rs:30–40` |
| Validates port 1–65535 | ✓ | `crates/mux-core/src/types.rs:63–74` |
| No connection at add time | ✓ | `crates/mux-cli/src/host.rs:24–71` |
| Rejects duplicate aliases | ✓ | `crates/mux-cli/src/host.rs:66` |
| Persists to `hosts` table | ✓ | `crates/mux-state/src/host_repo.rs` |
| `host list` sorted by alias, all columns | ✓ | `crates/mux-cli/src/host.rs:73–100` |
| `host remove --yes` / confirmation | ✓ | `crates/mux-cli/src/host.rs:104–145` |
| Cascade-remove fingerprints, sessions | ✓ | `migrations/001-initial-schema.sql` (ON DELETE CASCADE) |

### `mux host test` / `mux host trust`

| Requirement | Status | Evidence |
|---|---|---|
| Core probe logic (uname -m, $HOME, tmux -V) | ✓ EXISTS | `crates/mux-cli/src/host.rs:246–340` (cmd_test_core) |
| TOFU fingerprint prompt and store | ✓ EXISTS | `crates/mux-cli/src/host.rs:346–420` (cmd_trust_core) |
| CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/host.rs:18–20` (`bail!("SSH not yet implemented")`) |
| tmux_version persisted | ✗ GAP | `crates/mux-cli/src/host.rs:332–335` (TODO comment) |

### `mux agent deploy/logs/stop`

| Requirement | Status | Evidence |
|---|---|---|
| Core deploy logic (binary selection, upload, verify, chmod) | ✓ EXISTS | `crates/mux-cli/src/agent.rs` |
| Core logs logic (tail 200 lines of agent.log) | ✓ EXISTS | `crates/mux-cli/src/agent.rs` |
| Core stop logic (SIGTERM, SIGKILL fallback) | ✓ EXISTS | `crates/mux-cli/src/agent.rs` |
| CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:158–172` (`bail!("SSH execution not yet implemented")`) |

### `mux create / attach / list / status / kill`

| Requirement | Status | Evidence |
|---|---|---|
| Core domain logic fully implemented | ✓ EXISTS | `create.rs:832`, `attach.rs:501`, `list.rs:412`, `status.rs:174`, `kill.rs:425` lines each |
| Unit tests via MockSshHost/MockDeployHost | ✓ | `crates/mux-cli/tests/` (extensive) |
| CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:177–186` (`bail!("SSH execution not yet implemented")`) |

**Root cause of all SSH stubs:** `SshHost`, `RemoteExec`, and `DeployHost` traits have no production concrete implementation. The `mux-ssh` crate provides TOFU trust and transport probing, but not SSH command execution. A production executor (e.g., using `openssh` crate or spawning `ssh(1)` directly) is required to wire all the above commands.

---

## docs/02 — Naming and Repositories

| Requirement | Status | Evidence |
|---|---|---|
| HostAlias rules (alphanumeric/hyphen/underscore, 1–64, no leading `-`, no dots) | ✓ | `crates/mux-core/src/types.rs:19–32` + `tests/naming.rs` |
| RepoRef normalisation (owner/repo and git@host:path forms) | ✓ | `crates/mux-core/src/types.rs:190–332` |
| Rejects `owner/repo.git` shorthand | ✓ | `crates/mux-core/src/types.rs` |
| Produces `repo_slug` (lowercase, slash) | ✓ | `crates/mux-core/src/types.rs` |
| Produces `storage_slug` (hyphens for non-alnum) | ✓ | `crates/mux-core/src/types.rs` |
| Clone URL: `git@{host}:{owner}/{repo}.git` | ✓ | `crates/mux-core/src/types.rs` |
| Shortname sanitisation (lowercase, max 124 bytes, hyphen-boundary truncation) | ✓ | `crates/mux-core/src/shortname.rs:48–79` |
| `mux-` prefix on tmux session name | ✓ | `crates/mux-tmux/src/adapter.rs` |
| Workdir path safety (no symlinks, UUID-prefixed) | ✓ | `crates/mux-core/src/workdir.rs:49–95` |

No gaps.

---

## docs/03 — Local State

| Requirement | Status | Evidence |
|---|---|---|
| `$MUX_HOME/mux.db` at mode 0600 | ✓ | `crates/mux-state/src/store.rs:75–80` |
| Directory at mode 0700 | ✓ | `crates/mux-state/src/store.rs:75–80` |
| `journal_mode = WAL` | ✓ | `crates/mux-state/src/store.rs:53` |
| `busy_timeout = 5000` | ✓ | `crates/mux-state/src/store.rs:54` |
| `synchronous = NORMAL` | ✓ | `crates/mux-state/src/store.rs:55` |
| `foreign_keys = ON` | ✓ | `crates/mux-state/src/store.rs:56` |
| Forward-only migrations with `_migrations` table | ✓ | `crates/mux-state/src/migrations.rs:30–79` |
| `hosts` table: all columns per spec | ✓ | `migrations/001-initial-schema.sql:14–24` |
| `known_host_fingerprints` table: UNIQUE (host_id, algorithm), ON DELETE CASCADE | ✓ | `migrations/001-initial-schema.sql:26–33` |
| `agent_versions` table: UNIQUE on host_id | ✓ | `migrations/001-initial-schema.sql:35–41` |
| `sessions` table: all columns, indexes | ✓ | `migrations/001-initial-schema.sql:43–61` |
| SessionStatus: active/dead/unreachable/orphaned | ✓ | `crates/mux-core/src/types.rs` |
| Timestamps: Unix seconds (INTEGER), no SQLite datetime() | ✓ | `store.rs` + migration |

No gaps.

---

## docs/04 — SSH Trust and Transport

| Requirement | Status | Evidence |
|---|---|---|
| SSH agent key enumeration (SSH_AUTH_SOCK) | ✓ | `crates/mux-ssh/src/trust.rs` |
| TOFU: first contact shows fingerprint, prompts | ✓ | `crates/mux-ssh/src/trust.rs:29–69` |
| TOFU: subsequent connections verify stored fingerprint | ✓ | `crates/mux-ssh/src/trust.rs` |
| TOFU: read-only ops skip prompt (mark unreachable on mismatch) | ✓ | `crates/mux-ssh/src/trust.rs` |
| Host key algorithm preference order | ✓ | `crates/mux-ssh/src/trust.rs:20–25` |
| Transport: streamlocal (preferred) / TCP fallback | ✓ | `crates/mux-ssh/src/transport.rs:18–64` |
| `MUX_FORCE_TRANSPORT` env var | ✓ | `crates/mux-ssh/src/transport.rs` |
| Transport persisted per session | ✓ | `sessions.transport_mode` in schema |
| `ssh_agent_not_forwarded` validation before SSH ops | ✗ GAP | Not enforced as a precondition; only detected indirectly at clone time |

---

## docs/05 — Agent RPC and Lifecycle

| Requirement | Status | Evidence |
|---|---|---|
| Agent binary path: `~/.mux/bin/mux-agent` | ✓ | `crates/mux-cli/src/agent_start.rs:88–101` |
| Lock file: `~/.mux/agent.lock` | ✓ | `crates/mux-cli/src/agent_start.rs:93` |
| Log file: `~/.mux/agent.log` | ✓ | `crates/mux-cli/src/agent_start.rs:93` |
| RPC operations: Health, CreateSession, ListSessions, GetSession, KillSession, Shutdown | ✓ | `crates/mux-rpc/src/schema.rs:10–100` |
| StreamSessionEvents: defined as unimplemented | ✓ | `crates/mux-rpc/src/schema.rs:103–109` |
| CreateSession request fields (uuid, shortname, repo_slug, branch, workdir_parent, repo_leaf) | ✓ | `crates/mux-rpc/src/schema.rs:26–33` |
| KillSession response flags (tmux_killed, workdir_removed) | ✓ | `crates/mux-rpc/src/schema.rs:88–92` |
| Startup timeout: 60s | ✓ | `crates/mux-cli/src/agent_start.rs:9` (STARTUP_TIMEOUT=60s) |
| Health probe interval: 1s | ✓ | `crates/mux-cli/src/agent_start.rs:10` (PROBE_INTERVAL=1s) |
| RPC client 30s timeout | ✗ GAP | Defined conceptually, not wired in RPC client |
| Shutdown drain logic | ✗ PARTIAL | Shutdown flag defined (`crates/mux-rpc/src/server.rs:102–104`); full drain not implemented |
| Concurrent-start safety (O_CREAT\|O_EXCL) | ✗ GAP | `crates/mux-cli/src/agent_start.rs:507–514` (test is `#[ignore]`, `todo!()`) |

---

## docs/06 — tmux Behavior

| Requirement | Status | Evidence |
|---|---|---|
| Direct argv invocation (no shell) | ✓ | `crates/mux-tmux/src/adapter.rs:90, 108, 121` |
| No `-L` / `-g` / global flags | ✓ | `crates/mux-tmux/src/adapter.rs` (all calls verified) |
| Session naming: `mux-` prefix | ✓ | Throughout adapter.rs |
| tmux ≥ 3.0 required | ✓ | `crates/mux-cli/src/host.rs:223–240` (parse_tmux_version) |
| `new-session -d -s <name> -c <workdir>` then `set-option` | ✓ | `crates/mux-tmux/src/adapter.rs:75–96`, tests at `:809–825` |
| `list-sessions -F` with tab-separated format and CRLF handling | ✓ | `crates/mux-tmux/src/adapter.rs:114–125`, `:165–200` |
| `kill-session -t <name>` | ✓ | `crates/mux-tmux/src/adapter.rs:105–108`, test at `:861` |
| Malformed row tolerance | ✓ | `crates/mux-tmux/src/adapter.rs:165–200` |

No gaps.

---

## docs/07 — Create/List/Status/Kill Flows

| Requirement | Status | Evidence |
|---|---|---|
| Create preconditions: repo normalisation | ✓ EXISTS | `crates/mux-cli/src/create.rs:97` |
| Create preconditions: host alias resolution | ✓ EXISTS | `crates/mux-cli/src/create.rs:97–100` |
| Create preconditions: arch/home set | ✓ EXISTS | `crates/mux-cli/src/create.rs` |
| Create preconditions: ssh-agent available | ✗ GAP | SSH_AUTH_SOCK not validated as a precondition before SSH ops |
| Create transaction: UUID gen, DB reservation | ✓ EXISTS | `crates/mux-cli/src/create.rs` |
| Create transaction: TOFU probe | ✓ EXISTS | `crates/mux-cli/src/create.rs` |
| Create transaction: workdir mkdir, git clone with GIT_TERMINAL_PROMPT=0 | ✓ EXISTS | `crates/mux-cli/src/create.rs:220–321` |
| Create transaction: agent ensure_running | ✓ EXISTS | `crates/mux-cli/src/create.rs:335–350` |
| Create transaction: RPC CreateSession + activate | ✓ EXISTS | `crates/mux-cli/src/create.rs:352–410` |
| Create flow CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:177` |
| List reconciliation (import, orphaned, unreachable, resurrection) | ✓ EXISTS | `crates/mux-cli/src/list.rs:56–248` |
| List CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:181` |
| Status UUID/shortname resolution | ✓ EXISTS | `crates/mux-cli/src/status.rs` |
| Status CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:183` |
| Kill: TOFU, ownership, workdir removal | ✓ EXISTS | `crates/mux-cli/src/kill.rs` |
| Kill CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:186` |
| Attach: selector, dead rejection, TOFU, temp known_hosts, exec ssh | ✓ EXISTS | `crates/mux-cli/src/attach.rs:1–114` |
| Attach CLI wiring | ✗ **BLOCKED** | `crates/mux-cli/src/lib.rs:179` |

---

## docs/08 — Errors, Observability, and Tests

| Requirement | Status | Evidence |
|---|---|---|
| All errors prefixed `mux: ` | ✓ | `crates/mux/src/main.rs:24–43` + `crates/mux-core/src/error.rs` |
| Error exit codes 1 (user/host/remote) / 2 (internal) | ✓ | `crates/mux-core/src/error.rs:224–257` |
| Human-readable hints: `ssh_agent_not_forwarded` | ✓ | `crates/mux-core/src/error.rs` (hint defined; not yet triggered from SSH precondition) |
| Human-readable hints: `host_key_mismatch` | ✓ | `crates/mux-core/src/error.rs` |
| Human-readable hints: `workdir_pre_existing` | ✓ | `crates/mux-core/src/error.rs` |
| `create_duration_ms` observability | ✓ | `crates/mux-cli/src/create.rs:327` |
| `git_clone_duration_ms` observability | ✓ | `crates/mux-cli/src/create.rs:321–328` |
| `error_category` on create failures | ✓ | `crates/mux-cli/src/create.rs:330, 350, 382, 405` |
| EventBus: non-blocking pub/sub (capacity 64) | ✓ | `crates/mux-core/src/event_bus.rs`; tests at `tests/observability/` |
| RpcRequestEvent fields (method, duration_ms, success) | ✓ | `crates/mux-core/src/event_bus.rs` |
| CreateFlowEvent fields (create_duration_ms, git_clone_duration_ms, error_category, host) | ✓ | `crates/mux-core/src/event_bus.rs` |
| JSON logging for non-TTY output | ✗ GAP | Single-line only; docs/08 defers this to "future" |
| tracing + EnvFilter + default level INFO | ✓ | `crates/mux-agent/src/main.rs:34, 45` |
| Every error enum variant has at least one test | ✓ | `crates/mux-core/src/error.rs` (extensive test suite) |
| Integration tests: mux init | ✓ | `tests/integration/init.rs` (4 real tests) |
| Integration tests: host lifecycle stubs | ✓ STUBS | `tests/integration/host.rs` (documented #[ignore]) |
| Integration tests: session lifecycle stubs | ✓ STUBS | `tests/integration/session.rs` (documented #[ignore]) |
| Integration tests: agent lifecycle stubs | ✓ STUBS | `tests/integration/agent.rs` (documented #[ignore]) |
| CI gate: fmt | ✓ | `.github/workflows/ci.yml` (fmt job) |
| CI gate: lint | ✓ | `.github/workflows/ci.yml` (lint job) |
| CI gate: unit tests | ✓ | `.github/workflows/ci.yml` (unit job) |
| CI gate: integration tests | ✓ | `.github/workflows/ci.yml` (integration job) |
| CI gate: artifact checks | ✓ | `.github/workflows/ci.yml` (artifact-mux-cli, artifact-mux-agent) |
| Release workflow: mux CLI + mux-agent (musl) | ✓ | `.github/workflows/release.yml` |

---

## Gap Summary

### Critical (blocks shipped CLI)

| Gap | Location | Blocking |
|---|---|---|
| No production `SshHost`/`RemoteExec`/`DeployHost` implementation | No file (`mux-ssh` has trust/transport, not command execution) | `host test`, `host trust`, `agent deploy/logs/stop`, `create`, `attach`, `list`, `status`, `kill` |

All CLI commands that require SSH connections return `bail!("SSH execution not yet implemented")` in `crates/mux-cli/src/lib.rs`. The domain logic for each command exists and is tested via mock. Wiring requires a concrete `SshHost` implementation (e.g., using `openssh` crate or spawning `ssh(1)`) and a few lines in `lib.rs` per command.

### Important (spec requirement, not yet implemented)

| Gap | Location | Spec ref |
|---|---|---|
| `ssh_agent_not_forwarded` precondition check | Not enforced before SSH ops | docs/01, docs/04 |
| `tmux_version` not persisted after `host test` | `crates/mux-cli/src/host.rs:332–335` TODO | docs/01 §host-test |
| Concurrent agent-start safety (O_CREAT\|O_EXCL) | `crates/mux-cli/src/agent_start.rs:507–514` #[ignore] | docs/05 §concurrent-start |
| RPC client 30s timeout not wired | RPC client (`crates/mux-rpc/src/client.rs`) | docs/05 §rpc-timeouts |

### Deferred (spec says "future")

| Gap | Location | Spec ref |
|---|---|---|
| JSON logging for non-TTY | Not implemented | docs/08 ("future") |
| StreamSessionEvents RPC | Marked unimplemented | docs/05 §stream-session-events |
| Shutdown drain (complete in-flight CreateSession) | Partial | docs/05 §shutdown |

---

## Follow-up Beads

The following beads are recommended to close the identified gaps:

1. **Implement production SSH executor** — concrete `SshHost`/`RemoteExec`/`DeployHost` using `openssh` crate or OpenSSH subprocess spawning; wire all commands in `lib.rs`. This is the highest-priority follow-up: it unblocks the entire shipped CLI surface.

2. **Enforce `ssh_agent_not_forwarded` precondition** — validate `SSH_AUTH_SOCK` is set and has keys loaded before any SSH operation; surface as `MuxError::SshAgentNotForwarded`.

3. **Persist `tmux_version` after `host test`** — requires DB migration to add `tmux_version` column to `hosts` table (noted TODO at `host.rs:332–335`).

4. **Implement concurrent-start safety** — `O_CREAT | O_EXCL` advisory lock in `agent_start.rs`; remove the `#[ignore]` test.

5. **Wire RPC client 30s timeout** — add `tokio::time::timeout` around RPC calls in `crates/mux-rpc/src/client.rs`.

---

## Clean-Room Compliance

All reviewed code cites spec sections in comments, uses clean-room implementations of SSH/tmux behavior, and does not reference original-source code or prior implementations. No violations found.

---

*Audit complete. Every requirement in docs/00–08 has been mapped to evidence or a gap. All gaps are tracked above.*
