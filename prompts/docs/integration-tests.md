# Integration-Test Environment Plan

Spec: docs/08-errors-observability-and-tests.md §Integration tests  
Status: Active  
Linked from: prompts/docs/README.md

## Overview

Integration tests exercise the full mux CLI against real SSH and tmux infrastructure.
They run against one or more containerised **test hosts** built from `docker/test-host/`.
The test harness is a separate binary crate (`tests/integration/`) that spawns and tears
down containers, provisions test identities, and invokes `mux` as a subprocess.

Integration tests are skipped if Docker is unavailable. They are not gated on every PR
(too slow); they run in a dedicated CI step and can be run locally with
`cargo test --test integration`.

---

## Test host requirements

Each test host container must provide:

| Requirement | Detail |
|---|---|
| SSH server | OpenSSH `sshd`, port 22 inside the container |
| `tmux` ≥ 3.0 | Available on `PATH` |
| `uname` | Standard; must return `x86_64` on amd64 container |
| `sha256sum` | For agent deploy verification |
| No MOTD noise | Or a controlled, parseable MOTD for MOTD-noise tests |
| Writable `$HOME` | `/home/testuser` |
| `ssh-agent` forwarding support | `ForwardAgent yes` via `authorized_keys` options |

The SSH server accepts the **test identity key** (`docker/test-host/test_ed25519`)
in `authorized_keys`. The test identity is an ed25519 key generated once and committed
(private key included — it is test-only, never used for production access).

---

## Docker setup

### `docker/test-host/Dockerfile`

Builds a minimal Debian/Ubuntu image with `openssh-server` and `tmux`. The container:
- Creates user `testuser` with home `/home/testuser`
- Installs the test identity public key in `/home/testuser/.ssh/authorized_keys`
- Starts `sshd` on port 22 with `AllowAgentForwarding yes`
- Does NOT start tmux automatically (mux-agent does that on demand)

### `docker/test-host/docker-compose.yml`

Defines two services for multi-host tests:
- `mux-test-host-a`: standard host (port 2221 on the Docker bridge → container:22)
- `mux-test-host-b`: second host for multi-host session tests (port 2222)

Both services share the same image. Port mapping is fixed so test code can hardcode
`localhost:2221` and `localhost:2222`.

### Key file layout

```
docker/test-host/
├── Dockerfile
├── docker-compose.yml
├── sshd_config          # minimal sshd config (no PAM, AllowAgentForwarding yes)
├── test_ed25519         # test identity private key (committed; test-only)
└── test_ed25519.pub     # test identity public key
```

---

## Test harness

### Location

`tests/integration/` — scaffold for the integration test suite. The harness and module
structure live here; the Cargo wiring belongs in a dedicated crate (not the virtual
workspace manifest). When the first tests are written (mux-av5), a new workspace member
`crates/mux-integration-tests/` will be added with:

```toml
# crates/mux-integration-tests/Cargo.toml
[features]
integration-tests = []

[[test]]
name = "integration"
path = "../../tests/integration/main.rs"
required-features = ["integration-tests"]
```

Run with: `cargo test -p mux-integration-tests --test integration --features integration-tests`

### Helper crate: `tests/integration/harness.rs`

Provides:

```rust
pub struct TestHost {
    pub alias: String,
    pub addr: String,   // "127.0.0.1"
    pub port: u16,      // 2221 or 2222
    pub user: String,   // "testuser"
    pub key_path: PathBuf,
}

impl TestHost {
    /// Ensure the container is running. Panics if Docker unavailable.
    pub fn start(service: &str) -> Self { ... }
    /// SSH connection string: user@addr
    pub fn user_at_addr(&self) -> String { ... }
    /// Run mux CLI command, returning (exit_code, stdout, stderr).
    pub fn mux(&self, args: &[&str]) -> (i32, String, String) { ... }
}

impl Drop for TestHost {
    fn drop(&mut self) { /* docker compose stop + rm */ }
}
```

Each test gets a fresh `MUX_HOME` via `TempDir` so tests are isolated. The `SSH_AUTH_SOCK`
is populated by a per-test `ssh-agent` process loaded with `test_ed25519`.

---

## Test scenarios

### 1. `mux init`

| Scenario | How to exercise |
|---|---|
| Default `~/.mux` | `HOME=/tmp/testhome mux init`; assert `~/.mux/mux.db` created |
| `MUX_HOME` override | `MUX_HOME=/tmp/custom mux init`; assert `/tmp/custom/mux.db` |
| Private permissions | `stat -c%a /tmp/custom/mux.db` → `600`; dir → `700` |
| Repeated runs | `mux init && mux init` → idempotent (exit 0, no error) |

### 2. Host lifecycle

| Scenario | How to exercise |
|---|---|
| `host add` + `host list` | Add `mux-test-host-a`; assert appears in list |
| `host remove` | Add then remove; assert absent from list |
| `host test` | After add, run `host test`; assert arch=amd64, home captured |
| `host trust` (TOFU) | First connect triggers trust prompt; `--yes` auto-accepts |
| Arch normalisation | `uname -m` returns `x86_64`; stored arch must be `amd64` |

### 3. Agent deploy

| Scenario | How to exercise |
|---|---|
| Successful deploy | `MUX_AGENT_BINARY=<path> mux agent deploy host-a`; verify size+hash |
| Missing binary | `MUX_AGENT_BINARY=/nonexistent mux agent deploy` → exit 1, `mux: ` prefix |
| Host not tested | Deploy before `host test` → exit 1 (arch/home not set) |
| Already-running agent | Deploy while agent is running → graceful stop, then redeploy |
| Remote verify failure | Simulate truncated upload; assert deploy exits 1 |

### 4. streamlocal vs TCP fallback

Testing transport selection requires the ability to make the Unix socket unavailable.

| Scenario | How to exercise |
|---|---|
| Streamlocal success | Agent running with Unix socket; assert `sessions.transport_mode = streamlocal` |
| TCP fallback | Remove/block the Unix socket path; `MUX_FORCE_TRANSPORT=tcp mux create` |
| `MUX_FORCE_TRANSPORT=streamlocal` | Force streamlocal; if unavailable → exit 1 |
| `MUX_FORCE_TRANSPORT=tcp` | Force TCP regardless of socket availability |
| Transport persisted | After create, kill agent's socket; `mux attach` reads persisted mode |

The container's socket path is `/home/testuser/.mux/mux-agent.sock`. To simulate
streamlocal failure: `ssh testuser@host 'rm ~/.mux/mux-agent.sock'` after agent starts.

### 5. `mux create` / `mux list` / `mux status`

| Scenario | How to exercise |
|---|---|
| Success path | `mux create host-a /work/my-repo`; assert session in list |
| Workdir pre-existing | Pre-create the workdir on the remote; `mux create` → exit 1 |
| Git clone failure | Point to non-existent repo; `mux create` → exit 1 with hint |
| SSH agent missing | `SSH_AUTH_SOCK=` (unset); `mux create` → exit 1 with hint |
| Orphaned session | Kill tmux manually; `mux list` shows status=orphaned |
| Unreachable host | Stop the container; `mux list` shows status=unreachable |
| UUID lookup | `mux status <full-uuid>` returns session details |
| Shortname lookup | `mux status <shortname>` returns session details |

### 6. `mux kill`

| Scenario | How to exercise |
|---|---|
| Ownership match | Session owned by current user → kill succeeds |
| Mismatch refusal | Fake a different owner UUID; `mux kill` → exit 1 |
| No-op (dead session) | Session already dead; `mux kill` → exit 0, idempotent |
| Dead mark | After kill, `mux status` → status=dead |

### 7. `mux attach`

| Scenario | How to exercise |
|---|---|
| Dead session rejected | `mux attach` on dead session → exit 1 with message |
| Transport pinning | Created via TCP; `mux attach` must use TCP (not probe streamlocal) |

### 8. SSH agent forwarding failure simulation

The `create` command requires `ssh-agent` forwarding to authenticate the git clone.
To simulate a missing ssh-agent:

```bash
SSH_AUTH_SOCK="" mux create host-a /work/repo
```

Expected: exit 1, error message contains "ssh_agent_not_forwarded",
hint contains `ssh-add`.

Alternatively, start an ssh-agent with **no loaded keys** and point the repo to a
private origin that requires a key; the clone fails and mux exits 1.

### 9. Private clone failure

For testing private-repo clone failures without a real private remote:
- Configure the test git server as an SSH git server inside the container
- or use a bare git repo on the container filesystem at a path that requires auth

Simpler approach for v0.1: use a non-existent URL (`git@nxdomain.invalid:foo/bar.git`).
The clone will fail with a network error; mux must surface it with exit 1 and the `mux: ` prefix.

---

## Execution model

### Local developer run

Prerequisites:
1. Docker Desktop or Docker Engine running
2. `mux` binary on `PATH` (or `MUX_BIN` env var pointing to target build)
3. `docker compose` available

```bash
# Start test hosts (idempotent)
docker compose -f docker/test-host/docker-compose.yml up -d

# Run integration tests
cargo test --test integration --features integration-tests

# Tear down
docker compose -f docker/test-host/docker-compose.yml down
```

The test harness auto-starts containers if not running; the `TestHost::Drop` impl
tears down after each test. Tests can run concurrently because each test uses an
isolated `MUX_HOME` and container-side isolation (separate tmux sessions).

### CI run

Integration tests run in a dedicated job (`ci.yml` step 4) after unit tests pass.

The job:
1. Starts Docker service (`services: docker` or `setup-docker` action)
2. Builds the test-host image: `docker build docker/test-host`
3. Runs `cargo test --test integration --features integration-tests`
4. Tears down via `docker compose down`

Integration tests are NOT gated on feature branches — they run on `main` after merge
and on tags. They can be opt-in on PRs via a label (`run-integration-tests`).

### Skip condition

If Docker is not available (`docker info` fails), the test harness emits
`cargo test::ignore` for all integration tests and exits 0. This allows the unit-test
CI job to pass on runners without Docker.

---

## Test isolation invariants

1. **Isolated `MUX_HOME`**: each test creates a `TempDir` and sets `MUX_HOME`.
2. **Isolated `SSH_AUTH_SOCK`**: each test spawns a fresh `ssh-agent`, loads
   `test_ed25519`, captures `SSH_AUTH_SOCK`, and kills the agent on drop.
3. **Unique host aliases**: tests use `format!("test-host-{uuid}")` to avoid
   collisions in the shared db.
4. **No shared tmux sessions**: each test creates sessions with unique names derived
   from the test uuid; no test reads sessions created by another.
5. **Container-side cleanup**: the `TestHost::Drop` impl runs
   `tmux kill-server 2>/dev/null || true` on the remote to clean up stale sessions.

---

## Constraints and decisions

- **Docker required, not optional** — the alternative (a shared staging host) creates
  ordering dependencies between tests and makes CI non-reproducible.
- **Fixed host ports** (2221, 2222) — avoids dynamic port lookup complexity in test
  setup; acceptable because Docker Desktop binds these only when containers are running.
- **Test identity key committed** — the private key is test-only and grants access
  only to the ephemeral test container. Committing it is intentional and documented
  here to prevent false security alarms in secret-scanning.
- **No `cargo nextest` dependency** — tests use standard `#[test]` so the harness
  works with `cargo test` without installing nextest.
- **Integration tests gated behind a feature flag** — avoids Docker dependency in
  `cargo test --workspace` (the default unit-test run).
