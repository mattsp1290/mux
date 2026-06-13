# mux Spec Index

Cross-reference index for the mux clean-room specification (docs/00-08).
Implementation beads cite this index to find the canonical section for each requirement.

## Quick navigation

| Topic | Spec section | Prompts doc (when created) |
|---|---|---|
| Clean-room rules | [docs/00](../../docs/00-clean-room-guidelines.md) | [clean-room-guardrails.md](clean-room-guardrails.md) |
| CLI commands & flags | [docs/01](../../docs/01-cli-commands.md) | [cli-contract.md](cli-contract.md) |
| Repo normalisation | [docs/02 §Repo normalisation](../../docs/02-naming-and-repositories.md#repo-normalisation) | [cli-contract.md](cli-contract.md) |
| Host alias rules | [docs/02 §HostAlias rules](../../docs/02-naming-and-repositories.md#hostalias-rules) | |
| Shortname sanitisation | [docs/02 §Shortname sanitisation](../../docs/02-naming-and-repositories.md#shortname-sanitisation) | |
| Workdir safety | [docs/02 §Workdir safety](../../docs/02-naming-and-repositories.md#workdir-safety) | |
| SQLite setup | [docs/03 §SQLite connection settings](../../docs/03-local-state.md#sqlite-connection-settings) | [sqlite-state.md](sqlite-state.md) |
| Schema — hosts | [docs/03 §Hosts table](../../docs/03-local-state.md#hosts-table) | [sqlite-state.md](sqlite-state.md) |
| Schema — fingerprints | [docs/03 §Known host fingerprints table](../../docs/03-local-state.md#known-host-fingerprints-table) | [sqlite-state.md](sqlite-state.md) |
| Schema — sessions | [docs/03 §Sessions table](../../docs/03-local-state.md#sessions-table) | [sqlite-state.md](sqlite-state.md) |
| Session reservation | [docs/03 §Reservation semantics](../../docs/03-local-state.md#reservation-semantics) | [sqlite-state.md](sqlite-state.md) |
| TOFU / host trust | [docs/04 §TOFU](../../docs/04-ssh-trust-and-transport.md#tofu-trust-on-first-use) | [ssh-transport.md](ssh-transport.md) |
| Transport selection | [docs/04 §Transport selection](../../docs/04-ssh-trust-and-transport.md#transport-selection) | [ssh-transport.md](ssh-transport.md) |
| Attach key pinning | [docs/04 §Attach pinning](../../docs/04-ssh-trust-and-transport.md#attach-pinning) | [ssh-transport.md](ssh-transport.md) |
| SSH library decision | [docs/04](../../docs/04-ssh-trust-and-transport.md) | [stack-validation.md](stack-validation.md) |
| Agent RPC operations | [docs/05 §RPC protocol](../../docs/05-agent-rpc-and-lifecycle.md#rpc-protocol) | [rpc-protocol.md](rpc-protocol.md) |
| Agent startup | [docs/05 §Agent startup](../../docs/05-agent-rpc-and-lifecycle.md#agent-startup) | |
| Agent ownership | [docs/05 §Agent ownership](../../docs/05-agent-rpc-and-lifecycle.md#agent-ownership) | |
| tmux argv contract | [docs/06](../../docs/06-tmux-behavior.md) | [tmux-contract.md](tmux-contract.md) |
| tmux session naming | [docs/06 §Session naming](../../docs/06-tmux-behavior.md#session-naming) | [tmux-contract.md](tmux-contract.md) |
| tmux list parsing | [docs/06 §Session listing argv](../../docs/06-tmux-behavior.md#session-listing-argv) | [tmux-contract.md](tmux-contract.md) |
| Create flow | [docs/07 §Create flow](../../docs/07-create-list-status-kill-flows.md#create-flow) | [session-flows.md](session-flows.md) |
| List flow | [docs/07 §List flow](../../docs/07-create-list-status-kill-flows.md#list-flow) | [session-flows.md](session-flows.md) |
| Status flow | [docs/07 §Status flow](../../docs/07-create-list-status-kill-flows.md#status-flow) | [session-flows.md](session-flows.md) |
| Kill flow | [docs/07 §Kill flow](../../docs/07-create-list-status-kill-flows.md#kill-flow) | [session-flows.md](session-flows.md) |
| Attach flow | [docs/07 §Attach flow](../../docs/07-create-list-status-kill-flows.md#attach-flow) | [session-flows.md](session-flows.md) |
| Error categories | [docs/08 §Error categories](../../docs/08-errors-observability-and-tests.md#error-categories) | |
| Error hints | [docs/08 §Human-readable hints](../../docs/08-errors-observability-and-tests.md#human-readable-hints) | |
| Test strategy | [docs/08 §Test strategy](../../docs/08-errors-observability-and-tests.md#test-strategy) | |
| CI gates | [docs/08 §CI gates](../../docs/08-errors-observability-and-tests.md#ci-gates-separate-steps) | |
| Release & deploy | [docs/08](../../docs/08-errors-observability-and-tests.md) | [release.md](release.md) |
| Integration test env | [docs/08 §Integration tests](../../docs/08-errors-observability-and-tests.md#integration-tests) | [integration-tests.md](integration-tests.md) |
| Spec audit | all docs/00-08 | [spec-audit.md](spec-audit.md) |

## Prompts docs status

| File | Status | Created by |
|---|---|---|
| `clean-room-guardrails.md` | pending | mux-ii3 |
| `cli-contract.md` | pending | mux-pre |
| `sqlite-state.md` | pending | mux-8ep |
| `ssh-transport.md` | pending | mux-7nr |
| `stack-validation.md` | pending | mux-a2b |
| `rpc-protocol.md` | pending | mux-2n5 |
| `tmux-contract.md` | pending | mux-14j |
| `session-flows.md` | pending | mux-s66 |
| `release.md` | pending | mux-7ng |
| `integration-tests.md` | pending | mux-3bv |
| `spec-audit.md` | pending | mux-0h0 |

## How to cite this index

In a beads task or implementation comment, cite as:
```
docs/03 §Sessions table
docs/04 §TOFU
docs/07 §Create flow
```

Or link to this file for the full cross-reference:
```
prompts/docs/README.md
```
