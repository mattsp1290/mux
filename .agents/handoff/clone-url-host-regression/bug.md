# Bug: `mux create` clones from the SSH host instead of GitHub

**Date:** 2026-06-15
**Status:** FIXED (clone step) — a separate downstream error remains (see below)
**Area:** `mux create` / repo clone URL resolution

## Symptom

```text
mux create mattsp1290/dotfiles --host infra
mux: git clone failed with exit code 128: Cloning into '/home/infra-admin/.mux/<uuid>/dotfiles'...
Host key verification failed.
fatal: Could not read from remote repository.

Please make sure you have the correct access rights
and the repository exists.
```

`mattsp1290/dotfiles` is a public GitHub repo, and a manual
`git clone git@github.com:mattsp1290/dotfiles.git` *on the infra host succeeds*.
The feature also worked in the Go version at `~/git/mux-bak/`.

## Root cause

`mux create owner/repo --host <h>` built the git clone URL from the **SSH host's
address** instead of GitHub:

```rust
// crates/mux-cli/src/create.rs (before)
let clone_url = ctx.repo.clone_url_for(&ctx.host.addr);   // ctx.host.addr = "10.0.0.106"
```

For a bare `owner/repo` slug, `RepoRef::clone_url_for(default_host)`
(`crates/mux-core/src/types.rs:256`) substitutes the *default host* into
`git@{host}:{owner}/{repo}.git`. Passing `ctx.host.addr` produced:

```text
git@10.0.0.106:mattsp1290/dotfiles.git
```

i.e. the infra host tried to clone the repo **from itself**. It has no
`known_hosts` entry for `10.0.0.106`, so OpenSSH aborted with
`Host key verification failed`. The error wording is misleading — it has nothing
to do with github.com trust or SSH agent forwarding.

## Why it worked in `~/git/mux-bak/` (the Go version)

The Go implementation defaulted bare slugs to github.com:

```go
// ~/git/mux-bak/internal/cli/repo.go:49  (ResolveRepoURL)
return fmt.Sprintf("git@github.com:%s.git", slugOrURL), nil
```

The Rust rewrite regressed by passing `ctx.host.addr` as the default git host.

## How it was diagnosed

Two Opus subagents independently theorized SSH **agent forwarding** /
**login-shell env** problems. Both were wrong. The diagnosis came from
empirical probes, not theory:

1. A faithful mimic of mux's exec
   (`ssh -o BatchMode=yes infra-admin@10.0.0.106 'git clone git@github.com:...'`)
   **succeeded** — disproving the env/agent theories.
2. Forcing a clean `known_hosts` for the inner hop reproduced the exact error,
   and `StrictHostKeyChecking=accept-new` cleared it — confirming it was a
   host-key/trust failure on the *inner* git→remote SSH, not the mux→infra hop.
3. Shimming `ssh` on `$PATH` to log argv captured the real command and exposed
   the smoking gun: it was cloning from `git@10.0.0.106:...`, not
   `git@github.com:...`.

**Lesson:** capture the actual executed command before trusting a code-reading
theory. The `ssh`-shim trick (a wrapper that logs `"$@"` then `exec`s the real
`ssh`) was decisive.

## Fix

`crates/mux-cli/src/create.rs` (Step 7 clone) — default bare slugs to GitHub:

```rust
let clone_url = ctx.repo.clone_url_for("github.com");
```

Explicit-host inputs (`git@gitlab.com:owner/repo.git`) are unaffected:
`clone_url_for` only uses the default when the `RepoRef` parsed no host.

## Verification

- Captured clone command after fix:
  `git clone --branch 'main' 'git@github.com:mattsp1290/dotfiles.git' ...`
- The clone **succeeds**; the create flow advances past Step 7 (no more
  `git_clone_failed`).
- `cargo test -p mux-core -p mux-cli` → 38 passed, 0 failed.

## Remaining / follow-up

- **Separate downstream bug:** with the clone fixed, `mux create` now fails
  later with `agent error: internal: authentication failed` in the
  agent-start/RPC step (Step 8/9). This is unrelated to the clone host bug and
  is the next blocker for an end-to-end `mux create`.
- **Install:** the fix is built in `target/release/mux` but not yet installed to
  `~/.local/bin/mux`.
- **Commit:** the one-line fix in `create.rs` is not yet committed.
- **Roadmap:** make clone URL resolution try an ordered list of
  user-configurable URL templates (multi-forge, SSH+HTTPS). See
  `.agents/docs/roadmap.md` → "Configurable repo clone URL resolution".
