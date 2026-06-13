# 04 — SSH Trust and Transport

## Key loading and authentication

- Load keys from `ssh-agent` (via `SSH_AUTH_SOCK`).
- If no agent is running: error with `ssh_agent_not_forwarded`.
- Encrypted-key errors from the agent: surface as `ssh_key_encrypted` with a hint to
  unlock the key.
- Key auto-discovery: iterate agent-offered keys; try each against the host.

## TOFU (Trust On First Use)

### First contact

1. Attempt connection; collect server host key.
2. Look up `known_host_fingerprints` for this host and key algorithm.
3. No existing record: display fingerprint, prompt user to confirm trust.
4. User confirms: store fingerprint; proceed.
5. User declines: abort with `host_key_rejected`.
6. Non-interactive (no TTY): abort with `tofu_non_interactive`.

### Subsequent connections

1. Collect server host key.
2. Look up stored fingerprint by algorithm.
3. Match: proceed silently.
4. Mismatch: abort with `host_key_mismatch`. Never connect. Never update silently.

### Read-only refresh (list/status)

- TOFU prompt is SKIPPED; on mismatch, mark host as `unreachable` and continue.
- Purpose: `mux list` must not block on an interactive prompt.

### Attach pinning

- `mux attach` writes a single-entry temporary `known_hosts` file containing only the
  stored fingerprint for the session's host.
- SSH is invoked with `-o UserKnownHostsFile=<tmpfile>` and `-o HostKeyAlgorithms=<alg>`.
- Algorithm preference order: `ssh-ed25519`, `ecdsa-sha2-nistp256`, `rsa-sha2-512`,
  `rsa-sha2-256`. For unknown algorithms, use the stored algorithm verbatim.

## Transport selection

### Streamlocal (preferred)

- OpenSSH `SocketPath` forwarding over a Unix domain socket.
- Probe: attempt connection; classify as `streamlocal` on success.

### TCP fallback

- Direct TCP connection to the agent's listen port.
- Used when streamlocal probe fails.

### Transport persistence

- `transport_mode` is stored per host after `host test`.
- `mux create` persists the chosen transport to the session row.

### MUX_FORCE_TRANSPORT

- Valid values: `streamlocal`, `tcp`.
- Scope: `mux create` only. Does not affect existing sessions or `attach`/`status`.
- Error on invalid value.

## MOTD noise handling

Remote SSH sessions may emit MOTD/banner text before the sentinel command output.
Parse strategy: treat everything before the first line matching a sentinel pattern as
noise; discard it.

## Host key algorithms

Supported algorithms (in storage/precedence order):
1. `ssh-ed25519`
2. `ecdsa-sha2-nistp256`
3. `rsa-sha2-512`
4. `rsa-sha2-256`
5. Any other algorithm accepted verbatim for TOFU, stored as-is.

RSA SHA-2 ordering matters for `mux attach`: SHA-512 before SHA-256.
