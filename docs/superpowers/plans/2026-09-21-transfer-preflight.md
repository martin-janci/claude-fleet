# Transfer slice 3b (preflight) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Before a Transfer starts, the setup view shows what would travel and
what would be refused, from a read-only `dry_run` of `move_session` that runs
the move's own opening checks rather than a copy of them.

**Architecture:** The move's opening sequence — store checks, operator refusal,
source reconcile and idle check, source git inspection, transcript locate and
cap — is extracted into one `gather()` that both the real move and the dry run
call. The dry run adds read-only probes the move does not need (target state,
ignored-file split, session-state and memory counts) and returns a
`MovePreview`; `move_session` returns a tagged `MoveOutcome`. The frontend asks
for a preview on target selection, debounced, and never lets it gate Transfer.

**Tech Stack:** Rust (tokio, async-trait, serde, rusqlite), Tauri 2 commands, an
rmcp tool, Svelte 5 runes + Vitest, POSIX/bash scripts run over ssh as
`bash -lc`.

**Spec:** `docs/superpowers/specs/2026-09-21-transfer-preflight-design.md`. The
plan argues from the spec; where they disagree the spec wins — except for the
three corrections below, which the spec has been updated to match.

## Corrections to the spec, made while planning

Reading the move's opening sequence turned up five places the spec asked for
something the code cannot give without cost. Each is resolved here and the spec
now says the same thing. The first three were found reading the move; the last
two follow from them once the preview's types are written against real code.

1. **The dry run does not accumulate host-phase refusals.** Spec §5 said
   mid-operation, transcript-over-cap and target-dirty findings "accumulate".
   The move short-circuits on its first problem, emits `move:progress` step
   boundaries in the middle of that sequence, and reads the whole transcript
   immediately after the cap check. Making one sequence both accumulate (for
   the preview) and short-circuit (for the move) means restructuring the move's
   verdict logic — the exact risk the spec's §1 principle, "the preview cannot
   drift from the move", exists to avoid. So `gather()` short-circuits exactly
   as the move does, and a preview reports **the one refusal the move would
   raise**. The findings computed *after* `gather()` — the target's state, the
   ignored split, the session-state and memory counts — are independent of it
   and only run when it succeeds. Cost: a source that is mid-merge **and** over
   the transcript cap shows one of the two, as the move itself would.
2. **"Nothing writes" means nothing writes to a host.** `gather()` includes
   `hooks.refresh_host`, a reconcile that updates the local store's rows from
   tmux — the same thing the background tick does. The dry run keeps it, because
   an idle check against a stale status is the one thing a preflight must not
   get wrong. It writes nothing to either host.
3. **`TargetState` describes the path the move would aim at.** The move derives
   that path (`cwd_hint`) before `ensure_target_workspace`, which may resolve it
   differently when it repairs a workspace. The probe reports `cwd_hint`'s state
   and says which path it looked at.

4. **A refusal is returned as the error the move would return — so `refusals`,
   `Refusal` and `transcript_over_cap` are dropped.** This follows directly from
   correction 1. With `gather()` short-circuiting, every refusal a read-only
   pass can establish surfaces as `gather()`'s `Err` — including the transcript
   cap, which `gather()` checks — and a successful preview therefore never holds
   one. A `refusals` list that is always empty, or a `transcript_over_cap` that
   is always `false` on a preview that exists at all, would be fields that
   cannot be true. So a dry run returns `Err(e)` with **the same code and
   message the real move would return**, which is the strongest form of
   "cannot drift" available: it is the same value. An SSH failure also surfaces
   as `Err`, which is honest — the move would fail the same way.
5. **`unpushed_commits` is added, and `target_path` names what was probed.**
   `commits_ahead` (what the target's clone lacks) is `None` whenever the target
   has no clone yet — which is every first transfer to a host — so on its own
   the preview's "commits" half would usually be empty. The source inspection
   already yields the unpushed count exactly and for free (`SourceState.ahead`,
   `-1` when unknown), so the preview carries it as `unpushed_commits:
   Option<u32>`. `target_path` records the path the probe looked at, per
   correction 3.

## Global Constraints

- **Never run a dev build of the desktop app** (`cargo tauri dev`,
  `pnpm tauri dev`, `cargo run -p claude-fleet`). It derives its data dir from
  `HOME`, so it migrates the installed app's `state.db` irreversibly, and its
  singleton guard kills the running app. UI is verified by Vitest only.
- **Never ssh anywhere.** Every engine test runs over `FakeSsh` or a local
  `bash` in a temp dir. `carry_e2e` against a host is the controller's call.
- **Every value interpolated into a shell command string is quoted with
  `crate::shell::quote`** (in scope as `quote`). There is one implementation.
- **Sentinels are matched with `contains`**, never parsed positionally, and every
  script payload is anchored on `carry::OUT_MARKER`, because `bash -lc` sources
  a login profile that can print before the script body.
- **No new `E_*` error codes**, in Rust or TypeScript.
- **Wire rule:** report and outcome types derive `Serialize + Deserialize` with
  no `#[serde(default)]`. A new hub-routed argument needs `Serialize` **and** a
  field on the MCP params struct mapped through its single `into_args` **and** a
  non-default row in `src-tauri/src/backend/tests_routing.rs` — 3d shipped a
  routed argument that reached the service and the command but not the tool's
  params struct, so it silently never reached the hub.
- **The served MCP surface has 169 bytes of slack** (56,831 of
  `BUDGET_BYTES = 57_000`). The `dry_run` parameter's doc comment must be terse;
  if the budget test trips, trim your own wording — never raise the constant.
- **Never hold the `Store` mutex across an `.await`.**
- **Generated files are regenerated, never hand-edited:**
  `docs/control-api-reference.md` (`REGEN_DOCS=1 cargo test -p fleet-core
  reference_is_current`, then the same command without the variable — the regen
  run itself may report FAILED). The hub contract golden is **not** involved:
  none of its 32 types is move-related.
- **The test code in this plan was written without being run.** In slice 3d, one
  plan-written assertion turned out to be a tautology (`is_null() || is_null()`)
  and another test would have passed with the code it guarded deleted. Treat the
  code below as the specification of *what* to assert. If an assertion cannot
  fail, or would pass with the production change reverted, fix it and say so in
  your report.
- **RED is captured chronologically.** Write the tests, run them against the
  code as it stands, paste that output — before writing the implementation.
  Never write the implementation first and revert it to manufacture a failure.
- **Run every suite in the foreground, unpiped, with no test-name filter for the
  final run**: `cargo test -p fleet-core`, `cargo test -p claude-fleet`,
  `npx vitest run`, `npx svelte-check`, plus
  `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all`.
  `pnpm test`/`pnpm check` cannot find their binaries here; the `npx` forms are
  correct. Run `pnpm install --frozen-lockfile` before calling any frontend
  failure pre-existing.
- **Git:** every command is `git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 …`.
  Never `pull`, `push`, `rebase`, `checkout`, `stash`, `merge` or `reset`. No
  attribution lines. Never print full process command lines (`pgrep -fl`,
  `ps aux`) — MCP servers here carry API tokens in argv.

---

## File Structure

**One writer per file**; tasks sharing a file are strictly sequential.

| File | Responsibility | Task |
|---|---|---|
| `crates/fleet-core/src/service/move_session/mod.rs` | `gather()` extracted from the move's opening sequence; later the `dry_run` branch and the `MoveOutcome` return | 1, 3, 4 |
| `crates/fleet-core/src/service/move_session/probe.rs` | **new** — read-only target probe and the source-side commit count, with their parsers | 2 |
| `crates/fleet-core/src/service/move_session/preview.rs` | **new** — `MovePreview`, `TargetState`, `Refusal`, and `preview()` | 3 |
| `crates/fleet-core/src/mcp/tools/params.rs`, `lifecycle.rs`, `tests.rs` | `dry_run` on the params struct and `into_args`; the handler branching before `confirm_gate`; the source-pin test | 4 |
| `src-tauri/src/commands/move_session.rs`, `src-tauri/src/backend/tests_routing.rs` | the command's `MoveOutcome` return; the non-default `dry_run` row and the response payload | 4 |
| `src/lib/moveSession.ts`, `src/lib/moves.ts` | the `MoveOutcome` type, `previewMove`, and the run store refusing a preview | 5 |
| `src/lib/preflight.ts` | **new** — newest preview per `(sessionId, toHost)`, its age, its in-flight state, the debounce | 6 |
| `src/lib/TransferSheet.svelte` | the setup view's rendering | 7 |

Generated: `docs/control-api-reference.md` (Task 4).

## Execution order

```
Rust:     1 → 2 → 3 → 4        (1, 3 and 4 all write mod.rs)
Frontend: 5 → 6 → 7            (needs 4 only for the wire shape; 5 can follow 4 directly)
```

---
### Task 1: Extract the move's opening sequence into `gather()`

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — the opening of
  `move_session_inner` (currently from the `snapshot` block through the
  transcript cap check, about `mod.rs:1967-2050`).
- Test: the same file's test module.

**This task changes no behaviour.** It is an extraction so that Task 3's dry run
can run exactly the move's own checks. The acceptance criterion is not a new
test passing — it is **every existing `move_session` test passing with no edit
to any test**, and the `move:progress` stream coming out byte-for-byte the same.
If any existing test needs a real change to pass, stop and report it: it means
the extraction changed behaviour, and that finding is worth more than the task.

**Interfaces:**
- Produces, for Task 3:

```rust
/// What the move's opening sequence established. Everything a dry run reports
/// about the source comes from here, so it cannot disagree with the move.
pub(super) struct Gathered {
    pub snap: Snapshot,
    pub src: String,
    pub target: String,
    /// The Claude conversation id (`snap.claude_id`).
    pub id: String,
    pub state: SourceState,
    pub located: Located,
}

/// The move's opening checks, in the move's order, short-circuiting on the
/// first problem exactly as the move does. `progress` is `Some` for a real
/// move — it emits the `check` and `transcript` step boundaries at the points
/// the move always has — and `None` for a dry run, which emits nothing.
/// Does NOT take the move claim: the caller does, when it is a real move.
pub(super) async fn gather(
    args: &MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    progress: Option<&mut progress::Progress<'_>>,
) -> Result<Gathered, IpcError>;
```

`Snapshot` is currently private (`struct Snapshot` at `mod.rs:1404`); give it
`pub(super)` visibility and `pub(super)` fields as far as `preview.rs` will need
them in Task 3 — or, if you prefer to keep the fields private, add the
`pub(super)` accessors Task 3 needs and name them in your report.

- [ ] **Step 1: Record the baseline before touching anything**

Run the covering tests and paste the result into your report. They are the
proof of equivalence:

```bash
cargo test -p fleet-core --lib move_session::
```

Name, in your report, the existing tests that exercise the opening sequence —
at least the progress-stream tests (`a_clean_move_reports_all_nine_steps_in_order`,
`a_carry_failure_ends_the_stream_at_the_step_that_failed`), every refusal test
for a store-level check, the idle refusal, the mid-operation refusal, the
`strict` refusals, the missing-transcript and too-large refusals, and the 3d
twin refusal.

- [ ] **Step 2: Extract**

Move the code, unchanged, from just after the `MoveClaim::acquire` line through
the transcript cap check (`if located.size > snap.cap { return Err(too_large(…)) }`)
into `gather()`. The claim stays in `move_session_inner`, **before** the call.
The `progress.start(MoveStep::Check)` and `progress.start(MoveStep::Transcript)`
calls move into `gather()` behind `if let Some(p) = progress.as_deref_mut()` (or
equivalent), at exactly the points they sit today. Keep every comment — they
record why each check exists.

`move_session_inner` becomes:

```rust
    crate::validate::host_alias(&args.target_host_alias)?;
    let _claim = MoveClaim::acquire(store, args.session_id)?;
    let Gathered { snap, src, target, id, state, located } =
        gather(&args, store, ssh, hooks, Some(progress)).await?;
    let mut warnings: Vec<String> = Vec::new();
    let mut carried = carry::CarryReport {
        dirty_entries: state.dirty.clone(),
        ..Default::default()
    };
    // …the transcript READ and everything after it, unchanged…
```

Note the transcript **read** (`read_script`) stays in the move, after `gather()`:
a dry run must never read a transcript that can be 200 MB.

- [ ] **Step 3: Prove equivalence**

```bash
cargo test -p fleet-core --lib move_session::
```

Same count, all passing, **and `git diff` shows no line changed inside the test
module**. Paste both into the report.

- [ ] **Step 4: One new test, for the seam**

```rust
    /// `gather()` with no progress sink emits no `move:progress` event and takes
    /// no claim — the two properties Task 3's dry run depends on.
    #[tokio::test]
    async fn gather_without_progress_emits_nothing_and_takes_no_claim() {
        let (f, bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let a = args(&f, false);
        gather(&a, &f.store, &f.fake, &hooks, None)
            .await
            .expect("the fixture's opening sequence succeeds");
        assert!(progress_of(&f, &bus).is_empty(), "no move:progress event");
        // No claim was taken: a real move of the same session can still start.
        MoveClaim::acquire(&f.store, a.session_id).expect("the claim is free");
    }
```

Write it, run it against the code as it was **before** Step 2 if you can — it
will not compile, because `gather` does not exist, and that compile error is
the RED to paste. Then confirm it passes after Step 2. If `MoveClaim` releases
on drop, the second `acquire` must be held in a binding; check its semantics
before relying on this assertion, and fix it if it cannot fail.

- [ ] **Step 5: Full suite, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "refactor(move): the opening checks become gather(), shared with the preview"
```

---

### Task 2: Read-only target probes

**Files:**
- Create: `crates/fleet-core/src/service/move_session/probe.rs`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `pub mod probe;`
  only (one line; Task 1 is finished, so this is not a conflict).
- Test: `probe.rs`'s own `#[cfg(test)] mod tests`, running the scripts under a
  local `bash` in a temp dir, the way `carry.rs`'s script tests do.

**Why a new file:** `carry.rs` is ~2,900 lines of scripts that *move* work.
These scripts only *look*, and the distinction is the safety property of the
whole slice — so they live apart, where a reviewer can confirm at a glance that
nothing in the file writes.

**Interfaces:**
- Consumes: `crate::shell::quote`, `carry::OUT_MARKER`, `carry::STATUS_PORCELAIN`,
  `carry::payload_str` (it is `pub(super)`; that is enough from a sibling
  module), `safe_kill::parse_porcelain`, `safe_kill::DirtyFile`.
- Produces, for Task 3:

```rust
/// What sits at the path the move would aim at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probed {
    /// Nothing there: the move would create it.
    Absent,
    /// A git worktree, its HEAD, and its porcelain (empty when clean).
    Worktree { head: String, porcelain: Vec<DirtyFile> },
}

/// Look at `cwd` and change nothing. Prints `OUT_MARKER` then either `absent`,
/// or `worktree`, a HEAD line, and `STATUS_PORCELAIN`'s output.
pub fn target_probe_script(cwd: &str) -> String;
pub fn parse_target_probe(stdout: &str) -> Result<Probed, IpcError>;

/// On the TARGET: the tip of `branch` in the clone at `project_root`, or
/// nothing when there is no clone or no such branch.
pub fn target_tip_script(project_root: &str, branch: &str) -> String;
pub fn parse_target_tip(stdout: &str) -> Result<Option<String>, IpcError>;

/// On the SOURCE: how many commits `HEAD` has that `tip` lacks, or nothing
/// when the source does not have `tip` at all — then the target holds commits
/// the source has never seen, and a count would be a guess.
pub fn commits_ahead_script(worktree: &str, tip: &str) -> String;
pub fn parse_commits_ahead(stdout: &str) -> Result<Option<u32>, IpcError>;
```

- [ ] **Step 1: Write the failing tests**

Reuse `carry.rs`'s test helpers where they are reachable (`bash`, `git`,
`require`); if they are private to `carry`'s test module, copy the three small
functions into `probe.rs`'s test module rather than widening another module's
test API.

```rust
    #[test]
    fn a_missing_path_is_absent() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = bash(&target_probe_script(tmp.path().join("nope").to_str().unwrap()), tmp.path());
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(parse_target_probe(&String::from_utf8_lossy(&out.stdout)).unwrap(), Probed::Absent);
    }

    #[test]
    fn a_clean_worktree_reports_its_head_and_no_entries() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        std::fs::write(wt.join("a.txt"), "a\n").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-q", "-m", "a"]);
        let head = git(&wt, &["rev-parse", "HEAD"]).trim().to_string();
        let out = bash(&target_probe_script(wt.to_str().unwrap()), tmp.path());
        match parse_target_probe(&String::from_utf8_lossy(&out.stdout)).unwrap() {
            Probed::Worktree { head: h, porcelain } => {
                assert_eq!(h, head);
                assert!(porcelain.is_empty(), "{porcelain:?}");
            }
            other => panic!("expected a worktree, got {other:?}"),
        }
    }

    #[test]
    fn a_dirty_worktree_lists_what_git_status_lists() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        std::fs::write(wt.join("a.txt"), "a\n").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-q", "-m", "a"]);
        std::fs::write(wt.join("a.txt"), "changed\n").unwrap();
        std::fs::write(wt.join("new file.txt"), "n\n").unwrap();
        let out = bash(&target_probe_script(wt.to_str().unwrap()), tmp.path());
        let Probed::Worktree { porcelain, .. } =
            parse_target_probe(&String::from_utf8_lossy(&out.stdout)).unwrap()
        else {
            panic!("expected a worktree");
        };
        let paths: Vec<_> = porcelain.iter().map(|d| d.path.as_str()).collect();
        assert!(paths.contains(&"a.txt"), "{paths:?}");
        assert!(paths.iter().any(|p| p.contains("new file.txt")), "{paths:?}");
    }

    #[test]
    fn the_probe_writes_nothing() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        std::fs::write(wt.join("a.txt"), "a\n").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-q", "-m", "a"]);
        std::fs::write(wt.join("a.txt"), "changed\n").unwrap();
        // Settle the index's stat cache first, so the only thing that could
        // change it afterwards is the probe itself.
        git(&wt, &["update-index", "-q", "--refresh"]);
        let index_before = std::fs::read(wt.join(".git/index")).unwrap();
        bash(&target_probe_script(wt.to_str().unwrap()), tmp.path());
        let index_after = std::fs::read(wt.join(".git/index")).unwrap();
        // Byte-identical, not merely equivalent: the probe runs `git status`
        // under GIT_OPTIONAL_LOCKS=0, which never writes the index. Without
        // that variable `git status` refreshes the stat cache and this fails —
        // which is the point of asserting on the bytes.
        assert_eq!(index_before, index_after, "the probe wrote the index");
        assert!(!wt.join(".git/index.lock").exists(), "a lock was left behind");
        // And an absent path is not created.
        let ghost = tmp.path().join("ghost");
        bash(&target_probe_script(ghost.to_str().unwrap()), tmp.path());
        assert!(!ghost.exists(), "the probe created the path it looked at");
    }

    #[test]
    fn commits_ahead_counts_what_the_tip_lacks_and_refuses_to_guess() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        for n in ["a", "b", "c"] {
            std::fs::write(wt.join(n), n).unwrap();
            git(&wt, &["add", n]);
            git(&wt, &["commit", "-q", "-m", n]);
        }
        let first = git(&wt, &["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
        let out = bash(&commits_ahead_script(wt.to_str().unwrap(), &first), tmp.path());
        assert_eq!(parse_commits_ahead(&String::from_utf8_lossy(&out.stdout)).unwrap(), Some(2));
        // A tip the source has never seen: unknown, not zero.
        let out = bash(&commits_ahead_script(wt.to_str().unwrap(), &"f".repeat(40)), tmp.path());
        assert_eq!(parse_commits_ahead(&String::from_utf8_lossy(&out.stdout)).unwrap(), None);
    }

    #[test]
    fn target_tip_is_none_without_a_clone_or_a_branch() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = bash(&target_tip_script(tmp.path().join("no-clone").to_str().unwrap(), "feat"), tmp.path());
        assert_eq!(parse_target_tip(&String::from_utf8_lossy(&out.stdout)).unwrap(), None);
        let clone = tmp.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q", "-b", "main"]);
        std::fs::write(clone.join("a"), "a").unwrap();
        git(&clone, &["add", "a"]);
        git(&clone, &["commit", "-q", "-m", "a"]);
        let out = bash(&target_tip_script(clone.to_str().unwrap(), "feat"), tmp.path());
        assert_eq!(parse_target_tip(&String::from_utf8_lossy(&out.stdout)).unwrap(), None);
        let out = bash(&target_tip_script(clone.to_str().unwrap(), "main"), tmp.path());
        assert!(parse_target_tip(&String::from_utf8_lossy(&out.stdout)).unwrap().is_some());
    }
```

`the_probe_writes_nothing` asserts the index is **byte-identical**, which is
only true because the probe runs git with `GIT_OPTIONAL_LOCKS=0` (Step 3).
Confirm the assertion is real: temporarily drop that variable from the script,
watch the test fail, then restore it — and say in your report that you did.

- [ ] **Step 2: Watch them fail** — a compile error (the module is empty) is the
RED. Paste it.

- [ ] **Step 3: Implement the scripts**

Each script's first line is its marker, exactly: `# cf-probe:target`,
`# cf-probe:tip`, `# cf-probe:ahead`. Task 3's `FakeSsh` rules match on these
strings, so they are part of this file's contract.

Each script: `set +e`; `quote` every interpolated value; guard with the same
`cd -- "$x" 2>/dev/null` idiom `carry.rs` uses; print `OUT_MARKER` before the
payload; exit 0 for every *answer* (absent, no clone, unknown tip are answers,
not failures) and non-zero only when the host could not be asked. Use
`STATUS_PORCELAIN` (which already pins `core.quotePath`) so a path is spelled
exactly as the move spells it. Prefix every git invocation with
**`GIT_OPTIONAL_LOCKS=0`**: without it `git status` refreshes and rewrites the
index's stat cache, which is a write — small, but a probe whose whole promise
is "nothing here writes" must keep it literally. Do not run `git fetch`, `git
worktree`, `mkdir`, or anything else that writes. The file's module doc says in
its first line that nothing in it writes, and names `GIT_OPTIONAL_LOCKS=0` as
the reason that is true of `git status`.

- [ ] **Step 4: Full suite, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/probe.rs crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): read-only probes for what the target already holds"
```

---
### Task 3: `preview()` — the dry run itself

**Files:**
- Create: `crates/fleet-core/src/service/move_session/preview.rs` — the types and
  `preview()`.
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `pub mod
  preview;`, one behaviour-preserving extraction (`target_paths`, below), and the
  tests (they need `mod.rs`'s private fixtures, as 3d's seam tests did).

**Interfaces:**
- Consumes: `gather`, `Gathered`, `Snapshot` (Task 1); `probe::*` (Task 2);
  `carry::ignored_list_script`, `carry::parse_ignored_list` (or whatever
  `carry_ignored` uses to parse — mirror it), `carry::select_ignored`,
  `claude_state::session_list_script` + `parse_file_list`,
  `claude_state::memory_list_script` + `parse_memory_list`.
- Produces, for Task 4:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePreview {
    pub session_id: i64,
    pub from_host: String,
    pub to_host: String,
    pub branch: String,
    pub source_cwd: String,
    /// Commits the source has that origin lacks (`SourceState.ahead`); `None`
    /// when git could not say. Always available — unlike `commits_ahead`.
    pub unpushed_commits: Option<u32>,
    /// Commits the TARGET's clone lacks. `None` when it has no clone yet, or
    /// when its branch tip is a commit the source has never seen — then any
    /// number would be a guess. Not zero, which would read as "up to date".
    pub commits_ahead: Option<u32>,
    /// Exactly the rows the carry would replay.
    pub dirty: Vec<DirtyFile>,
    /// `carry::select_ignored`'s split under the move's own caps.
    pub ignored_carried: Vec<carry::IgnoredEntry>,
    pub ignored_left_behind: Vec<carry::LeftBehind>,
    pub transcript_bytes: u64,
    /// Present on the source. The move applies its own size caps to these, so
    /// a very large session directory may not all travel — the result view
    /// says what actually did.
    pub session_state_files: u32,
    pub session_state_bytes: u64,
    pub memory_files: u32,
    pub memory_bytes: u64,
    /// The path the move would aim at (correction 3).
    pub target_path: String,
    pub target: TargetState,
    /// What this preview cannot tell you, in words a person can read.
    pub unknowns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TargetState {
    Absent,
    Clean { head: String },
    /// Deliberately not classified as 3d's `ours` / `theirs`: that verdict
    /// needs `refs/fleet/transfer/<id>/*`, which a dry run never creates.
    Dirty { head: String, entries: Vec<DirtyFile> },
}

/// A read-only run of the move's own opening checks, plus the probes the move
/// does not need. A refusal is `Err` with the code and message the real move
/// would return — the same value, not a description of it.
pub(super) async fn preview(
    args: &MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
) -> Result<MovePreview, IpcError>;
```

No `#[serde(default)]` anywhere (wire rule). `DirtyFile`, `IgnoredEntry` and
`LeftBehind` already derive `Serialize + Deserialize`; check, and add the derive
to any that does not rather than working around it.

- [ ] **Step 1: Extract `target_paths`, behaviour-preserving**

The move's step 3 derives `(project_root, cwd_hint)` — from the snapshot for a
`local` target, from `ssh.remote_home` plus `remote_project_path` otherwise —
immediately before `carry::seed_script`, the first write. Extract exactly that
into:

```rust
/// Where the move would put the target's clone and worktree. Read-only: a
/// `remote_home` lookup and path arithmetic, nothing that creates either.
pub(super) async fn target_paths(
    snap: &Snapshot,
    target: &str,
    ssh: &dyn SshExec,
) -> Result<(String, String), IpcError>;
```

and have the move call it. Its error mapping (`before_target("resolving the
target $HOME", e)`) stays at the move's call site. Run
`cargo test -p fleet-core --lib move_session::` before and after; same result,
no test edited.

- [ ] **Step 2: Write the failing tests**

In `mod.rs`'s test module. Reuse `recorded_fixture`, `dirty_unpushed_carry`,
`FakeHooks`, `args`, `fast`, `out`, `scripts_with`, `progress_of`.

```rust
    /// Every script marker a dry run must never send: each one writes.
    const WRITING_MARKERS: &[&str] = &[
        "# cf-carry:seed",
        "# cf-carry:snapshot",
        "# cf-carry:chunk",
        "# cf-carry:fetch",
        "# cf-carry:apply",
        "# cf-carry:recover",
        "# cf-carry:ignored-pack",
        "# cf-move:prep",
        "# cf-move:prefetch",
    ];

    #[tokio::test]
    async fn a_dry_run_writes_nothing_on_either_host() {
        let (f, bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        preview(&args(&f, false), &f.store, &f.fake, &hooks)
            .await
            .expect("a preview");
        for host in ["alpha", "beta"] {
            for m in WRITING_MARKERS {
                assert!(
                    scripts_with(&f, host, m).is_empty(),
                    "a dry run sent {m} to {host}"
                );
            }
        }
        // No upload of any kind (the transcript copy is an upload).
        assert!(
            f.fake.calls().iter().all(|c| !c.is_upload()),
            "a dry run uploaded something"
        );
        assert!(hooks.started().is_empty(), "a dry run started a target session");
        assert!(!hooks.killed_any(), "a dry run killed a session");
        assert!(!hooks.ensured_any(), "a dry run created or repaired a workspace");
        assert!(progress_of(&f, &bus).is_empty(), "a dry run emitted move:progress");
        // Nor a timeline row: a preview in the durable record 3d leans on would
        // be noise, and would read as a move that happened.
        assert!(
            events(&f, f.source_id).is_empty(),
            "a dry run recorded a timeline event"
        );
    }
```

Use whatever `ssh_fake::Call` already exposes to detect an upload, and whatever
`FakeHooks` already records for `ensure_target_workspace` / `start_target` /
`kill_tmux_session`. If one of `is_upload`, `started`, `ensured_any` does not
exist, add the smallest recorder that makes the assertion true — and confirm the
assertion can fail by making `preview()` briefly call the thing it forbids.

```rust
    /// The preview cannot drift from the move: run both over ONE fixture and
    /// compare, rather than asserting two hand-written expectations that could
    /// drift together.
    #[tokio::test]
    async fn the_preview_reports_exactly_what_the_move_carries() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let p = preview(&args(&f, false), &f.store, &f.fake, &hooks)
            .await
            .expect("a preview");
        let rep = run(&f, &hooks, false).await.expect("the real move");
        assert_eq!(p.dirty, rep.carried.dirty_entries);
        assert_eq!(p.ignored_carried, rep.carried.ignored_carried);
        assert_eq!(p.ignored_left_behind, rep.carried.ignored_left_behind);
    }

    #[tokio::test]
    async fn a_refusal_is_the_error_the_move_would_return() {
        // A mid-operation source: gather() refuses before any carry step.
        let (f, _bus) = recorded_fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection_midop("", "", "0", "merge")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let from_preview = preview(&args(&f, false), &f.store, &f.fake, &hooks)
            .await
            .unwrap_err();
        let from_move = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(from_preview.code, from_move.code);
        assert_eq!(from_preview.message, from_move.message);
    }

    // Spec test 3 asked for every store-level refusal class. Under correction 4
    // that holds by construction — the preview's refusal IS `gather()`'s error,
    // the same value the move returns — so one representative (above) plus the
    // alias-validation test (Step 4) pins the mechanism rather than re-testing
    // each refusal a second time.

    #[tokio::test]
    async fn commits_ahead_is_unknown_without_a_target_clone() {
        let (f, _bus) = recorded_fixture();
        // The target has no clone: its tip lookup answers nothing.
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-probe:tip"),
            Reply::ok(&out("")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let p = preview(&args(&f, false), &f.store, &f.fake, &hooks)
            .await
            .expect("a preview");
        assert_eq!(p.commits_ahead, None, "unknown, not zero");
    }

    #[tokio::test]
    async fn an_absent_target_worktree_is_reported_as_absent() {
        let (f, _bus) = recorded_fixture();
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-probe:target"),
            Reply::ok(&out("absent\n")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let p = preview(&args(&f, false), &f.store, &f.fake, &hooks)
            .await
            .expect("a preview");
        assert!(matches!(p.target, TargetState::Absent), "{:?}", p.target);
        assert!(!p.target_path.is_empty(), "it says which path it looked at");
    }

    #[tokio::test]
    async fn the_bundle_size_is_named_as_unknown_and_ignored_flags_are_said() {
        let (f, _bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.strict = true;
        a.clean_target = true;
        let p = preview(&a, &f.store, &f.fake, &hooks).await.expect("a preview");
        let all = p.unknowns.join(" | ");
        assert!(all.contains("bundle"), "{all}");
        assert!(all.contains("strict"), "{all}");
        assert!(all.contains("clean_target"), "{all}");
    }
```

The markers `# cf-probe:target`, `# cf-probe:tip` and `# cf-probe:ahead` are
fixed by Task 2 — confirm them in `probe.rs` before relying on them.
`inspection_midop` is the fixture helper already in the test module (search for
it); if its argument order differs, follow the real signature. The two
`recorded_fixture()` tests that compare a preview with a real move must use
**one** fixture for both calls — that is the point of them.

- [ ] **Step 3: Watch them fail** — a compile error (`preview` does not exist)
is the RED. Paste it.

- [ ] **Step 4: Implement `preview()`**

In order, and **mirroring the move's own call sites** — read them first
(`carry_ignored`'s `ignored_list_script(worktree)` near `mod.rs:1015`;
`session_list_script(src_project_dir, id)` near `mod.rs:1134`;
`memory_list_script(src_worktree, Some(src_worktree))` near `mod.rs:1272`):

0. `crate::validate::host_alias(&args.target_host_alias)?;` — the move validates
   the alias *before* taking its claim, which is outside `gather()`, so a dry
   run must do it itself or it would accept an alias the move refuses. Add a test
   for it: a malformed `target_host_alias` gets the same error from `preview()`
   as from the move.
1. `let g = gather(args, store, ssh, hooks, None).await?;` — a refusal returns
   here, as the move's own error.
2. On the source: `ignored_list_script(&g.state.worktree)`, parsed the way
   `carry_ignored` parses it, then `select_ignored(listed, g.snap.ignored_entry_kb,
   g.snap.ignored_total_kb)`. A listing failure is **not** a refusal — the move
   treats the ignored carry as warn-only — so record it in `unknowns` and carry
   on with empty lists.
3. On the source: the session-state listing of the transcript's own directory
   (the parent of `g.located.path`) and the memory listing of the worktree, as
   counts and byte totals. Also warn-only: a failure goes to `unknowns`.
4. `let (project_root, cwd_hint) = target_paths(&g.snap, &g.target, ssh).await?;`
5. On the target: `target_probe_script(&cwd_hint)` → `TargetState`.
6. On the target: `target_tip_script(&project_root, &g.snap.branch)`; when it
   yields a tip, on the source `commits_ahead_script(&g.state.worktree, &tip)`.
7. `unknowns`: always the bundle size — say that it is decided by snapshotting
   and so cannot be known without writing, and that it is what `E_MOVE_TOO_LARGE`
   depends on. Add a line for `strict` and for `clean_target` when set, saying a
   dry run ignores them. Add a line when the target is `Absent`, saying its state
   will only exist once the move creates it.

`unpushed_commits` is `u32::try_from(g.state.ahead).ok()` — `-1` becomes `None`.

- [ ] **Step 5: Full suite, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo clippy -p fleet-core --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add crates/fleet-core/src/service/move_session/preview.rs crates/fleet-core/src/service/move_session/mod.rs
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): a read-only preview built from the move's own checks"
```

---
### Task 4: The wire — `dry_run`, `MoveOutcome`, and every caller

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `dry_run` on
  `MoveSessionArgs`, the `MoveOutcome` enum, the branch, the return type, and
  the `run`/`run_with` test helpers.
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` — `dry_run` on
  `MoveSessionParams`, mapped in `into_args`.
- Modify: `crates/fleet-core/src/mcp/tools/lifecycle.rs` — the handler skips
  `confirm_gate` for a dry run; the audit line names it.
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` — extend 3d's source-pin
  test; a confirm-gate test.
- Modify: `src-tauri/src/commands/move_session.rs` — returns `MoveOutcome`.
- Modify: `src-tauri/src/backend/tests_routing.rs` — non-default `dry_run` and
  the tagged response payload.
- Regenerate: `docs/control-api-reference.md`.

**One task, many files, because it is one change:** the return type of a routed
command. Split across tasks it would leave the build broken between them.

**Interfaces:**
- Consumes: `preview::{preview, MovePreview}` (Task 3).
- Produces, for Task 5 — the wire shape:

```rust
/// What `move_session` did. Internally tagged, so a `Moved` outcome is the
/// `MoveReport` object with one extra key, `"kind": "moved"` — every field a
/// caller already reads stays where it was.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MoveOutcome {
    Moved(MoveReport),
    Preview(MovePreview),
}
```

Because the tag is internal, `MoveReport`'s own fields stay at the top level of
the JSON; its nested `target: SessionRow` keeps its own `kind` field (the
session kind) one level down, so there is no collision — but prove it (Step 1).

- [ ] **Step 1: Write the failing tests**

In `mod.rs`'s test module:

```rust
    #[tokio::test]
    async fn a_moved_outcome_keeps_every_report_field_at_the_top_level() {
        let (f, _bus) = recorded_fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        // A real report from a real (faked) move, never one built field by field.
        let rep = run(&f, &hooks, false).await.expect("the fixture moves");
        let v = serde_json::to_value(MoveOutcome::Moved(rep.clone())).unwrap();
        assert_eq!(v["kind"], "moved");
        assert_eq!(v["target_session_id"], rep.target_session_id);
        // The session row's own `kind` sits under `target`, untouched by the tag.
        assert_eq!(v["target"]["kind"], rep.target.kind);
        let back: MoveOutcome = serde_json::from_value(v).unwrap();
        assert!(matches!(back, MoveOutcome::Moved(_)));
    }

    #[tokio::test]
    async fn dry_run_returns_a_preview_and_the_real_move_is_unchanged() {
        let (f, _bus) = recorded_fixture();
        dirty_unpushed_carry(&f);
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.dry_run = true;
        let out = move_session_with(a, &f.store, &f.fake, &hooks, fast())
            .await
            .expect("a dry run");
        assert!(matches!(out, MoveOutcome::Preview(_)), "{out:?}");
        // The same session can still be moved for real afterwards — the dry run
        // took no claim and left nothing behind.
        let rep = run(&f, &hooks, false).await.expect("the real move");
        assert!(rep.source_killed);
    }
```

The first test takes its `MoveReport` from a real `run` rather than building one
field by field, which would drift from the real type. `SessionRow.kind` is a
`String`, so the `v["target"]["kind"]` comparison works as written; if the
field's type differs, compare through `serde_json::to_value` on both sides.

In `crates/fleet-core/src/mcp/tools/tests.rs`: extend 3d's
`move_session_params_carry_clean_target_into_the_service_args` (or add a sibling)
so a `{"dry_run": true}` arriving at the tool reaches `MoveSessionArgs.dry_run`,
and extend its source-pin assertion so no args literal in `lifecycle.rs` may set
`dry_run:` either. Add a confirm-gate test: with `mcp.confirm_destructive` on, a
dry-run call does **not** answer `E_CONFIRM_REQUIRED`, and the same call without
`dry_run` still does — follow how the existing confirm-gate tests in that file
turn the setting on.

In `src-tauri/src/backend/tests_routing.rs`: `move_session`'s args row gains a
non-default `dry_run: true`, and its expected response payload gains
`"kind":"moved"`.

- [ ] **Step 2: Watch them fail** — paste the output. Expect compile errors
(`dry_run`, `MoveOutcome` do not exist) and the routing payload mismatch.

- [ ] **Step 3: Implement**

`MoveSessionArgs`:

```rust
    /// Report what this move would do and change nothing (see `preview`).
    /// Default false.
    #[serde(default)]
    pub dry_run: bool,
```

`move_session_with` — the branch sits **before** `move_session_steps`, which
builds the progress emitter and the carry cleanup a dry run needs neither of:

```rust
pub async fn move_session_with(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
) -> Result<MoveOutcome, IpcError> {
    if args.dry_run {
        return preview::preview(&args, store, ssh, hooks)
            .await
            .map(MoveOutcome::Preview);
    }
    // …unchanged, ending in:
    //     .map(MoveOutcome::Moved)
}
```

and `move_session`'s public signature likewise. Fix every construction site of
`MoveSessionArgs` the compiler names with `dry_run: false`.

The test helpers `run` and `run_with` keep returning `MoveReport`, by
unwrapping `MoveOutcome::Moved` inside the helper and panicking on `Preview` —
so **no existing test body changes**. That is the acceptance criterion for the
engine half: the existing tests pass unedited.

`MoveSessionParams` gains, mapped in `into_args`:

```rust
    /// Report what would travel; change nothing.
    #[serde(default)]
    pub dry_run: bool,
```

Keep that doc comment terse — it is served to every MCP client and the budget
has 169 bytes of slack.

The handler reads `p.dry_run` **before** `p.into_args(...)` consumes `p`, puts it
in the audit line, and wraps the confirm gate:

```rust
        if !dry_run {
            self.confirm_gate(/* …unchanged… */)?;
        }
```

The access checks (`resolve_target_row`, `require_move_hosts`) stay
unconditional — a preview reveals the source's file list and the target's
state, so a caller must be allowed on both hosts to see one. The skip is safe
only because `into_args` maps the same `dry_run` the handler branched on; say so
in a one-line comment there.

The Tauri command and its `routed` helper return `MoveOutcome`; `hub.route`'s
type parameter follows.

- [ ] **Step 4: Regenerate the reference**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

```bash
cargo test -p fleet-core reference_is_current
```

The second must pass. Never hand-edit the generated file.

- [ ] **Step 5: Full suites, the budget, clippy, fmt, commit**

```bash
cargo test -p fleet-core
```

```bash
cargo test -p claude-fleet
```

```bash
cargo test -p fleet-core the_served_definition_budget_stays_bounded -- --nocapture
```

Record the printed master byte count in your report. It must be at or under
57,000 without the constant changing.

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add -A
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(move): dry_run reaches the engine from the desktop, the tool and the hub"
```

---
### Task 5: `moveSession.ts` — the outcome type, and two narrowing wrappers

**Files:**
- Modify: `src/lib/moveSession.ts`
- Create: `src/lib/moveSession.test.ts` (none exists yet)

**Needs Task 4** for the wire shape. **`moves.ts` needs no change**, and that is
the point of this design: `moveSession()` narrows to `Moved` and a new
`previewMove()` narrows to `Preview`, so a preview **cannot reach the run store
by construction** — a stronger guarantee than `moves.ts` checking `kind`, which
is what spec §7 asked for. Its two call sites (`moves.ts:238`, `:346`) keep
calling `moveSession()` and keep receiving a `MoveReport`.

**Interfaces:**
- Produces, for Tasks 6 and 7:

```ts
export type TargetState =
  | { state: 'absent' }
  | { state: 'clean'; head: string }
  | { state: 'dirty'; head: string; entries: { status: string; path: string }[] };

/** Mirrors `service::move_session::preview::MovePreview`. */
export interface MovePreview {
  session_id: number;
  from_host: string;
  to_host: string;
  branch: string;
  source_cwd: string;
  unpushed_commits: number | null;
  commits_ahead: number | null;
  dirty: { status: string; path: string }[];
  ignored_carried: { path: string; bytes: number }[];
  ignored_left_behind: CarryReport['ignored_left_behind'];
  transcript_bytes: number;
  session_state_files: number;
  session_state_bytes: number;
  memory_files: number;
  memory_bytes: number;
  target_path: string;
  target: TargetState;
  unknowns: string[];
}

/** What `move_session` answers. Internally tagged: a `moved` outcome is the
 *  `MoveReport` with one extra key. */
export type MoveOutcome = ({ kind: 'moved' } & MoveReport) | ({ kind: 'preview' } & MovePreview);

/** A dry run: what this move would do, changing nothing. `Err` is the refusal
 *  the real move would return — same code, same message. */
export function previewMove(sessionId: number, targetHostAlias: string): Promise<Result<MovePreview>>;
```

- [ ] **Step 1: Write the failing tests** in `src/lib/moveSession.test.ts`,
mocking `invokeCmd` the way `moves.test.ts` does (read its top first).

```ts
  it('previewMove sends dry_run and returns the preview', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    const r = await previewMove(7, 'beta');
    expect(invoked.mock.calls[0][0]).toBe('move_session');
    expect(invoked.mock.calls[0][1].args).toMatchObject({
      session_id: 7,
      target_host_alias: 'beta',
      dry_run: true,
    });
    expect(r.ok && r.value.to_host).toBe('beta');
  });

  it('previewMove refuses to treat a real move as a preview', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    const r = await previewMove(7, 'beta');
    expect(r.ok).toBe(false);
  });

  it('previewMove passes the refusal through untouched', async () => {
    invoked.mockResolvedValueOnce(err('E_MOVE_MIDOP', 'mid-merge'));
    const r = await previewMove(7, 'beta');
    expect(!r.ok && r.error.code).toBe('E_MOVE_MIDOP');
  });

  it('moveSession never sends dry_run and refuses a preview answer', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'preview', ...previewFixture }));
    const r = await moveSession(7, 'beta');
    expect(invoked.mock.calls[0][1].args.dry_run).toBe(false);
    expect(r.ok).toBe(false);
  });

  it('moveSession still hands back the report of a real move', async () => {
    invoked.mockResolvedValueOnce(ok({ kind: 'moved', ...reportFixture }));
    const r = await moveSession(7, 'beta');
    expect(r.ok && r.value.target_session_id).toBe(reportFixture.target_session_id);
  });
```

`previewFixture` and `reportFixture` are yours to write as complete objects of
their types, so a missing field is a type error rather than a silent `undefined`.
The narrowing errors must use an existing code — `E_PARSE` is the honest one for
"the backend answered something of the wrong shape"; add no new `E_*` code.

- [ ] **Step 2: Watch them fail** — paste it.

- [ ] **Step 3: Implement.** `moveSession` sends `dry_run: false` explicitly and
narrows; `mergeSession(r.value.target)` stays where it is, applied only to a
`moved` outcome. `previewMove` sends `dry_run: true`, `keep_source: false`,
`strict: false`, `clean_target: false`, and narrows.

- [ ] **Step 4: Full suite, commit**

```bash
npx vitest run
```

```bash
npx svelte-check
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/moveSession.ts src/lib/moveSession.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): a preview and a move each arrive as exactly what they are"
```

---

### Task 6: `preflight.ts` — the newest preview per session and host

**Files:**
- Create: `src/lib/preflight.ts`
- Create: `src/lib/preflight.test.ts`

**Needs Task 5.**

**Interfaces:**
- Consumes: `previewMove`, `MovePreview` (Task 5).
- Produces, for Task 7:

```ts
export const PREFLIGHT_DEBOUNCE_MS = 250;
/** Older than this, a result shows its age rather than presenting as current. */
export const PREFLIGHT_STALE_MS = 30_000;

export interface PreflightEntry {
  sessionId: number;
  toHost: string;
  status: 'loading' | 'ready' | 'refused';
  preview: MovePreview | null;
  /** The refusal the real move would return. */
  error: IpcError | null;
  /** When the answer arrived; null while loading. */
  at: number | null;
}

export const preflights: Readable<Map<string, PreflightEntry>>;
/** Ask for a preview, debounced per session: a burst of requests for one
 *  session fires only the last. */
export function requestPreflight(sessionId: number, toHost: string): void;
export function preflightFor(
  map: Map<string, PreflightEntry>,
  sessionId: number,
  toHost: string,
): PreflightEntry | undefined;
/** Milliseconds since the answer, or null while loading. */
export function preflightAge(entry: PreflightEntry, now: number): number | null;
export function resetPreflightsForTest(): void;
```

**Two things that are the whole difficulty of this file:**

1. **The debounce is per session**, so flicking through five hosts fires one
   call — for the host you stopped on.
2. **A late answer must never overwrite a newer one.** Keying the map by
   `(sessionId, toHost)` already stops host A's slow answer landing under host
   B. But the same key can be requested twice (reopen the sheet, pick the same
   host), and the first request's answer can arrive second. Give each key a
   request sequence number and drop an answer whose sequence is not the latest
   for its key. This is the same shape as 3d's straggler bug in `moves.ts` —
   two racing streams, where the older one must not win — and it gets a test.

- [ ] **Step 1: Write the failing tests**, with `vi.useFakeTimers()` and
`previewMove` mocked (`vi.mock('./moveSession', …)` spreading the real module).

```ts
  it('fires once for a burst of target changes, for the last target', async () => {
    requestPreflight(7, 'a');
    requestPreflight(7, 'b');
    requestPreflight(7, 'c');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(previewMove).toHaveBeenCalledTimes(1);
    expect(previewMove).toHaveBeenCalledWith(7, 'c');
  });

  it('debounces per session, not globally', async () => {
    requestPreflight(7, 'a');
    requestPreflight(8, 'a');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(previewMove).toHaveBeenCalledTimes(2);
  });

  it('shows loading, then the answer with the time it arrived', async () => {
    let resolve!: (v: Result<MovePreview>) => void;
    vi.mocked(previewMove).mockReturnValueOnce(new Promise((r) => (resolve = r)));
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(preflightFor(get(preflights), 7, 'beta')?.status).toBe('loading');
    resolve({ ok: true, value: previewFixture });
    await Promise.resolve();
    const e = preflightFor(get(preflights), 7, 'beta')!;
    expect(e.status).toBe('ready');
    expect(e.at).not.toBeNull();
  });

  it('records a refusal as refused, keeping the error the move would return', async () => {
    vi.mocked(previewMove).mockResolvedValueOnce({
      ok: false,
      error: { code: 'E_MOVE_MIDOP', message: 'mid-merge', details: null },
    });
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    const e = preflightFor(get(preflights), 7, 'beta')!;
    expect(e.status).toBe('refused');
    expect(e.error?.code).toBe('E_MOVE_MIDOP');
  });

  it('never lets an older answer overwrite a newer one for the same key', async () => {
    let first!: (v: Result<MovePreview>) => void;
    vi.mocked(previewMove)
      .mockReturnValueOnce(new Promise((r) => (first = r)))
      .mockResolvedValueOnce({ ok: true, value: { ...previewFixture, transcript_bytes: 2 } });
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    // The second answer is in; now the first, older one arrives late.
    first({ ok: true, value: { ...previewFixture, transcript_bytes: 1 } });
    await Promise.resolve();
    expect(preflightFor(get(preflights), 7, 'beta')?.preview?.transcript_bytes).toBe(2);
  });

  it('reports age from the moment the answer arrived', () => {
    const e = { at: 1_000 } as PreflightEntry;
    expect(preflightAge(e, 31_000)).toBe(30_000);
    expect(preflightAge({ at: null } as PreflightEntry, 5)).toBeNull();
  });
```

Check the late-answer test can fail: remove the sequence check briefly and watch
it go red, then restore it — say so in the report.

- [ ] **Step 2: Watch them fail** — paste it.
- [ ] **Step 3: Implement**, with Svelte's `writable` like `moves.ts` does, and a
comment above the sequence check naming the race it closes.
- [ ] **Step 4: Full suite, commit**

```bash
npx vitest run
```

```bash
npx svelte-check
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/preflight.ts src/lib/preflight.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): the newest preview per session and host, debounced, never overwritten by a late one"
```

---

### Task 7: The setup view shows what would travel

**Files:**
- Modify: `src/lib/TransferSheet.svelte` — the setup view (the `<select
  data-testid="move-target">` block and its buttons).
- Test: `src/lib/TransferSheet.test.ts`

**Needs Tasks 5 and 6.**

**Interfaces:**
- Consumes: `requestPreflight`, `preflights`, `preflightFor`, `preflightAge`,
  `PREFLIGHT_STALE_MS` (Task 6); `MovePreview`, `TargetState` (Task 5);
  `describeMoveError` (existing).

**The one rule this task must not break: Transfer is never gated by the
preflight.** Its `disabled` stays exactly `!target || blocked !== null`. Not
while a preview is loading, not when one was refused, not when one is stale.
The engine re-checks everything authoritatively; a preview that is wrong or old
must never be the thing that stops you. There is a test for each of those three
states.

- [ ] **Step 1: Write the failing tests**

```ts
  it('asks for a preview for the selected target', async () => {
    renderSetup({ targets: ['beta', 'gamma'] });
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(requestPreflight).toHaveBeenLastCalledWith(7, 'beta');
  });

  it('asks again when the target changes', async () => {
    const { getByTestId } = renderSetup({ targets: ['beta', 'gamma'] });
    await fireEvent.change(getByTestId('move-target'), { target: { value: 'gamma' } });
    expect(requestPreflight).toHaveBeenLastCalledWith(7, 'gamma');
  });

  it.each(['loading', 'refused', 'stale'])('keeps Transfer enabled while %s', async (state) => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: state });
    expect((getByTestId('move-transfer') as HTMLButtonElement).disabled).toBe(false);
  });

  it('renders what would travel and what would be left behind, with reasons', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'ready' });
    const view = getByTestId('transfer-preflight').textContent ?? '';
    expect(view).toContain('src/lib.rs'); // a dirty entry
    expect(view).toContain('.env'); // an ignored file carried
    expect(view).toContain('node_modules'); // one left behind…
    expect(view).toMatch(/too large|deny/i); // …and why
  });

  it('shows a refusal in the words the failure view would use', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'refused' });
    expect(getByTestId('transfer-preflight-refusal').textContent).toContain(
      describeMoveError(refusalFixture, 'failed', 'beta', null).what,
    );
  });

  it('names what it cannot know', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'ready' });
    expect(getByTestId('transfer-preflight-unknowns').textContent).toMatch(/bundle/i);
  });

  it('shows the age of a stale preview', () => {
    const { getByTestId } = renderSetup({ targets: ['beta'], preflight: 'stale' });
    expect(getByTestId('transfer-preflight-age').textContent).toMatch(/\d+\s*s/);
  });
```

`renderSetup` is yours: mount the sheet on the setup view (no run for the
session), seed `hosts` and `sessions` the way the existing setup-view tests do,
mock `./preflight`'s `requestPreflight`, and seed `preflights` with an entry
for the requested state. Find the Transfer button's real `data-testid` in the
file; if it has none, add `move-transfer`.

- [ ] **Step 2: Watch them fail** — paste it.

- [ ] **Step 3: Implement**

- An `$effect` on `id` and `target` calls `requestPreflight(id, target)` when
  both are set and the sheet is on its setup view (no run).
- A `$derived` picks `preflightFor($preflights, id, target)`.
- Render, under `data-testid="transfer-preflight"`: a loading line; or the
  refusal via `describeMoveError(entry.error, 'failed', target, null).what` under
  `transfer-preflight-refusal` — the same sentence the failure view would show,
  so a user never meets the same refusal in two wordings; or the preview —
  unpushed commits and (when known) what the target lacks, the dirty entries,
  the ignored files carried with sizes, those left behind with their reason, the
  transcript size, the session-state and memory counts, the target's state and
  the path that was looked at, and `unknowns` under
  `transfer-preflight-unknowns`.
- Age: a small ticking `now` while the setup view is open (an `$effect` with an
  interval, cleared on teardown); when `preflightAge(entry, now) >
  PREFLIGHT_STALE_MS`, show it under `transfer-preflight-age`.
- Transfer's `disabled` is unchanged.
- Use the file's existing classes and the voice of its existing copy; no new
  colour literals (the app has no theme variables for danger or warning, a
  recorded debt).

- [ ] **Step 4: Full suite, commit**

```bash
npx vitest run
```

```bash
npx svelte-check
```

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 add src/lib/TransferSheet.svelte src/lib/TransferSheet.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/transfer-roadmap-3d-b38d59 commit -m "feat(ui): the Transfer sheet shows what would travel before you press it"
```

---

## Before the PR

1. `git fetch origin` and compare — `main` moves ~50 commits a day. Merge
   `origin/main` **locally** (never squash) and re-run every suite on the merged
   tree: a clean text merge has hidden a semantic break here twice.
2. The whole-branch review on the most capable model, with the controller's own
   worries listed — then ONE fix wave, one scoped re-review, residuals
   adjudicated. On 3d it found two Criticals that twelve task reviews missed.
3. Ask before any `carry_e2e` run against a host. Never run a dev build.
