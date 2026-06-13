# Agent RPC Protocol

Spec: docs/05-agent-rpc-and-lifecycle.md  
Status: Active  
Linked from: prompts/docs/README.md

## Protocol decision (mux-2n5)

The RPC mechanism must be decided and documented here before client/server work begins.
This document captures the spec-derived requirements and the decision.

**Interpretation**: The spec (docs/05) defines required operations and a JSON-like request/response
schema but does not mandate a wire protocol. The two viable candidates for a Rust project with
Unix-socket and TCP transport are:

| Candidate | Framing | Pros | Cons |
|-----------|---------|------|------|
| JSON-over-length-prefix | `u32` LE length + JSON body | Simple, debuggable, no codegen | Manual schema, no streaming |
| gRPC/tonic + prost | HTTP/2 framing, Protobuf | Typed, streaming ready, tooling | Heavier, HTTP/2 over Unix socket is unusual |

**Selected**: JSON-over-length-prefix (`u32` little-endian length-prefix + UTF-8 JSON body)
for v0.1. Rationale:
- No codegen required; aligns with the clean-room guideline (no external toolchains).
- Easily debuggable over a Unix socket with `nc`.
- `StreamSessionEvents` is unimplemented in v0.1 — streaming requirement is future.
- Can be upgraded to gRPC in a future migration with the same operation names.

Wire format: `[len: u32 LE][body: UTF-8 JSON]` in each direction.

## Agent binary and filesystem layout

| Path | Purpose |
|------|---------|
| `<home>/.mux/bin/mux-agent` | Agent binary (deployed by `mux agent deploy`) |
| `<home>/.mux/agent.lock` | PID + listen URLs, written at startup |
| `<home>/.mux/agent.log` | Log file; last 200 lines served by `mux agent logs` |
| `<home>/.mux/agent.sock` | Streamlocal socket (if supported) |

## Agent startup protocol

1. Check for `agent.lock`. If it exists, check if PID is alive.
2. Stale lock (process dead): remove lock, remove stale socket/port, proceed.
3. Held lock (process alive): return existing listen URLs from lock file. Done.
4. Start agent process.
5. Agent writes `agent.lock` with `{ pid, streamlocal_url?, tcp_url }` when ready.
6. Client polls agent socket/port for up to **60 seconds** (1-second interval).
7. Timeout: collect last 50 lines from `agent.log`; return `agent_start_timeout` error.
8. Concurrent-start safety: lock file created with `O_CREAT | O_EXCL`.

## RPC operations

### Timeouts

| Timer | Value |
|-------|-------|
| RPC request timeout | 30 seconds |
| Agent startup timeout | 60 seconds |
| Health probe interval during startup | 1 second |

### Health

```
Request:  {}
Response: { "ok": true }
```

Used as startup readiness probe and connection keepalive check.

### CreateSession

```
Request: {
  "uuid":         "<v4 UUID>",
  "shortname":    "<validated shortname>",
  "repo_slug":    "<owner/repo>",
  "branch":       "<git ref>",
  "workdir_parent": "<home>/.mux/<uuid>",
  "repo_leaf":    "<repo component>"
}

Response: {
  "uuid":      "<v4 UUID>",
  "shortname": "<shortname>",
  "tmux_name": "mux-<shortname>"
}
```

The agent:
1. Creates the tmux session with `tmux new-session -d -s <tmux_name> -c <workdir>`.
2. Adds `{ uuid → tmux_name }` to its in-memory ownership map.
3. Returns `{ uuid, shortname, tmux_name }`.

### ListSessions

```
Request:  {}
Response: [
  {
    "uuid":      "<v4 UUID>",
    "shortname": "<shortname>",
    "tmux_name": "mux-<shortname>",
    "workdir":   "<absolute path>",
    "status":    "active" | "dead" | "unreachable" | "orphaned"
  },
  ...
]
```

Returns all sessions in the agent's ownership map that still exist as tmux sessions.
Sessions whose tmux session no longer exists are reported as `dead`.

### GetSession

```
Request:  { "uuid": "<v4 UUID>" }
Response: {
  "uuid":      "<v4 UUID>",
  "shortname": "<shortname>",
  "tmux_name": "mux-<shortname>",
  "status":    "active" | "dead" | "unreachable" | "orphaned"
}
```

### KillSession

```
Request: {
  "uuid":      "<v4 UUID>",
  "repo_slug": "<owner/repo>"
}

Response: {
  "tmux_killed":    true | false,
  "workdir_removed": true | false
}
```

The agent:
1. Validates ownership: `uuid` must be in the in-memory map AND `repo_slug` must match.
2. On ownership failure: return `{ "error": "not_owned" }` — do NOT kill.
3. Kill tmux session: `tmux kill-session -t <tmux_name>`.
4. Remove workdir only if it was mux-created (path matches `$MUX_HOME/<uuid>/<repo-leaf>`
   with no symlinks; see docs/02 §Workdir safety).
5. Remove `{ uuid → tmux_name }` from in-memory map.

`workdir_removed` is false for imported sessions (agent never removes non-mux-created dirs).

### Shutdown

```
Request:  {}
Response: {}
```

Agent completes any in-flight `CreateSession` before exiting. Refuses to start new
sessions after `Shutdown` is received.

### StreamSessionEvents

```
Request:  {}
Response: UNIMPLEMENTED in v0.1 — return { "error": "internal", "message": "streaming not implemented" }.
```

### Error response format

All operations may return an error response:
```json
{ "error": "<error_key>", "message": "<human-readable detail>" }
```

Defined error keys:

| Key | Source |
|-----|--------|
| `not_owned` | `KillSession` — uuid not in ownership map |
| `not_found` | `GetSession` — uuid unknown |
| `tmux_error` | Any operation where tmux command fails |
| `internal` | Unexpected errors; also returned for unimplemented operations |
| `agent_start_timeout` | Client-side, pre-connection: agent did not become ready within 60s |

## Agent ownership model

- Agent owns: creating/listing/getting/killing tmux sessions.
- Agent does NOT read or write local SQLite.
- Agent maintains an in-memory `uuid → tmux_name` map for sessions it created.
- On agent restart, the in-memory map is empty. Sessions created before restart are
  still running in tmux but are invisible to the agent. The client reconciles these as
  `orphaned` during `mux list` (docs/07 §List flow reconciliation).

## Shutdown drain

On `Shutdown` RPC:
1. Set a flag refusing new `CreateSession` requests.
2. Complete any in-flight `CreateSession`.
3. Exit cleanly.
