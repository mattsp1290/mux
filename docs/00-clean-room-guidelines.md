# 00 — Clean-Room Guidelines

## Purpose

`mux` is a **clean-room Rust reimplementation**. Implementation agents must derive all
behaviour exclusively from these specification documents (docs/00-08). No original-source
inspection, no compatibility testing against the original binary, and no copying of
original code or configuration files is permitted.

## Permitted sources

- These specification documents (docs/00-08).
- Public Rust crate documentation (docs.rs, crates.io).
- IETF/SSH/SQLite/tmux public specifications.
- General Rust language and stdlib documentation.

## Forbidden sources

- The original `mux` source repository or any fork.
- Any binary from an existing `mux` installation.
- Any configuration file produced by an existing `mux` installation.
- Any test output from an existing `mux` installation used as a compatibility oracle.

## Compatibility stance

Compatibility with the original `mux` is **not a goal**. The implementation must satisfy
the contracts in docs/01-08. Where a spec section is silent, pick the simplest correct
behaviour and document it in `prompts/docs/`.

## Guardrails for implementation agents

1. Cite the specific spec section (e.g. `docs/03 §Hosts table`) before implementing.
2. If a requirement appears in two spec sections, prefer the more specific one.
3. If the spec is ambiguous, record the interpretation in `prompts/docs/` and proceed.
4. Never relax a documented invariant (e.g. ownership check before workdir removal)
   for implementation convenience.
5. All public API contracts (CLI flags, error messages, exit codes, wire types) must
   match the spec exactly; internal implementation details are unconstrained.

## Audit trail

The final audit (mux-0h0) will verify requirement-by-requirement coverage. Every P0
implementation bead must trace to at least one spec section.
