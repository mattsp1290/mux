# 02 — Naming and Repositories

## HostAlias rules

- Characters: ASCII alphanumeric, hyphens (`-`), underscores (`_`).
- First character: must be ASCII alphanumeric (leading `-` forbidden — argv injection).
- Length: 1–64 characters.
- Dots are NOT permitted (no FQDN-aliasing risk).
- Case: case-sensitive; `Host` and `host` are distinct aliases.

## Repo normalisation

Accepted input forms:

| Input form | Example |
|---|---|
| `owner/repo` | `mattsp1290/mux` |
| `git@host:path.git` | `git@github.com:mattsp1290/mux.git` |

Rejected: `owner/repo.git` shorthand (ambiguous `.git` suffix).

Normalisation steps:
1. Strip trailing `.git` from the path component.
2. Extract `owner` and `repo` from the path.
3. Produce `repo_slug`: `{owner}/{repo}` (forward-slash canonical form, lowercase).
   This is the authoritative owner/repo identifier used in all storage, RPC, and
   kill-ownership comparisons.
4. Produce `storage_slug`: `{owner}-{repo}` (lowercase, hyphens for non-alnum).
   Used only for filesystem paths where `/` is invalid.
5. Clone URL: `git@{host}:{owner}/{repo}.git`.
6. Repo leaf: the `repo` component.
7. Owner/repo split is stored; never re-derived from the clone URL.

## Shortname sanitisation

A shortname is a human-readable label for a tmux session and SQLite row.

Rules:
- Characters: lowercase ASCII alphanumeric and hyphens only.
- Max length: 124 bytes (tmux session name limit with `mux-` prefix overhead).
- Truncation: truncate at a hyphen boundary where possible; hard-truncate at 124 if no
  boundary is available.
- `mux-` prefix is prepended to every tmux session name to avoid collisions with
  non-mux sessions.

### Main-branch shortname

Format: `{repo-leaf}-{adjective}-{noun}` where adjective+noun is randomly selected.
- Example: `mux-happy-panda`.
- The `main`/`master` suffix is NOT appended; a random suffix is used instead.
- Uniqueness: iterate adjective-noun pairs until no collision in the session store.

### Non-main-branch shortname

Format: `{repo-leaf}-{sanitised-branch}`.
- `sanitised-branch`: lowercase, replace non-alnum with `-`, collapse runs of `-`.
- Deterministic: same repo+branch always produces the same shortname.
- Collision: resolved by appending `-2`, `-3`, etc. (not random).

## Workdir safety

A session workdir is considered **mux-created** (and therefore removable) only if:
- Path matches `$MUX_HOME/<uuid>/<repo-leaf>` (where `$MUX_HOME` is the resolved
  state directory — `MUX_HOME` env var, or `~/.mux` by default).
- No symlinks in the path.

Workdirs not matching this pattern (imported sessions) must never be removed.
