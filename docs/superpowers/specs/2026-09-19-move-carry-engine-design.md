# Move carry engine: transfer a session with the work as it is

**Date:** 2026-09-19
**Status:** Design — not yet implemented
**Supersedes:** the preflight clause of `docs/adr/0001-descope-freeze-ship-move.md`
(through a new ADR 0002, part of this work)
**Slice:** 1 of 3 of the "easy transfer button" effort. Slice 2 (sidechain
transcripts + Claude project memory) and slice 3 (the Transfer button,
preflight sheet, progress events, wait-for-idle, retry-from-step) get their
own specs and build on the report this slice produces.

## Goal

`move_session` succeeds on a session whose worktree is dirty, whose branch
has unpushed commits or is not on origin at all, and whose target host has no
clone and no working route to origin. The work arrives **as it is**:
uncommitted stays uncommitted, staged stays staged, untracked stays
untracked, unpushed stays unpushed. Small git-ignored config files
(`.env`, `.claude/settings.local.json`, …) travel too; large rebuildable
directories do not, and the report says which.

The only guaranteed network path is orchestrator (desktop or hub) → each host
over SSH. Hosts are never assumed to reach each other or origin. Everything
relays through the orchestrator, the way the transcript does today.

## Principle (ADR 0002)

ADR 0001 said the move "never makes a git decision for the user" and
implemented it as a refusal. ADR 0002 keeps the principle and drops the
refusal:

> The move never pushes, never commits to or moves a user branch on the
> source, never stashes, and never modifies the source working tree or
> index. It carries the work as it is.

What the move does write on the source, and nothing else: git objects for
the snapshot (blobs, two trees, two commits — unreferenced once the move
ends, so `git gc` reclaims them), a private ref namespace
`refs/fleet/transfer/<claude_id>/*`, and a temp directory
`~/.cache/claude-fleet/transfer/<claude_id>/`. The refs and the directory are
deleted at the end of the move.

## Step flow

Steps marked *unchanged* are today's code. Every new step runs before the
target tmux session starts, so a failure in any of them keeps today's
`before_target` semantics: the source is untouched and the step's own error
is returned.

1. **Idle check** — unchanged.
2. **Inspect (extended)** — today's inspection plus a mid-operation probe:
   `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`,
   `rebase-merge/`, `rebase-apply/` under `git rev-parse --git-dir`. Any of
   them → `E_MOVE_MIDOP`. In carry mode, dirty / unpushed / not-on-origin are
   inputs, not refusals. `HEAD` unreadable and wrong-branch stay refusals.
3. **Transcript locate + read** — unchanged.
4. **Seed the target** — ensure the main clone exists at the layout-derived
   project root: present → `existing`; absent → `git clone <origin url>`
   (`cloned`); clone fails → `git init` + `git remote add origin <url>`
   (`initialized`). The clone runs with `GIT_TERMINAL_PROMPT=0` and
   `GIT_SSH_COMMAND='ssh -oBatchMode=yes'` under `COPY_TIMEOUT`, so a host
   without credentials falls through to `init` instead of hanging on a
   prompt; a partial clone directory is removed before the `init`. Then the
   existing best-effort prefetch. Then list the target's **haves**:
   `git for-each-ref --format='%(objectname)'`, de-duplicated.
5. **Snapshot + bundle + relay** — see below. Ends with the transfer refs
   fetched into the target's main clone and `refs/heads/<branch>` created at
   the source HEAD when the target has no such local branch.
6. **`ensure_target_workspace`** — unchanged. `repair`'s Mirror add checks
   `refs/heads/<branch>` before asking origin, so it finds the branch step 5
   landed.
7. **Target prep (extended)** — today's script (fast-forward to the source
   HEAD, `DIVERGED` refusal, transcript path resolution), then **apply the
   snapshot** and print the resulting porcelain for verification.
8. **Ignored files** — select on the source, tar, relay, extract.
9. **Transcript put → start → confirm → kill source → events** — unchanged.
10. **Cleanup** — best effort, on success and on every failure path after
    step 5 began: delete `refs/fleet/transfer/<claude_id>/*` on both hosts and
    `~/.cache/claude-fleet/transfer/<claude_id>/` on both hosts.

## Git mechanics

### Snapshot (source, one script)

Runs in the source worktree. Touches neither the working tree, the real
index, nor any ref outside `refs/fleet/transfer/`.

```sh
itree=$(git write-tree)                                  # what is staged
tmp=$(mktemp); cp "$(git rev-parse --git-path index)" "$tmp"
wtree=$(GIT_INDEX_FILE="$tmp" git add -A && GIT_INDEX_FILE="$tmp" git write-tree)
rm -f "$tmp"
# fixed identity so a host without user.name/user.email cannot fail here
export GIT_AUTHOR_NAME=claude-fleet GIT_AUTHOR_EMAIL=fleet@localhost \
       GIT_COMMITTER_NAME=claude-fleet GIT_COMMITTER_EMAIL=fleet@localhost
ix=$(git commit-tree "$itree" -p HEAD -m 'fleet transfer: index')
wt=$(git commit-tree "$wtree" -p HEAD -m 'fleet transfer: worktree')
git update-ref refs/fleet/transfer/$id/ix   "$ix"
git update-ref refs/fleet/transfer/$id/wt   "$wt"
git update-ref refs/fleet/transfer/$id/head HEAD
```

`git add -A` honours `.gitignore`, so ignored files stay out of the snapshot
and are handled by step 8. `git write-tree` fails on an unmerged index; step
2's mid-op refusal makes that unreachable in practice, and a failure is
`E_MOVE_CARRY` regardless.

When the worktree is clean, `ix` and `wt` trees both equal `HEAD^{tree}`; the
snapshot is still taken (one code path) and the apply is a no-op.

### Bundle (source)

The orchestrator passes the target's haves to the source. The source keeps
those it also has (`git cat-file -e <sha>^{commit}`) and runs:

```sh
git bundle create "$dir/carry.bundle" \
  refs/fleet/transfer/$id/head refs/fleet/transfer/$id/ix refs/fleet/transfer/$id/wt \
  --not <kept haves…>
```

`$dir` is `~/.cache/claude-fleet/transfer/<claude_id>/`, created `0700`.
The haves reach the script as a quoted heredoc read line by line (validated
as 40/64-hex on the orchestrator first), never as argv, so a target with
thousands of refs cannot overflow the command line. `CarryReport.commits` is
`git rev-list --count refs/fleet/transfer/$id/head --not <kept haves>`.

**The bundle is never skipped.** Even when the target already has the source
HEAD, the `ix` / `wt` commits are new objects, so the bundle is never empty.
For a clean, pushed, up-to-date session it is a few hundred bytes. One code
path, no special case.

The bundle's size is checked on the source against `move.max_bundle_mb`
(default 500) before it is downloaded → `E_MOVE_TOO_LARGE` with
`details.payload = "bundle"`.

### Relay

Source temp file → `download_file` → orchestrator `TempFile` (0600, removed
on drop) → `upload_file` → `$dir/carry.bundle` on the target. Neither payload
is held in memory.

### Fetch (target main clone)

```sh
git -C "$root" fetch -q "$dir/carry.bundle" \
  '+refs/fleet/transfer/*:refs/fleet/transfer/*'
git -C "$root" show-ref --verify -q "refs/heads/$br" \
  || git -C "$root" branch -- "$br" "refs/fleet/transfer/$id/head"
```

An existing local `refs/heads/<branch>` is never moved here; the unchanged
prep script fast-forwards it (or refuses with `DIVERGED`) inside the
worktree, where it is checked out.

### Apply (target worktree, inside the prep script)

Precondition: `git status --porcelain` is empty → else `E_MOVE_TARGET_DIRTY`.
A freshly created worktree always passes; a pre-existing one with local
changes is never overwritten. Precondition 2: `HEAD` equals the source HEAD
(today's prep may leave the target *ahead* when origin is newer; with a
non-trivial snapshot that is `E_MOVE_CARRY` step `apply` — "target is ahead
of the source; cannot replay uncommitted work onto a different base". With a
clean source it stays today's warning).

```sh
git read-tree -u --reset "refs/fleet/transfer/$id/wt^{tree}"   # tree + index := snapshot
git read-tree            "refs/fleet/transfer/$id/ix^{tree}"   # index := what was staged
git status --porcelain=v1
```

### Verification

The orchestrator compares the target's post-apply porcelain with the source's
inspection porcelain, as sorted line sets. Any difference → `E_MOVE_CARRY`
step `verify`, with both sides in `details`. A successful carry is a checked
claim.

## Ignored files (step 8)

Selection, on the source worktree:

1. `git ls-files -o -i --exclude-standard --directory -z` — top-level ignored
   entries; a wholly ignored directory is one entry, so `node_modules` is
   never walked.
2. Drop any entry whose final path component is in the deny-list:
   `node_modules target .venv venv dist build out .next .nuxt .svelte-kit
   __pycache__ .gradle .cache .turbo coverage` → left behind, reason
   `denylisted`.
3. Size each survivor (`du -sk`). Over `move.ignored_entry_kb` (default 1024)
   → left behind, reason `over_cap`.
4. Add survivors smallest-first until `move.ignored_total_mb` (default 20)
   would be exceeded; the rest are `over_cap`.

The kept entries are tarred (`tar -czf "$dir/ignored.tgz" -C <worktree> --null
-T <list>`), relayed like the bundle, and extracted in the target worktree
with keep-existing semantics (`tar -xzkf`, GNU and BSD both support `-k`), so
a file already present on the target wins.

This step cannot fail the move. A selection, tar, relay or extract failure
becomes a report warning ("ignored files were not carried: …") and
`ignored_carried` is empty. With nothing selected the step is skipped.

## API

- `MoveSessionArgs` / MCP `MoveSessionParams`: new `strict: bool`, serde
  default `false`. `strict: true` runs today's `preflight_verdict` unchanged
  (`E_MOVE_DIRTY`, `E_MOVE_UNPUSHED`) and then the same flow — the carry steps
  still run, they just have nothing dirty or unpushed to carry.
- The frontend wrapper `src/lib/moveSession.ts` gains the optional `strict`
  argument and the new report fields; the existing dialog passes nothing
  (carry). No UI work beyond keeping the types in step — that is slice 3.
- The `move_session` tool description is rewritten → regenerate
  `docs/control-api-reference.md` (`REGEN_DOCS=1 cargo test -p fleet-core
  reference_is_current`).

`MoveReport` gains:

```rust
pub struct CarryReport {
    pub commits: u32,                    // commits in the bundle besides ix/wt
    pub bundle_bytes: u64,
    pub dirty_entries: Vec<DirtyFile>,   // porcelain rows restored on the target
    pub ignored_carried: Vec<IgnoredEntry>,
    pub ignored_left_behind: Vec<LeftBehind>,
    pub target_seeded: TargetSeed,       // Existing | Cloned | Initialized
}
pub struct IgnoredEntry { pub path: String, pub bytes: u64 }
pub struct LeftBehind   { pub path: String, pub bytes: u64, pub reason: LeftReason } // Denylisted | OverCap
```

The same struct is embedded in the `session_moved` event detail.

Report warnings added by this slice:

- dirty entries were carried → "the source worktree `<path>` on `<host>`
  still holds a copy of the uncommitted work" (the source worktree is left
  exactly as it was, including after the source session is killed);
- `.gitmodules` present → submodule contents were not carried;
- `.gitattributes` mentions `filter=lfs` → LFS objects were not carried;
- `target_seeded = Initialized` → "origin was unreachable from `<target>`;
  the clone was initialised from the bundle and cannot fetch or push until
  origin is reachable".

## Errors

All fire before the target tmux session starts; the source is untouched.

| Code | When |
|---|---|
| `E_MOVE_MIDOP` (new) | source worktree is mid merge / rebase / cherry-pick / revert / bisect |
| `E_MOVE_TARGET_DIRTY` (new) | a pre-existing target worktree has uncommitted changes |
| `E_MOVE_CARRY` (new) | seed / snapshot / bundle / download / upload / fetch / apply / verify failed; `details.step` names it, `details.stderr` carries the output |
| `E_MOVE_TOO_LARGE` (reused) | bundle over `move.max_bundle_mb`; `details.payload` = `"bundle"` (`"transcript"` for today's case) |
| `E_MOVE_DIRTY`, `E_MOVE_UNPUSHED` | only with `strict: true` |

`E_MOVE_PARTIAL` semantics are unchanged: they begin when the target tmux
session exists, which is after every carry step.

## Transport addition

`SshExec` gains the mirror of `upload_file`:

```rust
async fn download_file(&self, host: &str, remote_path: &str,
                       local_path: &Path, timeout: Duration) -> Result<(), IpcError>;
```

`ssh … -- host 'cat -- <quoted path>'` with stdout redirected to the local
file (created `0600` by the caller's `TempFile`), bounded by the same
wall-clock as uploads, failing with a new `E_DOWNLOAD`. Implemented on
`SshClient`, the local exec, and `FakeSsh` (which serves bytes registered by
the test). `TempFile` gains an empty-file constructor for download targets.

## Module layout

`service/move_session.rs` (2,450 lines) becomes a directory:

```
service/move_session/
  mod.rs     today's file moved as-is; the step flow gains the carry calls,
             `strict`, and CarryReport plumbing
  carry.rs   pure script builders + parsers + CarryReport types:
             seed_script, haves_script, snapshot_script, bundle_script,
             fetch_script, apply_script (appended into the prep script),
             ignored_select_script, ignored_pack_script,
             ignored_extract_script, cleanup_script
```

`carry.rs` follows the house pattern: `fn …_script(..) -> String` with every
interpolated value through `crate::shell::quote`, sentinel markers for
recognisable failures, and pure parsers for each script's output. The async
glue lives in `mod.rs` and goes through `&dyn SshExec`, so the whole flow
keeps running end-to-end over `FakeSsh`. The file move is its own commit with
no content change, so the diff of the behavioural commits stays readable.

## Testing

1. **Real-git round trip** (`carry.rs` tests; skipped with a clear message
   when `git` or `bash` is absent). A temp source repo with: a modified file,
   a staged new file, a staged-then-modified file, an untracked file, a
   deleted tracked file, a mode change, a symlink, a file with spaces and a
   quote in its name, and two unpushed commits. The generated scripts run
   through `bash` locally: snapshot → bundle → fetch into a second repo →
   worktree add → apply. Assertions: identical `status --porcelain=v1` line
   sets, identical contents and modes of every listed file, equal `HEAD`.
   Variants: target has no repo (`init` seed); target has the base commit
   (thin bundle, asserted smaller than the full one); clean source (apply is a
   no-op, porcelain empty on both). Source invariants, asserted before/after:
   working-tree content hash, index file bytes, and `for-each-ref refs/heads
   refs/remotes refs/tags` are identical; after cleanup `for-each-ref
   refs/fleet` is empty on both repos.
2. **Ignored selection**, real filesystem: deny-listed dir, over-entry-cap
   file, total-cap overflow (smallest-first order), a small `.env` carried;
   extract does not overwrite a pre-existing target file.
3. **`FakeSsh` flow** (existing fixture style): dirty + unpushed now succeeds
   and reports `CarryReport`; `strict: true` still refuses with the old codes
   before anything runs on the target; mid-op refused; dirty target refused
   before apply; porcelain mismatch → `E_MOVE_CARRY` `verify`; bundle over the
   cap refused before download; an ignored-step failure is a warning and the
   move succeeds; cleanup scripts ran on both hosts on a failure after step 5
   and on success; every carry failure leaves the source row and session
   untouched (`assert_source_untouched`).
4. **Quoting**: `scripts_quote_every_interpolated_value` extended to every
   new builder, with hostile values (spaces, quotes, `$()`, newlines) for
   paths, branch, id and haves.
5. **MCP/params**: `strict` defaults to `false` when absent; the reference
   doc test passes after regeneration.

## Out of scope

- Sidechain/subagent transcripts, Claude project memory — slice 2.
- Transfer button, preflight sheet ("what travels / what stays"), progress
  events, wait-for-idle, retry-from-step, "delete the source worktree"
  action — slice 3.
- Carrying mid-merge/rebase state, git stashes, submodule contents, LFS
  objects, hooks or per-repo git config.
- Direct host-to-host transfer. Everything relays through the orchestrator.
