#!/usr/bin/env bash
# Project: mux
# Generated: 2026-06-13
#
# Creates a Beads task graph for the clean-room reimplementation described by
# /Users/punk1290/.agents/projects/mux/docs/prompts/clean-room-reimplementation-of-mux.md.

set -euo pipefail

existing_beads_json=$(mktemp)
trap 'rm -f "$existing_beads_json"' EXIT

if bd --json list --all --limit 1 >"$existing_beads_json" 2>/dev/null; then
  if grep -q '"id"' "$existing_beads_json"; then
    echo "Refusing to create duplicate graph: existing beads found." >&2
    exit 1
  fi
else
  bd init --non-interactive --skip-agents --skip-hooks
fi

create_bead() {
  local title="$1"
  local priority="$2"
  local label="$3"
  bd create "$title" -p "$priority" --label "$label" --silent
}

echo "Creating mux clean-room reimplementation task graph..."

# ========================================
# Phase 1: Workspace, Contracts, Decisions
# ========================================

WORKSPACE=$(create_bead "Initialize Rust workspace with mux and mux-agent binaries plus shared crates; cites docs/00-clean-room-guidelines.md; reservation: Cargo.toml crates/** README.md; acceptance: cargo fmt, clippy, and test commands are documented and runnable" 0 workspace)

SPEC_INDEX=$(create_bead "Create prompts/docs clean-room spec index for docs/00-08 with command, state, RPC, SSH, tmux, flow, testing cross-references; cites docs/00-clean-room-guidelines.md; reservation: prompts/docs/**; acceptance: every later bead can cite exact spec sections" 0 docs)
bd dep add "$SPEC_INDEX" "$WORKSPACE"

CONTRIBUTING_GUARDRAILS=$(create_bead "Document clean-room guardrails for implementation agents including forbidden original-source inspection and compatibility-only contracts; cites docs/00-clean-room-guidelines.md; reservation: prompts/docs/clean-room-guardrails.md; acceptance: instructions are explicit and linked from spec index" 0 docs)
bd dep add "$CONTRIBUTING_GUARDRAILS" "$SPEC_INDEX"

FORMAT_LINT_TEST_SCAFFOLD=$(create_bead "Add workspace formatting, linting, unit test, and integration test command scaffolding; cites docs/08-errors-observability-and-tests.md; reservation: Cargo.toml crates/** scripts/**; acceptance: cargo fmt --check, cargo clippy, cargo test commands are defined" 0 workspace)
bd dep add "$FORMAT_LINT_TEST_SCAFFOLD" "$WORKSPACE"

STACK_VALIDATION=$(create_bead "Validate Rust SSH stack support for public-key auth, encrypted-key behavior, agent forwarding, host-key callbacks, direct-streamlocal, direct-tcpip, remote exec, and OpenSSH attach needs; cites docs/04-ssh-trust-and-transport.md; reservation: prompts/docs/stack-validation.md; acceptance: decision matrix names library gaps and subprocess fallbacks" 0 architecture)
bd dep add "$STACK_VALIDATION" "$SPEC_INDEX"

RPC_DECISION=$(create_bead "Decide mux-agent RPC mechanism and schema strategy covering Health, CreateSession, ListSessions, GetSession, KillSession, Shutdown, StreamSessionEvents, timeouts, compatibility stance, and error mapping; cites docs/05-agent-rpc-and-lifecycle.md; reservation: prompts/docs/rpc-protocol.md proto/** crates/mux-rpc/**; acceptance: implementable protocol contract exists before client/server work" 0 agent-rpc)
bd dep add "$RPC_DECISION" "$STACK_VALIDATION"

CLI_CONTRACT_DOC=$(create_bead "Create CLI contract and command matrix for init, host, agent, create, attach, list, status, kill, completions, MUX_HOME, error prefixes, and home-dir failures; cites docs/01-cli-commands.md; reservation: prompts/docs/cli-contract.md; acceptance: command/flag/output/error matrix covers every documented command" 0 cli)
bd dep add "$CLI_CONTRACT_DOC" "$SPEC_INDEX"

STATE_DESIGN_DOC=$(create_bead "Create SQLite schema and migration notes for hosts, fingerprints, agent_versions, sessions, WAL, busy timeout, foreign keys, cascades, timestamps, and reservation semantics; cites docs/03-local-state.md; reservation: prompts/docs/sqlite-state.md migrations/**; acceptance: schema design maps every logical record and status value" 0 state)
bd dep add "$STATE_DESIGN_DOC" "$SPEC_INDEX"

SSH_TRANSPORT_DOC=$(create_bead "Create SSH authentication, TOFU, attach pinning, streamlocal probe, TCP fallback, and MUX_FORCE_TRANSPORT design notes; cites docs/04-ssh-trust-and-transport.md; reservation: prompts/docs/ssh-transport.md; acceptance: trust and transport decision tables are captured" 0 ssh-trust)
bd dep add "$SSH_TRANSPORT_DOC" "$STACK_VALIDATION"

TMUX_CONTRACT_DOC=$(create_bead "Create tmux argv, validation, list parsing, mux-prefix, and option ordering contract; cites docs/06-tmux-behavior.md; reservation: prompts/docs/tmux-contract.md; acceptance: command shapes and validation rules are implementation-ready" 0 tmux)
bd dep add "$TMUX_CONTRACT_DOC" "$SPEC_INDEX"

FLOW_CONTRACT_DOC=$(create_bead "Create create/list/status/attach/kill flow orchestration notes with sequencing, cleanup, reconciliation, and mutation rules; cites docs/07-create-list-status-kill-flows.md; reservation: prompts/docs/session-flows.md; acceptance: each flow has preconditions, side effects, and failure behavior" 0 flows)
bd dep add "$FLOW_CONTRACT_DOC" "$SPEC_INDEX"

TEST_ENV_DOC=$(create_bead "Create integration-test environment plan for controlled SSH/tmux hosts, streamlocal/TCP fallback, agent deploy, and private clone or forwarding-failure simulation; cites docs/08-errors-observability-and-tests.md; reservation: prompts/docs/integration-tests.md docker/** tests/integration/**; acceptance: test host prerequisites and execution model are documented" 1 testing)
bd dep add "$TEST_ENV_DOC" "$FORMAT_LINT_TEST_SCAFFOLD"

RELEASE_DESIGN_DOC=$(create_bead "Create release and deployment notes for local CLI packaging, Linux amd64/arm64 mux-agent builds, MUX_AGENT_BINARY override, and deploy path verification; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: prompts/docs/release.md .github/workflows/** Cross.toml dist/**; acceptance: release tool choice and artifacts are defined" 1 release)
bd dep add "$RELEASE_DESIGN_DOC" "$WORKSPACE"

# ========================================
# Phase 2: Shared Domain, State, CLI
# ========================================

CORE_TYPES=$(create_bead "Implement shared domain types for host aliases, endpoints, ports, transport modes, session statuses, UUID selectors, and command-context errors; cites docs/01-cli-commands.md docs/03-local-state.md; reservation: crates/mux-core/**; acceptance: validation rejects invalid aliases, ports, statuses, and selectors" 0 architecture)
bd dep add "$CORE_TYPES" "$STATE_DESIGN_DOC"

CORE_TYPES_TESTS=$(create_bead "Verify shared domain type validation and command-context error formatting; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: crates/mux-core/** tests/core/**; acceptance: unit tests cover invalid ports, aliases, statuses, UUID-vs-shortname resolution, and error prefixes" 1 testing)
bd dep add "$CORE_TYPES_TESTS" "$CORE_TYPES"

STATE_MIGRATIONS=$(create_bead "Implement SQLite migrations and store opening with private directory creation, WAL, busy timeout, normal synchronous mode, foreign keys on every connection, concurrency-safe migrations, and Unix-second timestamps; cites docs/03-local-state.md; reservation: crates/mux-state/** migrations/**; acceptance: mux.db is created under MUX_HOME or ~/.mux and all logical tables exist" 0 state)
bd dep add "$STATE_MIGRATIONS" "$CORE_TYPES"

STATE_REPOSITORIES=$(create_bead "Implement SQLite repositories for hosts, known_host_fingerprints, agent_versions, sessions, cascade deletes, host sorting, session reservation, status updates, and import records; cites docs/03-local-state.md; reservation: crates/mux-state/**; acceptance: CRUD APIs preserve documented logical records and constraints" 0 state)
bd dep add "$STATE_REPOSITORIES" "$STATE_MIGRATIONS"

STATE_TESTS=$(create_bead "Verify SQLite migrations, private directory behavior, WAL pragmas, foreign-key cascades, concurrent open, host list sorting, placeholder fields, session reservation conflicts, and status transitions; cites docs/03-local-state.md docs/08-errors-observability-and-tests.md; reservation: crates/mux-state/** tests/state/**; acceptance: unit tests prove every documented state invariant" 1 testing)
bd dep add "$STATE_TESTS" "$STATE_REPOSITORIES"

CLI_ROOT=$(create_bead "Implement mux CLI root parsing, MUX_HOME resolution, default ~/.mux, home-directory failure behavior, command-context error wrapping, and shell completion generation; cites docs/01-cli-commands.md; reservation: crates/mux-cli/** crates/mux/** tests/cli/**; acceptance: global behavior applies consistently to all subcommands" 0 cli)
bd dep add "$CLI_ROOT" "$STATE_REPOSITORIES"
bd dep add "$CLI_ROOT" "$CLI_CONTRACT_DOC"

CLI_INIT=$(create_bead "Implement mux init with private state directory, mux.db creation, migrations, repeatability, and no v0.1 config file; cites docs/01-cli-commands.md docs/03-local-state.md; reservation: crates/mux-cli/** tests/cli/init.rs; acceptance: repeated init succeeds and creates only documented state" 0 cli)
bd dep add "$CLI_INIT" "$CLI_ROOT"

CLI_INIT_TESTS=$(create_bead "Verify mux init behavior for MUX_HOME, default ~/.mux, private permissions, repeated runs, and migration application; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: tests/cli/init.rs tests/integration/init.rs; acceptance: CLI and integration tests cover init success and home-dir failures" 1 testing)
bd dep add "$CLI_INIT_TESTS" "$CLI_INIT"

# ========================================
# Phase 3: Naming and Repositories
# ========================================

REPO_NORMALIZATION=$(create_bead "Implement repo input normalization for owner/repo and git@host:path.git forms, storage slug, clone URL, owner/repo split, repo leaf, and rejection of owner/repo.git shorthand; cites docs/02-naming-and-repositories.md; reservation: crates/mux-core/** tests/naming/**; acceptance: examples and invalid shorthand match spec" 0 naming)
bd dep add "$REPO_NORMALIZATION" "$CORE_TYPES"

SHORTNAME_BUILDER=$(create_bead "Implement ASCII shortname sanitization, deterministic non-main names, random main adjective-noun suffixes, 124-byte cap, truncation policy, collision iteration, and mux- tmux prefix; cites docs/02-naming-and-repositories.md; reservation: crates/mux-core/**; acceptance: main suffix is preserved and non-main names are stable" 0 naming)
bd dep add "$SHORTNAME_BUILDER" "$REPO_NORMALIZATION"

WORKDIR_RULES=$(create_bead "Implement canonical remote workdir construction and safety classification for mux-created versus imported sessions; cites docs/02-naming-and-repositories.md docs/05-agent-rpc-and-lifecycle.md; reservation: crates/mux-core/** crates/mux-agent/**; acceptance: only <home>/.mux/<uuid>/<leaf> non-symlink paths are removable" 0 naming)
bd dep add "$WORKDIR_RULES" "$SHORTNAME_BUILDER"

NAMING_TESTS=$(create_bead "Verify repo normalization, shortname sanitization, main suffix generation, truncation, collision handling, tmux prefix, and workdir safety edge cases; cites docs/02-naming-and-repositories.md docs/08-errors-observability-and-tests.md; reservation: tests/naming/** crates/mux-core/**; acceptance: tests cover all documented examples and cap behavior" 1 testing)
bd dep add "$NAMING_TESTS" "$WORKDIR_RULES"

# ========================================
# Phase 4: Host Inventory, SSH Trust, Transport
# ========================================

HOST_ADD_LIST_REMOVE=$(create_bead "Implement mux host add/list/remove with user@addr parsing, key auto-discovery, tilde expansion, port validation, no-connect add, sorted list placeholders, confirmation, and cascade removal; cites docs/01-cli-commands.md docs/03-local-state.md; reservation: crates/mux-cli/** crates/mux-state/** tests/cli/host.rs; acceptance: outputs and persistence match host command contract" 0 cli)
bd dep add "$HOST_ADD_LIST_REMOVE" "$CLI_ROOT"

HOST_ADD_LIST_REMOVE_TESTS=$(create_bead "Verify host add/list/remove parsing, output rendering, empty inventory, confirmation decline, --yes removal, and cascade deletion; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: tests/cli/host.rs tests/state/**; acceptance: unit and CLI tests cover documented host inventory behavior" 1 testing)
bd dep add "$HOST_ADD_LIST_REMOVE_TESTS" "$HOST_ADD_LIST_REMOVE"

SSH_AUTH_TOFU=$(create_bead "Implement SSH key loading, encrypted-key errors, key auto-discovery integration, TOFU first contact prompt, match, mismatch refusal, non-interactive refusal, fingerprint persistence by algorithm, and host trust rotation logic; cites docs/04-ssh-trust-and-transport.md docs/01-cli-commands.md; reservation: crates/mux-ssh/** crates/mux-cli/** tests/ssh/**; acceptance: trust decisions match the documented table" 0 ssh-trust)
bd dep add "$SSH_AUTH_TOFU" "$HOST_ADD_LIST_REMOVE"
bd dep add "$SSH_AUTH_TOFU" "$STACK_VALIDATION"

TRANSPORT_PROBE=$(create_bead "Implement streamlocal capability probe classification, TCP fallback mode, persisted transport_mode, direct-tcpip loopback behavior, and MUX_FORCE_TRANSPORT selection limited to mux create; cites docs/04-ssh-trust-and-transport.md; reservation: crates/mux-ssh/** crates/mux-rpc/** tests/transport/**; acceptance: probe classifications and override errors match spec" 0 transport)
bd dep add "$TRANSPORT_PROBE" "$SSH_AUTH_TOFU"
bd dep add "$TRANSPORT_PROBE" "$RPC_DECISION"

HOST_TEST_TRUST=$(create_bead "Implement mux host test/trust with sentinel preflight parsing, required tools, tmux >=3.0, readable home, amd64/arm64 normalization, transport persistence, fingerprint confirmation, and unchanged-key behavior; cites docs/01 docs/04; reservation: crates/mux-cli/** crates/mux-ssh/** tests/cli/host_test.rs; acceptance: host test persists arch, home, versions, tools, and transport" 0 ssh-trust)
bd dep add "$HOST_TEST_TRUST" "$TRANSPORT_PROBE"

SSH_TRANSPORT_TESTS=$(create_bead "Verify SSH auth, TOFU first contact/match/mismatch/refusal/non-interactive cases, host trust rotation, transport probe classification, MOTD-noise preflight parsing, and MUX_FORCE_TRANSPORT scope; cites docs/04-ssh-trust-and-transport.md docs/08-errors-observability-and-tests.md; reservation: tests/ssh/** tests/transport/**; acceptance: unit tests cover every trust and transport decision branch" 1 testing)
bd dep add "$SSH_TRANSPORT_TESTS" "$HOST_TEST_TRUST"

HOST_TEST_INTEGRATION=$(create_bead "Add controlled SSH host integration test for mux host test with required tools, architecture normalization, home capture, tmux version, and transport persistence; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: docker/** tests/integration/host_test.rs; acceptance: test runs against containerized SSH/tmux host" 2 testing)
bd dep add "$HOST_TEST_INTEGRATION" "$HOST_TEST_TRUST"
bd dep add "$HOST_TEST_INTEGRATION" "$TEST_ENV_DOC"

# ========================================
# Phase 5: Tmux Adapter and Agent RPC
# ========================================

TMUX_ADAPTER=$(create_bead "Implement tmux adapter with direct argv for new-session, set-option, kill-session, ls -F, name/workdir/option validation, mux-prefix filtering, malformed row tolerance, per-session status-bar settings, numeric status values, no -g/-L flags, no user config mutation, and option ordering; cites docs/06; reservation: crates/mux-tmux/** crates/mux-agent/**; acceptance: no shell invocation and all options are session-scoped" 0 tmux)
bd dep add "$TMUX_ADAPTER" "$TMUX_CONTRACT_DOC"
bd dep add "$TMUX_ADAPTER" "$WORKDIR_RULES"

TMUX_TESTS=$(create_bead "Verify tmux argv shapes, validation, option constraints, numeric status values, absence of -g/-L, no user config mutation, list parsing with CRLF/malformed rows, mux-prefix filtering, duplicate preservation, and session-scoped option ordering; cites docs/06 docs/08; reservation: tests/tmux/** crates/mux-tmux/**; acceptance: tests cover documented tmux behavior" 1 testing)
bd dep add "$TMUX_TESTS" "$TMUX_ADAPTER"

RPC_SCHEMA=$(create_bead "Implement typed RPC schema and generated/shared client-server bindings for Health, CreateSession, ListSessions, GetSession, KillSession, Shutdown, and v0.1 unimplemented StreamSessionEvents; cites docs/05-agent-rpc-and-lifecycle.md; reservation: crates/mux-rpc/** proto/**; acceptance: request/response fields and status mapping match protocol doc" 0 agent-rpc)
bd dep add "$RPC_SCHEMA" "$RPC_DECISION"

AGENT_SERVER=$(create_bead "Implement mux-agent RPC server with Health, CreateSession validation, tmux session creation, in-memory UUID map, ListSessions discovery, GetSession by shortname, KillSession effects, Shutdown drain, and StreamSessionEvents unimplemented; cites docs/05-agent-rpc-and-lifecycle.md docs/06-tmux-behavior.md; reservation: crates/mux-agent/** crates/mux-rpc/**; acceptance: agent owns only live tmux operations and never reads local SQLite" 0 agent-rpc)
bd dep add "$AGENT_SERVER" "$RPC_SCHEMA"
bd dep add "$AGENT_SERVER" "$TMUX_ADAPTER"

AGENT_SERVER_TESTS=$(create_bead "Verify agent RPC validation, CreateSession tmux call order, ListSessions empty-server handling, GetSession not-found behavior, KillSession cleanup safety, Shutdown validation, and StreamSessionEvents unimplemented response; cites docs/05-agent-rpc-and-lifecycle.md docs/08-errors-observability-and-tests.md; reservation: tests/rpc/** crates/mux-agent/**; acceptance: unit tests prove every RPC operation contract" 1 testing)
bd dep add "$AGENT_SERVER_TESTS" "$AGENT_SERVER"

RPC_CLIENT_TRANSPORT=$(create_bead "Implement local RPC client over SSH-forwarded streamlocal and TCP fallback channels with request timeouts, health probes, transport fallback logs, and error mapping; cites docs/04-ssh-trust-and-transport.md docs/05-agent-rpc-and-lifecycle.md; reservation: crates/mux-rpc/** crates/mux-ssh/**; acceptance: client can talk through selected SSH channel and classifies RPC failures" 0 agent-rpc)
bd dep add "$RPC_CLIENT_TRANSPORT" "$RPC_SCHEMA"
bd dep add "$RPC_CLIENT_TRANSPORT" "$TRANSPORT_PROBE"

AGENT_LIFECYCLE=$(create_bead "Implement remote agent start protocol with agent.lock, stale socket/port cleanup, streamlocal and TCP listen URLs, readiness polling, log tail on timeout, validation of alias/home/transport, and concurrent-start handling; cites docs/05-agent-rpc-and-lifecycle.md; reservation: crates/mux-cli/** crates/mux-ssh/** crates/mux-rpc/**; acceptance: ensure-agent-running is race-safe and reports startup timeout logs" 0 agent-rpc)
bd dep add "$AGENT_LIFECYCLE" "$RPC_CLIENT_TRANSPORT"

AGENT_LIFECYCLE_TESTS=$(create_bead "Verify agent start protocol for healthy existing agent, stale cleanup, held lock, readiness timeout with last 50 log lines, streamlocal start, TCP fallback start, and invalid home/transport rejection; cites docs/05 docs/08; reservation: tests/rpc/** tests/integration/agent_lifecycle.rs; acceptance: tests cover lifecycle races and failures" 1 testing)
bd dep add "$AGENT_LIFECYCLE_TESTS" "$AGENT_LIFECYCLE"
bd dep add "$AGENT_LIFECYCLE_TESTS" "$TEST_ENV_DOC"

# ========================================
# Phase 6: Agent Deployment Commands
# ========================================

AGENT_DEPLOY=$(create_bead "Implement mux agent deploy with host/home/arch preconditions, arch-specific binary selection, MUX_AGENT_BINARY, upload to ~/.mux/bin/mux-agent, size/hash verification, chmod, log/lock init, graceful stop before kill fallback, version parsing, and local persistence; cites docs/01 docs/03; reservation: crates/mux-cli/** crates/mux-ssh/** crates/mux-state/**; acceptance: persist version only after verified upload" 1 cli)
bd dep add "$AGENT_DEPLOY" "$HOST_TEST_TRUST"
bd dep add "$AGENT_DEPLOY" "$AGENT_LIFECYCLE"

AGENT_LOGS_STOP=$(create_bead "Implement mux agent logs and mux agent stop with home-dir validation, last 200 lines, follow mode, graceful Shutdown RPC when transport known, process-kill fallback, and no-process-found success handling; cites docs/01-cli-commands.md docs/05-agent-rpc-and-lifecycle.md; reservation: crates/mux-cli/** crates/mux-ssh/** crates/mux-rpc/**; acceptance: logs and stop match command behavior and validation rules" 1 cli)
bd dep add "$AGENT_LOGS_STOP" "$AGENT_DEPLOY"

AGENT_COMMAND_TESTS=$(create_bead "Verify agent deploy/logs/stop for preconditions, MUX_AGENT_BINARY error, upload size/hash, chmod, version parsing, log tail/follow, graceful shutdown, kill fallback, and no-process-found success; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: tests/cli/agent.rs tests/integration/agent.rs; acceptance: unit and integration tests cover agent command contracts" 1 testing)
bd dep add "$AGENT_COMMAND_TESTS" "$AGENT_LOGS_STOP"
bd dep add "$AGENT_COMMAND_TESTS" "$TEST_ENV_DOC"

# ========================================
# Phase 7: Session Flows
# ========================================

CREATE_ERRORS_OBSERVABILITY=$(create_bead "Implement create-flow error categories and observability for create duration, git clone duration, error counts by host/category, human hints, and optional event bus; cites docs/08; reservation: crates/mux-core/** crates/mux-cli/** crates/mux-agent/**; acceptance: categories include workdir_pre_existing, git_clone_failed, ssh_agent_not_forwarded, session_already_exists, shortname_exhausted, rpc_error, other" 0 errors-observability)
bd dep add "$CREATE_ERRORS_OBSERVABILITY" "$CORE_TYPES"

CREATE_FLOW=$(create_bead "Implement mux create transaction: repo normalization, branch validation, ssh-agent precheck, host/home load, transport, TOFU SSH, DB reservation, workdir absence check, mkdir/clone/checkout with GIT_TERMINAL_PROMPT=0 in one forwarded SSH session, agent start, CreateSession RPC, active mark, and cleanup; cites docs/01 docs/02 docs/07; reservation: crates/mux-cli/** crates/mux-state/** crates/mux-ssh/** crates/mux-rpc/**; acceptance: active only after RPC success" 0 flows)
bd dep add "$CREATE_FLOW" "$AGENT_LIFECYCLE"
bd dep add "$CREATE_FLOW" "$SHORTNAME_BUILDER"
bd dep add "$CREATE_FLOW" "$CREATE_ERRORS_OBSERVABILITY"
bd dep add "$CREATE_FLOW" "$STATE_REPOSITORIES"

CREATE_FLOW_TESTS=$(create_bead "Verify mux create conflicts, main suffix retries, shortname exhaustion, missing home, forced transport, existing workdir, clone cleanup, GIT_TERMINAL_PROMPT=0, SSH_AGENT_NOT_FORWARDED, RPC cleanup, and active mark sequencing; cites docs/07 docs/08; reservation: tests/cli/create.rs tests/integration/create.rs; acceptance: tests prove transaction and cleanup behavior" 1 testing)
bd dep add "$CREATE_FLOW_TESTS" "$CREATE_FLOW"

LIST_FLOW=$(create_bead "Implement mux list with per-host agent fetch using stored SSH key while skipping TOFU for read-only refresh, mux-prefix filtering, import unknown live sessions, mark missing active sessions unreachable/orphaned, resurrect live dead/unreachable sessions, unreachable-host non-mutation fallback, dead skipping, grouping, sorting, and --plain; cites docs/01 docs/03 docs/04 docs/07; reservation: crates/mux-cli/** crates/mux-state/** crates/mux-rpc/**; acceptance: documented reconciliation behavior" 1 flows)
bd dep add "$LIST_FLOW" "$RPC_CLIENT_TRANSPORT"
bd dep add "$LIST_FLOW" "$STATE_REPOSITORIES"

LIST_FLOW_TESTS=$(create_bead "Verify mux list reconciliation for import, orphan, resurrection, unreachable host without mutation, read-only refresh skipping TOFU prompts, dead skipping, non-prefixed ignore, grouping, sorting, placeholders, and --plain output; cites docs/04 docs/07 docs/08; reservation: tests/cli/list.rs tests/state/**; acceptance: tests cover all reconciliation cases" 1 testing)
bd dep add "$LIST_FLOW_TESTS" "$LIST_FLOW"

STATUS_FLOW=$(create_bead "Implement mux status resolving UUID before shortname, rejecting host aliases as not found, loading local session/host, live GetSession by shortname, local-data success on unreachable host, and no TOFU during current refresh behavior; cites docs/01-cli-commands.md docs/07-create-list-status-kill-flows.md; reservation: crates/mux-cli/** crates/mux-state/** crates/mux-rpc/**; acceptance: status succeeds from local data when host cannot be contacted" 1 flows)
bd dep add "$STATUS_FLOW" "$RPC_CLIENT_TRANSPORT"
bd dep add "$STATUS_FLOW" "$STATE_REPOSITORIES"

STATUS_FLOW_TESTS=$(create_bead "Verify mux status UUID-vs-shortname resolution, unknown UUID no fallback, host alias not found, live success, unreachable-host local fallback, and missing session errors; cites docs/07-create-list-status-kill-flows.md docs/08-errors-observability-and-tests.md; reservation: tests/cli/status.rs; acceptance: tests prove status command contract" 1 testing)
bd dep add "$STATUS_FLOW_TESTS" "$STATUS_FLOW"

ATTACH_FLOW=$(create_bead "Implement mux attach with UUID/shortname resolution, unknown UUID no fallback, dead-session rejection, TOFU probe, one-key temporary known-hosts file, OpenSSH exec replacement, pinned HostKeyAlgorithms with RSA SHA-2 ordering, and stored tmux_name target; cites docs/01 docs/04 docs/07; reservation: crates/mux-cli/** crates/mux-ssh/**; acceptance: ssh argv pins verified key and stored tmux name" 1 flows)
bd dep add "$ATTACH_FLOW" "$SSH_AUTH_TOFU"
bd dep add "$ATTACH_FLOW" "$STATE_REPOSITORIES"

ATTACH_FLOW_TESTS=$(create_bead "Verify mux attach rejects dead sessions, handles UUID lookup without fallback, writes one-key known-hosts file, pins HostKeyAlgorithms for ed25519/ECDSA/RSA/unknown, uses stored tmux_name, and emits documented errors; cites docs/04-ssh-trust-and-transport.md docs/08-errors-observability-and-tests.md; reservation: tests/cli/attach.rs tests/ssh/**; acceptance: argv-pinning tests cover attach security contract" 1 testing)
bd dep add "$ATTACH_FLOW_TESTS" "$ATTACH_FLOW"

KILL_FLOW=$(create_bead "Implement mux kill with UUID/shortname resolution, TOFU host-key verification for state-changing SSH, agent connection, repo_slug ownership, KillSession workdir/ownership request, warning display, and local-dead mark only after tmux_killed or workdir_removed effect; cites docs/01 docs/04 docs/05 docs/07; reservation: crates/mux-cli/** crates/mux-state/** crates/mux-rpc/** crates/mux-ssh/**; acceptance: no-op leaves state unchanged and fingerprint mismatch refuses mutation" 1 flows)
bd dep add "$KILL_FLOW" "$SSH_AUTH_TOFU"
bd dep add "$KILL_FLOW" "$RPC_CLIENT_TRANSPORT"
bd dep add "$KILL_FLOW" "$STATE_REPOSITORIES"

KILL_FLOW_TESTS=$(create_bead "Verify mux kill TOFU mismatch/refusal before mutation, ownership mapping, warning display, no-op non-mutation, dead mark after tmux kill or workdir removal, imported-session no cleanup, and retryable failure behavior; cites docs/04 docs/05 docs/08; reservation: tests/cli/kill.rs tests/rpc/** tests/ssh/**; acceptance: tests cover mutation gates and host-key safety" 1 testing)
bd dep add "$KILL_FLOW_TESTS" "$KILL_FLOW"

# ========================================
# Phase 8: Observability and Integration
# ========================================

OBSERVABILITY_AGENT=$(create_bead "Implement RPC server request counts/durations, agent startup/shutdown logs, tmux failure logs, workdir cleanup warning logs, transport fallback logs, and bounded non-blocking in-process event bus semantics; cites docs/08-errors-observability-and-tests.md; reservation: crates/mux-core/** crates/mux-agent/** crates/mux-cli/**; acceptance: observable signals exist without changing persistence semantics" 2 errors-observability)
bd dep add "$OBSERVABILITY_AGENT" "$AGENT_SERVER"
bd dep add "$OBSERVABILITY_AGENT" "$CREATE_ERRORS_OBSERVABILITY"

OBSERVABILITY_TESTS=$(create_bead "Verify event bus publish/subscribe/drop behavior, create error metrics labels, RPC duration counters, transport fallback logs, startup/shutdown logs, and workdir cleanup warning logs; cites docs/08-errors-observability-and-tests.md; reservation: tests/observability/** crates/mux-core/**; acceptance: tests cover non-blocking event semantics and required signal names" 2 testing)
bd dep add "$OBSERVABILITY_TESTS" "$OBSERVABILITY_AGENT"

FULL_FLOW_INTEGRATION=$(create_bead "Add full controlled-host integration tests for mux init, host add/list/remove, host test, agent deploy/stop/logs, create/list/status/attach/kill, streamlocal, TCP fallback, and SSH-agent-forwarding failure simulation; cites docs/08-errors-observability-and-tests.md; reservation: tests/integration/** docker/** fixtures/**; acceptance: integration suite exercises end-to-end documented workflows" 2 testing)
bd dep add "$FULL_FLOW_INTEGRATION" "$CREATE_FLOW"
bd dep add "$FULL_FLOW_INTEGRATION" "$LIST_FLOW"
bd dep add "$FULL_FLOW_INTEGRATION" "$STATUS_FLOW"
bd dep add "$FULL_FLOW_INTEGRATION" "$ATTACH_FLOW"
bd dep add "$FULL_FLOW_INTEGRATION" "$KILL_FLOW"
bd dep add "$FULL_FLOW_INTEGRATION" "$AGENT_COMMAND_TESTS"
bd dep add "$FULL_FLOW_INTEGRATION" "$TEST_ENV_DOC"

# ========================================
# Phase 9: CI, Release, Final Verification
# ========================================

CI_PIPELINE=$(create_bead "Add CI workflow for cargo fmt, cargo clippy, unit tests, integration tests where available, and artifact checks; cites docs/08-errors-observability-and-tests.md; reservation: .github/workflows/** scripts/**; acceptance: CI reports separate fmt, lint, unit, and integration gates" 1 release)
bd dep add "$CI_PIPELINE" "$FORMAT_LINT_TEST_SCAFFOLD"
bd dep add "$CI_PIPELINE" "$CORE_TYPES_TESTS"
bd dep add "$CI_PIPELINE" "$STATE_TESTS"
bd dep add "$CI_PIPELINE" "$CLI_INIT_TESTS"
bd dep add "$CI_PIPELINE" "$HOST_ADD_LIST_REMOVE_TESTS"
bd dep add "$CI_PIPELINE" "$SSH_TRANSPORT_TESTS"
bd dep add "$CI_PIPELINE" "$NAMING_TESTS"
bd dep add "$CI_PIPELINE" "$TMUX_TESTS"
bd dep add "$CI_PIPELINE" "$AGENT_SERVER_TESTS"
bd dep add "$CI_PIPELINE" "$AGENT_LIFECYCLE_TESTS"
bd dep add "$CI_PIPELINE" "$AGENT_COMMAND_TESTS"
bd dep add "$CI_PIPELINE" "$CREATE_FLOW_TESTS"
bd dep add "$CI_PIPELINE" "$LIST_FLOW_TESTS"
bd dep add "$CI_PIPELINE" "$STATUS_FLOW_TESTS"
bd dep add "$CI_PIPELINE" "$ATTACH_FLOW_TESTS"
bd dep add "$CI_PIPELINE" "$KILL_FLOW_TESTS"
bd dep add "$CI_PIPELINE" "$OBSERVABILITY_TESTS"
bd dep add "$CI_PIPELINE" "$HOST_TEST_INTEGRATION"
bd dep add "$CI_PIPELINE" "$FULL_FLOW_INTEGRATION"

RELEASE_BUILDS=$(create_bead "Implement release tooling for local mux CLI packaging/install and Linux amd64/arm64 mux-agent builds with deploy-path verification and MUX_AGENT_BINARY override coverage; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: Cargo.toml Cross.toml dist/** .github/workflows/**; acceptance: release job produces architecture-specific mux-agent artifacts and validates deploy lookup" 1 release)
bd dep add "$RELEASE_BUILDS" "$RELEASE_DESIGN_DOC"
bd dep add "$RELEASE_BUILDS" "$AGENT_DEPLOY"

RELEASE_TESTS=$(create_bead "Verify release artifact naming, Linux amd64/arm64 agent selection, MUX_AGENT_BINARY override, local CLI packaging/install command, and deploy-path lookup errors; cites docs/01-cli-commands.md docs/08-errors-observability-and-tests.md; reservation: tests/release/** scripts/** .github/workflows/**; acceptance: release verification covers architecture binaries and override path" 2 testing)
bd dep add "$RELEASE_TESTS" "$RELEASE_BUILDS"

DOCS_USER_GUIDE=$(create_bead "Write user-facing clean-room mux guide for init, host setup/test/trust, agent deploy/logs/stop, create/list/status/attach/kill, MUX_HOME, MUX_FORCE_TRANSPORT, and troubleshooting common errors; cites docs/01-cli-commands.md docs/04-ssh-trust-and-transport.md docs/08-errors-observability-and-tests.md; reservation: README.md docs/**; acceptance: guide matches implemented CLI contract and avoids original-source details" 2 docs)
bd dep add "$DOCS_USER_GUIDE" "$CLI_INIT"
bd dep add "$DOCS_USER_GUIDE" "$CREATE_FLOW"
bd dep add "$DOCS_USER_GUIDE" "$AGENT_LOGS_STOP"

FINAL_SPEC_AUDIT=$(create_bead "Perform requirement-by-requirement audit against docs/00-08, confirming implementation, tests, docs, CI, release artifacts, guardrails, and no original-source contamination; cites all clean-room specs; reservation: prompts/docs/spec-audit.md; acceptance: audit maps every explicit requirement to evidence or a follow-up bead" 0 testing)
bd dep add "$FINAL_SPEC_AUDIT" "$CI_PIPELINE"
bd dep add "$FINAL_SPEC_AUDIT" "$RELEASE_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$FULL_FLOW_INTEGRATION"
bd dep add "$FINAL_SPEC_AUDIT" "$DOCS_USER_GUIDE"
bd dep add "$FINAL_SPEC_AUDIT" "$OBSERVABILITY_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$SSH_TRANSPORT_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$NAMING_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$CREATE_FLOW_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$LIST_FLOW_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$STATUS_FLOW_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$ATTACH_FLOW_TESTS"
bd dep add "$FINAL_SPEC_AUDIT" "$KILL_FLOW_TESTS"

echo ""
echo "Bead graph created. Useful commands:"
echo "  bd ready"
echo "  bd list --label workspace"
echo "  bd list --label testing"
