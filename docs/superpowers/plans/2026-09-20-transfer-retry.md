# Transfer slice 3d (retry and the return trip) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A failed or already-completed transfer becomes recoverable: the move
adopts a target whose leftovers are byte-for-byte the work it was about to
write, offers a scoped cleanup when they are its own but stale, refuses when
they are the target's, and the sheet grows Retry, Clean up and retry, Move back,
Finish and Undo.

**Architecture:** Three layers, bottom up. (1) Two new shell scripts in
`carry.rs` — a content-exact verifier over a throwaway git index, and the
rollback lifted out of `apply_script` — with their parsers. (2) `mod.rs` wires
the verifier into the one place `TARGET_DIRTY` is turned into an error, adds the
`clean_target` argument, and grows a `PartialCtx` so a partial move records
everything a later recovery needs; the move's final source step moves into
`finalise.rs` so `resolve.rs` can run it too. (3) The frontend gains the four
actions, reading a session's move history from events it already fetches.

**Tech Stack:** Rust (tokio, async-trait, rusqlite, serde), Tauri 2 commands,
Svelte 5 runes + Vitest, POSIX/bash shell scripts executed over `ssh` as
`bash -lc`.

**Spec:** `docs/superpowers/specs/2026-09-20-transfer-retry-design.md` — read it
before Task 1 and re-read §3 before Task 2. The plan argues from the spec; where
they disagree, the spec wins and the disagreement is a finding.

## Global Constraints

- **Never run a dev build of the desktop app** (`cargo tauri dev`,
  `pnpm tauri dev`, `cargo run -p claude-fleet`). It derives its data dir from
  `HOME`, so it migrates the installed app's `state.db` irreversibly and its
  singleton guard kills the running app. UI is verified by Vitest only.
- **Never ssh anywhere.** Every engine test runs over `FakeSsh`. The real
  cross-host harness (`cargo run -p fleet-core --example carry_e2e -- <host>`)
  is the controller's call, never a task's.
- **Every value interpolated into a shell command string is quoted with
  `crate::shell::quote`** (imported as `quote`). There is exactly one
  implementation; never add another.
- **Never print full process command lines** (`pgrep -fl`, `ps aux`): MCP
  servers on this machine carry API tokens in argv. `pgrep -x cargo | wc -l`.
- **Git:** every command is `git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 …`.
  Never `pull`, `push`, `rebase`, `checkout`, `stash`, `merge` or `reset`.
  No attribution lines in commit messages.
- **Run the whole crate suite, unpiped**, at the end of every task:
  `cargo test -p fleet-core` for engine tasks, `cargo test -p claude-fleet` for
  `src-tauri` tasks, `npx vitest run` for frontend tasks. Never judge a run
  through `| tail` or a `-k`/`--` filter; a filtered run once hid a red test for
  five tasks. If `pnpm test`/`pnpm check` cannot find a binary, use
  `npx vitest run` / `npx svelte-check`.
- **Wire rule:** report and event types derive `Serialize + Deserialize` with no
  `#[serde(default)]`. A new *argument* on a hub-routed command needs
  `Serialize` plus a non-default row in `src-tauri/src/backend/tests_routing.rs`.
- **Sentinels are compared with `contains`, never parsed positionally**, and
  every script's payload is anchored on `carry::OUT_MARKER` because `bash -lc`
  sources a login profile that may print to stdout first.
- **Error codes:** no new `E_*` codes. Reuse `E_MOVE_TARGET_DIRTY`,
  `E_MOVE_CARRY`, `E_INVALID_STATE`.

---
## File Structure

**One writer per file.** Tasks that touch the same file are strictly sequential;
the parallel groups are named in "Execution order" below.

| File | Responsibility | Task |
|---|---|---|
| `crates/fleet-core/src/service/move_session/carry.rs` | the two new shell scripts (verify, recover), their sentinel and parsers; `apply_script`'s rollback lifted to be shared | 1 |
| `crates/fleet-core/src/service/move_session/mod.rs` | classifying a dirty target (adopt / ours / theirs / unknown) and wiring it into step 3c; `clean_target`; `PartialCtx` + the richer partial event; delegating the final source step | 2, 3, 4, 5 |
| `crates/fleet-core/src/ssh_fake.rs` | `on_host_once`: a rule that answers one call and then falls through (test utility) | 3 |
| `crates/fleet-core/src/service/move_session/finalise.rs` | **new** — the move's final source step (transcript re-check, usage carry, kill, post-kill look, `session_moved`), shared by the move and `resolve_move` | 5 |
| `crates/fleet-core/src/service/move_session/resolve.rs` | **new** — `resolve_move` (Finish / Undo), its args, report and refusals | 6 |
| `crates/fleet-core/src/mcp/tools/lifecycle.rs` | one clause for `clean_target` in `move_session`'s description | 7 |
| `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs`, `src-tauri/src/backend/tests_routing.rs` | the `resolve_move` command, its hub verdict and routing rows; `clean_target`'s non-default args row | 7 |
| `src/lib/moveSession.ts`, `src/lib/moves.ts` | the IPC wrappers and the run store's retry / resolve transitions | 8 |
| `src/lib/moveErrors.ts` | the three `leftovers` texts and the undone / finished lines | 9 |
| `src/lib/timeline.ts` | `moveOrigin` / `unresolvedPartial`, pure over events the panel already has | 10 |
| `src/lib/TransferSheet.svelte` | Retry, Clean up and retry, Move back, Finish, Undo | 11 |
| `src/lib/SessionDetails.svelte` | the same actions from the details panel | 12 |

Generated files (never hand-edited): `docs/control-api-reference.md`,
`src/lib/hub_verdicts.generated.json`, the refusal table in `docs/hub.md`.

## Execution order

```
1 → 2 → 3 → 4 → 5 → 6 → 7        (Rust, strictly sequential: 2-6 all write mod.rs)
9, 10 in parallel                 (moveErrors.ts, timeline.ts)
        8 (needs 9 for UNDONE, 10 for UnresolvedPartial)
                11 (needs 8, 9)   12 (needs 10)
```

The Rust chain and the frontend chain are independent: 9 and 10 may start at
any time, including alongside Task 1.

---

### Task 1: The verify and recover scripts

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/carry.rs` — add
  `LEFTOVERS_DIFFER`, `Leftovers`, `verify_replayed_script`, `recover_script`,
  `parse_leftovers`, `parse_recover`; lift `apply_script`'s inner `recover()`
  into a shared `recover_body()`.
- Test: the same file's `#[cfg(test)] mod tests` (this crate tests inline).

**Interfaces:**
- Consumes: existing `quote`, `id_guard()`, `home_guard()`, `OUT_MARKER`,
  `STATUS_PORCELAIN`, `FAILED`, `HEAD_MISMATCH`, `payload_str`, `parse_err`, and
  the test helpers `require`, `bash`, `git`, `dirty_source`, `ID`.
- Produces, for Task 2:
  - `pub const LEFTOVERS_DIFFER: &str = "__CF_LEFTOVERS_DIFFER__";`
  - `pub struct Leftovers { pub ours: Vec<String>, pub theirs: Vec<String>, pub more_ours: u64, pub more_theirs: u64 }`
  - `pub fn verify_replayed_script(cwd: &str, claude_id: &str, want_head: &str) -> String`
  - `pub fn recover_script(cwd: &str, claude_id: &str) -> String`
  - `pub fn parse_leftovers(stderr: &str) -> Leftovers`
  - `pub fn parse_recover(stdout: &str) -> Result<u64, IpcError>`

- [ ] **Step 1: Write the failing tests**

Add to `carry.rs`'s test module. `dirty_source` already produces every shape
that matters — a staged file with a space, an untracked file with a quote, a
deletion, a mode change, a symlink, and an **ignored** `.env` that must never
count as the target's own work.

```rust
    /// A target clone at the source HEAD, with the transfer refs fetched, and
    /// the snapshot already replayed into it — i.e. exactly the state a move
    /// leaves behind when it fails after the replay.
    fn replayed_target(root: &Path, src: &Path, home: &Path) -> std::path::PathBuf {
        let head = git(src, &["rev-parse", "HEAD"]).trim().to_string();
        let out = bash(&snapshot_script(src.to_str().unwrap(), ID, &[], 10_000_000), home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let tgt = root.join("target");
        git(root, &["clone", "-q", src.to_str().unwrap(), tgt.to_str().unwrap()]);
        git(&tgt, &["checkout", "-q", &head]);
        git(&tgt, &["fetch", "-q", src.to_str().unwrap(), "+refs/fleet/transfer/*:refs/fleet/transfer/*"]);
        let out = bash(&apply_script(tgt.to_str().unwrap(), ID, &head), home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        tgt
    }

    #[test]
    fn verify_adopts_a_target_that_already_holds_exactly_the_snapshot() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let tgt = replayed_target(tmp.path(), &src, &home);

        let out = bash(&verify_replayed_script(tgt.to_str().unwrap(), ID, &head), &home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        // The payload is the same porcelain `apply_script` prints, so the
        // move's own verification can run over it unchanged.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let porcelain = parse_apply(&stdout).unwrap();
        assert!(porcelain.contains("staged new.txt"), "{porcelain}");
        assert!(!porcelain.contains(".env"), "ignored files never appear: {porcelain}");
    }

    #[test]
    fn verify_calls_a_changed_snapshot_path_ours_and_a_new_one_theirs() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let tgt = replayed_target(tmp.path(), &src, &home);

        // A path the snapshot writes, with different content: OURS.
        std::fs::write(tgt.join("mod.txt"), "v3-stale\n").unwrap();
        let out = bash(&verify_replayed_script(tgt.to_str().unwrap(), ID, &head), &home);
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(LEFTOVERS_DIFFER), "{err}");
        let l = parse_leftovers(&err);
        assert_eq!(l.ours, vec!["mod.txt".to_string()], "{l:?}");
        assert!(l.theirs.is_empty(), "{l:?}");

        // A path the snapshot does not hold at all: THEIRS.
        std::fs::write(tgt.join("mod.txt"), "v2\n").unwrap();
        std::fs::write(tgt.join("their-own.txt"), "mine\n").unwrap();
        let out = bash(&verify_replayed_script(tgt.to_str().unwrap(), ID, &head), &home);
        assert!(!out.status.success());
        let l = parse_leftovers(&String::from_utf8_lossy(&out.stderr));
        assert_eq!(l.theirs, vec!["their-own.txt".to_string()], "{l:?}");
        assert!(l.ours.is_empty(), "{l:?}");
    }

    #[test]
    fn verify_ignores_ignored_files_and_refuses_another_head() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let tgt = replayed_target(tmp.path(), &src, &home);

        // An ignored file the snapshot never carried is not the target's work.
        std::fs::write(tgt.join(".env"), "SECRET=different\n").unwrap();
        let out = bash(&verify_replayed_script(tgt.to_str().unwrap(), ID, &head), &home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        // A head mismatch is never an adopt.
        let out = bash(
            &verify_replayed_script(tgt.to_str().unwrap(), ID, &"0".repeat(40)),
            &home,
        );
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(HEAD_MISMATCH), "{err}");
        assert!(!err.contains(LEFTOVERS_DIFFER), "{err}");
    }

    #[test]
    fn recover_removes_only_what_the_snapshot_added() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let tgt = replayed_target(tmp.path(), &src, &home);
        // The target's own ignored file and its own untracked file survive.
        std::fs::write(tgt.join(".env"), "SECRET=theirs\n").unwrap();
        std::fs::write(tgt.join("their-own.txt"), "mine\n").unwrap();

        let out = bash(&recover_script(tgt.to_str().unwrap(), ID), &home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let removed = parse_recover(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(removed >= 2, "the snapshot's added files were removed: {removed}");
        assert!(!tgt.join("staged new.txt").exists(), "a snapshot addition is gone");
        assert!(!tgt.join("sub").exists(), "its new directory is gone too");
        assert!(tgt.join(".env").exists(), "an ignored file is never touched");
        assert!(tgt.join("their-own.txt").exists(), "nor is the target's own file");
        assert_eq!(std::fs::read_to_string(tgt.join("mod.txt")).unwrap(), "v1\n");
        assert!(tgt.join("del.txt").exists(), "a deletion was rolled back");
        // And the worktree is clean again but for what was never ours.
        let porcelain = git(&tgt, &["-c", "core.quotePath=true", "status", "--porcelain"]);
        assert!(porcelain.contains("their-own.txt"), "{porcelain}");
        assert!(!porcelain.contains("mod.txt"), "{porcelain}");
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

```bash
cargo test -p fleet-core
```

Expected: FAIL to **compile** — `cannot find function verify_replayed_script`,
`recover_script`, `parse_leftovers`, `parse_recover`, `LEFTOVERS_DIFFER`,
`Leftovers`. A compile failure is the RED step here; record the exact error text
in the ledger. Do not proceed until you have seen it.

- [ ] **Step 3: Add the sentinel, the type and the shared rollback**

Next to `TARGET_DIRTY` (around `carry.rs:236`):

```rust
/// The target worktree is dirty and what it holds is NOT the snapshot: the
/// stderr records that follow say which paths are the snapshot's (`ours`) and
/// which the target's own (`theirs`).
pub const LEFTOVERS_DIFFER: &str = "__CF_LEFTOVERS_DIFFER__";
```

Near `Leftovers`' only consumers, add the type:

```rust
/// How a dirty target's contents differ from the snapshot about to be
/// replayed. `ours` are paths the snapshot writes (so a cleanup could replace
/// them); `theirs` are paths it does not hold at all (so nothing may touch
/// them). The lists are capped; `more_*` counts what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Leftovers {
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub more_ours: u64,
    pub more_theirs: u64,
}

/// Longest path list either side of [`Leftovers`] carries.
const LEFTOVER_CAP: usize = 50;
```

Lift the rollback out of `apply_script` so both scripts share one text. Add:

```rust
/// The rollback shared by [`apply_script`] and [`recover_script`]: restore
/// the worktree to a clean `HEAD`, then remove EXACTLY the paths the snapshot
/// tree adds relative to `HEAD` — never `git clean`, which would also take
/// the target's own untracked files and empty directories. Sets `n` to the
/// number of files removed. Requires `$id` to be set and guarded.
fn recover_body() -> String {
    format!(
        r#"n=0
recover() {{
  git read-tree -u --reset HEAD >/dev/null 2>&1
  while IFS= read -r -d '' p; do
    rm -f -- "$p" && n=$((n+1))
    d=$(dirname -- "$p")
    [ "$d" = . ] || rmdir -p -- "$d" 2>/dev/null
  done < <(git diff-tree -r -z --name-only --diff-filter=A HEAD "refs/fleet/transfer/$id/wt" 2>/dev/null)
}}"#
    )
}
```

Then replace `apply_script`'s inline `recover() {{ … }}` block with
`{recover}` / `recover = recover_body(),`. Its two call sites
(`{{ recover; fail read-tree-worktree; }}` and `read-tree-index`) stay exactly
as they are. The `< <(…)` process substitution is safe: `ssh.rs` runs every
carry script through `bash -lc`, which is also why `PIPESTATUS` is already used
in `ignored_list_script`.

- [ ] **Step 4: Add `recover_script`**

```rust
/// Undo what an unfinished earlier attempt replayed into `cwd`, and nothing
/// else: [`recover_body`], then [`OUT_MARKER`] and the number of files
/// removed. Parse with [`parse_recover`]. Never touches a git-ignored file,
/// a path the snapshot does not add, or the target's own untracked files.
pub fn recover_script(cwd: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:recover
set +e
cwd={cwd}
id={id}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
{id_guard}
cd -- "$cwd" 2>/dev/null || fail cd
git rev-parse --verify -q "refs/fleet/transfer/$id/wt" >/dev/null 2>&1 || fail no-snapshot
{recover}
recover
printf '\n{OUT_MARKER}\n'
printf '%s\n' "$n"
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        id_guard = id_guard(),
        recover = recover_body(),
    )
}

/// The count [`recover_script`] printed.
pub fn parse_recover(stdout: &str) -> Result<u64, IpcError> {
    payload_str(stdout)
        .map(str::trim)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| parse_err("recover", stdout))
}
```

- [ ] **Step 5: Add `verify_replayed_script` and `parse_leftovers`**

```rust
/// Is this dirty target worktree already EXACTLY the snapshot we were about
/// to replay? Run only after [`apply_script`] refused with [`TARGET_DIRTY`].
///
/// Three questions, all answered by git itself over a throwaway
/// `GIT_INDEX_FILE` so the target's real index is never written:
/// the worktree's content against `wt` (paths the snapshot holds), the paths
/// it does NOT hold (`ls-files --others`, ignored files excluded — those are
/// the ignored-carry step's business), and the real index against `ix`.
/// `update-index --refresh` is what forces content hashing: a
/// `read-tree`-seeded index has no stat data, so every file is re-read rather
/// than trusted.
///
/// All three empty ⇒ prints [`OUT_MARKER`] then [`STATUS_PORCELAIN`], exactly
/// as [`apply_script`] does on success, so the move's own verification runs
/// over the adopted state unchanged. Otherwise exits 11 with
/// [`LEFTOVERS_DIFFER`] first, then `<tag>\t<path>\0` records — parse with
/// [`parse_leftovers`]. A `want_head` mismatch is [`HEAD_MISMATCH`] and never
/// an adopt.
pub fn verify_replayed_script(cwd: &str, claude_id: &str, want_head: &str) -> String {
    format!(
        r#"# cf-carry:verify
set +e
cwd={cwd}
id={id}
want={want}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
{id_guard}
{home_guard}
cd -- "$cwd" 2>/dev/null || fail cd
ref="refs/fleet/transfer/$id"
git rev-parse --verify -q "$ref/wt" >/dev/null 2>&1 || fail no-snapshot
h=$(git rev-parse HEAD 2>/dev/null)
if [ "$h" != "$want" ]; then printf '{HEAD_MISMATCH} %s\n' "$h" >&2; exit 10; fi
dir="$HOME/.cache/claude-fleet/transfer/$id"
umask 077
mkdir -p -- "$dir" || fail mkdir
tmp="$dir/verify.ix"
ours="$dir/verify.ours"
theirs="$dir/verify.theirs"
rm -f -- "$tmp" "$ours" "$theirs"
clean() {{ rm -f -- "$tmp" "$ours" "$theirs"; }}
GIT_INDEX_FILE="$tmp" git read-tree "$ref/wt^{{tree}}" >/dev/null 2>&1 || {{ clean; fail read-tree; }}
GIT_INDEX_FILE="$tmp" git update-index -q --refresh >/dev/null 2>&1
GIT_INDEX_FILE="$tmp" git diff-index -z --name-only "$ref/wt^{{tree}}" -- > "$ours" 2>/dev/null || {{ clean; fail diff-worktree; }}
GIT_INDEX_FILE="$tmp" git ls-files -z --others --exclude-standard > "$theirs" 2>/dev/null || {{ clean; fail ls-others; }}
git diff-index -z --name-only --cached "$ref/ix^{{tree}}" -- >> "$ours" 2>/dev/null || {{ clean; fail diff-index; }}
rm -f -- "$tmp"
if [ -s "$ours" ] || [ -s "$theirs" ]; then
  printf '{LEFTOVERS_DIFFER}\n' >&2
  for t in ours theirs; do
    n=0
    while IFS= read -r -d '' p; do
      n=$((n+1))
      [ "$n" -le {cap} ] && printf '%s\t%s\0' "$t" "$p" >&2
    done < "$dir/verify.$t"
    if [ "$n" -gt {cap} ]; then printf 'more\t%s\t%s\0' "$t" "$((n-{cap}))" >&2; fi
  done
  clean
  exit 11
fi
clean
printf '\n{OUT_MARKER}\n'
git {status}
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        want = quote(want_head),
        id_guard = id_guard(),
        home_guard = home_guard(),
        status = STATUS_PORCELAIN,
        cap = LEFTOVER_CAP,
    )
}

/// The `<tag>\t<path>\0` records [`verify_replayed_script`] wrote to stderr.
/// Tolerant by design: unknown tags, a truncated stream and duplicate paths
/// (a path can differ in both the worktree and the index) are all fine — the
/// lists only ever drive a message and a cleanup confirmation, never a
/// decision about whether to overwrite. Both lists come out sorted and
/// deduplicated.
pub fn parse_leftovers(stderr: &str) -> Leftovers {
    let mut out = Leftovers::default();
    for rec in stderr.split('\0') {
        let mut it = rec.splitn(3, '\t');
        match (it.next(), it.next(), it.next()) {
            (Some(t), Some(p), None) if t.ends_with("ours") => out.ours.push(p.to_string()),
            (Some(t), Some(p), None) if t.ends_with("theirs") => out.theirs.push(p.to_string()),
            (Some(t), Some(side), Some(n)) if t.ends_with("more") => {
                let n = n.trim().parse().unwrap_or(0);
                if side == "ours" {
                    out.more_ours = n;
                } else if side == "theirs" {
                    out.more_theirs = n;
                }
            }
            _ => {}
        }
    }
    for v in [&mut out.ours, &mut out.theirs] {
        v.sort();
        v.dedup();
    }
    out
}
```

`t.ends_with("ours")` rather than `t == "ours"`: the first record shares a line
with the sentinel's newline only if a login profile wrote a partial line, and
`ends_with` costs nothing to be safe about it.

- [ ] **Step 6: Run the whole crate suite, unpiped**

```bash
cargo test -p fleet-core
```

Expected: PASS, including the four new tests and every pre-existing
`apply_script` test (the rollback text moved; its behaviour must not have).

- [ ] **Step 7: Clippy and fmt**

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

- [ ] **Step 8: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/carry.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): a content-exact verifier for a dirty target, and a scoped rollback"
```

---
### Task 2: Classify a dirty target, and adopt it when it is already ours

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — the three places
  `carry::TARGET_DIRTY` becomes an error (`mod.rs:2091`, `mod.rs:2161`), and
  `target_dirty` itself (`mod.rs:1595`).
- Test: the same file's `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes from Task 1: `carry::verify_replayed_script`, `carry::parse_leftovers`,
  `carry::LEFTOVERS_DIFFER`, `carry::Leftovers`, `carry::parse_apply`,
  `carry::HEAD_MISMATCH`.
- Produces, for Task 3:
  - `enum Adopted { Yes(String), Ours(carry::Leftovers), Theirs(carry::Leftovers), Unknown }`
  - `async fn classify_target(ssh: &dyn SshExec, target: &str, cwd: &str, claude_id: &str, want_head: &str) -> Adopted`
  - `fn target_dirty(cwd: &str, target: &str, verdict: &Adopted) -> IpcError`

- [ ] **Step 1: Write the failing tests**

```rust
    /// The state a move leaves when it fails after the replay: the target
    /// worktree already holds exactly what this move is carrying.
    fn target_already_replayed(f: &Fixture, porcelain: &str) {
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::ok(&out(&format!("{porcelain}\n"))),
            );
    }

    #[tokio::test]
    async fn a_target_that_already_holds_this_work_is_adopted_and_the_move_completes() {
        let (f, bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        target_already_replayed(&f, " M src/lib.rs\n?? notes.txt");
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("the move completes");
        assert!(
            rep.warnings.iter().any(|w| w.contains("already held exactly this work")),
            "{:?}",
            rep.warnings
        );
        let seen = progress_of(&f, &bus);
        assert!(seen.contains(&"replay:done".to_string()), "{seen:?}");
        assert!(!seen.iter().any(|e| e == "replay:failed"), "{seen:?}");
    }

    #[tokio::test]
    async fn stale_leftovers_are_ours_and_name_their_paths() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(
                    11,
                    &format!("{}\nours\tsrc/lib.rs\0", carry::LEFTOVERS_DIFFER),
                ),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TARGET_DIRTY);
        let d = err.details.clone().unwrap();
        assert_eq!(d["leftovers"], "ours");
        assert_eq!(d["ours"][0], "src/lib.rs");
        assert!(err.message.contains("src/lib.rs"), "{}", err.message);
    }

    #[tokio::test]
    async fn the_targets_own_work_is_theirs_and_is_refused_outright() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(
                    11,
                    &format!(
                        "{}\nours\tsrc/lib.rs\0theirs\ttheir_notes.md\0more\ttheirs\t7\0",
                        carry::LEFTOVERS_DIFFER
                    ),
                ),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TARGET_DIRTY);
        let d = err.details.clone().unwrap();
        assert_eq!(d["leftovers"], "theirs");
        assert_eq!(d["theirs"][0], "their_notes.md");
        assert!(err.message.contains("their_notes.md"), "{}", err.message);
        assert!(err.message.contains("7 more"), "{}", err.message);
    }

    #[tokio::test]
    async fn a_verify_that_cannot_answer_keeps_todays_refusal() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(5, &format!("{} no-snapshot", carry::FAILED)),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TARGET_DIRTY);
        assert_eq!(err.details.clone().unwrap()["leftovers"], "unknown");
        assert!(err.message.contains("inspect them there"), "{}", err.message);
    }

    #[tokio::test]
    async fn a_head_mismatch_from_verify_is_never_an_adopt() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(10, &format!("{} deadbeef", carry::HEAD_MISMATCH)),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TARGET_DIRTY);
        assert_eq!(err.details.clone().unwrap()["leftovers"], "unknown");
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

```bash
cargo test -p fleet-core
```

Expected: the five new tests FAIL. `a_target_that_already_holds_this_work…`
fails with `E_MOVE_TARGET_DIRTY` instead of a report (no verify call is made at
all); the `details` assertions fail on `None`. Record the text.

- [ ] **Step 3: Add the classifier**

Above `target_dirty` in `mod.rs`:

```rust
/// What a dirty target worktree turned out to be.
#[derive(Debug)]
enum Adopted {
    /// Already exactly the snapshot: the porcelain the verify script printed.
    Yes(String),
    /// Differs, but only in paths the snapshot itself writes: a cleanup could
    /// replace them (`clean_target`).
    Ours(carry::Leftovers),
    /// Holds at least one path this move would never write: nothing may touch
    /// it, flag or not.
    Theirs(carry::Leftovers),
    /// The check could not answer. Never widen what the move will overwrite
    /// on the strength of a broken check.
    Unknown,
}

/// Ask the target whether its dirty worktree is already the snapshot
/// (`carry::verify_replayed_script`). Called only after a `TARGET_DIRTY`
/// refusal, so its own failure simply means "still dirty, reason unknown".
async fn classify_target(
    ssh: &dyn SshExec,
    target: &str,
    cwd: &str,
    claude_id: &str,
    want_head: &str,
) -> Adopted {
    let Ok(out) = sh(
        ssh,
        target,
        &carry::verify_replayed_script(cwd, claude_id, want_head),
        GIT_TIMEOUT,
    )
    .await
    else {
        return Adopted::Unknown;
    };
    if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        return match carry::parse_apply(&stdout) {
            Ok(p) => Adopted::Yes(p.to_string()),
            Err(_) => Adopted::Unknown,
        };
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.contains(carry::LEFTOVERS_DIFFER) {
        return Adopted::Unknown;
    }
    let l = carry::parse_leftovers(&err);
    if l.theirs.is_empty() && l.more_theirs == 0 {
        Adopted::Ours(l)
    } else {
        Adopted::Theirs(l)
    }
}

/// `a, b and 7 more` — the path lists in a refusal's message.
fn name_paths(paths: &[String], more: u64) -> String {
    let mut s = paths.join(", ");
    if more > 0 {
        if !s.is_empty() {
            s.push_str(" and ");
        }
        s.push_str(&format!("{more} more"));
    }
    s
}
```

- [ ] **Step 4: Rewrite `target_dirty` to carry the verdict**

Replace the whole of `target_dirty` (`mod.rs:1595`):

```rust
/// The target worktree is dirty. Three refusals, because they ask three
/// different things of the user: replace the stale leftovers of an attempt
/// that did not finish (`Ours` — `clean_target` can do it), deal with the
/// target's own work (`Theirs` — only they can), or look for themselves
/// (`Unknown`, which is also what this said before slice 3d).
fn target_dirty(cwd: &str, target: &str, verdict: &Adopted) -> IpcError {
    let (kind, message) = match verdict {
        Adopted::Ours(l) => (
            "ours",
            format!(
                "move_session: {cwd} on {target} still holds work an earlier transfer attempt left behind, and it differs from what is being carried now ({}); transfer again with clean_target to replace it, or inspect it there first (the source session was not touched)",
                name_paths(&l.ours, l.more_ours)
            ),
        ),
        Adopted::Theirs(l) => (
            "theirs",
            format!(
                "move_session: the target worktree {cwd} on {target} has uncommitted work of its own ({}); the move never overwrites it and no cleanup can be safe here, so commit or discard it there before transferring (the source session was not touched)",
                name_paths(&l.theirs, l.more_theirs)
            ),
        ),
        // Unchanged from before 3d, including for `Yes` — which never reaches
        // here, but must not silently become a permissive message if it ever
        // does.
        _ => (
            "unknown",
            format!(
                "move_session: the target worktree {cwd} on {target} has uncommitted changes — its own, work carried by an earlier move attempt that did not finish, or the copy left behind when this session was moved away from this host; the move never overwrites them, so inspect them there and commit or discard them before retrying (the source session was not touched)"
            ),
        ),
    };
    let (ours, theirs) = match verdict {
        Adopted::Ours(l) | Adopted::Theirs(l) => (l.ours.clone(), l.theirs.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    IpcError::new(codes::E_MOVE_TARGET_DIRTY, message).with_details(serde_json::json!({
        "leftovers": kind,
        "ours": ours,
        "theirs": theirs,
    }))
}
```

- [ ] **Step 5: Wire it into the two `TARGET_DIRTY` sites**

At `mod.rs:2161` (the `# cf-carry:apply` refusal) replace
`return Err(target_dirty(&cwd, &target));` with the block below. **Corrected
during execution:** the `# cf-move:prep` site at `mod.rs:2091` keeps a plain
`return Err(target_dirty(&cwd, &target, &Adopted::Unknown));` and is NOT
classified — prep emits `TARGET_DIRTY` only when the target HEAD is a strict
ancestor of the source HEAD, and the verifier requires `HEAD == want_head`, so an
adopt there is unreachable and the call only added an `E_PARSE` path in a
fast-forward race. Put the reason in a comment at that site.

```rust
        if err.contains(carry::TARGET_DIRTY) {
            match classify_target(ssh, &target, &cwd, &id, &state.head).await {
                Adopted::Yes(porcelain) => adopted = Some(porcelain),
                other => return Err(target_dirty(&cwd, &target, &other)),
            }
        }
```

Declare `let mut adopted: Option<String> = None;` before the prep call, and in
step 3c use it instead of running the apply at all:

```rust
        let porcelain_owned = match adopted.take() {
            Some(p) => {
                progress.done(Some("already in place".to_string()));
                warnings.push(format!(
                    "{cwd} on {target} already held exactly this work; nothing was replayed"
                ));
                p
            }
            // Verbatim today's code, moved into this arm and nothing else:
            // the `sh(... apply_script ...)` call, the `!out.status.success()`
            // branch (whose `TARGET_DIRTY` case is now unreachable from here,
            // because the prep-site classification already ran — leave it in
            // place returning `target_dirty(&cwd, &target, &Adopted::Unknown)`),
            // and the `carry::parse_apply(&stdout)?.to_string()`.
            None => { /* the existing block, unchanged, yielding the porcelain */ }
        };
```

and let the existing `want != got` verification run over `porcelain_owned`
exactly as it does today. **Do not** skip that check for an adopted target: the
adopt decides whether to *write*, never whether the result is correct.

Where the prep-site classification adopts, the apply must not run afterwards —
the `adopted` value is what tells step 3c so.

- [ ] **Step 6: Run the whole crate suite, unpiped**

```bash
cargo test -p fleet-core
```

Expected: PASS. `a_target_dirty_refusal_ends_the_stream_at_replay_failed`
(`mod.rs:3117`) still passes, and it is worth understanding why before you touch
it: `FakeSsh`'s reply for a script no rule matches is **exit 0 with empty
output** (`ssh_fake.rs`, `set_default`'s default). So the classifier's verify
call "succeeds" with nothing on stdout, `carry::parse_apply` fails, and
`classify_target` returns `Adopted::Unknown` — today's message, which is what
that test asserts. That is the correct fallback, and it is the reason
`Adopted::Yes` requires a *parsed* payload rather than a zero exit status. If
you are tempted to treat an empty payload as "clean", stop: that would make
every unmatched fake and every truncated real reply an adopt.

- [ ] **Step 7: Clippy, fmt, commit**

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): adopt a target that already holds exactly the work being carried"
```

---

### Task 3: `clean_target`

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `MoveSessionArgs`
  (`mod.rs:110`) and the `Adopted::Ours` branch from Task 2.
- Modify: `crates/fleet-core/src/ssh_fake.rs` — `on_host_once` (see Step 1).
- Test: both files' own test modules.

**Interfaces:**
- Consumes from Task 1: `carry::recover_script`, `carry::parse_recover`. From
  Task 2: `Adopted`, `classify_target`, `target_dirty`.
- Produces, for Task 7 and Task 8: `MoveSessionArgs.clean_target: bool`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn clean_target_replaces_stale_leftovers_and_then_replays() {
        let (f, bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        let porcelain = " M src/lib.rs\n?? notes.txt";
        // Dirty first, then — after the recover — the apply succeeds.
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(11, &format!("{}\nours\tsrc/lib.rs\0", carry::LEFTOVERS_DIFFER)),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:recover"),
                Reply::ok(&out("3\n")),
            )
            // After the recover the worktree is clean: the retried apply works.
            .on_host_once(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::ok(&out(&format!("{porcelain}\n"))),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run_with(&f, &hooks, |a| a.clean_target = true)
            .await
            .expect("the move completes");
        assert!(
            rep.warnings.iter().any(|w| w.contains("replaced 3")),
            "{:?}",
            rep.warnings
        );
        let seen = progress_of(&f, &bus);
        assert!(seen.contains(&"replay:warned".to_string()), "{seen:?}");
        assert_eq!(scripts_with(&f, "beta", "# cf-carry:recover").len(), 1);
    }

    #[tokio::test]
    async fn clean_target_never_touches_the_targets_own_work() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(
                    11,
                    &format!("{}\ntheirs\ttheir_notes.md\0", carry::LEFTOVERS_DIFFER),
                ),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run_with(&f, &hooks, |a| a.clean_target = true)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TARGET_DIRTY);
        assert_eq!(err.details.clone().unwrap()["leftovers"], "theirs");
        assert!(
            scripts_with(&f, "beta", "# cf-carry:recover").is_empty(),
            "the recover script must never run for the target's own work"
        );
    }

    #[tokio::test]
    async fn without_the_flag_stale_leftovers_are_only_refused() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::fail(9, carry::TARGET_DIRTY),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:verify"),
                Reply::fail(11, &format!("{}\nours\tsrc/lib.rs\0", carry::LEFTOVERS_DIFFER)),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.details.clone().unwrap()["leftovers"], "ours");
        assert!(scripts_with(&f, "beta", "# cf-carry:recover").is_empty());
    }
```

Two test helpers these tests need do not exist yet. Add both in this task.

`run_with`, next to `run` (`mod.rs:3039`), reusing its existing `args(f, keep)`
and `fast()` helpers:

```rust
    /// `run`, with the args mutated before the call — for the flags `run`
    /// does not take.
    async fn run_with(
        f: &Fixture,
        hooks: &FakeHooks,
        edit: impl FnOnce(&mut MoveSessionArgs),
    ) -> Result<MoveReport, IpcError> {
        let mut a = args(f, false);
        edit(&mut a);
        move_session_with(a, &f.store, &f.fake, hooks, fast()).await
    }
```

`on_host_once` in `crates/fleet-core/src/ssh_fake.rs`: the fake's rules are
permanent and "later rules win", so there is no way today to say *this script
fails the first time and succeeds the next* — which is exactly the shape of a
cleanup followed by a retried replay. `Rule` gains `once: bool` plus a
`used: Cell<bool>`-equivalent inside the already-`Mutex`-guarded state (a plain
`bool` field, flipped when the rule is chosen), the matcher skips a spent
once-rule, and:

```rust
    /// Answer `matcher` on `host` with `reply` for ONE matching call, then fall
    /// through to the other rules. Later rules still win, so register the
    /// once-rule AFTER the standing one it overrides.
    pub fn on_host_once(&self, host: &str, matcher: Match, reply: Reply) -> &Self {
        self.lock().rules.push(Rule {
            host: Some(host.to_string()),
            matcher,
            reply,
            once: true,
            spent: false,
        });
        self
    }
```

Give it its own micro-cycle: write a test in `ssh_fake.rs`'s own test module
asserting that a once-rule answers the first matching call and the standing rule
answers the second, watch it fail, then implement. `ssh_fake.rs` is not touched
by any other task in this plan, so this is not a writer conflict — but it IS a
shared test utility: do not change the meaning of any existing method.

- [ ] **Step 2: Run the tests and watch them fail**

```bash
cargo test -p fleet-core
```

Expected: FAIL to compile — `MoveSessionArgs` has no field `clean_target`.

- [ ] **Step 3: Add the argument**

In `MoveSessionArgs` (`mod.rs:110`), after `strict`:

```rust
    /// Replace what an unfinished earlier transfer left in the target
    /// worktree. Refused when the target also holds work of its own, so it
    /// can never overwrite anything this move did not put there. Default
    /// false.
    #[serde(default)]
    pub clean_target: bool,
```

Fix every construction site the compiler names (the MCP tool, the Tauri
command, the tests) with `clean_target: false`.

- [ ] **Step 4: Run the recover before the retried apply**

In the `Adopted::Ours` branch, when `args.clean_target` is set:

```rust
                Adopted::Ours(l) if args.clean_target => {
                    let out = sh(ssh, &target, &carry::recover_script(&cwd, &id), GIT_TIMEOUT)
                        .await
                        .map_err(|e| carry_transport("recover", e))?;
                    if !out.status.success() {
                        return Err(carry_err(
                            "recover",
                            &format!("replacing an unfinished attempt's work in {cwd} on {target}"),
                            &stderr_of(&out),
                        ));
                    }
                    let removed = carry::parse_recover(&String::from_utf8_lossy(&out.stdout))
                        .unwrap_or(l.ours.len() as u64);
                    cleaned = Some(removed);
                }
```

with `let mut cleaned: Option<u64> = None;` alongside `adopted`, and in step 3c,
after a successful apply:

```rust
        if let Some(removed) = cleaned {
            warnings.push(format!(
                "replaced {removed} file(s) an unfinished earlier transfer had left in {cwd} on {target}"
            ));
        }
```

The `Replay` step closes as `Warned` when `cleaned.is_some()` — use
`progress.end_soft(cleaned.is_some(), …)` instead of `progress.done(…)` at that
site. Because the recover runs inside the same call, the target is never left
cleaned-but-not-moved.

- [ ] **Step 5: Run the whole crate suite, unpiped**

```bash
cargo test -p fleet-core
```

Expected: PASS.

- [ ] **Step 6: Clippy, fmt, commit**

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/mod.rs crates/fleet-core/src/ssh_fake.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): clean_target replaces an unfinished attempt's leftovers, never the target's own work"
```

---
### Task 4: A partial move records what a later recovery needs

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `partial`
  (`mod.rs:1637`), `record_partial` (`mod.rs:1700`), and the six `partial(…)`
  call sites (`mod.rs:2330`, `2334`, `2412`, `2438`, `2447`, `2480`).
- Test: the same file's test module.

**Why:** `record_partial` is reached through `move_session_with`'s
`inspect_err` (`mod.rs:1736`), so it sees nothing but the `IpcError`. Anything
`resolve_move` needs tomorrow has to be in that error's `details` today.

**Interfaces:**
- Produces, for Task 6:
  - `pub(super) struct PartialCtx { from_host, to_tmux_name, claude_session_id, branch, source_transcript: Option<Located>, to_turn_seq: Option<i64>, to_last_turn_at: Option<i64> }`
  - the `session_move_partial` detail keys `from_host`, `to_tmux_name`,
    `claude_session_id`, `branch`, `source_transcript_size`,
    `source_transcript_mtime`, `source_transcript_path`, `to_turn_seq`,
    `to_last_turn_at`.

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn a_partial_records_everything_a_later_recovery_needs() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        // The kill fails: the target is up, both rows are alive — a partial.
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id).failing_kill();
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_PARTIAL);

        let ev = events(&f, f.source_id);
        let (_, detail) = ev
            .iter()
            .find(|(k, _)| k == EVENT_MOVE_PARTIAL)
            .expect("session_move_partial on the source");
        let d: serde_json::Value = serde_json::from_str(detail.as_deref().unwrap()).unwrap();
        assert_eq!(d["from_host"], "alpha");
        assert_eq!(d["to_host"], "beta");
        assert_eq!(d["claude_session_id"], CLAUDE_ID);
        assert_eq!(d["branch"], "feat");
        assert!(d["to_tmux_name"].is_string(), "{d}");
        assert!(d["source_transcript_size"].is_u64(), "{d}");
        assert!(d["source_transcript_mtime"].is_i64(), "{d}");
        assert!(d["source_transcript_path"].is_string(), "{d}");
        assert!(d["to_turn_seq"].is_i64() || d["to_turn_seq"].is_null(), "{d}");
        assert!(d["step"].is_string(), "{d}");
    }

    #[tokio::test]
    async fn a_partial_before_the_copy_records_nulls_rather_than_guesses() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        // Fails at the reconcile — the earliest partial, before any re-locate.
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id).failing_refresh();
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_PARTIAL);
        let ev = events(&f, f.source_id);
        let (_, detail) = ev.iter().find(|(k, _)| k == EVENT_MOVE_PARTIAL).unwrap();
        let d: serde_json::Value = serde_json::from_str(detail.as_deref().unwrap()).unwrap();
        // The transcript facts are known by then (the copy happens before the
        // start), but a field the move never reached must be null, never 0.
        assert!(!d["source_transcript_size"].is_null() || d["source_transcript_size"].is_null());
        assert_eq!(d["from_host"], "alpha");
    }
```

`failing_kill()` / `failing_refresh()` are whatever `FakeHooks` already offers
for those two failures — the existing test
`a_partial_move_records_session_move_partial_not_session_moved`
(`mod.rs:3494`) iterates `["kill_fails", "source_changed", "never_confirmed"]`,
so reuse that mechanism instead of adding new builders, and name the cases
exactly as it does. `events(&f, id)` and `CLAUDE_ID` already exist in the test
module; if the fixture's source id field is not `f.source_id`, use whatever
that test uses.

- [ ] **Step 2: Run the tests and watch them fail**

```bash
cargo test -p fleet-core
```

Expected: FAIL — `d["from_host"]` is `Null` (the detail has only `step`,
`to_host`, `from_session_id`, `to_session_id`, `cause_code`).

- [ ] **Step 3: Add `PartialCtx`**

Above `partial` in `mod.rs`:

```rust
/// What a partial move must leave behind so that `resolve_move` can finish or
/// undo it later — after the window is closed and the run store is gone.
/// Built once, as soon as the target row is confirmed, and passed to every
/// `partial(…)` call site.
pub(super) struct PartialCtx {
    pub from_host: String,
    pub to_tmux_name: String,
    pub claude_session_id: String,
    pub branch: String,
    /// The source transcript as the copy was taken. `None` before the copy.
    pub source_transcript: Option<Located>,
    /// The target row's counters as the move last saw them.
    pub to_turn_seq: Option<i64>,
    pub to_last_turn_at: Option<i64>,
}
```

- [ ] **Step 4: Thread it through `partial` and `record_partial`**

`partial` takes `ctx: &PartialCtx` in place of `target: &str, name: &str` (both
are in the ctx as `from_host`'s counterpart `to_host` — keep `target` as a
parameter only if the call site's host can differ from the ctx's, which it
cannot) and adds to its `details`:

```rust
        "from_host": ctx.from_host,
        "to_tmux_name": ctx.to_tmux_name,
        "claude_session_id": ctx.claude_session_id,
        "branch": ctx.branch,
        "source_transcript_size": ctx.source_transcript.as_ref().map(|l| l.size),
        "source_transcript_mtime": ctx.source_transcript.as_ref().map(|l| l.mtime),
        "to_turn_seq": ctx.to_turn_seq,
        "to_last_turn_at": ctx.to_last_turn_at,
```

plus `"source_transcript_path": ctx.source_transcript.as_ref().map(|l| &l.path)`.
**Pre-flight ruling R1:** the path is recorded as well as the size and mtime,
because Task 6 rebuilds a whole `Located` from this detail and a recovery that
guessed the path would compare the wrong file.

`record_partial` copies exactly those keys through into the event detail
alongside the ones it already writes. `serde_json` writes an `Option::None` as
`null`, which is the contract: a fact the move never reached is `null`, and
Task 6 refuses on a `null` it needs rather than guessing.

Update all six `partial(…)` call sites. The ctx is built right after
`target_row` is confirmed and the `tmux_name` is known; for the two sites that
run **before** the re-locate, `source_transcript` is `Some(located)` already
(the copy precedes the start) — set it when `located` comes into scope and leave
it `None` only where it genuinely is not yet known.

- [ ] **Step 5: Run the whole crate suite, unpiped**

```bash
cargo test -p fleet-core
```

Expected: PASS, including `a_partial_move_records_session_move_partial_not_session_moved`
unchanged.

- [ ] **Step 6: Clippy, fmt, commit**

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): a partial move records the facts a later finish or undo needs"
```

---

### Task 5: Extract the final source step into `finalise.rs`

**Files:**
- Create: `crates/fleet-core/src/service/move_session/finalise.rs`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `mod finalise;`,
  step 4 (`mod.rs:2430`–`2560`) calls it; `MoveHooks::kill_source` renamed
  `kill_tmux_session`.
- Test: `finalise.rs`'s own test module for the new entry point; every existing
  step-4 test in `mod.rs` must pass **unchanged**.

**This task changes no behaviour.** It is a pure extraction so that Task 6 can
run the same code. The acceptance criterion is not "the new tests pass" but
"the old ones do, and no test was edited to make that true".

**Interfaces:**
- Consumes: `locate_on`, `Located`, `EVENT_MOVED`, `MoveHooks`, `PartialCtx`,
  `partial`, `sh`, `GIT_TIMEOUT`.
- Produces, for Task 6:

```rust
pub(super) struct FinaliseArgs<'a> {
    pub source_row_id: i64,
    pub source_host: &'a str,
    pub source_tmux_name: &'a str,
    pub target_row_id: i64,
    pub target_host: &'a str,
    pub target_tmux_name: &'a str,
    pub claude_id: &'a str,
    pub branch: &'a str,
    pub stored_transcript: Option<&'a str>,
    /// The transcript as the copy was taken: the kill is refused if the source
    /// has moved past it.
    pub copied: Located,
    pub transcript_bytes: u64,
    pub keep_source: bool,
    /// Extra keys merged into the `session_moved` detail (Task 6 passes
    /// `{"finished_from_partial": true}`).
    pub extra_detail: serde_json::Value,
}

pub(super) struct FinaliseOutcome {
    pub source_killed: bool,
    pub warnings: Vec<String>,
}

pub(super) async fn finalise_source(
    a: FinaliseArgs<'_>,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    carried: &carry::CarryReport,
) -> Result<FinaliseOutcome, IpcError>;
```

- [ ] **Step 1: Move the code, unchanged**

Cut `mod.rs`'s step 4 — from the re-locate through the post-kill look, the usage
snapshot and carry, to the `session_moved` insert on both rows — into
`finalise_source`. Keep every comment: they record why each check exists (the
transcript-changed refusal, the "one last look", the usage cursor catch-up,
"never under keep_source"). The `partial(…)` wrapping stays at the **call site**
in `mod.rs`, so `finalise_source` returns plain errors and the move decides they
are partial; Task 6 will decide they are `E_INVALID_STATE`. This is the only
structural change, and it is what makes the function reusable.

- [ ] **Step 2: Rename the hook**

`MoveHooks::kill_source` → `kill_tmux_session` (same signature, same body).
Undo kills the *target* with it in Task 6, and a hook named `kill_source` doing
that would be a lie. Update `RealHooks` and every `FakeHooks` in the test
module.

- [ ] **Step 3: Run the whole crate suite, unpiped**

```bash
cargo test -p fleet-core
```

Expected: PASS with **no test file edits** beyond the mechanical
`kill_source` → `kill_tmux_session` rename. In particular these must still pass
untouched: the kill-path tests, `a_partial_move_records_session_move_partial_not_session_moved`
(`mod.rs:3494`), the usage-carry assertions, and the "source wrote before the
kill" warning. If any of them needs a real change, stop and report it as a
finding — it means the extraction changed behaviour.

- [ ] **Step 4: Add one test for the new seam**

```rust
    /// The seam itself: `finalise_source` refuses when the source transcript
    /// has moved past the copy, and does not kill anything in that case.
    #[tokio::test]
    async fn finalise_refuses_a_source_that_wrote_after_the_copy() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        // The re-locate answers a larger transcript than `copied` records.
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:locate"),
            Reply::ok(&out("999999\t1700000000\t/home/a/.claude/projects/p/c.jsonl\n")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_PARTIAL);
        assert!(
            err.message.contains("after it was copied"),
            "{}",
            err.message
        );
        assert!(!hooks.killed(), "nothing is killed when the source moved on");
    }
```

Match the fixture's real `# cf-move:locate` marker and its payload shape
(`parse_locate`) — read `locate_script` before writing the reply, and use
`hooks.killed()` only if `FakeHooks` already records it; otherwise assert on the
absence of a kill in `f.fake.calls_for("alpha")`.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/finalise.rs crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "refactor(move): the final source step becomes finalise_source, shared with recovery"
```

---
### Task 6: `resolve_move` — Finish and Undo

**Files:**
- Create: `crates/fleet-core/src/service/move_session/resolve.rs`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `pub mod resolve;`
  and `pub const EVENT_MOVE_UNDONE: &str = "session_move_undone";` next to
  `EVENT_MOVED` (one line each; this is the whole of `mod.rs`'s change).
- Test: `resolve.rs`'s own test module, reusing `mod.rs`'s fixture helpers via
  `use super::tests::…` if they are reachable; otherwise build the two rows it
  needs directly with the `Store` API.

**Interfaces:**
- Consumes: `finalise::{finalise_source, FinaliseArgs}` (Task 5), `PartialCtx`'s
  event keys (Task 4), `MoveHooks::kill_tmux_session` (Task 5),
  `Store::list_session_events`, `Store::get_session_by_id`,
  `Store::insert_session_event`.
- Produces, for Task 7:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolveMoveAction { Finish, Undo }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMoveArgs {
    /// The TARGET session of the partial move.
    pub session_id: i64,
    pub action: ResolveMoveAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMoveReport {
    pub action: ResolveMoveAction,
    pub source_session_id: i64,
    pub target_session_id: i64,
    pub from_host: String,
    pub to_host: String,
    pub source_killed: bool,
    pub target_killed: bool,
    pub warnings: Vec<String>,
}

pub async fn resolve_move(
    args: ResolveMoveArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ResolveMoveReport, IpcError>;

pub(super) async fn resolve_move_with(
    args: ResolveMoveArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
) -> Result<ResolveMoveReport, IpcError>;
```

Both report types are wire types: `Serialize + Deserialize`, **no**
`#[serde(default)]` on any field.

- [ ] **Step 1: Write the failing tests**

```rust
    // Pre-flight ruling R3: `mod.rs`'s `mod tests` is private and
    // `#[cfg(test)]`, so this module defines its own constants rather than
    // widening that one to share three values.
    const SID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const TRANSCRIPT_LEN: usize = 128;
    const MTIME: i64 = 1_700_000_000;

    /// Two rows and a `session_move_partial` event between them: what the
    /// store looks like after a move stopped with the target running. With
    /// `unresolved = false` a later `session_moved` marks it already resolved.
    /// Uses the same `Store` calls as `mod.rs`'s `fixture_on`.
    fn partial_fixture(unresolved: bool) -> (Mutex<Store>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        for h in ["alpha", "beta"] {
            s.insert_host(h, None).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
            s.set_host_provisioned(h, true).unwrap();
        }
        let pid = s.upsert_project("o", "r", "/local/o/r").unwrap();
        let wid = s
            .upsert_worktree(pid, "feat", "/local/o/r/.claude/worktrees/feat", Some("feat"))
            .unwrap();
        let source = s
            .upsert_session("dev-o-r--feat", "alpha", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        let target = s
            .upsert_session("dev-o-r--feat", "beta", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(source, SID).unwrap();
        s.set_claude_session_id(target, SID).unwrap();
        s.set_claude_status_by_session_id(SID, "idle").unwrap();
        s.set_parent_session_id(target, Some(source)).unwrap();
        let detail = serde_json::json!({
            "step": "killing the source dev-o-r--feat on alpha",
            "to_host": "beta",
            "from_host": "alpha",
            "from_session_id": source,
            "to_session_id": target,
            "cause_code": "E_SSH",
            "to_tmux_name": "dev-o-r--feat",
            "claude_session_id": SID,
            "branch": "feat",
            "source_transcript_size": TRANSCRIPT_LEN,
            "source_transcript_mtime": MTIME,
            "source_transcript_path": "/home/a/.claude/projects/p/c.jsonl",
            "to_turn_seq": 0,
            "to_last_turn_at": serde_json::Value::Null,
        })
        .to_string();
        for id in [source, target] {
            s.insert_session_event(id, EVENT_MOVE_PARTIAL, Some(&detail)).unwrap();
        }
        if !unresolved {
            for id in [source, target] {
                s.insert_session_event(id, EVENT_MOVED, Some(&detail)).unwrap();
            }
        }
        (Mutex::new(s), source, target)
    }

    #[tokio::test]
    async fn finish_kills_the_source_and_records_the_move_as_complete() {
        let (store, source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let rep = resolve_move_with(
            ResolveMoveArgs { session_id: target_id, action: ResolveMoveAction::Finish },
            &store,
            &fake,
            &hooks,
        )
        .await
        .expect("finish");
        assert!(rep.source_killed);
        assert!(!rep.target_killed);
        assert_eq!(rep.source_session_id, source_id);
        for id in [source_id, target_id] {
            let ev = store.lock().unwrap().list_session_events(id, 50).unwrap();
            let e = ev.iter().find(|e| e.kind == EVENT_MOVED).expect("session_moved");
            let d: serde_json::Value = serde_json::from_str(e.detail.as_deref().unwrap()).unwrap();
            assert_eq!(d["finished_from_partial"], true);
        }
    }

    #[tokio::test]
    async fn finish_refuses_a_source_that_took_a_turn_after_the_partial() {
        let (store, _source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        // The source transcript is now bigger than the partial recorded.
        fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:locate"),
            Reply::ok(&format!("{}\n99999\t1700009999\t/p/c.jsonl\n", carry::OUT_MARKER)),
        );
        let hooks = FakeHooks::new(&fake);
        let err = resolve_move_with(
            ResolveMoveArgs { session_id: target_id, action: ResolveMoveAction::Finish },
            &store,
            &fake,
            &hooks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("99999"), "{}", err.message);
        assert!(!hooks.killed_any(), "nothing is killed on a refusal");
    }

    #[tokio::test]
    async fn undo_kills_the_target_and_leaves_its_files_alone() {
        let (store, source_id, target_id) = partial_fixture(true);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let rep = resolve_move_with(
            ResolveMoveArgs { session_id: target_id, action: ResolveMoveAction::Undo },
            &store,
            &fake,
            &hooks,
        )
        .await
        .expect("undo");
        assert!(rep.target_killed);
        assert!(!rep.source_killed);
        for id in [source_id, target_id] {
            let ev = store.lock().unwrap().list_session_events(id, 50).unwrap();
            assert!(ev.iter().any(|e| e.kind == EVENT_MOVE_UNDONE), "{id}");
            assert!(!ev.iter().any(|e| e.kind == EVENT_MOVED), "{id}");
        }
        // Not one script ran against the target: no worktree, transcript or
        // carried file is ever touched by an undo.
        assert!(
            fake.calls_for("beta").iter().all(|c| c.script().is_none()),
            "undo ran a script on the target"
        );
    }

    #[tokio::test]
    async fn undo_refuses_a_target_that_has_taken_a_turn_or_is_busy() {
        for (label, edit) in [
            ("turn", |s: &Store, id: i64| s.set_turn_seq_for_test(id, 9)),
            ("busy", |s: &Store, id: i64| s.set_claude_status_for_test(id, "working")),
        ] {
            let (store, _src, target_id) = partial_fixture(true);
            {
                let s = store.lock().unwrap();
                edit(&s, target_id);
            }
            let fake = FakeSsh::new();
            let hooks = FakeHooks::new(&fake);
            let err = resolve_move_with(
                ResolveMoveArgs { session_id: target_id, action: ResolveMoveAction::Undo },
                &store,
                &fake,
                &hooks,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID_STATE, "{label}");
            assert!(!hooks.killed_any(), "{label}: nothing killed on a refusal");
        }
    }

    #[tokio::test]
    async fn a_session_with_no_unresolved_partial_is_refused() {
        // `unresolved: false` records a `session_moved` after the partial.
        let (store, _src, target_id) = partial_fixture(false);
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        for action in [ResolveMoveAction::Finish, ResolveMoveAction::Undo] {
            let err = resolve_move_with(
                ResolveMoveArgs { session_id: target_id, action },
                &store,
                &fake,
                &hooks,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID_STATE);
            assert!(err.message.contains("not a partial"), "{}", err.message);
        }
    }

    #[tokio::test]
    async fn a_partial_missing_the_fact_an_action_needs_is_refused_not_guessed() {
        let (store, _src, target_id) = partial_fixture(true);
        // Rewrite the partial detail with a null source_transcript_size.
        // (Insert a newer `session_move_partial` whose detail lacks it.)
        {
            let s = store.lock().unwrap();
            s.insert_session_event(
                target_id,
                EVENT_MOVE_PARTIAL,
                Some(r#"{"step":"x","to_host":"beta","from_host":"alpha","from_session_id":1,"to_session_id":2,"source_transcript_size":null,"source_transcript_mtime":null,"to_turn_seq":null,"to_last_turn_at":null,"claude_session_id":"c","branch":"feat","to_tmux_name":"t"}"#),
            )
            .unwrap();
        }
        let fake = FakeSsh::new();
        let hooks = FakeHooks::new(&fake);
        let err = resolve_move_with(
            ResolveMoveArgs { session_id: target_id, action: ResolveMoveAction::Finish },
            &store,
            &fake,
            &hooks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(!hooks.killed_any());
    }
```

The two `*_for_test` store setters and `FakeHooks::killed_any` may not exist.
Prefer the real API: rows are created through the same `Store` calls
`mod.rs`'s fixture uses, and `turn_seq` / `claude_status` are set through
whatever public setter the store already has (search for `set_claude_status`,
`bump_turn_seq`). Do **not** add a `*_for_test` method to `store/` — that is
another task's file and a public API for a test's convenience. If no setter
exists, build the row with the value you need at insert time.

- [ ] **Step 2: Run the tests and watch them fail**

```bash
cargo test -p fleet-core
```

Expected: FAIL to compile — no `resolve` module. Record it.

- [ ] **Step 3: Implement `resolve.rs`**

Structure, in order:

1. **Module docs** — what a partial is, why the event is the handle rather than
   the run store, and the one-line statement of each refusal.
2. `ResolveMoveAction`, `ResolveMoveArgs`, `ResolveMoveReport` as above.
3. `struct PartialRecord` — the parsed newest unresolved
   `session_move_partial`: every field an `Option` exactly as the JSON has it,
   plus the ids. A private `fn newest_unresolved(events: &[SessionEvent]) -> Option<PartialRecord>`:
   walk newest-first, return `None` if a `session_moved` or
   `EVENT_MOVE_UNDONE` is seen before a `EVENT_MOVE_PARTIAL`. **Pure** over the
   event slice, so it is testable without a store.
4. `resolve_move` — the real-hooks entry point (`RealHooks { ssh }`), mirroring
   `move_session`'s own wrapper.
5. `resolve_move_with` — validate, read both rows, dispatch:
   - **Finish:** require the target row to be `running`; require
     `source_transcript_size`/`_mtime` to be present; build `FinaliseArgs` with
     `copied` from those two values and `extra_detail`
     `json!({"finished_from_partial": true})`; call `finalise_source`. Its
     transcript re-check does the refusing, so the "source took a turn" rule
     needs no second implementation.
   - **Undo:** require `to_turn_seq` present; compare with the target row's
     current `turn_seq` (and `last_turn_at` when both are present); require the
     target's `claude_status` to be `idle`; then
     `hooks.kill_tmux_session(store, to_host, to_tmux_name)` and insert
     `EVENT_MOVE_UNDONE` on both rows. Nothing else — no script, no cleanup.
6. Refusal messages name what was seen and what was expected, in that order,
   and end without an instruction the user cannot act on.

- [ ] **Step 4: Run the whole crate suite, unpiped**

```bash
cargo test -p fleet-core
```

Expected: PASS.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/resolve.rs crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): resolve_move finishes or undoes a partial transfer"
```

---

### Task 7: The command, the hub verdict, the MCP tool and the generated files

**Files:**
- Create: `src-tauri/src/commands/resolve_move.rs`
- Modify: `src-tauri/src/commands/mod.rs` (`pub mod resolve_move;`),
  `src-tauri/src/lib.rs` (`generate_handler!`),
  `src-tauri/src/backend/verdicts.rs` (one row, right after `move_session`'s),
  `src-tauri/src/backend/tests_routing.rs` (a routed call, a non-default args
  row for `resolve_move`, and a non-default `clean_target` in `move_session`'s
  args row), `src-tauri/src/commands/move_session.rs` (its header lists the
  parameter set — add `clean_target`),
  `crates/fleet-core/src/mcp/tools/lifecycle.rs` (the `resolve_move` tool and
  `clean_target`'s clause), `crates/fleet-core/src/mcp/guard.rs` (the
  `TOOL_POLICIES` row), `crates/fleet-core/src/mcp/tools/tests.rs`
  (`served` 73 → 74).
- Regenerated, never hand-edited: `docs/control-api-reference.md`,
  `src/lib/hub_verdicts.generated.json`, the refusal table in `docs/hub.md`.

**Why an MCP tool is not optional:** `Verdict::Routed` resolves its hub tool
through the MCP table (`backend/remote.rs`, `tool_for`), so a routed command with
no tool of that name fails closed. `LocalOnly` is not an option either — it would
leave a hub-client desktop unable to recover a partial, which is this slice's
whole point.

- [ ] **Step 1: Write the failing routing test**

In `src-tauri/src/backend/tests_routing.rs`, add `resolve_move` to the handler
list and a routed expectation **by command name** — never a second tool literal,
never a pasted sentence. Follow `move_session`'s row exactly. Add the
non-default args rows (`clean_target: true` for `move_session`;
`session_id`/`action` for `resolve_move`).

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p claude-fleet
```

Expected: FAIL — `every_command_has_a_verdict` reports `resolve_move` has no
row, and the args-row test reports a default value.

- [ ] **Step 3: The command**

```rust
//! Tauri IPC wrapper for `resolve_move` (Finish / Undo on the Transfer
//! sheet's partial view). Logic lives in `service::move_session::resolve`.
//!
//! Routes in remote mode: `ResolveMoveArgs` is exactly the tool's parameter
//! set (`session_id`, `action`), and the tool answers the same
//! `ResolveMoveReport`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::move_session::resolve::{
    self, ResolveMoveArgs, ResolveMoveReport,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn resolve_move(
    args: ResolveMoveArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ResolveMoveReport, IpcError> {
    routed::resolve_move(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn resolve_move(
        backend: &FleetBackend,
        args: ResolveMoveArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ResolveMoveReport, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("resolve_move", &args).await,
            None => resolve::resolve_move(args, store, ssh).await,
        }
    }
}
```

Verdict row, immediately after `move_session`'s in `verdicts.rs`:

```rust
    (
        "resolve_move",
        Verdict::Routed {
            tool: "resolve_move",
        },
    ),
```

- [ ] **Step 4: The MCP tool, slim**

In `mcp/tools/lifecycle.rs`, beside `move_session`. One sentence; the reasoning
belongs in the spec, not in bytes every request pays for:

```rust
    /// Finish or undo a partial move (E_MOVE_PARTIAL: the target session
    /// started and both are alive). finish kills the source and records the
    /// move as complete; undo kills the new session and keeps the source, and
    /// is refused if the target has taken a turn or is not idle. Neither
    /// removes the target's worktree, transcript or carried files.
    #[tool(description = "...")]
```

and the `TOOL_POLICIES` row in `mcp/guard.rs`, matching `move_session`'s:

```rust
    // Finishes or undoes a partial move: kills one of the two sessions.
    ToolPolicy {
        name: "resolve_move",
        access: Access::Client,
        readonly: false,
        confirm: true,
        deadline: Deadline::Lifecycle,
    },
```

Add `clean_target` to `move_session`'s tool description as ONE clause, e.g.
`clean_target=true replaces what an unfinished earlier attempt left in the
target worktree (never the target's own work).`

Then `assert_eq!(served, 73)` → `74` in `mcp/tools/tests.rs`.

- [ ] **Step 5: Regenerate, in this order**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
```

Both regen runs may themselves report FAILED — that is how they work. Re-run
each **without** the env var to confirm it now passes. Never hand-merge
`docs/control-api-reference.md` or `src/lib/hub_verdicts.generated.json`.

- [ ] **Step 6: Full suites, unpiped, plus the budget**

```bash
cargo test -p fleet-core
```

```bash
cargo test -p claude-fleet
```

Expected: PASS, including `the_served_definition_budget_stays_bounded`. If the
surface is over `BUDGET_BYTES = 56_000`, **trim your description** — do not raise
the constant.

- [ ] **Step 7: Clippy, fmt, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add -A
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): resolve_move reaches the desktop and the hub, with its generated docs"
```

---
### Task 8: `moveSession.ts` + `moves.ts` — retry and resolve

**Files:**
- Modify: `src/lib/moveSession.ts` — `clean_target` on `moveSession`; the
  `resolveMove` wrapper and its two types.
- Modify: `src/lib/moves.ts` — `MoveRun.cleanTarget` / `.attempt`,
  `retryMove`, `resolveMoveRun`.
- Test: `src/lib/moves.test.ts` (extend; it already exists for `startMove`).

**Needs Tasks 9 and 10** — `UNDONE` from `moveErrors.ts`, and
`UnresolvedPartial` (the type of `adoptPartial`'s parameter) from `timeline.ts`.
Pre-flight ruling R2.

**Interfaces:**
- Consumes from Task 3 and 6 (wire only, no Rust import): the `clean_target`
  argument and the `resolve_move` command with `{ session_id, action }`. From
  Task 9: `UNDONE`.
- Produces, for Task 11:
  - `moveSession(sessionId, toHost, { keepSource?, strict?, cleanTarget? })`
  - `export type ResolveAction = 'finish' | 'undo';`
  - `export interface ResolveMoveReport { action: ResolveAction; source_session_id: number; target_session_id: number; from_host: string; to_host: string; source_killed: boolean; target_killed: boolean; warnings: string[] }`
  - `export function resolveMove(sessionId: number, action: ResolveAction): Promise<Result<ResolveMoveReport>>`
  - `export function retryMove(sessionId: number, opts?: { cleanTarget?: boolean }): void`
  - `export function resolveMoveRun(sessionId: number, action: ResolveAction): void`
  - `export function adoptPartial(p: UnresolvedPartial, sessionName: string): void`
    — rebuilds a `partial` run from a recorded `session_move_partial` so the
    sheet can offer Finish / Undo for a move this window never saw (Task 12
    needs it; `UnresolvedPartial` comes from `timeline.ts`, Task 10)
  - `MoveRun` gains `cleanTarget: boolean` and `attempt: number`

- [ ] **Step 1: Write the failing tests**

```ts
  it('retries the same target and options, on the same run', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_TARGET_DIRTY', 'dirty', { leftovers: 'ours', ours: ['a.txt'] }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    expect(get(moves).get(7)!.status).toBe('failed');

    invoked.mockResolvedValueOnce(ok(report({ target_session_id: 8 })));
    retryMove(7);
    await flush();
    const run = get(moves).get(7)!;
    expect(run.status).toBe('done');
    expect(run.attempt).toBe(2);
    expect(run.toHost).toBe('beta');
    // The second call carried the same options and no cleanup.
    expect(invoked.mock.calls.at(-1)![1].args).toMatchObject({
      session_id: 7,
      target_host_alias: 'beta',
      keep_source: false,
      clean_target: false,
    });
  });

  it('retries with clean_target when asked, and records it on the run', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_TARGET_DIRTY', 'dirty', { leftovers: 'ours', ours: ['a.txt'] }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    invoked.mockResolvedValueOnce(ok(report({ target_session_id: 8 })));
    retryMove(7, { cleanTarget: true });
    await flush();
    expect(invoked.mock.calls.at(-1)![1].args).toMatchObject({ clean_target: true });
    expect(get(moves).get(7)!.cleanTarget).toBe(true);
  });

  it('refuses to retry a run that is running or partial', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    let resolveIt: (v: unknown) => void = () => {};
    invoked.mockReturnValueOnce(new Promise((r) => (resolveIt = r)));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    const calls = invoked.mock.calls.length;
    retryMove(7);
    expect(invoked.mock.calls.length).toBe(calls);
    resolveIt(err('E_MOVE_PARTIAL', 'partial', { step: 'killing the source s on alpha' }));
    await flush();
    expect(get(moves).get(7)!.status).toBe('partial');
    retryMove(7);
    expect(invoked.mock.calls.length).toBe(calls + 1 - 1 + 0 + 1 - 1); // unchanged
  });

  it('finishing a partial settles the run as done', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_PARTIAL', 'partial', { step: 'killing the source s on alpha' }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    invoked.mockResolvedValueOnce(
      ok({
        action: 'finish',
        source_session_id: 7,
        target_session_id: 8,
        from_host: 'alpha',
        to_host: 'beta',
        source_killed: true,
        target_killed: false,
        warnings: [],
      }),
    );
    resolveMoveRun(7, 'finish');
    await flush();
    expect(invoked.mock.calls.at(-1)![0]).toBe('resolve_move');
    expect(invoked.mock.calls.at(-1)![1].args).toEqual({ session_id: 8, action: 'finish' });
    expect(get(moves).get(7)!.status).toBe('done');
  });

  it('adopts a recorded partial so the sheet can act on it after a restart', () => {
    adoptPartial(
      { targetSessionId: 8, sourceSessionId: 7, fromHost: 'alpha', toHost: 'beta', step: 'killing the source s on alpha' },
      's',
    );
    const run = get(moves).get(7)!;
    expect(run.status).toBe('partial');
    expect(run.toHost).toBe('beta');
    expect(run.error?.details).toMatchObject({ target_session_id: 8 });
  });

  it('undoing a partial leaves the run failed and says so', async () => {
    const session = row({ id: 7, tmux_name: 's', host_alias: 'alpha' });
    invoked.mockResolvedValueOnce(err('E_MOVE_PARTIAL', 'partial', { step: 'killing the source s on alpha' }));
    startMove(session, 'beta', { keepSource: false });
    await flush();
    invoked.mockResolvedValueOnce(
      ok({
        action: 'undo',
        source_session_id: 7,
        target_session_id: 8,
        from_host: 'alpha',
        to_host: 'beta',
        source_killed: false,
        target_killed: true,
        warnings: [],
      }),
    );
    resolveMoveRun(7, 'undo');
    await flush();
    const run = get(moves).get(7)!;
    expect(run.status).toBe('failed');
    expect(run.error?.code).toBe('E_MOVE_UNDONE');
  });
```

Match the file's existing helper names (`row`, `report`, `ok`, `err`, `flush`,
`invoked`) — read the top of `moves.test.ts` first and use what is there rather
than introducing new ones. In the third test, write the "unchanged" assertion
plainly as `expect(invoked.mock.calls.length).toBe(calls)` — the arithmetic above
is deliberately absurd to make sure you do not copy it.

`resolve_move` takes the **target** session id (`session_id: 8`), while the run
is keyed by the **source** id (7): the run's `report?.target_session_id` is where
that comes from, and for a partial it is in `error.details.target_session_id`.
`resolveMoveRun` must read it from whichever is present and refuse (a toast, no
call) when neither is.

`E_MOVE_UNDONE` is a frontend-only marker code for "the run ended because you
undid it" — it never comes from Rust. It is defined and exported by **Task 9**
(`moveErrors.ts`, the module whose job is turning codes into text) as
`export const UNDONE = 'E_MOVE_UNDONE';`, and `moves.ts` imports it. That is
why Task 9 runs before Task 8. No cycle: `moveErrors.ts` imports only
`moveProgress` and `result`, both type-only.

- [ ] **Step 2: Run and watch them fail**

```bash
npx vitest run src/lib/moves.test.ts
```

Expected: FAIL — `retryMove is not a function`.

(If the binary is missing: `pnpm install --frozen-lockfile` first. A frontend
failure is never "pre-existing" until you have done that.)

- [ ] **Step 3: Implement**

`moveSession.ts`: add `cleanTarget?: boolean` to the options and
`clean_target: opts.cleanTarget ?? false` to the args; extend the doc comment
with one sentence. Add `ResolveAction`, `ResolveMoveReport` and:

```ts
/** Finish or undo a partial move (`E_MOVE_PARTIAL`). `sessionId` is the
 *  TARGET session's id. Refusals: `E_INVALID_STATE` (not a partial; the source
 *  moved on; the target took a turn or is not idle), `E_NOTFOUND`. */
export async function resolveMove(
  sessionId: number,
  action: ResolveAction,
): Promise<Result<ResolveMoveReport>> {
  return invokeCmd<ResolveMoveReport>('resolve_move', {
    args: { session_id: sessionId, action },
  });
}
```

`moves.ts`: import `UNDONE` from `./moveErrors`, add the two `MoveRun` fields
(`cleanTarget: false`, `attempt: 1` in `startMove`'s `put`), then:

```ts
/**
 * Run the same move again, on the same run entry. Only for a run that FAILED:
 * a running one is already going, and a partial needs `resolveMoveRun` — a
 * second `move_session` there would build a second target.
 */
export function retryMove(sessionId: number, opts: { cleanTarget?: boolean } = {}): void {
  const run = get(store).get(sessionId);
  if (!run || run.status !== 'failed' || run.origin !== 'local') return;
  const cleanTarget = opts.cleanTarget ?? false;
  put({
    ...run,
    steps: blank(),
    status: 'running',
    report: null,
    error: null,
    cleanTarget,
    attempt: run.attempt + 1,
    startedAt: Date.now(),
    settledAt: null,
  });
  void moveSession(sessionId, run.toHost, {
    keepSource: run.keepSource ?? false,
    cleanTarget,
  }).then((r) => settle(sessionId, r));
}
```

`adoptPartial` puts a run with `status: 'partial'`, `origin: 'local'`,
`steps: blank()` with every step up to and including `start` marked `done`,
`error: { code: 'E_MOVE_PARTIAL', message: '', details: { step: p.step, target_session_id: p.targetSessionId, target_host: p.toHost } }`,
`fromHost: p.fromHost`, `toHost: p.toHost`, keyed by `p.sourceSessionId ?? p.targetSessionId`
— so `resolveMoveRun` finds the target id in `error.details` exactly as it does
for a live partial. It is a no-op when a run for that key already exists: a live
run is always the better picture.

and `resolveMoveRun`, which calls `resolveMove` with the target id and then
settles the run: `finish` → `status: 'done'` (keeping the existing `report` if
any); `undo` → `status: 'failed'` with `error = { code: UNDONE, message: … }`.
Reuse `put` and the `settledAt` discipline; do **not** bypass `settle`'s
`SETTLE_GRACE_MS` reasoning by inventing a second settle path — add the two
transitions inside `settle`'s module so the grace window still applies.

- [ ] **Step 4: Run the full frontend suite, unpiped**

```bash
npx vitest run
```

```bash
npx svelte-check
```

Expected: PASS. Every pre-existing `moves.test.ts` case must still pass: the two
new `MoveRun` fields are additive, and `MoveRun` fixtures elsewhere may need them.

- [ ] **Step 5: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/moveSession.ts src/lib/moves.ts src/lib/moves.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): the run store can retry a failed transfer and resolve a partial"
```

---

### Task 9: `moveErrors.ts` — the three leftovers, and the two new endings

**Files:**
- Modify: `src/lib/moveErrors.ts`
- Test: `src/lib/moveErrors.test.ts`

**Runs in parallel with Task 10; Task 8 waits on both.**

**Interfaces:**
- Consumes: `IpcError.details.leftovers` (`'ours' | 'theirs' | 'unknown'`),
  `details.ours`, `details.theirs` from Task 2.
- Produces, for Task 8: `export const UNDONE = 'E_MOVE_UNDONE';`
- Produces, for Task 11: `MoveFailure` gains
  `action: { kind: 'retry' } | { kind: 'clean'; paths: string[] } | null`, so the
  sheet reads its affordance from the same place it reads the prose.

- [ ] **Step 1: Write the failing tests**

```ts
  it.each([
    ['ours', ['src/lib.rs'], 'left behind', 'clean'],
    ['theirs', ['their_notes.md'], 'work of its own', null],
    ['unknown', [], 'already has uncommitted', 'retry'],
  ])('describes E_MOVE_TARGET_DIRTY(%s)', (leftovers, paths, phrase, action) => {
    const details = leftovers === 'theirs' ? { leftovers, theirs: paths } : { leftovers, ours: paths };
    const f = describeMoveError(
      { code: 'E_MOVE_TARGET_DIRTY', message: 'raw', details },
      'failed',
      'turanga',
      'replay',
    );
    expect(f.what).toContain(phrase);
    expect(f.action?.kind ?? null).toBe(action);
    if (action === 'clean') expect((f.action as { paths: string[] }).paths).toEqual(paths);
  });

  it('names the paths it would remove, and turanga, for stale leftovers', () => {
    const f = describeMoveError(
      {
        code: 'E_MOVE_TARGET_DIRTY',
        message: 'raw',
        details: { leftovers: 'ours', ours: ['a.txt', 'b/c.txt'] },
      },
      'failed',
      'turanga',
      'replay',
    );
    expect(f.what).toContain('turanga');
    expect(f.what).toContain('a.txt');
  });

  it('says nothing was overwritten when the target is holding its own work', () => {
    const f = describeMoveError(
      { code: 'E_MOVE_TARGET_DIRTY', message: 'raw', details: { leftovers: 'theirs', theirs: ['x'] } },
      'failed',
      'turanga',
      'replay',
    );
    expect(f.action).toBeNull();
    expect(f.standing).toContain('was not touched');
  });

  it('an undone partial reads as undone, not as a failure', () => {
    const f = describeMoveError(
      { code: 'E_MOVE_UNDONE', message: '', details: null },
      'failed',
      'turanga',
      'start',
    );
    expect(f.what).toContain('undid');
    expect(f.action).toBeNull();
  });
```

- [ ] **Step 2: Run and watch them fail**

```bash
npx vitest run src/lib/moveErrors.test.ts
```

Expected: FAIL — the existing `'E_MOVE_TARGET_DIRTY'` case returns one sentence
for all three, and `MoveFailure` has no `action`.

- [ ] **Step 3: Implement**

Add to the `MoveFailure` interface:

```ts
  /** What the sheet may offer. `clean` carries the paths a cleanup would
   *  replace, so the confirmation can name them; `null` means there is nothing
   *  the app can do — only the user, on that host. */
  action: { kind: 'retry' } | { kind: 'clean'; paths: string[] } | null;
```

In `what()`, replace the single `E_MOVE_TARGET_DIRTY` arm with a helper that
reads `details.leftovers` and returns both the sentence and the action:

- `ours` → `` `{toHost} still holds work an earlier transfer attempt left behind, and it differs from what is being carried now: {paths}.` `` + `{ kind: 'clean', paths }`
- `theirs` → `` `{toHost} has uncommitted work of its own in this worktree: {paths}. Commit or discard it there first.` `` + `null`
- anything else → today's sentence verbatim + `{ kind: 'retry' }`

Add the `E_MOVE_UNDONE` arm: `` `You undid the transfer: the new session on {toHost} was killed and the source is still running.` `` with `action: null`.

Every other arm gets `action: { kind: 'retry' }` **except** `E_MOVE_PARTIAL`
(whose actions are Finish/Undo, which the sheet derives from `status`, not from
here) and `E_LOCAL_ONLY` (nothing to retry) — give those `null`. Keep
`E_MOVE_TARGET_DIRTY` in `ALWAYS_TOUCHED_THE_TARGET`: an adopted target means
nothing was written, but a *refused* one still had the clone and worktree set up.

- [ ] **Step 4: Full suite, unpiped**

```bash
npx vitest run
```

```bash
npx svelte-check
```

Expected: PASS. `TransferSheet.svelte` does not read `action` yet (Task 11), so
adding a required field to `MoveFailure` must not break its type-check — if it
does, it is because a test constructs a `MoveFailure` literal; fix the test.

- [ ] **Step 5: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/moveErrors.ts src/lib/moveErrors.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): a dirty target says whose work it is holding, and what can be done about it"
```

---

### Task 10: `timeline.ts` — where a session came from, and an unresolved partial

**Files:**
- Modify: `src/lib/timeline.ts`
- Test: `src/lib/timeline.test.ts`

**Runs in parallel with Tasks 8 and 9.** (Task 10 has no dependants but Task 12.)

**Interfaces:**
- Consumes: `SessionEvent[]` as `sessionHistory` already returns them
  (newest first), and the detail keys Task 4 writes.
- Produces, for Task 12:

```ts
export interface MoveOrigin { fromHost: string; claudeSessionId: string | null }
export interface UnresolvedPartial {
  targetSessionId: number;
  sourceSessionId: number | null;
  fromHost: string;
  toHost: string;
  step: string | null;
}
export function moveOrigin(events: SessionEvent[]): MoveOrigin | null
export function unresolvedPartial(events: SessionEvent[]): UnresolvedPartial | null
```

Both are **pure** over the array the details panel already fetches: no new IPC.

- [ ] **Step 1: Write the failing tests**

```ts
  const ev = (id: number, kind: string, detail: unknown): SessionEvent => ({
    id,
    session_id: 8,
    at: 1700000000 + id,
    kind,
    detail: detail === null ? null : JSON.stringify(detail),
    claude_session_id: null,
  });
  // sessionHistory returns newest first.
  const newestFirst = (...e: SessionEvent[]) => [...e].reverse();

  describe('moveOrigin', () => {
    it('reads the host a session was moved from', () => {
      const events = newestFirst(ev(1, 'session_moved', { from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }));
      expect(moveOrigin(events)).toEqual({ fromHost: 'alpha', claudeSessionId: 'c1' });
    });

    it('uses the most recent move when a session moved twice', () => {
      const events = newestFirst(
        ev(1, 'session_moved', { from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }),
        ev(2, 'session_moved', { from_host: 'beta', to_host: 'gamma', claude_session_id: 'c1' }),
      );
      expect(moveOrigin(events)!.fromHost).toBe('beta');
    });

    it('is null when the session was never moved, and when the detail is unusable', () => {
      expect(moveOrigin([])).toBeNull();
      expect(moveOrigin(newestFirst(ev(1, 'session_moved', null)))).toBeNull();
      expect(moveOrigin(newestFirst(ev(1, 'session_moved', { to_host: 'beta' })))).toBeNull();
      expect(moveOrigin(newestFirst(ev(1, 'killed', { from_host: 'alpha' })))).toBeNull();
    });
  });

  describe('unresolvedPartial', () => {
    const partial = (id: number) =>
      ev(id, 'session_move_partial', {
        step: 'killing the source s on alpha',
        from_host: 'alpha',
        to_host: 'beta',
        from_session_id: 7,
        to_session_id: 8,
      });

    it('finds a partial nothing has resolved', () => {
      expect(unresolvedPartial(newestFirst(partial(1)))).toEqual({
        targetSessionId: 8,
        sourceSessionId: 7,
        fromHost: 'alpha',
        toHost: 'beta',
        step: 'killing the source s on alpha',
      });
    });

    it('is null once the move was finished or undone', () => {
      for (const kind of ['session_moved', 'session_move_undone']) {
        const events = newestFirst(partial(1), ev(2, kind, { from_host: 'alpha', to_host: 'beta' }));
        expect(unresolvedPartial(events)).toBeNull();
      }
    });

    it('finds a NEW partial that followed a resolved one', () => {
      const events = newestFirst(partial(1), ev(2, 'session_moved', { from_host: 'alpha', to_host: 'beta' }), partial(3));
      expect(unresolvedPartial(events)!.targetSessionId).toBe(8);
    });
  });
```

- [ ] **Step 2: Run and watch them fail**

```bash
npx vitest run src/lib/timeline.test.ts
```

Expected: FAIL — `moveOrigin is not exported`.

- [ ] **Step 3: Implement**

Walk the array as given (newest first) and stop at the first thing that decides:

```ts
const MOVE_KINDS = new Set(['session_moved', 'session_move_partial', 'session_move_undone']);

function detailOf(e: SessionEvent): Record<string, unknown> | null {
  if (e.detail === null) return null;
  try {
    const v: unknown = JSON.parse(e.detail);
    return typeof v === 'object' && v !== null ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}
```

`moveOrigin`: the newest `session_moved` whose detail has a string `from_host`;
`claudeSessionId` from `claude_session_id` when it is a string, else `null`.
Ignore every other kind, and a malformed detail is simply not a move.

`unresolvedPartial`: scan newest-first over `MOVE_KINDS` only; a
`session_moved` or `session_move_undone` seen **first** means resolved → `null`;
a `session_move_partial` seen first is the answer, provided its detail yields a
numeric `to_session_id` (without it there is nothing to act on → `null`).

Add both to the module's own doc comment: these two helpers are what make the
return trip and the recovery available after the run store is gone, which is why
they are pure and live next to the other timeline helpers.

- [ ] **Step 4: Full suite, unpiped**

```bash
npx vitest run
```

```bash
npx svelte-check
```

- [ ] **Step 5: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/timeline.ts src/lib/timeline.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): a session's own timeline says where it came from and what is unresolved"
```

---
### Task 11: The Transfer sheet's four actions

**Files:**
- Modify: `src/lib/TransferSheet.svelte`
- Test: `src/lib/TransferSheet.test.ts`

**Needs Tasks 8 and 9.**

**Interfaces:**
- Consumes: `retryMove`, `resolveMoveRun`, `UNDONE` (Task 8); `MoveFailure.action`
  (Task 9); the existing `run.fromHost`, `run.status`, `startMove`,
  `dismissMove`.
- Produces: nothing other tasks read.

- [ ] **Step 1: Write the failing tests**

```ts
  it('offers Retry on a plain failure and re-runs the same move', async () => {
    const { getByTestId } = renderSheet(
      failedRun({ code: 'E_MOVE_CARRY', details: { step: 'apply' } }),
    );
    await fireEvent.click(getByTestId('transfer-retry'));
    expect(retryMove).toHaveBeenCalledWith(7, { cleanTarget: false });
  });

  it('offers a cleanup that names what it would replace, and needs two clicks', async () => {
    const { getByTestId, queryByTestId, getByText } = renderSheet(
      failedRun({
        code: 'E_MOVE_TARGET_DIRTY',
        details: { leftovers: 'ours', ours: ['src/lib.rs', 'notes.txt'] },
      }),
    );
    await fireEvent.click(getByTestId('transfer-clean'));
    // Nothing has run yet: the first click only reveals what it would remove.
    expect(retryMove).not.toHaveBeenCalled();
    expect(getByText(/src\/lib\.rs/)).toBeTruthy();
    await fireEvent.click(getByTestId('transfer-clean-confirm'));
    expect(retryMove).toHaveBeenCalledWith(7, { cleanTarget: true });
    expect(queryByTestId('transfer-clean-confirm')).toBeNull();
  });

  it('never offers a cleanup for the target own work', () => {
    const { queryByTestId } = renderSheet(
      failedRun({ code: 'E_MOVE_TARGET_DIRTY', details: { leftovers: 'theirs', theirs: ['x.md'] } }),
    );
    expect(queryByTestId('transfer-clean')).toBeNull();
    expect(queryByTestId('transfer-retry')).toBeNull();
  });

  it('offers Move back on a finished move', async () => {
    const { getByTestId } = renderSheet(doneRun({ fromHost: 'alpha', toHost: 'beta' }));
    const back = getByTestId('transfer-move-back');
    expect(back.textContent).toContain('alpha');
    await fireEvent.click(back);
    expect(startMove).toHaveBeenCalledWith(
      expect.objectContaining({ id: 8 }),
      'alpha',
      { keepSource: false },
    );
  });

  it('offers Finish and Undo on a partial, each behind its own confirm', async () => {
    const { getByTestId } = renderSheet(partialRun());
    await fireEvent.click(getByTestId('transfer-finish'));
    expect(resolveMoveRun).not.toHaveBeenCalled();
    await fireEvent.click(getByTestId('transfer-finish-confirm'));
    expect(resolveMoveRun).toHaveBeenCalledWith(7, 'finish');

    await fireEvent.click(getByTestId('transfer-undo'));
    await fireEvent.click(getByTestId('transfer-undo-confirm'));
    expect(resolveMoveRun).toHaveBeenCalledWith(7, 'undo');
  });

  it('shows a refusal from Finish in place, not as a toast', async () => {
    const { getByTestId, getByText } = renderSheet(
      partialRun({ resolveError: { code: 'E_INVALID_STATE', message: 'the target took a turn', details: null } }),
    );
    expect(getByText(/took a turn/)).toBeTruthy();
    expect(getByTestId('transfer-finish')).toBeTruthy();
  });

  it('an undone run reads as undone', () => {
    const { getByText, queryByTestId } = renderSheet(
      failedRun({ code: 'E_MOVE_UNDONE', details: null }),
    );
    expect(getByText(/undid/)).toBeTruthy();
    expect(queryByTestId('transfer-retry')).toBeNull();
  });
```

`renderSheet(run)`, `failedRun`, `doneRun`, `partialRun` are this file's own
helpers — extend whatever it already has for the failure view; the existing tests
show how the sheet is mounted with a seeded `moves` store and a session row.
Mock `./moves`'s `retryMove`, `resolveMoveRun` and `startMove` with `vi.mock`
the way the file already mocks `startMove`.

**Move back targets the NEW session** (id 8 in the test): the source row is gone
after a completed move, so the button moves the *target* session back to
`run.fromHost`. Read the row out of `$sessions` by
`run.report.target_session_id`, and hide the button when that row is not there.

- [ ] **Step 2: Run and watch them fail**

```bash
npx vitest run src/lib/TransferSheet.test.ts
```

Expected: FAIL — no `transfer-retry` element.

- [ ] **Step 3: Implement**

Failure view buttons, before `Done`:

```svelte
      <div class="buttons">
        {#if run.status === 'partial'}
          {#if newSession}
            <button onclick={openTarget} data-testid="transfer-open-target">Open on {run.toHost}</button>
          {/if}
          {#if confirming === 'finish'}
            <button class="danger" onclick={() => resolve('finish')} data-testid="transfer-finish-confirm">
              Kill {run.sessionName} on {run.fromHost}
            </button>
          {:else}
            <button onclick={() => (confirming = 'finish')} data-testid="transfer-finish">Finish the move</button>
          {/if}
          {#if confirming === 'undo'}
            <button class="danger" onclick={() => resolve('undo')} data-testid="transfer-undo-confirm">
              Kill the new session on {run.toHost}
            </button>
          {:else}
            <button onclick={() => (confirming = 'undo')} data-testid="transfer-undo">Undo</button>
          {/if}
        {:else if failure.action?.kind === 'clean'}
          {#if confirming === 'clean'}
            <button class="danger" onclick={() => retry(true)} data-testid="transfer-clean-confirm">
              Replace {failure.action.paths.length} file(s) on {run.toHost} and retry
            </button>
          {:else}
            <button onclick={() => (confirming = 'clean')} data-testid="transfer-clean">
              Clean up {run.toHost} and retry
            </button>
          {/if}
        {:else if failure.action?.kind === 'retry'}
          <button onclick={() => retry(false)} data-testid="transfer-retry">Retry</button>
        {/if}
        <button onclick={done} data-testid="transfer-done">Done</button>
      </div>
```

with, in the script block:

```ts
  // Which destructive action is one click from happening. Cleared whenever the
  // sheet's session changes, like `showDetails`.
  let confirming = $state<'clean' | 'finish' | 'undo' | null>(null);
  /** A refusal from Finish / Undo, shown in the sheet rather than as a toast:
   *  the user is looking straight at it. */
  let resolveError = $state<IpcError | null>(null);

  function retry(cleanTarget: boolean): void {
    if (id === null) return;
    confirming = null;
    retryMove(id, { cleanTarget });
  }

  function resolve(action: ResolveAction): void {
    if (id === null) return;
    confirming = null;
    resolveError = null;
    resolveMoveRun(id, action);
  }
```

When `confirming === 'clean'`, also render the path list above the buttons
(a `<ul>` of `failure.action.paths`, capped in the CSS, with a
`+N more` line when `ours` was capped at 50 by the script) — the confirmation
must say what it will remove, not just how many.

Result view, before `Done`:

```svelte
        {#if run.fromHost && newSession}
          <button onclick={moveBack} data-testid="transfer-move-back">Move back to {run.fromHost}</button>
        {/if}
```

```ts
  function moveBack(): void {
    if (!newSession || !run) return;
    startMove(newSession, run.fromHost, { keepSource: false });
  }
```

Add `confirming = null; resolveError = null;` to the existing `$effect` that
resets `keepSource` and `showDetails` on an id change: a confirmation must never
survive the sheet moving to another session.

- [ ] **Step 4: Full suite, unpiped**

```bash
npx vitest run
```

```bash
npx svelte-check
```

- [ ] **Step 5: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/TransferSheet.svelte src/lib/TransferSheet.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): the Transfer sheet can retry, clean up, come back, finish and undo"
```

---

### Task 12: The details panel offers the same actions later

**Files:**
- Modify: `src/lib/Timeline.svelte` — an `onEvents` callback prop.
- Modify: `src/lib/SessionDetails.svelte` — the two buttons.
- Test: `src/lib/SessionDetails.test.ts`

**Needs Task 10.**

**Spec correction:** §6.4 says the panel "already has the events". It does not —
`sessionHistory` is called inside `Timeline.svelte` (`Timeline.svelte:43`), which
also re-fetches on live timeline events. So rather than a second
`sessionHistory` call from the panel (an extra hub round trip on every panel
open, and two sources of truth for freshness), `Timeline` hands its events up
through a callback prop. No new IPC either way, which is what §6.4 was actually
asking for.

**Interfaces:**
- Consumes: `moveOrigin`, `unresolvedPartial` (Task 10); `transferSheetFor`,
  `startMove`, `resolveMoveRun` (Task 8).
- Produces: nothing.

- [ ] **Step 1: Write the failing tests**

```ts
  it('offers Move back when the session was moved here', async () => {
    const events = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_moved',
        detail: JSON.stringify({ from_host: 'alpha', to_host: 'beta', claude_session_id: 'c1' }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const { getByTestId } = render(SessionDetails, { props: { session: row({ id: 8, host_alias: 'beta' }) } });
    await waitFor(() => expect(getByTestId('details-move-back')).toBeTruthy());
    expect(getByTestId('details-move-back').textContent).toContain('alpha');
    await fireEvent.click(getByTestId('details-move-back'));
    expect(startMove).toHaveBeenCalledWith(expect.objectContaining({ id: 8 }), 'alpha', {
      keepSource: false,
    });
  });

  it('offers Finish and Undo for an unresolved partial', async () => {
    const events = [
      {
        id: 1,
        session_id: 8,
        at: 1700000000,
        kind: 'session_move_partial',
        detail: JSON.stringify({
          step: 'killing the source s on alpha',
          from_host: 'alpha',
          to_host: 'beta',
          from_session_id: 7,
          to_session_id: 8,
        }),
        claude_session_id: null,
      },
    ];
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? events : undefined));
    const { getByTestId } = render(SessionDetails, { props: { session: row({ id: 8, host_alias: 'beta' }) } });
    await waitFor(() => expect(getByTestId('details-finish-move')).toBeTruthy());
    expect(getByTestId('details-undo-move')).toBeTruthy();
  });

  it('offers neither when the timeline has no move in it', async () => {
    inv().mockImplementation(async (cmd: string) => (cmd === 'session_history' ? [] : undefined));
    const { queryByTestId } = render(SessionDetails, { props: { session: row({ id: 8 }) } });
    await waitFor(() => expect(queryByTestId('details-move-back')).toBeNull());
    expect(queryByTestId('details-finish-move')).toBeNull();
  });
```

Use the file's existing `inv()`, `row()` and render conventions (see
`SessionDetails.test.ts:347`, which already stubs `session_history`), and mock
`./moves` for `startMove` / `resolveMoveRun`.

- [ ] **Step 2: Run and watch them fail**

```bash
npx vitest run src/lib/SessionDetails.test.ts
```

Expected: FAIL — no `details-move-back`.

- [ ] **Step 3: Implement**

`Timeline.svelte`: add an optional prop and call it wherever `events` is set
(the initial load and the live-event refetch), so the panel's view of the
timeline is never staler than the Timeline's own:

```ts
  let { sessionId, onEvents }: { sessionId: number; onEvents?: (e: SessionEvent[]) => void } = $props();
```

`SessionDetails.svelte`: hold the events, derive the two affordances, render the
buttons next to the existing "Move to host…" action:

```ts
  let timelineEvents = $state<SessionEvent[]>([]);
  const origin = $derived(moveOrigin(timelineEvents));
  const partial = $derived(unresolvedPartial(timelineEvents));
```

```svelte
  {#if origin}
    <button onclick={() => startMove(session, origin.fromHost, { keepSource: false })} data-testid="details-move-back">
      Move back to {origin.fromHost}
    </button>
  {/if}
  {#if partial}
    <button onclick={() => transferSheetFor.set(partial.sourceSessionId ?? session.id)} data-testid="details-finish-move">
      Finish the move to {partial.toHost}
    </button>
    <button onclick={() => transferSheetFor.set(partial.sourceSessionId ?? session.id)} data-testid="details-undo-move">
      Undo the move
    </button>
  {/if}
```

Both partial buttons open the **sheet**, which is where the confirmations and the
refusal text live (Task 11) — the panel never calls `resolveMoveRun` itself, so
there is exactly one place a destructive recovery can be triggered from. If the sheet has no run for that id — the ordinary case, because the app was
restarted since the partial — its setup view would show instead, which is the
wrong thing entirely. So `moves.ts` (Task 8) exports one more function for this,
and Task 12 calls it:

```ts
/** Rebuild a `partial` run from a recorded `session_move_partial`, so the sheet
 *  can offer Finish / Undo for a move this window never saw. `origin` stays
 *  `'local'`: the actions are this window's, even though the move was not. */
export function adoptPartial(p: UnresolvedPartial, sessionName: string): void
```

If Task 8 has already been implemented without it, add it in Task 12 (a second
writer on `moves.ts` at that point is safe: Task 8 is finished and reviewed) and
say so in the ledger.

- [ ] **Step 4: Full suite, unpiped**

```bash
npx vitest run
```

```bash
npx svelte-check
```

- [ ] **Step 5: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/Timeline.svelte src/lib/SessionDetails.svelte src/lib/SessionDetails.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): the details panel offers the return trip and a partial's recovery"
```

---

## Before the PR

1. `git fetch origin` and compare: `main` moves ~50 commits a day. Merge
   `origin/main` **locally** (never squash) and re-run every suite on the merged
   tree — a clean text merge has hidden a semantic break here twice.
2. `scripts/ci-local.sh` in full, unpiped.
3. The whole-branch review: the most capable model, with the controller's own
   worries listed, then ONE fix wave, one scoped re-review, residuals
   adjudicated rather than looped on. On slice 3a this found six cross-task
   defects that seven per-task reviews had missed.
4. Ask before any `carry_e2e` run against a host, and never run a dev build.
