# Clean-Room Guardrails for Implementation Agents

Spec: docs/00-clean-room-guidelines.md  
Status: Active  
Linked from: prompts/docs/README.md

## What is forbidden

You **must not**:
- Read, search, or inspect the original `mux` source code or any fork.
- Run an existing `mux` binary and observe its output.
- Use any configuration file produced by an existing `mux` installation.
- Use test output from an existing `mux` installation as a correctness oracle.
- Access any URL referencing the original project's source repository.

If you are unsure whether a source is "original-source": assume it is and avoid it.

## What is permitted

You **may**:
- Read and cite docs/00-08 in this repository.
- Read public Rust crate documentation (docs.rs, crates.io).
- Read IETF RFCs, OpenSSH documentation, SQLite docs, tmux man pages.
- Read general Rust language, stdlib, and book documentation.
- Use your own training knowledge about these public specifications.

## How to cite the spec

In every implementation bead and pull request, cite the specific spec section(s) you
are implementing. Format:

```
Implements: docs/03 §Sessions table, docs/07 §Create flow
```

If you interpret an ambiguous section, document the interpretation in the relevant
`prompts/docs/` file and note it in your PR description.

## When the spec is ambiguous

1. Pick the simplest, most conservative behaviour.
2. Document the interpretation in `prompts/docs/` (not in code comments).
3. Flag it in your PR description so it can be reviewed.

**Do not** resolve a contradiction between two spec sections unilaterally. Contradictions
must be escalated (open a PR against the spec docs) before implementing. See docs/00
§Compatibility stance.

## Invariants that must never be relaxed

These invariants from the spec protect data safety and security:
- `docs/02 §Workdir safety`: Never remove a workdir unless its path matches `$MUX_HOME/<uuid>/<repo-leaf>` exactly AND contains no symlink components.
- `docs/04 §TOFU`: Never silently update a stored fingerprint on mismatch. Always abort.
- `docs/04 §TOFU`: Never skip TOFU for state-changing operations (create, kill, attach).
- `docs/07 §Kill flow`: Never mutate session state before TOFU verification.
- `docs/07 §Create flow`: Never leave a partial session row in the DB. Always roll back.

## Audit

The final spec audit (mux-0h0) will verify that every requirement in docs/00-08 maps
to implementation evidence. Keep your bead/commit references in order.
