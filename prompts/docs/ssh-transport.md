# SSH Trust and Transport

Spec: docs/04-ssh-trust-and-transport.md  
Status: Active  
Linked from: prompts/docs/README.md

## Key authentication rules

- Load keys exclusively from `ssh-agent` via `SSH_AUTH_SOCK`. No password auth.
- If no agent is running (`SSH_AUTH_SOCK` unset or socket unreachable): error `ssh_agent_not_forwarded`.
- Encrypted-key errors from the agent: surface as `ssh_key_encrypted` with a hint to unlock.
- Key auto-discovery: iterate agent-offered keys; try each against the host until one succeeds.

## TOFU state machine

### First contact (no stored fingerprint)

1. Attempt connection; collect server host key.
2. Look up `known_host_fingerprints` by `(host_id, algorithm)` — no match found.
3. Display fingerprint to user; prompt for trust confirmation.
4. User confirms → store fingerprint in DB; proceed.
5. User declines → abort with `host_key_rejected`.
6. No TTY (non-interactive) → abort with `tofu_non_interactive`.

### Subsequent connections (fingerprint exists)

1. Collect server host key.
2. Look up stored fingerprint by `(host_id, algorithm)`.
3. Match → proceed silently.
4. Mismatch → abort with `host_key_mismatch`. **Never connect. Never update silently.**

### Read-only refresh (mux list / mux status)

- Skip the TOFU prompt entirely.
- On fingerprint mismatch: mark host as `unreachable` and continue to the next host.
- Purpose: `mux list` must not block on an interactive prompt.

### Attach pinning

- `mux attach` writes a single-entry temporary `known_hosts` file containing only the
  stored fingerprint for the session's host.
- SSH is invoked with:
  ```
  -o UserKnownHostsFile=<tmpfile>
  -o HostKeyAlgorithms=<stored_alg>
  ```
- If no fingerprint is stored: TOFU prompt as first-contact.
- If fingerprint mismatch: abort before `exec ssh` with `host_key_mismatch`. No mutation.

## TOFU decision table

| Scenario | Action |
|----------|--------|
| No stored fingerprint, interactive | Prompt, store on accept |
| No stored fingerprint, non-interactive | Abort (`tofu_non_interactive`) |
| Match | Proceed silently |
| Mismatch, state-changing operation | Abort (`host_key_mismatch`), no mutation |
| Mismatch, read-only (list/status) | Mark host unreachable, continue |

## Host key algorithms

Supported algorithms in storage/precedence order:
1. `ssh-ed25519`
2. `ecdsa-sha2-nistp256`
3. `rsa-sha2-512`
4. `rsa-sha2-256`
5. Any other algorithm accepted verbatim for TOFU, stored as-is.

RSA SHA-2 ordering matters for `mux attach`: SHA-512 before SHA-256.

## Transport selection

### Streamlocal (preferred)

- OpenSSH `SocketPath` forwarding over a Unix domain socket.
- Agent listen URL: `unix:<home>/.mux/agent.sock`
- Probe: attempt connection; classify as `streamlocal` on success.

### TCP fallback

- Direct TCP connection to the agent's dynamically-chosen listen port.
- Agent listen URL: `tcp://127.0.0.1:<port>`
- Used when streamlocal probe fails.

### Transport persistence

- `hosts.transport` stores the per-host default transport (set by `mux host test`).
- `sessions.transport_mode` stores the per-session transport (set during `mux create`).
- On subsequent connects, read `sessions.transport_mode` (not `hosts.transport`) to
  reproduce the exact channel used at create time.

### MUX_FORCE_TRANSPORT

| Value | Effect |
|-------|--------|
| `streamlocal` | Force streamlocal probe; error if unavailable |
| `tcp` | Force TCP; skip streamlocal probe |
| anything else | Error |

Scope: `mux create` only. Does not affect existing sessions or `attach`/`status`.

## MOTD noise handling

Remote SSH sessions may emit MOTD/banner text before sentinel command output.
Parsing strategy: treat everything before the first line matching a sentinel pattern as
noise; discard it. Do not fail on noise.

## State-changing operation invariants

- Never connect for a state-changing operation (create, kill, attach) without completing
  TOFU verification first.
- Never update a stored fingerprint on mismatch — always abort.
- `mux list` and `mux status` skip the TOFU prompt (read-only refresh path).
