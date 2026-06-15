# mux roadmap / backlog

Forward-looking work items that are not yet scheduled into a plan under
`.agents/plan/`. Each entry should be self-contained enough to spin out into a
plan or a bead later.

## Configurable repo clone URL resolution

**Status:** proposed (not started)

**Motivation:** Today `mux create owner/repo --host <h>` hard-codes the git
remote host. After the host-key regression fix
(`.agents/handoff/clone-url-host-regression/bug.md`), a bare `owner/repo` slug
always resolves to `git@github.com:owner/repo.git`
(`crates/mux-cli/src/create.rs` → `RepoRef::clone_url_for`,
`crates/mux-core/src/types.rs:256`). That is correct for GitHub users but
hard-codes a single forge and a single transport (SSH).

**Goal:** Resolve a bare `owner/repo` slug against a *series of
user-configurable candidate URL templates*, tried in order, instead of one
hard-coded `git@github.com:...` form. This should support:

- Multiple forges (github.com, gitlab.com, self-hosted Forgejo/Gitea, etc.).
- Both SSH (`git@{host}:{owner}/{repo}.git`) and HTTPS
  (`https://{host}/{owner}/{repo}.git`) transports — HTTPS lets public repos
  clone with no host-key/credential setup on the remote.
- A defined precedence and, ideally, a fast existence/reachability probe or
  fallback-on-failure so the first working candidate wins.

**Sketch of approach (to be designed in a real plan):**

- Add a config surface (global config file and/or per-host override in the
  `hosts` table) holding an ordered list of URL templates with `{owner}`,
  `{repo}`, `{host}` placeholders.
- Keep `RepoRef`'s explicit-host inputs (`git@host:owner/repo.git`) authoritative
  — only host-less slugs go through template resolution.
- Decide the fallback semantics: try-next-on-clone-failure vs. resolve-once.
  Try-next is more robust but multiplies clone attempts; gate it behind a clear
  failure classifier (host-key vs. auth vs. not-found).
- Preserve the current default (github.com SSH) as the last/implicit candidate
  so existing behavior is unchanged when no config is present.

**Pointers:**

- Resolution today: `crates/mux-core/src/types.rs:249-259`
  (`clone_url`, `clone_url_for`).
- Call site: `crates/mux-cli/src/create.rs` (Step 7 clone).
- Prior art: the Go version defaulted bare slugs to `git@github.com:%s.git`
  (`~/git/mux-bak/internal/cli/repo.go:49`, `ResolveRepoURL`).
