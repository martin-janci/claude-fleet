# Base-branch choice for new-worktree sessions

**Date:** 2026-05-23
**Status:** Design approved, pending implementation plan

## Problem

When a session is created with a *new worktree*, `worktree_add_script`
(`src-tauri/src/service/sessions.rs`) always forks the new branch off the
repo's **default branch** (resolved from `origin/HEAD`, falling back to the
current `HEAD`). There is no way to base the new worktree on another branch —
e.g. `dev` — so work that should branch off an integration branch instead
branches off `main`/`master`.

## Goal

Let the user choose the base branch per session at creation time, defaulting to
the repo's default branch. Empty selection preserves today's behavior exactly.

## Non-goals

- No per-project or global "preferred base" configuration (rejected during
  brainstorming in favor of a per-session choice).
- No branch-list dropdown / lookup (rejected in favor of free-text input).
- No `git fetch` before resolving a remote base (see Known limitations).

## Behavior

The new-worktree flow accepts an optional **base branch**. Resolution when a
non-empty base (e.g. `dev`) is given:

1. Local branch `refs/heads/<base>` exists → fork from it.
2. Else remote-tracking `refs/remotes/origin/<base>` exists → fork from
   `origin/<base>`.
3. Else **fall back to the repo's default branch** — silently; the worktree is
   created regardless of whether the requested base resolved.

Empty / absent base → use the default branch (current behavior, unchanged).

This silent fallback is intentional (chosen during brainstorming): the user
prefers a created worktree over an error when the typed base does not exist.

## Architecture

The change threads one optional value from the dialog down to the shell script;
no new components or data flow are introduced. Both the local and remote
new-worktree code paths already funnel through `worktree_add_script`, so a
single script change covers both hosts.

### 1. Shell script — `worktree_add_script(root, name, base: Option<&str>)`

`src-tauri/src/service/sessions.rs:686`. Adds a `base` parameter. The script
still computes `def` (the default branch) as today, then selects the start
point:

```sh
# ... existing base-dir detection + `def` computation unchanged ...
if [ -n "$base" ]; then
  if   git show-ref --verify --quiet "refs/heads/$base";          then start="$base"
  elif git show-ref --verify --quiet "refs/remotes/origin/$base"; then start="origin/$base"
  else start="$def"   # requested base not found → fall back to default
  fi
else
  start="$def"
fi
git worktree add "$wt" -b "$name" "$start" 1>&2
```

`base` is shell-quoted exactly like `root`/`name` (it is interpolated into a
remote SSH command string — see the shell-quoting convention in `CLAUDE.md`).
The script's stdout contract is unchanged: the only stdout line is the
worktree's absolute path.

### 2. Service layer

- `NewSessionArgs` gains `pub base_branch: Option<String>`
  (`src-tauri/src/service/sessions.rs`).
- `create_worktree_local(root, name, base)` — new `base` arg, forwarded to
  `worktree_add_script`.
- Remote new-worktree call site (≈ sessions.rs:829) passes `base` into
  `worktree_add_script`.
- The value is normalized: an empty / whitespace-only string is treated as
  `None` (= default branch) at the service boundary, so the script never
  receives a blank `base`.

### 3. Tauri command + frontend

- The `new_session` Tauri command already forwards `NewSessionArgs`; the new
  field rides along.
- `src/lib/NewSessionDialog.svelte`: add a **"Base branch"** text input,
  rendered only when the "new worktree" option is active. Placeholder text
  `default branch`; empty value sends `base_branch: null`. No default-branch
  lookup — empty means default.

### 4. MCP control API (parity)

- The `new_session` MCP tool (`src-tauri/src/mcp/tools.rs`) gains an optional
  `base_branch` param, documented as "branch to fork a new worktree from;
  defaults to the repo's default branch; ignored unless creating a new
  worktree."
- Regenerate `docs/control-api-reference.md`
  (`REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`)
  or CI fails.

## Testing

- **Unit (`worktree_add_script`):** assert the script contains all three
  resolution branches — `refs/heads/$base`, `refs/remotes/origin/$base`, and
  the `$def` fallback — and that `base` is quoted. Keep the existing
  default-branch-detection assertions.
- **Service round-trip:** a `base_branch` of `Some(" ")` / `Some("")`
  normalizes to `None` (default-branch path); `Some("dev")` reaches the script.
- **Frontend:** `NewSessionDialog` test that the base-branch input appears only
  in new-worktree mode and that its value is passed through to the
  `new_session` invocation (null when empty).

## Known limitations

- On a remote host with an **existing** clone, `origin/<base>` may be stale
  because the flow does not `git fetch` before resolving. This is the same
  property the default-branch resolution has today; addressing it is out of
  scope.
- The actual base a worktree was created from is not surfaced back to the user;
  on fallback there is no notice (per the chosen silent-fallback behavior).

## Files touched

- `src-tauri/src/service/sessions.rs` — script + `NewSessionArgs` + call sites + tests
- `src-tauri/src/mcp/tools.rs` — MCP `new_session` param
- `docs/control-api-reference.md` — regenerated
- `src/lib/NewSessionDialog.svelte` + its test — base-branch input
