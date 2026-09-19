# Move Carry Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `move_session` carries a session's work as it is — uncommitted, staged, untracked, unpushed, plus small git-ignored files — to a target host that may have no clone and no route to origin.

**Architecture:** A new pure module `service/move_session/carry.rs` builds shell scripts and parses their output (snapshot via a temporary index, thin `git bundle`, chunked relay through the orchestrator, `read-tree` replay, ignored-file tar). `service/move_session/mod.rs` (today's file, moved) calls them between the transcript read and the target start, verifies the target's porcelain equals the source's, and always cleans the private refs and temp dirs up. `strict: true` keeps the old refusals.

**Tech Stack:** Rust (fleet-core, tokio, serde), bash + git on the hosts, `FakeSsh` for flow tests, real `git`/`bash` for the round-trip test, TypeScript for the frontend wrapper types.

**Spec:** `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md` — read it before starting any task.

## Global Constraints

- Every value interpolated into a shell script goes through `crate::shell::quote`. No other quoting helper exists or may be added.
- Never hold the `Store` mutex guard across an `.await`.
- Every carry step runs **before** the target tmux session starts; a failure returns its own error and leaves the source untouched. `E_MOVE_PARTIAL` semantics do not change.
- The source working tree, the source index and every source ref outside `refs/fleet/transfer/` are never modified.
- Script markers are `# cf-carry:<name>` (first line), matching the existing `# cf-move:<name>` convention `FakeSsh` rules key on.
- Settings (exact keys/defaults): `move.max_bundle_mb` = 500, `move.ignored_entry_kb` = 1024, `move.ignored_total_mb` = 20.
- Error codes (exact): `E_MOVE_MIDOP`, `E_MOVE_TARGET_DIRTY`, `E_MOVE_CARRY`; `E_MOVE_TOO_LARGE` reused with `details.payload`.
- After **every** task run the whole backend suite, unfiltered and unpiped: `cargo test -p fleet-core`. Do not judge a run through `| tail`/`| grep`. Frontend: `npx vitest run` and `npx svelte-check` (the `pnpm` script aliases are not on PATH here).
- Git: work only in this worktree; use `git -C <worktree path>` for every git command; never `pull`/`push`/`rebase`/`checkout`/`stash`. Before each commit run `git -C <wt> branch --show-current` and confirm it is `feature/fleet-transfer-workflow-81ece7`. Never `git add -A` — `.reticle/` is untracked and not ours. No attribution lines in commit messages.
- If `cargo` fails with ENOSPC or a missing `/Volumes/CargoSD`, set `CARGO_TARGET_DIR` to a directory under the session scratchpad and retry; that is an environment gap, not a code error.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/move_session/mod.rs` | Today's `move_session.rs`, moved. Step flow, hooks, transcript copy, relay helpers, cleanup wrapper, report. |
| `crates/fleet-core/src/service/move_session/carry.rs` | **New.** Pure: constants, `CarryReport` types, script builders, output parsers, `select_ignored` policy. No I/O, no `async`. |
| `crates/fleet-core/src/ipc_error.rs` | Three new codes. |
| `crates/fleet-core/src/service/settings.rs` | Three new setting specs. |
| `crates/fleet-core/src/mcp/tools/params.rs`, `lifecycle.rs` | `strict` param, tool description. |
| `src/lib/moveSession.ts` | `strict` option + `CarryReport` types. |
| `docs/adr/0002-move-carries-work-as-is.md` | **New.** The ADR. |
| `docs/control-api-reference.md` | Regenerated. |

---

### Task 1: Turn `move_session.rs` into a module directory

**Files:**
- Move: `crates/fleet-core/src/service/move_session.rs` → `crates/fleet-core/src/service/move_session/mod.rs`
- Create: `crates/fleet-core/src/service/move_session/carry.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: module path `crate::service::move_session::carry` (empty for now). Every existing `crate::service::move_session::*` path keeps working.

- [ ] **Step 1: Move the file with git, no content change**

```bash
WT=/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/fleet-transfer-workflow-81ece7
mkdir -p $WT/crates/fleet-core/src/service/move_session
git -C $WT mv crates/fleet-core/src/service/move_session.rs crates/fleet-core/src/service/move_session/mod.rs
```

- [ ] **Step 2: Create the empty carry module**

`crates/fleet-core/src/service/move_session/carry.rs`:

```rust
//! The carry engine of `move_session`: script builders and output parsers
//! that take a worktree's state — unpushed commits, staged, modified and
//! untracked files, small git-ignored files — from the source host to the
//! target without origin. Pure: no I/O, no `async`; `mod.rs` runs the
//! scripts. See `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`.
```

In `mod.rs`, directly after the `//!` module docs and before the `use` lines, add:

```rust
pub mod carry;
```

- [ ] **Step 3: Verify nothing changed**

Run: `cargo test -p fleet-core`
Expected: PASS, same test count as before the move (the `move_session::tests` names are unchanged).

- [ ] **Step 4: Commit**

```bash
git -C $WT branch --show-current   # feature/fleet-transfer-workflow-81ece7
git -C $WT add crates/fleet-core/src/service/move_session
git -C $WT commit -m "refactor(move): move_session.rs becomes a module directory"
```

---

### Task 2: Error codes and settings

**Files:**
- Modify: `crates/fleet-core/src/ipc_error.rs` (after `E_MOVE_PARTIAL`, ~line 145)
- Modify: `crates/fleet-core/src/service/move_session/carry.rs`
- Modify: `crates/fleet-core/src/service/settings.rs` (consts ~line 98, `Spec` table ~line 183, tests ~line 480)

**Interfaces:**
- Produces: `codes::E_MOVE_MIDOP`, `codes::E_MOVE_TARGET_DIRTY`, `codes::E_MOVE_CARRY`; in `carry`: `SETTING_MAX_BUNDLE_MB`, `SETTING_IGNORED_ENTRY_KB`, `SETTING_IGNORED_TOTAL_MB` (all `&str`), `DEFAULT_MAX_BUNDLE_MB: u64 = 500`, `DEFAULT_IGNORED_ENTRY_KB: u64 = 1024`, `DEFAULT_IGNORED_TOTAL_MB: u64 = 20`; in `settings`: `MOVE_MAX_BUNDLE_MB`, `MOVE_IGNORED_ENTRY_KB`, `MOVE_IGNORED_TOTAL_MB`.

- [ ] **Step 1: Write the failing settings test**

In `settings.rs` `mod tests`, next to the existing `MOVE_MAX_TRANSCRIPT_MB` assertions:

```rust
    #[test]
    fn carry_settings_have_specs_defaults_and_bounds() {
        assert_eq!(spec(MOVE_MAX_BUNDLE_MB).unwrap().default, "500");
        assert_eq!(spec(MOVE_IGNORED_ENTRY_KB).unwrap().default, "1024");
        assert_eq!(spec(MOVE_IGNORED_TOTAL_MB).unwrap().default, "20");
        for key in [MOVE_MAX_BUNDLE_MB, MOVE_IGNORED_ENTRY_KB, MOVE_IGNORED_TOTAL_MB] {
            assert!(validate(key, "1").is_ok(), "{key}");
            assert!(validate(key, "0").is_err(), "{key}");
            assert!(validate(key, "abc").is_err(), "{key}");
        }
        assert!(validate(MOVE_MAX_BUNDLE_MB, "4096").is_ok());
        assert!(validate(MOVE_MAX_BUNDLE_MB, "4097").is_err());
        assert_eq!(resolve(MOVE_IGNORED_TOTAL_MB, None), "20");
        assert_eq!(resolve(MOVE_IGNORED_TOTAL_MB, Some("5")), "5");
    }
```

- [ ] **Step 2: Run it — it must fail to compile**

Run: `cargo test -p fleet-core carry_settings`
Expected: FAIL — `cannot find value MOVE_MAX_BUNDLE_MB`.

- [ ] **Step 3: Implement**

`carry.rs`, below the module docs:

```rust
/// `settings` key: largest git bundle (MiB) a move relays.
pub const SETTING_MAX_BUNDLE_MB: &str = "move.max_bundle_mb";
pub const DEFAULT_MAX_BUNDLE_MB: u64 = 500;
/// `settings` key: largest single git-ignored entry (KiB) a move carries.
pub const SETTING_IGNORED_ENTRY_KB: &str = "move.ignored_entry_kb";
pub const DEFAULT_IGNORED_ENTRY_KB: u64 = 1024;
/// `settings` key: total git-ignored payload (MiB) a move carries.
pub const SETTING_IGNORED_TOTAL_MB: &str = "move.ignored_total_mb";
pub const DEFAULT_IGNORED_TOTAL_MB: u64 = 20;
```

`settings.rs`, after `MOVE_MAX_TRANSCRIPT_MB_MAX`:

```rust
/// Largest git bundle (MiB) `move_session` relays (`E_MOVE_TOO_LARGE` above it).
pub const MOVE_MAX_BUNDLE_MB: &str = crate::service::move_session::carry::SETTING_MAX_BUNDLE_MB;
/// Largest single git-ignored entry (KiB) `move_session` carries; bigger ones
/// are reported as left behind.
pub const MOVE_IGNORED_ENTRY_KB: &str = crate::service::move_session::carry::SETTING_IGNORED_ENTRY_KB;
/// Total git-ignored payload (MiB) `move_session` carries.
pub const MOVE_IGNORED_TOTAL_MB: &str = crate::service::move_session::carry::SETTING_IGNORED_TOTAL_MB;
```

In the `Spec` table, after the `MOVE_MAX_TRANSCRIPT_MB` entry:

```rust
    Spec {
        key: MOVE_MAX_BUNDLE_MB,
        default: "500",
        kind: Kind::Int { min: 1, max: 4096 },
    },
    Spec {
        key: MOVE_IGNORED_ENTRY_KB,
        default: "1024",
        kind: Kind::Int { min: 1, max: 1_048_576 },
    },
    Spec {
        key: MOVE_IGNORED_TOTAL_MB,
        default: "20",
        kind: Kind::Int { min: 1, max: 1024 },
    },
```

`ipc_error.rs`, after the `E_MOVE_PARTIAL` constant:

```rust
    /// `move_session`: the source worktree is mid merge / rebase /
    /// cherry-pick / revert / bisect (`details.operation`); finish or abort it.
    pub const E_MOVE_MIDOP: &str = "E_MOVE_MIDOP";
    /// `move_session`: a pre-existing target worktree has uncommitted
    /// changes; the move never overwrites them.
    pub const E_MOVE_TARGET_DIRTY: &str = "E_MOVE_TARGET_DIRTY";
    /// `move_session`: carrying the work failed before the target started
    /// (`details.step`, `details.stderr`); the source is untouched.
    pub const E_MOVE_CARRY: &str = "E_MOVE_CARRY";
```

If `ipc_error.rs` has a test or list that enumerates every code (search the file for `E_MOVE_PARTIAL` a second time), add the three there too. Likewise, if the full suite in Step 4 fails in a test that checks every setting key or error code against a doc or a frontend mirror, the failure names the file — add the new keys/codes there.

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p fleet-core`
Expected: PASS, including `carry_settings_have_specs_defaults_and_bounds`.

- [ ] **Step 5: Commit**

```bash
git -C $WT add crates/fleet-core/src/ipc_error.rs crates/fleet-core/src/service/settings.rs crates/fleet-core/src/service/move_session/carry.rs
git -C $WT commit -m "feat(move): carry error codes and size settings"
```

---

### Task 3: Carry report types and the ignored-file policy

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/carry.rs`

**Interfaces:**
- Consumes: `crate::service::safe_kill::DirtyFile` (`#[derive(Debug, Clone, Serialize)]`, fields `status: String`, `path: String`).
- Produces (all `pub`, in `carry`):
  - `enum TargetSeed { Existing, Cloned, Initialized }` (serde `snake_case`, `Default = Existing`)
  - `enum LeftReason { Denylisted, OverCap, UnsupportedName }` (serde `snake_case`)
  - `struct IgnoredEntry { path: String, bytes: u64 }`
  - `struct LeftBehind { path: String, bytes: Option<u64>, reason: LeftReason }`
  - `struct CarryReport { commits: u32, bundle_bytes: u64, dirty_entries: Vec<DirtyFile>, ignored_carried: Vec<IgnoredEntry>, ignored_left_behind: Vec<LeftBehind>, target_seeded: TargetSeed }` (`Default`)
  - `struct ListedIgnored { path: String, kb: Option<u64>, valid_name: bool }`
  - `struct IgnoredSelection { carry: Vec<IgnoredEntry>, left: Vec<LeftBehind> }`
  - `const DENYLIST: &[&str]`, `const MAX_IGNORED_ENTRIES: usize = 500`
  - `fn parse_ignored_list(stdout: &[u8]) -> Vec<ListedIgnored>`
  - `fn select_ignored(listed: Vec<ListedIgnored>, entry_kb: u64, total_kb: u64) -> IgnoredSelection`

- [ ] **Step 1: Write the failing tests**

At the bottom of `carry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn listed(path: &str, kb: Option<u64>) -> ListedIgnored {
        ListedIgnored { path: path.into(), kb, valid_name: true }
    }

    #[test]
    fn ignored_list_parses_nul_records_and_flags_bad_names() {
        let mut out = b"4\t.env\0-1\tnode_modules/\0".to_vec();
        out.extend_from_slice(b"1\tbad\xff.txt\0");
        out.extend_from_slice(b"garbage-without-tab\0");
        let got = parse_ignored_list(&out);
        assert_eq!(got.len(), 3, "the record without a tab is dropped");
        assert_eq!(got[0].path, ".env");
        assert_eq!(got[0].kb, Some(4));
        assert_eq!(got[1].path, "node_modules/");
        assert_eq!(got[1].kb, None, "-1 means the script never walked it");
        assert!(!got[2].valid_name, "non-UTF-8 path");
    }

    #[test]
    fn selection_applies_denylist_caps_and_smallest_first() {
        let sel = select_ignored(
            vec![
                listed("big.bin", Some(2048)),          // over the 1024 entry cap
                listed("web/node_modules/", Some(1)),   // deny-listed by basename
                listed("target/", None),                // deny-listed by the script
                listed("c.cfg", Some(600)),
                listed(".env", Some(4)),
                listed("b.cfg", Some(500)),
                ListedIgnored { path: "bad\u{fffd}".into(), kb: Some(1), valid_name: false },
            ],
            1024,
            1000, // total cap: .env(4) + b.cfg(500) fit, c.cfg(600) does not
        );
        let carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(carried, vec![".env", "b.cfg"], "smallest first");
        assert_eq!(sel.carry[0].bytes, 4 * 1024);
        let left = |p: &str| sel.left.iter().find(|l| l.path == p).unwrap_or_else(|| panic!("{p}"));
        assert_eq!(left("big.bin").reason, LeftReason::OverCap);
        assert_eq!(left("big.bin").bytes, Some(2048 * 1024));
        assert_eq!(left("c.cfg").reason, LeftReason::OverCap);
        assert_eq!(left("web/node_modules/").reason, LeftReason::Denylisted);
        assert_eq!(left("target/").reason, LeftReason::Denylisted);
        assert_eq!(left("target/").bytes, None, "never walked, size unknown");
        assert_eq!(left("bad\u{fffd}").reason, LeftReason::UnsupportedName);
    }

    #[test]
    fn selection_stops_at_the_entry_count_bound() {
        let many: Vec<_> = (0..MAX_IGNORED_ENTRIES + 3)
            .map(|i| listed(&format!("f{i:04}"), Some(1)))
            .collect();
        let sel = select_ignored(many, 1024, u64::MAX);
        assert_eq!(sel.carry.len(), MAX_IGNORED_ENTRIES);
        assert_eq!(sel.left.len(), 3);
        assert!(sel.left.iter().all(|l| l.reason == LeftReason::OverCap));
    }

    #[test]
    fn carry_report_serializes_snake_case() {
        let v = serde_json::to_value(CarryReport {
            target_seeded: TargetSeed::Initialized,
            ignored_left_behind: vec![LeftBehind {
                path: "target/".into(),
                bytes: None,
                reason: LeftReason::Denylisted,
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v["target_seeded"], "initialized");
        assert_eq!(v["ignored_left_behind"][0]["reason"], "denylisted");
        assert!(v["ignored_left_behind"][0]["bytes"].is_null());
    }
}
```

- [ ] **Step 2: Run — must fail to compile**

Run: `cargo test -p fleet-core carry::tests`
Expected: FAIL — `cannot find type ListedIgnored`.

- [ ] **Step 3: Implement**

In `carry.rs`, after the constants from Task 2:

```rust
use crate::service::safe_kill::DirtyFile;
use serde::{Deserialize, Serialize};

/// Final path components that are never carried: rebuildable, often huge,
/// frequently platform-specific. `worktrees` / `.worktrees`: nested git
/// worktrees never travel.
pub const DENYLIST: &[&str] = &[
    "node_modules", "target", ".venv", "venv", "dist", "build", "out", ".next", ".nuxt",
    ".svelte-kit", "__pycache__", ".gradle", ".cache", ".turbo", "coverage", "worktrees",
    ".worktrees",
];
/// Bound on carried ignored entries: they reach `tar` as argv.
pub const MAX_IGNORED_ENTRIES: usize = 500;

/// How the target's main clone came to exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSeed {
    #[default]
    Existing,
    Cloned,
    /// `git init` + the bundle: origin was unreachable from the target.
    Initialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftReason {
    Denylisted,
    OverCap,
    /// The path is not valid UTF-8.
    UnsupportedName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredEntry {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftBehind {
    pub path: String,
    /// `None` for a deny-listed entry: it is never walked, so never sized.
    pub bytes: Option<u64>,
    pub reason: LeftReason,
}

/// What a move carried besides the transcript.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CarryReport {
    /// Commits in the bundle besides the two snapshot commits.
    pub commits: u32,
    pub bundle_bytes: u64,
    /// The porcelain rows restored on the target.
    pub dirty_entries: Vec<DirtyFile>,
    pub ignored_carried: Vec<IgnoredEntry>,
    pub ignored_left_behind: Vec<LeftBehind>,
    pub target_seeded: TargetSeed,
}

/// One record of the ignored-list script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedIgnored {
    pub path: String,
    /// `None`: the script recognised a deny-listed name and did not size it.
    pub kb: Option<u64>,
    /// `false` when the path was not valid UTF-8 (`path` is then lossy).
    pub valid_name: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoredSelection {
    pub carry: Vec<IgnoredEntry>,
    pub left: Vec<LeftBehind>,
}

/// Parse `<kb>\t<path>\0` records; a record without a tab is dropped.
pub fn parse_ignored_list(stdout: &[u8]) -> Vec<ListedIgnored> {
    stdout
        .split(|b| *b == 0)
        .filter(|rec| !rec.is_empty())
        .filter_map(|rec| {
            let tab = rec.iter().position(|b| *b == b'\t')?;
            let kb = std::str::from_utf8(&rec[..tab]).ok()?.trim().parse::<i64>().ok()?;
            let raw = &rec[tab + 1..];
            let (path, valid_name) = match std::str::from_utf8(raw) {
                Ok(p) => (p.to_string(), true),
                Err(_) => (String::from_utf8_lossy(raw).into_owned(), false),
            };
            Some(ListedIgnored {
                path,
                kb: u64::try_from(kb).ok(),
                valid_name,
            })
        })
        .collect()
}

fn denylisted(path: &str) -> bool {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    DENYLIST.contains(&base)
}

/// The carry policy: deny-list, per-entry cap, then smallest-first up to the
/// total cap and [`MAX_IGNORED_ENTRIES`].
pub fn select_ignored(listed: Vec<ListedIgnored>, entry_kb: u64, total_kb: u64) -> IgnoredSelection {
    let mut sel = IgnoredSelection::default();
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for l in listed {
        let bytes = l.kb.map(|k| k.saturating_mul(1024));
        if !l.valid_name {
            sel.left.push(LeftBehind { path: l.path, bytes, reason: LeftReason::UnsupportedName });
        } else if l.kb.is_none() || denylisted(&l.path) {
            sel.left.push(LeftBehind { path: l.path, bytes: None, reason: LeftReason::Denylisted });
        } else if l.kb.unwrap_or(0) > entry_kb {
            sel.left.push(LeftBehind { path: l.path, bytes, reason: LeftReason::OverCap });
        } else {
            candidates.push((l.kb.unwrap_or(0), l.path));
        }
    }
    candidates.sort();
    let mut used = 0u64;
    for (kb, path) in candidates {
        let bytes = kb.saturating_mul(1024);
        if sel.carry.len() < MAX_IGNORED_ENTRIES && used.saturating_add(kb) <= total_kb {
            used += kb;
            sel.carry.push(IgnoredEntry { path, bytes });
        } else {
            sel.left.push(LeftBehind { path, bytes: Some(bytes), reason: LeftReason::OverCap });
        }
    }
    sel
}
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p fleet-core`
Expected: PASS (4 new `carry::tests`).

- [ ] **Step 5: Commit**

```bash
git -C $WT add crates/fleet-core/src/service/move_session/carry.rs
git -C $WT commit -m "feat(move): carry report types and the ignored-file selection policy"
```

---

### Task 4: Git carry scripts, parsers, and the real-git round trip

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/carry.rs`

**Interfaces:**
- Consumes: `TargetSeed` (Task 3), `crate::shell::quote`, `IpcError`/`codes`.
- Produces (all `pub`, in `carry`):
  - sentinels `FAILED = "__CF_CARRY_FAILED__"`, `BUNDLE_TOO_LARGE = "__CF_BUNDLE_TOO_LARGE__"`, `TARGET_DIRTY = "__CF_TARGET_DIRTY__"`, `HEAD_MISMATCH = "__CF_HEAD_MISMATCH__"`
  - `const CHUNK_BYTES: u64 = 8 * 1024 * 1024`
  - `fn seed_script(project_root: &str, clone_url: &str) -> String` → stdout one word; `fn parse_seed(stdout: &str) -> Result<TargetSeed, IpcError>`
  - `fn haves_script(project_root: &str, claude_id: &str) -> String`; `fn parse_haves(stdout: &str) -> Result<(String, Vec<String>), IpcError>` → `(absolute transfer dir, hex shas)`
  - `fn snapshot_script(worktree: &str, claude_id: &str, haves: &[String], cap_bytes: u64) -> String`; `struct BundleInfo { bytes: u64, commits: u32, submodules: bool, lfs: bool, path: String }`; `fn parse_snapshot(stdout: &str) -> Result<BundleInfo, IpcError>`
  - `fn chunk_script(path: &str, offset: u64, len: u64) -> String`
  - `fn fetch_script(project_root: &str, bundle_path: &str, claude_id: &str, branch: &str) -> String`
  - `fn apply_script(cwd: &str, claude_id: &str, want_head: &str) -> String` → stdout is `git status --porcelain=v1`
  - `fn cleanup_script(repo_dir: &str, claude_id: &str) -> String`

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `carry.rs`)

```rust
    use std::path::Path;
    use std::process::{Command, Output};

    fn have(bin: &str) -> bool {
        Command::new(bin).arg("--version").output().is_ok_and(|o| o.status.success())
    }

    /// Run a generated script the way a host would, with an isolated `$HOME`
    /// (so `~/.cache/claude-fleet/transfer` lands in the temp dir) and no
    /// user/system git config.
    fn bash(script: &str, home: &Path) -> Output {
        Command::new("bash")
            .args(["-c", script])
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("bash")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn sorted_lines(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        v.sort();
        v
    }

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    /// A source repo with every kind of state the carry must reproduce.
    /// Returns (repo dir, sha of the "pushed" base commit).
    fn dirty_source(root: &Path) -> (std::path::PathBuf, String) {
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q", "-b", "feat"]);
        for (f, body) in [("keep.txt", "keep\n"), ("mod.txt", "v1\n"), ("del.txt", "bye\n"),
                          ("both.txt", "v1\n"), ("mode.sh", "#!/bin/sh\n")] {
            std::fs::write(src.join(f), body).unwrap();
        }
        std::fs::write(src.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "base"]);
        let base = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        git(&src, &["branch", "pushed"]); // stands in for origin/feat: a sha is only fetchable as a ref tip
        for n in ["one", "two"] {
            std::fs::write(src.join(format!("{n}.txt")), n).unwrap();
            git(&src, &["add", "-A"]);
            git(&src, &["commit", "-q", "-m", n]); // two "unpushed" commits
        }
        std::fs::write(src.join("mod.txt"), "v2\n").unwrap(); // modified, unstaged
        std::fs::write(src.join("staged new.txt"), "new\n").unwrap(); // staged, space in name
        git(&src, &["add", "staged new.txt"]);
        std::fs::write(src.join("both.txt"), "staged\n").unwrap(); // staged...
        git(&src, &["add", "both.txt"]);
        std::fs::write(src.join("both.txt"), "then modified\n").unwrap(); // ...then modified
        std::fs::write(src.join("it's untracked.txt"), "u\n").unwrap(); // untracked, quote in name
        std::fs::remove_file(src.join("del.txt")).unwrap(); // deleted, unstaged
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(src.join("mode.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("keep.txt", src.join("link")).unwrap();
        }
        std::fs::write(src.join(".env"), "SECRET=1\n").unwrap(); // ignored: must NOT be in the snapshot
        (src, base)
    }

    /// Everything that must be byte-identical on the source before and after.
    fn source_fingerprint(src: &Path) -> (String, Vec<u8>, String) {
        (
            // --no-optional-locks: a plain `git status` may refresh and rewrite the index.
            git(src, &["--no-optional-locks", "status", "--porcelain=v1"]),
            std::fs::read(src.join(".git/index")).unwrap(),
            git(src, &["for-each-ref", "refs/heads", "refs/remotes", "refs/tags"]),
        )
    }

    /// snapshot → bundle → fetch → worktree add → apply, all through the real
    /// generated scripts. `seed_target` prepares the target's main clone.
    fn round_trip(seed_target: impl Fn(&Path, &Path, &str)) {
        if !have("git") || !have("bash") {
            eprintln!("skipping: git or bash is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, base) = dirty_source(tmp.path());
        let before = source_fingerprint(&src);
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        // Target main clone.
        let root = tmp.path().join("tgt");
        seed_target(&src, &root, &base);
        let out = bash(&haves_script(root.to_str().unwrap(), ID), &home_b);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let (tgt_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();

        // Source: snapshot + bundle.
        let out = bash(&snapshot_script(src.to_str().unwrap(), ID, &haves, u64::MAX), &home_a);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(info.bytes, std::fs::metadata(&info.path).unwrap().len());
        assert_eq!(source_fingerprint(&src), before, "the source tree, index and refs are untouched");

        // Relay: chunked read (tiny chunk to exercise the loop), plain copy in.
        let mut got = Vec::new();
        while (got.len() as u64) < info.bytes {
            let out = bash(&chunk_script(&info.path, got.len() as u64, 1000), &home_a);
            assert!(!out.stdout.is_empty(), "empty chunk at {}", got.len());
            got.extend_from_slice(&out.stdout);
        }
        assert_eq!(got, std::fs::read(&info.path).unwrap(), "chunks reassemble the bundle");
        let tgt_bundle = format!("{tgt_dir}/carry.bundle");
        std::fs::write(&tgt_bundle, &got).unwrap();

        // Target: fetch, worktree add from the LOCAL branch (no origin), apply.
        let out = bash(&fetch_script(root.to_str().unwrap(), &tgt_bundle, ID, "feat"), &home_b);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let wt = tmp.path().join("tgt-wt");
        git(&root, &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"]);
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]).trim(), src_head, "unpushed commits arrived");
        let out = bash(&apply_script(wt.to_str().unwrap(), ID, &src_head), &home_b);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        // The claim: identical porcelain, contents, modes.
        assert_eq!(
            sorted_lines(&String::from_utf8_lossy(&out.stdout)),
            sorted_lines(&before.0),
            "target porcelain equals the source's"
        );
        for f in ["mod.txt", "staged new.txt", "both.txt", "it's untracked.txt", "one.txt"] {
            assert_eq!(std::fs::read(wt.join(f)).unwrap(), std::fs::read(src.join(f)).unwrap(), "{f}");
        }
        assert!(!wt.join("del.txt").exists(), "the deletion travelled");
        assert!(!wt.join(".env").exists(), "ignored files are not in the snapshot");
        assert_eq!(
            git(&wt, &["diff", "--cached", "--name-only"]),
            git(&src, &["diff", "--cached", "--name-only"]),
            "the same files are staged"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(wt.join("mode.sh")).unwrap().permissions().mode() & 0o111, 0o111);
            assert_eq!(std::fs::read_link(wt.join("link")).unwrap().to_str(), Some("keep.txt"));
        }

        // Cleanup leaves no private refs and no temp dirs on either side.
        assert!(bash(&cleanup_script(src.to_str().unwrap(), ID), &home_a).status.success());
        assert!(bash(&cleanup_script(root.to_str().unwrap(), ID), &home_b).status.success());
        assert_eq!(git(&src, &["for-each-ref", "refs/fleet"]), "");
        assert_eq!(git(&root, &["for-each-ref", "refs/fleet"]), "");
        assert!(!home_a.join(".cache/claude-fleet/transfer").join(ID).exists());
        assert!(!home_b.join(".cache/claude-fleet/transfer").join(ID).exists());
        assert_eq!(source_fingerprint(&src), before, "still untouched after cleanup");
    }

    #[test]
    fn round_trip_into_a_target_that_has_the_base_commit() {
        round_trip(|src, root, base| {
            // A clone cut back to the "pushed" base: the bundle must be thin.
            std::fs::create_dir_all(root).unwrap();
            git(root, &["init", "-q", "-b", "main"]);
            git(root, &["fetch", "-q", src.to_str().unwrap(), "pushed:refs/remotes/origin/feat"]);
            assert_eq!(git(root, &["rev-parse", "refs/remotes/origin/feat"]).trim(), base);
        });
    }

    #[test]
    fn round_trip_into_an_initialized_empty_target() {
        round_trip(|_src, root, _base| {
            let tmp_home = root.parent().unwrap().join("home-seed");
            std::fs::create_dir_all(&tmp_home).unwrap();
            // An unreachable origin: the seed falls through to `git init`.
            let out = bash(&seed_script(root.to_str().unwrap(), "/nonexistent/origin.git"), &tmp_home);
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(parse_seed(&String::from_utf8_lossy(&out.stdout)).unwrap(), TargetSeed::Initialized);
        });
    }

    #[test]
    fn a_thin_bundle_is_smaller_than_a_full_one_and_a_clean_source_still_bundles() {
        if !have("git") || !have("bash") { eprintln!("skipping: git or bash is not available"); return; }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, base) = dirty_source(tmp.path());
        let size = |haves: &[String]| {
            let out = bash(&snapshot_script(src.to_str().unwrap(), ID, haves, u64::MAX), &home);
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap()
        };
        let full = size(&[]);
        let thin = size(&[base.clone()]);
        assert!(thin.bytes < full.bytes, "thin {} < full {}", thin.bytes, full.bytes);
        assert_eq!(thin.commits, 2, "the two unpushed commits");
        assert_eq!(full.commits, 3);
        // Even when the target has HEAD, the snapshot commits make a bundle.
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let none = size(&[head]);
        assert_eq!(none.commits, 0);
        assert!(none.bytes > 0);
    }

    #[test]
    fn a_bundle_over_the_cap_and_a_dirty_or_moved_target_are_recognisable() {
        if !have("git") || !have("bash") { eprintln!("skipping: git or bash is not available"); return; }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let out = bash(&snapshot_script(src.to_str().unwrap(), ID, &[], 10), &home);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(BUNDLE_TOO_LARGE));

        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        // The source itself is dirty: applying onto it must refuse, not overwrite.
        let out = bash(&apply_script(src.to_str().unwrap(), ID, &head), &home);
        assert!(String::from_utf8_lossy(&out.stderr).contains(TARGET_DIRTY));
        // A clean checkout at another commit: refused as a head mismatch.
        let clean = tmp.path().join("clean");
        git(tmp.path(), &["clone", "-q", src.to_str().unwrap(), clean.to_str().unwrap()]);
        let out = bash(&apply_script(clean.to_str().unwrap(), ID, &"0".repeat(40)), &home);
        assert!(String::from_utf8_lossy(&out.stderr).contains(HEAD_MISMATCH));
    }

    #[test]
    fn seed_never_deletes_an_existing_non_git_directory() {
        if !have("git") || !have("bash") { eprintln!("skipping: git or bash is not available"); return; }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("precious");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("data.txt"), "mine").unwrap();
        let out = bash(&seed_script(root.to_str().unwrap(), "/nonexistent/origin.git"), tmp.path());
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert_eq!(std::fs::read_to_string(root.join("data.txt")).unwrap(), "mine");
    }

    #[test]
    fn parsers_reject_malformed_output() {
        assert!(parse_seed("weird\n").is_err());
        assert_eq!(parse_seed("cloned\n").unwrap(), TargetSeed::Cloned);
        assert!(parse_haves("relative/dir\n").is_err());
        let (dir, haves) = parse_haves(&format!("/h/.cache/x\n{}\nnot-a-sha\n{}\n", "a".repeat(40), "b".repeat(64))).unwrap();
        assert_eq!(dir, "/h/.cache/x");
        assert_eq!(haves.len(), 2, "non-hex lines are dropped");
        assert!(parse_snapshot("12\t3\t0\t1\t/abs/carry.bundle\n").is_ok());
        assert!(parse_snapshot("12\t3\t0\t1\trelative\n").is_err());
        assert!(parse_snapshot("x\t3\t0\t1\t/abs\n").is_err());
    }

    #[test]
    fn carry_scripts_quote_every_interpolated_value() {
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        for script in [
            seed_script(evil, evil),
            haves_script(evil, evil),
            snapshot_script(evil, evil, &[], 1),
            chunk_script(evil, 0, 1),
            fetch_script(evil, evil, evil, evil),
            apply_script(evil, evil, evil),
            cleanup_script(evil, evil),
        ] {
            assert!(script.contains(&q), "quoted value present: {script}");
            let without = script.replace(&q, "");
            assert!(!without.contains("touch /tmp/pwn"), "raw value leaked: {script}");
        }
        // Haves are hex-only: anything else never reaches the heredoc.
        let s = snapshot_script("/w", ID, &["zz; rm -rf /".into(), "a".repeat(40)], 1);
        assert!(!s.contains("rm -rf /"), "{s}");
        assert!(s.contains(&"a".repeat(40)));
    }
```

- [ ] **Step 2: Run — must fail to compile**

Run: `cargo test -p fleet-core carry::tests`
Expected: FAIL — `cannot find function haves_script`.

- [ ] **Step 3: Implement the scripts and parsers**

In `carry.rs`, after `select_ignored` (add `use crate::ipc_error::{codes, IpcError};` and `use crate::shell::quote;` to the imports):

```rust
pub const FAILED: &str = "__CF_CARRY_FAILED__";
pub const BUNDLE_TOO_LARGE: &str = "__CF_BUNDLE_TOO_LARGE__";
pub const TARGET_DIRTY: &str = "__CF_TARGET_DIRTY__";
pub const HEAD_MISMATCH: &str = "__CF_HEAD_MISMATCH__";
/// Bytes per relay chunk: the orchestrator's peak memory for a payload.
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;

fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_err(what: &str, got: &str) -> IpcError {
    IpcError::new(codes::E_PARSE, format!("unexpected {what} output: {got:?}"))
}

/// Make sure the target's main clone exists. Prints `existing`, `cloned` or
/// `initialized`. Never prompts (a host without credentials falls through to
/// `git init`) and never removes a directory it did not create.
pub fn seed_script(project_root: &str, clone_url: &str) -> String {
    format!(
        r#"# cf-carry:seed
set +e
r={r}
url={url}
export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -oBatchMode=yes'
if [ -e "$r/.git" ]; then printf 'existing\n'; exit 0; fi
if [ -e "$r" ]; then printf '{FAILED} %s exists and is not a git repository\n' "$r" >&2; exit 5; fi
mkdir -p -- "$(dirname -- "$r")" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
if git clone -q -- "$url" "$r" >/dev/null 2>&1; then printf 'cloned\n'; exit 0; fi
rm -rf -- "$r"
git init -q -- "$r" >/dev/null 2>&1 && git -C "$r" remote add origin "$url" || {{ printf '{FAILED} init\n' >&2; exit 5; }}
printf 'initialized\n'
"#,
        r = quote(project_root),
        url = quote(clone_url),
    )
}

pub fn parse_seed(stdout: &str) -> Result<TargetSeed, IpcError> {
    match stdout.trim() {
        "existing" => Ok(TargetSeed::Existing),
        "cloned" => Ok(TargetSeed::Cloned),
        "initialized" => Ok(TargetSeed::Initialized),
        other => Err(parse_err("seed", other)),
    }
}

/// Create the private transfer dir on the target and list every ref tip it
/// has. Line 1: the absolute dir; then one object name per line.
pub fn haves_script(project_root: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:haves
set +e
r={r}
id={id}
dir="$HOME/.cache/claude-fleet/transfer/$id"
rm -rf -- "$dir"
( umask 077; mkdir -p -- "$dir" ) || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
printf '%s\n' "$dir"
git -C "$r" for-each-ref --format='%(objectname)' 2>/dev/null | sort -u
exit 0
"#,
        r = quote(project_root),
        id = quote(claude_id),
    )
}

pub fn parse_haves(stdout: &str) -> Result<(String, Vec<String>), IpcError> {
    let mut lines = stdout.lines();
    let dir = lines
        .next()
        .map(str::trim)
        .filter(|d| d.starts_with('/'))
        .ok_or_else(|| parse_err("haves", stdout))?;
    let haves = lines.map(str::trim).filter(|l| is_sha(l)).map(str::to_string).collect();
    Ok((dir.to_string(), haves))
}

/// What the snapshot script produced on the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    pub bytes: u64,
    /// Commits the target lacked, not counting the two snapshot commits.
    pub commits: u32,
    pub submodules: bool,
    pub lfs: bool,
    /// Absolute path of the bundle on the source.
    pub path: String,
}

/// Snapshot the worktree (temporary index — the real index, the working tree
/// and every user ref stay untouched), park it under `refs/fleet/transfer/`,
/// and bundle what the target lacks. Both index reads go through COPIES:
/// even `git write-tree` rewrites the index it reads (the cache-tree
/// extension). The target's haves reach `git rev-list` on stdin (there can be
/// thousands); only the resulting boundary — a handful of hex shas, left
/// unquoted on purpose — reaches `git bundle create` as argv, because
/// `bundle create --stdin` regressed in some git releases. Prints `<bytes>\t<commits>\t<submodules 0|1>\t<lfs 0|1>\t<path>`.
pub fn snapshot_script(worktree: &str, claude_id: &str, haves: &[String], cap_bytes: u64) -> String {
    let haves: String = haves.iter().filter(|h| is_sha(h)).map(|h| format!("{h}\n")).collect();
    format!(
        r#"# cf-carry:snapshot
set +e
wt={wt}
id={id}
cap={cap}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
cd -- "$wt" 2>/dev/null || fail cd
dir="$HOME/.cache/claude-fleet/transfer/$id"
rm -rf -- "$dir"
( umask 077; mkdir -p -- "$dir" ) || fail mkdir
export GIT_AUTHOR_NAME=claude-fleet GIT_AUTHOR_EMAIL=fleet@localhost GIT_COMMITTER_NAME=claude-fleet GIT_COMMITTER_EMAIL=fleet@localhost
real=$(git rev-parse --git-path index)
cp -- "$real" "$dir/index.ix" 2>/dev/null && cp -- "$real" "$dir/index.wt" 2>/dev/null || fail index
itree=$(GIT_INDEX_FILE="$dir/index.ix" git write-tree 2>/dev/null) || fail write-tree-index
GIT_INDEX_FILE="$dir/index.wt" git add -A >/dev/null 2>&1 || fail add
wtree=$(GIT_INDEX_FILE="$dir/index.wt" git write-tree 2>/dev/null) || fail write-tree-worktree
rm -f -- "$dir/index.ix" "$dir/index.wt"
ix=$(git commit-tree "$itree" -p HEAD -m 'fleet transfer: index' 2>/dev/null) || fail commit-index
w=$(git commit-tree "$wtree" -p HEAD -m 'fleet transfer: worktree' 2>/dev/null) || fail commit-worktree
ref="refs/fleet/transfer/$id"
git update-ref "$ref/ix" "$ix" && git update-ref "$ref/wt" "$w" && git update-ref "$ref/head" HEAD || fail update-ref
: > "$dir/nots"
while IFS= read -r h; do
  [ -n "$h" ] || continue
  if git cat-file -e "$h^{{commit}}" 2>/dev/null; then printf '^%s\n' "$h" >> "$dir/nots"; fi
done <<'CF_HAVES'
{haves}CF_HAVES
commits=$(git rev-list --count "$ref/head" --stdin < "$dir/nots" 2>/dev/null)
bnd=$(git rev-list --boundary "$ref/head" "$ref/ix" "$ref/wt" --stdin < "$dir/nots" 2>/dev/null | sed -n 's/^-/^/p' | tr '\n' ' ')
git bundle create "$dir/carry.bundle" "$ref/head" "$ref/ix" "$ref/wt" $bnd >/dev/null 2>&1 || fail bundle
n=$(wc -c < "$dir/carry.bundle" | tr -d ' ')
if [ "$n" -gt "$cap" ]; then printf '{BUNDLE_TOO_LARGE} %s\n' "$n" >&2; exit 8; fi
sub=0; [ -f .gitmodules ] && sub=1
lfs=0; grep -qs 'filter=lfs' .gitattributes && lfs=1
printf '%s\t%s\t%s\t%s\t%s\n' "$n" "${{commits:-0}}" "$sub" "$lfs" "$dir/carry.bundle"
"#,
        wt = quote(worktree),
        id = quote(claude_id),
        cap = cap_bytes,
    )
}

pub fn parse_snapshot(stdout: &str) -> Result<BundleInfo, IpcError> {
    let line = stdout.trim_end_matches('\n');
    let p: Vec<&str> = line.splitn(5, '\t').collect();
    let bad = || parse_err("snapshot", line);
    if p.len() != 5 || !p[4].starts_with('/') {
        return Err(bad());
    }
    Ok(BundleInfo {
        bytes: p[0].trim().parse().map_err(|_| bad())?,
        commits: p[1].trim().parse().map_err(|_| bad())?,
        submodules: p[2].trim() == "1",
        lfs: p[3].trim() == "1",
        path: p[4].to_string(),
    })
}

/// `len` bytes of `path` starting at `offset` (binary-safe, GNU and BSD).
pub fn chunk_script(path: &str, offset: u64, len: u64) -> String {
    format!(
        "# cf-carry:chunk\ntail -c +{} -- {} | head -c {len}\n",
        offset + 1,
        quote(path)
    )
}

/// Verify and fetch the bundle into the target's main clone; create the
/// local branch at the source HEAD when the target has none. An existing
/// local branch is never moved here.
pub fn fetch_script(project_root: &str, bundle_path: &str, claude_id: &str, branch: &str) -> String {
    format!(
        r#"# cf-carry:fetch
set +e
r={r}
b={b}
id={id}
br={br}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
git -C "$r" bundle verify "$b" >/dev/null 2>&1 || fail verify
git -C "$r" fetch -q "$b" '+refs/fleet/transfer/*:refs/fleet/transfer/*' >/dev/null 2>&1 || fail fetch
if ! git -C "$r" show-ref --verify --quiet "refs/heads/$br"; then
  git -C "$r" branch -- "$br" "refs/fleet/transfer/$id/head" >/dev/null 2>&1 || fail branch
fi
printf 'ok\n'
"#,
        r = quote(project_root),
        b = quote(bundle_path),
        id = quote(claude_id),
        br = quote(branch),
    )
}

/// Replay the snapshot in the target worktree: working tree := snapshot,
/// index := what was staged. Refuses a dirty worktree ([`TARGET_DIRTY`]) and
/// one not at `want_head` ([`HEAD_MISMATCH`]). Prints the resulting
/// `git status --porcelain=v1`.
pub fn apply_script(cwd: &str, claude_id: &str, want_head: &str) -> String {
    format!(
        r#"# cf-carry:apply
set +e
cwd={cwd}
id={id}
want={want}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
cd -- "$cwd" 2>/dev/null || fail cd
if [ -n "$(git status --porcelain 2>/dev/null)" ]; then printf '{TARGET_DIRTY}\n' >&2; exit 9; fi
h=$(git rev-parse HEAD 2>/dev/null)
if [ "$h" != "$want" ]; then printf '{HEAD_MISMATCH} %s\n' "$h" >&2; exit 10; fi
git read-tree -u --reset "refs/fleet/transfer/$id/wt^{{tree}}" >/dev/null 2>&1 || fail read-tree-worktree
git read-tree "refs/fleet/transfer/$id/ix^{{tree}}" >/dev/null 2>&1 || fail read-tree-index
git status --porcelain=v1
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        want = quote(want_head),
    )
}

/// Best effort: drop the private refs and the transfer dir. Always exits 0.
pub fn cleanup_script(repo_dir: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:cleanup
set +e
r={r}
id={id}
git -C "$r" for-each-ref --format='%(refname)' "refs/fleet/transfer/$id/" 2>/dev/null | while IFS= read -r ref; do
  git -C "$r" update-ref -d "$ref" >/dev/null 2>&1
done
rm -rf -- "$HOME/.cache/claude-fleet/transfer/$id"
exit 0
"#,
        r = quote(repo_dir),
        id = quote(claude_id),
    )
}
```

- [ ] **Step 4: Run the new tests, then the full suite**

Run: `cargo test -p fleet-core carry::tests -- --nocapture`
Expected: PASS and **no** `skipping:` line on this machine (git and bash are present). If a round-trip assertion fails, fix the script — never the assertion: the porcelain equality is the feature.

Run: `cargo test -p fleet-core`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C $WT add crates/fleet-core/src/service/move_session/carry.rs
git -C $WT commit -m "feat(move): snapshot, bundle, fetch and apply scripts with a real-git round trip"
```

---

### Task 5: Ignored-file scripts

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/carry.rs`

**Interfaces:**
- Consumes: `DENYLIST`, `parse_ignored_list`, `select_ignored`, `IgnoredEntry` (Task 3); `FAILED` (Task 4).
- Produces:
  - `fn ignored_list_script(worktree: &str) -> String` → stdout `<kb>\t<path>\0…` for `parse_ignored_list`
  - `fn ignored_pack_script(worktree: &str, claude_id: &str, paths: &[String]) -> String` → stdout `<bytes>\t<absolute archive path>`
  - `fn parse_pack(stdout: &str) -> Result<(u64, String), IpcError>`
  - `fn ignored_extract_script(cwd: &str, archive: &str) -> String`

- [ ] **Step 1: Write the failing test** (inside `mod tests`)

```rust
    #[test]
    fn ignored_files_are_listed_selected_packed_and_extracted_without_overwriting() {
        if !have("git") || !have("bash") || !have("tar") { eprintln!("skipping: git, bash or tar is not available"); return; }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(src.join("conf.d")).unwrap();
        git(&src, &["init", "-q", "-b", "feat"]);
        std::fs::write(src.join(".gitignore"), ".env\nnode_modules/\nbig.bin\nconf.d/\nit's.cfg\n").unwrap();
        std::fs::write(src.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(src.join("it's.cfg"), "q\n").unwrap();
        std::fs::write(src.join("conf.d/a.conf"), "a\n").unwrap();
        std::fs::write(src.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(src.join("big.bin"), vec![0u8; 3 * 1024 * 1024]).unwrap();
        std::fs::write(src.join("tracked.txt"), "t\n").unwrap();
        git(&src, &["add", ".gitignore", "tracked.txt"]);
        git(&src, &["commit", "-q", "-m", "base"]);

        let out = bash(&ignored_list_script(src.to_str().unwrap()), &home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let sel = select_ignored(parse_ignored_list(&out.stdout), 1024, 20 * 1024);
        let mut carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        carried.sort();
        assert_eq!(carried, vec![".env", "conf.d/", "it's.cfg"]);
        let left = |p: &str| sel.left.iter().find(|l| l.path == p).map(|l| l.reason);
        assert_eq!(left("node_modules/"), Some(LeftReason::Denylisted));
        assert_eq!(left("big.bin"), Some(LeftReason::OverCap));

        let paths: Vec<String> = sel.carry.iter().map(|e| e.path.clone()).collect();
        let out = bash(&ignored_pack_script(src.to_str().unwrap(), ID, &paths), &home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let (bytes, archive) = parse_pack(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(bytes, std::fs::metadata(&archive).unwrap().len());

        // The target already has its own .env: it must win.
        let tgt = tmp.path().join("tgt");
        std::fs::create_dir_all(&tgt).unwrap();
        std::fs::write(tgt.join(".env"), "SECRET=target\n").unwrap();
        let out = bash(&ignored_extract_script(tgt.to_str().unwrap(), &archive), &home);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_to_string(tgt.join(".env")).unwrap(), "SECRET=target\n", "never overwritten");
        assert_eq!(std::fs::read_to_string(tgt.join("it's.cfg")).unwrap(), "q\n");
        assert_eq!(std::fs::read_to_string(tgt.join("conf.d/a.conf")).unwrap(), "a\n");
        assert!(!tgt.join("node_modules").exists());

        // A corrupt archive is recognised, nothing is extracted.
        std::fs::write(&archive, b"not a tarball").unwrap();
        let out = bash(&ignored_extract_script(tgt.to_str().unwrap(), &archive), &home);
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
    }

    #[test]
    fn ignored_scripts_quote_every_interpolated_value() {
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        for script in [
            ignored_list_script(evil),
            ignored_pack_script(evil, evil, &[evil.to_string()]),
            ignored_extract_script(evil, evil),
        ] {
            assert!(script.contains(&q) || script.contains(&quote(&format!("./{evil}"))), "{script}");
            let without = script.replace(&q, "").replace(&quote(&format!("./{evil}")), "");
            assert!(!without.contains("touch /tmp/pwn"), "raw value leaked: {script}");
        }
        assert!(parse_pack("12\t/abs/ignored.tgz\n").is_ok());
        assert!(parse_pack("12\trelative\n").is_err());
    }
```

- [ ] **Step 2: Run — must fail to compile**

Run: `cargo test -p fleet-core carry::tests::ignored`
Expected: FAIL — `cannot find function ignored_list_script`.

- [ ] **Step 3: Implement**

```rust
/// List the worktree's top-level git-ignored entries as `<kb>\t<path>\0`.
/// A wholly ignored directory is one entry; a deny-listed name is printed
/// with `-1` and never walked by `du`.
pub fn ignored_list_script(worktree: &str) -> String {
    format!(
        r#"# cf-carry:ignored-list
set +e
wt={wt}
deny=' {deny} '
cd -- "$wt" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
git ls-files -o -i --exclude-standard --directory -z 2>/dev/null | while IFS= read -r -d '' p; do
  b=$(basename -- "${{p%/}}")
  case "$deny" in
    *" $b "*) k=-1 ;;
    *) k=$(du -sk -- "$p" 2>/dev/null | cut -f1) ;;
  esac
  printf '%s\t%s\0' "${{k:-0}}" "$p"
done
exit 0
"#,
        wt = quote(worktree),
        deny = DENYLIST.join(" "),
    )
}

/// Tar the chosen entries into the transfer dir. Each path is a quoted argv
/// word prefixed with `./` (so a leading `-` is never an option);
/// `COPYFILE_DISABLE` keeps macOS `._*` files out. Prints `<bytes>\t<path>`.
pub fn ignored_pack_script(worktree: &str, claude_id: &str, paths: &[String]) -> String {
    let argv: Vec<String> = paths.iter().map(|p| quote(&format!("./{p}"))).collect();
    format!(
        r#"# cf-carry:ignored-pack
set +e
wt={wt}
id={id}
cd -- "$wt" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
dir="$HOME/.cache/claude-fleet/transfer/$id"
( umask 077; mkdir -p -- "$dir" ) || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
COPYFILE_DISABLE=1 tar -czf "$dir/ignored.tgz" {argv} >/dev/null 2>&1 || {{ printf '{FAILED} tar\n' >&2; exit 5; }}
n=$(wc -c < "$dir/ignored.tgz" | tr -d ' ')
printf '%s\t%s\n' "$n" "$dir/ignored.tgz"
"#,
        wt = quote(worktree),
        id = quote(claude_id),
        argv = argv.join(" "),
    )
}

pub fn parse_pack(stdout: &str) -> Result<(u64, String), IpcError> {
    let line = stdout.trim_end_matches('\n');
    let (n, path) = line.split_once('\t').ok_or_else(|| parse_err("ignored-pack", line))?;
    let bytes = n.trim().parse::<u64>().map_err(|_| parse_err("ignored-pack", line))?;
    if !path.starts_with('/') {
        return Err(parse_err("ignored-pack", line));
    }
    Ok((bytes, path.to_string()))
}

/// Extract in the target worktree; a file already there wins
/// (`--skip-old-files` on GNU tar, `-k` on BSD tar — GNU's `-k` reports
/// existing files as errors).
pub fn ignored_extract_script(cwd: &str, archive: &str) -> String {
    format!(
        r#"# cf-carry:ignored-extract
set +e
cwd={cwd}
a={a}
cd -- "$cwd" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
tar -tzf "$a" >/dev/null 2>&1 || {{ printf '{FAILED} corrupt archive\n' >&2; exit 5; }}
if tar --version 2>/dev/null | grep -q 'GNU tar'; then k=--skip-old-files; else k=-k; fi
tar -xzf "$a" $k >/dev/null 2>&1 || {{ printf '{FAILED} extract\n' >&2; exit 5; }}
printf 'ok\n'
"#,
        cwd = quote(cwd),
        a = quote(archive),
    )
}
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p fleet-core`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C $WT add crates/fleet-core/src/service/move_session/carry.rs
git -C $WT commit -m "feat(move): list, pack and extract small git-ignored files"
```

---

### Task 6: Mid-operation probe, `strict`, and the carry verdict

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — `MoveSessionArgs` (~L80), `SourceState` (~L308), `parse_inspection` (~L320), `preflight_verdict` (~L343), `inspect_script` (~L457), test helpers `inspection` (~L1684) and `args` (~L1769), test `inspection_parses_and_verdicts`
- Modify: `crates/fleet-core/src/mcp/tools/lifecycle.rs:386-390` (add `strict: false` for now — Task 8 wires the param)

**Interfaces:**
- Produces:
  - `MoveSessionArgs.strict: bool` (`#[serde(default)]`)
  - `SourceState.midop: Option<String>`
  - `pub fn carry_verdict(state: &SourceState, branch: &str) -> Result<(), IpcError>` — refuses mid-op (`E_MOVE_MIDOP`), unreadable HEAD (`E_GIT`), wrong branch (`E_INVALID_STATE`); accepts dirty / unpushed.
  - `preflight_verdict` (strict) now calls `carry_verdict` first, then its dirty / unpushed refusals.
  - The inspect output has **7** `\x1e` fields; the 7th is the mid-op name or empty.

- [ ] **Step 1: Write the failing tests** (in `mod.rs` `mod tests`)

Change the helper so every existing caller keeps compiling, and add one with a mid-op:

```rust
    fn inspection(porcelain: &str, rsha: &str, ahead: &str) -> String {
        inspection_midop(porcelain, rsha, ahead, "")
    }

    fn inspection_midop(porcelain: &str, rsha: &str, ahead: &str, midop: &str) -> String {
        format!(
            "/home/a/p/o/r/.claude/worktrees/feat\x1e{porcelain}\x1e{HEAD}\x1efeat\x1e{rsha}\x1e{ahead}\x1e{midop}"
        )
    }
```

Add `strict: false` to the `args()` helper's `MoveSessionArgs { .. }`, and add:

```rust
    #[test]
    fn carry_verdict_accepts_dirty_and_unpushed_but_refuses_midop_and_wrong_branch() {
        let dirty_unpushed = parse_inspection(&inspection(" M a.rs\n?? n.txt", "", "-1")).unwrap();
        assert!(carry_verdict(&dirty_unpushed, "feat").is_ok());
        assert_eq!(
            preflight_verdict(&dirty_unpushed, "feat").unwrap_err().code,
            codes::E_MOVE_DIRTY,
            "strict still refuses"
        );

        let mid = parse_inspection(&inspection_midop("", HEAD, "0", "rebase")).unwrap();
        assert_eq!(mid.midop.as_deref(), Some("rebase"));
        for verdict in [carry_verdict(&mid, "feat"), preflight_verdict(&mid, "feat")] {
            let e = verdict.unwrap_err();
            assert_eq!(e.code, codes::E_MOVE_MIDOP);
            assert!(e.message.contains("rebase"), "{}", e.message);
            assert_eq!(e.details.unwrap()["operation"], "rebase");
        }

        let clean = parse_inspection(&inspection("", HEAD, "0")).unwrap();
        assert_eq!(clean.midop, None);
        assert_eq!(carry_verdict(&clean, "other").unwrap_err().code, codes::E_INVALID_STATE);
    }

    #[test]
    fn inspect_script_probes_every_in_progress_operation() {
        let s = inspect_script("n", None, "feat");
        for marker in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "BISECT_LOG", "rebase-merge", "rebase-apply"] {
            assert!(s.contains(marker), "{marker}: {s}");
        }
    }
```

- [ ] **Step 2: Run — must fail to compile**

Run: `cargo test -p fleet-core move_session::tests::carry_verdict`
Expected: FAIL — `cannot find function carry_verdict` / no field `midop`.

- [ ] **Step 3: Implement**

`MoveSessionArgs` — add the field:

```rust
    /// Refuse a dirty worktree (`E_MOVE_DIRTY`) or an unpushed branch
    /// (`E_MOVE_UNPUSHED`) instead of carrying them. Default false.
    #[serde(default)]
    pub strict: bool,
```

`SourceState` — add:

```rust
    /// An operation in progress (`merge`, `rebase`, `cherry-pick`, `revert`,
    /// `bisect`); its state is not carried, so the move is refused.
    pub midop: Option<String>,
```

`parse_inspection` — `if parts.len() != 7`, and in the struct literal:

```rust
        midop: Some(parts[6].trim()).filter(|m| !m.is_empty()).map(str::to_string),
```

`inspect_script` — before the final `printf`, add the probe, and extend the `printf` to seven fields:

```sh
gd=$(git -C "$wt" rev-parse --absolute-git-dir 2>/dev/null)
midop=''
if [ -n "$gd" ]; then
  if [ -d "$gd/rebase-merge" ] || [ -d "$gd/rebase-apply" ]; then midop=rebase
  elif [ -f "$gd/MERGE_HEAD" ]; then midop=merge
  elif [ -f "$gd/CHERRY_PICK_HEAD" ]; then midop=cherry-pick
  elif [ -f "$gd/REVERT_HEAD" ]; then midop=revert
  elif [ -f "$gd/BISECT_LOG" ]; then midop=bisect
  fi
fi
printf '%s\036%s\036%s\036%s\036%s\036%s\036%s' "$wt" "$porcelain" "$head" "$cur" "$rsha" "$ahead" "$midop"
```

(The script lives in a `format!` raw string: there are no `{`/`}` in the added lines, so nothing needs doubling. In a linked worktree `--absolute-git-dir` is the per-worktree git dir, which is where these files live.)

Split the verdict. Add above `preflight_verdict`:

```rust
/// The refusals that hold in every mode: an operation in progress, an
/// unreadable HEAD, a worktree on another branch. Dirty and unpushed work is
/// not refused here — the carry engine takes it along.
pub fn carry_verdict(state: &SourceState, branch: &str) -> Result<(), IpcError> {
    if let Some(op) = state.midop.as_deref() {
        return Err(IpcError::new(
            codes::E_MOVE_MIDOP,
            format!(
                "the source worktree {} is in the middle of a {op}; finish or abort it first — move_session does not carry an operation in progress",
                state.worktree
            ),
        )
        .with_details(serde_json::json!({ "operation": op })));
    }
    if state.head.is_empty() {
        return Err(IpcError::new(
            codes::E_GIT,
            format!("could not read HEAD in {}", state.worktree),
        ));
    }
    if state.current_branch != branch {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the source worktree is on {:?} but the session's branch is {branch:?}; check out {branch} first",
                state.current_branch
            ),
        ));
    }
    Ok(())
}
```

In `preflight_verdict`: make `carry_verdict(state, branch)?;` its first line, keep the dirty block right after it, and **delete** its own `state.head.is_empty()` and `state.current_branch != branch` blocks (now in `carry_verdict`). Note the order change: mid-op / HEAD / branch are now checked before dirty. Update its doc comment to: `/// The strict-mode verdict: [`carry_verdict`] plus today's refusals of uncommitted and unpushed work.`

`lifecycle.rs` — add `strict: false,` to the `MoveSessionArgs { .. }` literal so the crate compiles.

In the existing test `inspection_parses_and_verdicts`, any hand-written 6-field inspection string needs a trailing `\x1e`; if it asserts "wrong field count is `E_PARSE`" with a 5-field input, leave that as is.

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p fleet-core`
Expected: PASS. The flow still calls `preflight_verdict`, so every existing flow test behaves as before.

- [ ] **Step 5: Commit**

```bash
git -C $WT add crates/fleet-core/src/service/move_session/mod.rs crates/fleet-core/src/mcp/tools/lifecycle.rs
git -C $WT commit -m "feat(move): probe operations in progress and split the strict verdict from the carry verdict"
```

---

### Task 7: Wire the carry into the move flow

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` — constants (~L64), `MoveReport` (~L90), `TempFile` (~L692), `Snapshot`/`snapshot()` (~L782-923), `too_large` (~L925), `move_session_steps` (~L1044-1515), tests fixture (~L1691) and the tests named below

**Interfaces:**
- Consumes (from `carry`, Tasks 2–5): every script builder and parser, `CarryReport`, `TargetSeed`, `BundleInfo`, `select_ignored`, `parse_ignored_list`, `CHUNK_BYTES`, sentinels, setting keys/defaults. From Task 6: `carry_verdict`, `preflight_verdict`, `MoveSessionArgs.strict`, `SourceState.midop`.
- Produces: `MoveReport.carried: CarryReport`; `session_moved` event detail key `"carried"`; `E_MOVE_TOO_LARGE` details gain `"payload": "transcript" | "bundle"`.

- [ ] **Step 1: Extend the `FakeSsh` fixture and write the failing flow tests**

In `mod tests`, add constants and extend `fixture()` with the carry replies (append to the existing `.on_host(..)` chain, before `Fixture { .. }`):

```rust
    const BUNDLE: &str = "FAKE-BUNDLE-BYTES";
    const SRC_DIR: &str = "/home/a/.cache/claude-fleet/transfer/550e8400-e29b-41d4-a716-446655440000";
    const TGT_DIR: &str = "/home/b/.cache/claude-fleet/transfer/550e8400-e29b-41d4-a716-446655440000";

    fn snapshot_out(bytes: usize, commits: u32) -> String {
        format!("{bytes}\t{commits}\t0\t0\t{SRC_DIR}/carry.bundle\n")
    }
```

```rust
        fake.on_host("beta", Match::script_contains("# cf-carry:seed"), Reply::ok("existing\n"))
            .on_host("beta", Match::script_contains("# cf-carry:haves"), Reply::ok(&format!("{TGT_DIR}\n{HEAD}\n")))
            .on_host("alpha", Match::script_contains("# cf-carry:snapshot"), Reply::ok(&snapshot_out(BUNDLE.len(), 0)))
            .on_host("alpha", Match::script_contains("# cf-carry:chunk"), Reply::ok(BUNDLE))
            .on_host("beta", Match::script_contains("# cf-carry:fetch"), Reply::ok("ok\n"))
            .on_host("beta", Match::script_contains("# cf-carry:apply"), Reply::ok(""))
            .on_host("alpha", Match::script_contains("# cf-carry:ignored-list"), Reply::ok(""));
```

New tests:

```rust
    #[tokio::test]
    async fn a_dirty_unpushed_source_is_carried_and_reported() {
        let f = fixture();
        let porcelain = " M src/lib.rs\n?? notes.txt";
        f.fake
            .on_host("alpha", Match::script_contains("# cf-move:inspect"), Reply::ok(&inspection(porcelain, "", "-1")))
            .on_host("alpha", Match::script_contains("# cf-carry:snapshot"), Reply::ok(&snapshot_out(BUNDLE.len(), 2)))
            .on_host("beta", Match::script_contains("# cf-carry:seed"), Reply::ok("initialized\n"))
            .on_host("beta", Match::script_contains("# cf-carry:apply"), Reply::ok(&format!("{porcelain}\n")))
            .on_host("alpha", Match::script_contains("# cf-carry:ignored-list"), Reply::Exit {
                code: 0,
                stdout: b"4\t.env\0-1\tnode_modules/\0".to_vec(),
                stderr: Vec::new(),
            })
            .on_host("alpha", Match::script_contains("# cf-carry:ignored-pack"), Reply::ok(&format!("{}\t{SRC_DIR}/ignored.tgz\n", BUNDLE.len())))
            .on_host("beta", Match::script_contains("# cf-carry:ignored-extract"), Reply::ok("ok\n"));
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("a dirty, unpushed source now moves");

        assert_eq!(rep.carried.commits, 2);
        assert_eq!(rep.carried.bundle_bytes, BUNDLE.len() as u64);
        assert_eq!(rep.carried.dirty_entries.len(), 2);
        assert_eq!(rep.carried.target_seeded, carry::TargetSeed::Initialized);
        assert_eq!(rep.carried.ignored_carried[0].path, ".env");
        assert_eq!(rep.carried.ignored_left_behind[0].path, "node_modules/");
        assert!(rep.warnings.iter().any(|w| w.contains("still holds a copy of the uncommitted work")), "{:?}", rep.warnings);
        assert!(rep.warnings.iter().any(|w| w.contains("origin was unreachable")), "{:?}", rep.warnings);

        // The bundle reached the target's transfer dir through upload_file.
        let uploads: Vec<String> = f.fake.calls_for("beta").into_iter().filter(|c| c.stdin.is_some()).map(|c| c.command()).collect();
        assert_eq!(uploads[0], format!("cat > {}", quote(&format!("{TGT_DIR}/carry.bundle"))));
        // Nothing was pushed, committed or stashed on the source.
        for c in f.fake.calls_for("alpha") {
            let s = c.script().unwrap_or_default();
            assert!(!s.contains("git push") && !s.contains("git stash") && !s.contains("git commit "), "{s}");
        }
        // Both sides were cleaned up, and the event carries the report.
        for host in ["alpha", "beta"] {
            assert!(f.fake.calls_for(host).iter().any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:cleanup"))), "{host}");
        }
        let ev = events(&f, rep.target_session_id);
        let detail = ev.iter().find(|(k, _)| k == EVENT_MOVED).unwrap().1.clone().unwrap();
        let d: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(d["carried"]["commits"], 2);
    }

    #[tokio::test]
    async fn strict_still_refuses_dirty_and_unpushed_before_the_target_is_touched() {
        let f = fixture();
        f.fake.on_host("alpha", Match::script_contains("# cf-move:inspect"), Reply::ok(&inspection(" M a.rs", HEAD, "0")));
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.strict = true;
        let err = move_session_with(a, &f.store, &f.fake, &hooks, fast()).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_DIRTY);
        assert!(f.fake.calls_for("beta").is_empty(), "target untouched");
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn a_midop_source_is_refused_before_anything_else_runs() {
        let f = fixture();
        f.fake.on_host("alpha", Match::script_contains("# cf-move:inspect"), Reply::ok(&inspection_midop("UU a.rs", HEAD, "0", "merge")));
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_MIDOP);
        assert!(f.fake.calls_for("beta").is_empty());
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn carry_failures_leave_the_source_untouched_and_still_clean_up() {
        // (rule to break, expected code, expected details.step)
        let cases: Vec<(&str, &str, Reply, &str, Option<&str>)> = vec![
            ("beta", "# cf-carry:seed", Reply::fail(5, "__CF_CARRY_FAILED__ init"), codes::E_MOVE_CARRY, Some("seed")),
            ("alpha", "# cf-carry:snapshot", Reply::fail(5, "__CF_CARRY_FAILED__ bundle"), codes::E_MOVE_CARRY, Some("snapshot")),
            ("alpha", "# cf-carry:snapshot", Reply::fail(8, "__CF_BUNDLE_TOO_LARGE__ 999999999"), codes::E_MOVE_TOO_LARGE, None),
            ("alpha", "# cf-carry:chunk", Reply::ok(""), codes::E_MOVE_CARRY, Some("download")),
            ("beta", "# cf-carry:fetch", Reply::fail(5, "__CF_CARRY_FAILED__ verify"), codes::E_MOVE_CARRY, Some("fetch")),
            ("beta", "# cf-carry:apply", Reply::fail(9, "__CF_TARGET_DIRTY__"), codes::E_MOVE_TARGET_DIRTY, None),
            ("beta", "# cf-carry:apply", Reply::ok("?? surprise.txt\n"), codes::E_MOVE_CARRY, Some("verify")),
        ];
        for (host, marker, reply, code, step) in cases {
            let f = fixture();
            f.fake.on_host(host, Match::script_contains(marker), reply);
            let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            let err = run(&f, &hooks, false).await.unwrap_err();
            assert_eq!(err.code, code, "{marker}: {}", err.message);
            if let Some(step) = step {
                assert_eq!(err.details.as_ref().unwrap()["step"], step, "{marker}");
            }
            if code == codes::E_MOVE_TOO_LARGE {
                assert_eq!(err.details.as_ref().unwrap()["payload"], "bundle");
            }
            assert!(
                !f.fake.calls_for("beta").iter().any(|c| c.script().is_some_and(|s| s.contains("tmux new-session"))),
                "{marker}: the target session never started"
            );
            assert_source_untouched(&f, &hooks);
            if marker != "# cf-carry:seed" {
                assert!(
                    f.fake.calls_for("alpha").iter().any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:cleanup"))),
                    "{marker}: the source was cleaned up"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_clean_source_with_a_newer_target_skips_the_apply_and_warns() {
        let f = fixture();
        let newer = "2222222222222222222222222222222222222222";
        f.fake.on_host("beta", Match::script_contains("# cf-move:prep"), Reply::ok(&format!("{newer}\t{TGT_ENC}\t{}\t-1\n", tgt_path())));
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        assert!(rep.warnings.iter().any(|w| w.contains("newer on origin")));
        assert!(!f.fake.calls_for("beta").iter().any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:apply"))));

        // The same with a dirty source cannot replay onto another base.
        let f = fixture();
        f.fake
            .on_host("alpha", Match::script_contains("# cf-move:inspect"), Reply::ok(&inspection(" M a.rs", HEAD, "0")))
            .on_host("beta", Match::script_contains("# cf-move:prep"), Reply::ok(&format!("{newer}\t{TGT_ENC}\t{}\t-1\n", tgt_path())));
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_CARRY);
        assert_eq!(err.details.unwrap()["step"], "apply");
    }

    #[tokio::test]
    async fn an_ignored_files_failure_is_a_warning_not_a_failed_move() {
        let f = fixture();
        f.fake
            .on_host("alpha", Match::script_contains("# cf-carry:ignored-list"), Reply::Exit { code: 0, stdout: b"4\t.env\0".to_vec(), stderr: Vec::new() })
            .on_host("alpha", Match::script_contains("# cf-carry:ignored-pack"), Reply::fail(5, "__CF_CARRY_FAILED__ tar"));
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("the move still succeeds");
        assert!(rep.carried.ignored_carried.is_empty());
        assert!(rep.warnings.iter().any(|w| w.contains("ignored files were not carried")), "{:?}", rep.warnings);
    }
```

Update three existing tests:
- `happy_path_copies_the_transcript_resumes_on_target_and_kills_the_source`: uploads to `beta` are now two. Replace `assert_eq!(uploads.len(), 1);` and the two `uploads[0]` assertions with:

```rust
        assert_eq!(uploads.len(), 2, "the bundle, then the transcript");
        assert_eq!(uploads[0].command(), format!("cat > {}", quote(&format!("{TGT_DIR}/carry.bundle"))));
        assert_eq!(uploads[1].command(), format!("cat > {}", quote(&tgt_path())));
        assert_eq!(uploads[1].stdin_str().as_deref(), Some(TRANSCRIPT));
```

  and add `assert_eq!(rep.carried.commits, 0);`.
- `unpushed_branch_is_refused_before_the_target_is_touched` and `dirty_source_is_refused_and_lists_the_files`: these are strict-mode behaviours now. In each, replace every `run(&f, &hooks, false)` with:

```rust
        move_session_with(MoveSessionArgs { strict: true, ..args(&f, false) }, &f.store, &f.fake, &hooks, fast())
```

Other existing tests may assert things the carry legitimately changes. Fix the
assertion to say what it meant, never weaken it:
- "nothing was uploaded to `beta`" / "before the copy" (e.g.
  `diverged_target_worktree_is_refused_before_the_copy`,
  `a_larger_existing_target_transcript_is_never_overwritten`): the bundle is
  now uploaded before prep. Assert instead that no upload targeted the
  transcript path: `!uploads.iter().any(|c| c.command().contains(".jsonl"))`.
- exact-equality on `E_MOVE_TOO_LARGE` details
  (`transcript_over_the_cap_is_refused_before_it_is_read`): details gain
  `"payload": "transcript"`.
- `target_that_fails_to_start_leaves_the_source_untouched` and the partial-move
  tests need no change: the carry replies come from the fixture.

- [ ] **Step 2: Run — the new tests must fail**

Run: `cargo test -p fleet-core move_session::tests`
Expected: FAIL to compile — `no field carried on MoveReport`.

- [ ] **Step 3: Implement the plumbing**

Constants, next to `COPY_TIMEOUT`:

```rust
/// Wall clock for the seed (a clone) and the snapshot + bundle scripts.
const CARRY_TIMEOUT: Duration = Duration::from_secs(300);
```

`MoveReport` — add after `warnings`:

```rust
    /// What travelled besides the transcript.
    pub carried: carry::CarryReport,
```

`TempFile` — add a second constructor sharing the naming logic. Refactor `write` into:

```rust
    fn create(ext: &str) -> Result<(Self, std::fs::File), IpcError> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "claude-fleet-move-{}-{seq}-{nanos}.{ext}",
            std::process::id()
        ));
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok((TempFile(path), f))
    }

    fn write(bytes: &[u8]) -> Result<Self, IpcError> {
        use std::io::Write;
        let (guard, mut f) = Self::create("jsonl")?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(guard)
    }
```

(Keep the existing doc comment on the struct; the `SEQ` static moves into `create`, so the concurrent-uniqueness test keeps passing.)

`Snapshot` — add fields and fill them in `snapshot()`:

```rust
    bundle_cap: u64,
    ignored_entry_kb: u64,
    ignored_total_kb: u64,
```

```rust
    let setting = |key: &str, default: u64| {
        crate::service::settings::get_string(s, key).parse::<u64>().ok().filter(|n| *n > 0).unwrap_or(default)
    };
    let bundle_cap = setting(carry::SETTING_MAX_BUNDLE_MB, carry::DEFAULT_MAX_BUNDLE_MB).saturating_mul(1024 * 1024);
    let ignored_entry_kb = setting(carry::SETTING_IGNORED_ENTRY_KB, carry::DEFAULT_IGNORED_ENTRY_KB);
    let ignored_total_kb = setting(carry::SETTING_IGNORED_TOTAL_MB, carry::DEFAULT_IGNORED_TOTAL_MB).saturating_mul(1024);
```

`too_large` — add `"payload": "transcript"` to its details JSON, and add beside it:

```rust
fn bundle_too_large(bytes: u64, cap: u64) -> IpcError {
    IpcError::new(
        codes::E_MOVE_TOO_LARGE,
        format!(
            "the git bundle is {bytes} bytes, over the {} MiB cap ({}); push the branch first or raise the setting",
            cap / (1024 * 1024),
            carry::SETTING_MAX_BUNDLE_MB
        ),
    )
    .with_details(serde_json::json!({ "bytes": bytes, "cap_bytes": cap, "payload": "bundle" }))
}

/// A carry step failed before the target started.
fn carry_err(step: &str, what: &str, stderr: &str) -> IpcError {
    IpcError::new(
        codes::E_MOVE_CARRY,
        format!("move_session: carrying the work failed at {step}: {what} (the source session was not touched)"),
    )
    .with_details(serde_json::json!({ "step": step, "stderr": stderr }))
}
```

Relay helpers, in the `// ── transport ──` section:

```rust
/// Run a carry script with the long wall clock.
async fn sh_long(ssh: &dyn SshExec, host: &str, script: &str) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell_bounded(ssh, host, script, GIT_TIMEOUT, CARRY_TIMEOUT).await
}

/// Pull `bytes` of `remote_path` on `host` into a private temp file, one
/// [`carry::CHUNK_BYTES`] at a time, so a payload is never held in memory.
async fn download(ssh: &dyn SshExec, host: &str, remote_path: &str, bytes: u64, ext: &str) -> Result<TempFile, IpcError> {
    use std::io::Write;
    let (guard, mut f) = TempFile::create(ext)?;
    let mut got = 0u64;
    while got < bytes {
        let want = carry::CHUNK_BYTES.min(bytes - got);
        let out = sh(ssh, host, &carry::chunk_script(remote_path, got, want), COPY_TIMEOUT).await?;
        if !out.status.success() || out.stdout.is_empty() {
            return Err(carry_err("download", &format!("reading {remote_path} on {host} stopped at {got} of {bytes} bytes"), &stderr_of(&out)));
        }
        f.write_all(&out.stdout)?;
        got += out.stdout.len() as u64;
    }
    f.sync_all()?;
    if got != bytes {
        return Err(carry_err("download", &format!("{remote_path} on {host} gave {got} bytes, expected {bytes}"), ""));
    }
    Ok(guard)
}

/// Put a local file at `path` on `host` (a plain copy when `host` is local).
async fn put_file(ssh: &dyn SshExec, host: &str, local: &std::path::Path, path: &str) -> Result<(), IpcError> {
    if host == LOCAL {
        crate::service::hub::ensure_local_allowed(host)?;
        return tokio::fs::copy(local, path)
            .await
            .map(|_| ())
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("write {path}: {e}")));
    }
    ssh.upload_file(host, local, path, COPY_TIMEOUT).await
}

/// What the end-of-move cleanup must undo; filled in as the carry progresses.
#[derive(Default)]
struct CarryCleanup {
    id: String,
    /// (host, source worktree)
    source: Option<(String, String)>,
    /// (host, target project root)
    target: Option<(String, String)>,
}

impl CarryCleanup {
    /// Best effort on both hosts: a failure here never changes the move's result.
    async fn run(&self, ssh: &dyn SshExec) {
        for (host, dir) in self.source.iter().chain(self.target.iter()) {
            if let Err(e) = sh(ssh, host, &carry::cleanup_script(dir, &self.id), GIT_TIMEOUT).await {
                tracing::warn!(host = %host, error = %e.message, "[move_session] carry cleanup failed");
            }
        }
    }
}
```

- [ ] **Step 4: Implement the flow**

Rename today's `move_session_steps` to `move_session_inner` and give it one more parameter, `cleanup: &mut CarryCleanup`. Add the wrapper under the old name:

```rust
async fn move_session_steps(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
) -> Result<MoveReport, IpcError> {
    let mut cleanup = CarryCleanup::default();
    let result = move_session_inner(args, store, ssh, hooks, opts, &mut cleanup).await;
    cleanup.run(ssh).await;
    result
}
```

Inside `move_session_inner`:

**(a)** Replace `preflight_verdict(&state, &snap.branch)?;` with:

```rust
    if args.strict {
        preflight_verdict(&state, &snap.branch)?;
    } else {
        carry_verdict(&state, &snap.branch)?;
    }
    let mut carried = carry::CarryReport {
        dirty_entries: state.dirty.clone(),
        ..Default::default()
    };
```

**(b)** Directly after the `let (project_root, cwd_hint) = …;` block and **before** the existing best-effort `prefetch_script` call, seed the target:

```rust
    // 3a. Seed: the target needs a main clone before anything can be fetched
    //     into it. Never prompts; falls back to `git init` without origin.
    let clone_url = crate::repo_url::clone_url_for(&snap.owner, &snap.repo);
    let out = sh_long(ssh, &target, &carry::seed_script(&project_root, &clone_url)).await?;
    if !out.status.success() {
        return Err(carry_err("seed", &format!("preparing the clone at {project_root} on {target}"), &stderr_of(&out)));
    }
    carried.target_seeded = carry::parse_seed(&String::from_utf8_lossy(&out.stdout))?;
```

**(c)** Directly after the existing `prefetch_script` call (so the haves include a freshly fetched `origin/<branch>`), and before `let tmux_name = pick_target_name(…)`:

```rust
    // 3b. Carry the git state: snapshot + thin bundle on the source, relayed
    //     through this process, fetched into the target's main clone.
    let out = sh(ssh, &target, &carry::haves_script(&project_root, &id), GIT_TIMEOUT).await?;
    if !out.status.success() {
        return Err(carry_err("seed", &format!("listing refs in {project_root} on {target}"), &stderr_of(&out)));
    }
    let (target_dir, haves) = carry::parse_haves(&String::from_utf8_lossy(&out.stdout))?;
    cleanup.id = id.clone();
    cleanup.target = Some((target.clone(), project_root.clone()));
    cleanup.source = Some((src.clone(), state.worktree.clone()));

    let out = sh_long(ssh, &src, &carry::snapshot_script(&state.worktree, &id, &haves, snap.bundle_cap)).await?;
    if !out.status.success() {
        let err = stderr_of(&out);
        if let Some(rest) = err.split(carry::BUNDLE_TOO_LARGE).nth(1) {
            let bytes = rest.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
            return Err(bundle_too_large(bytes, snap.bundle_cap));
        }
        return Err(carry_err("snapshot", &format!("snapshotting {} on {src}", state.worktree), &err));
    }
    let bundle = carry::parse_snapshot(&String::from_utf8_lossy(&out.stdout))?;
    carried.commits = bundle.commits;
    carried.bundle_bytes = bundle.bytes;
    if bundle.submodules {
        warnings.push("the repository has submodules; their contents were not carried".into());
    }
    if bundle.lfs {
        warnings.push("the repository uses Git LFS; LFS objects were not carried".into());
    }
    let target_bundle = format!("{target_dir}/carry.bundle");
    {
        let local = download(ssh, &src, &bundle.path, bundle.bytes, "bundle").await?;
        put_file(ssh, &target, &local.0, &target_bundle)
            .await
            .map_err(|e| carry_err("upload", &format!("writing {target_bundle} on {target}: {}", e.message), ""))?;
    }
    let out = sh_long(ssh, &target, &carry::fetch_script(&project_root, &target_bundle, &id, &snap.branch)).await?;
    if !out.status.success() {
        return Err(carry_err("fetch", &format!("fetching the bundle into {project_root} on {target}"), &stderr_of(&out)));
    }
```

**(d)** After the existing `prep.existing` checks and **before** `put(ssh, &target, &prep.path, &bytes)`, replay and verify:

```rust
    // 3c. Replay the uncommitted work and check the claim: the target's
    //     porcelain must equal the source's.
    if prep.head != state.head {
        // Today's "newer on origin" case. A clean source has nothing to
        // replay; a dirty one cannot be replayed onto another base.
        if !state.dirty.is_empty() {
            return Err(carry_err(
                "apply",
                &format!("the target worktree is at {} while the source is at {}; uncommitted work cannot be replayed onto a different commit", prep.head, state.head),
                "",
            ));
        }
    } else {
        let out = sh(ssh, &target, &carry::apply_script(&cwd, &id, &state.head), GIT_TIMEOUT).await?;
        if !out.status.success() {
            let err = stderr_of(&out);
            if err.contains(carry::TARGET_DIRTY) {
                return Err(IpcError::new(
                    codes::E_MOVE_TARGET_DIRTY,
                    format!("move_session: the target worktree {cwd} on {target} has uncommitted changes of its own; commit or discard them there first (the source session was not touched)"),
                ));
            }
            return Err(carry_err("apply", &format!("replaying the work in {cwd} on {target}"), &err));
        }
        let line_set = |files: &[DirtyFile]| -> std::collections::BTreeSet<String> {
            files.iter().map(|d| format!("{}\t{}", d.status, d.path)).collect()
        };
        let (want, got) = (line_set(&state.dirty), line_set(&parse_porcelain(&String::from_utf8_lossy(&out.stdout))));
        if want != got {
            return Err(IpcError::new(
                codes::E_MOVE_CARRY,
                format!("move_session: after replaying the work, {cwd} on {target} does not match the source (the source session was not touched)"),
            )
            .with_details(serde_json::json!({ "step": "verify", "source": want, "target": got })));
        }
    }
    if !state.dirty.is_empty() {
        warnings.push(format!(
            "the source worktree {} on {src} still holds a copy of the uncommitted work",
            state.worktree
        ));
    }
    if carried.target_seeded == carry::TargetSeed::Initialized {
        warnings.push(format!(
            "origin was unreachable from {target}; the clone was initialised from the bundle and cannot fetch or push until origin is reachable"
        ));
    }

    // 3d. Small git-ignored files. Never fails the move.
    match carry_ignored(ssh, &src, &target, &state.worktree, &cwd, &target_dir, &id, &snap).await {
        Ok((kept, left)) => {
            carried.ignored_carried = kept;
            carried.ignored_left_behind = left;
        }
        Err((left, why)) => {
            carried.ignored_left_behind = left;
            warnings.push(format!("ignored files were not carried: {why}"));
        }
    }
```

`DirtyFile` needs importing in non-test code already (`use crate::service::safe_kill::{parse_porcelain, DirtyFile};` exists).

Add the helper next to `CarryCleanup`:

```rust
type Ignored = (Vec<carry::IgnoredEntry>, Vec<carry::LeftBehind>);

/// List, select, pack, relay and extract the small git-ignored files. `Err`
/// carries what was left behind plus the reason, for a report warning.
#[allow(clippy::too_many_arguments)]
async fn carry_ignored(
    ssh: &dyn SshExec,
    src: &str,
    target: &str,
    worktree: &str,
    cwd: &str,
    target_dir: &str,
    id: &str,
    snap: &Snapshot,
) -> Result<Ignored, (Vec<carry::LeftBehind>, String)> {
    let fail = |left: &[carry::LeftBehind], why: String| (left.to_vec(), why);
    let out = sh(ssh, src, &carry::ignored_list_script(worktree), COPY_TIMEOUT)
        .await
        .map_err(|e| fail(&[], e.message))?;
    if !out.status.success() {
        return Err(fail(&[], format!("listing on {src} failed: {}", stderr_of(&out))));
    }
    let sel = carry::select_ignored(carry::parse_ignored_list(&out.stdout), snap.ignored_entry_kb, snap.ignored_total_kb);
    if sel.carry.is_empty() {
        return Ok((Vec::new(), sel.left));
    }
    let paths: Vec<String> = sel.carry.iter().map(|e| e.path.clone()).collect();
    let out = sh(ssh, src, &carry::ignored_pack_script(worktree, id, &paths), COPY_TIMEOUT)
        .await
        .map_err(|e| fail(&sel.left, e.message))?;
    if !out.status.success() {
        return Err(fail(&sel.left, format!("packing on {src} failed: {}", stderr_of(&out))));
    }
    let (bytes, archive) = carry::parse_pack(&String::from_utf8_lossy(&out.stdout)).map_err(|e| fail(&sel.left, e.message))?;
    let local = download(ssh, src, &archive, bytes, "tgz").await.map_err(|e| fail(&sel.left, e.message))?;
    let target_archive = format!("{target_dir}/ignored.tgz");
    put_file(ssh, target, &local.0, &target_archive).await.map_err(|e| fail(&sel.left, e.message))?;
    let out = sh(ssh, target, &carry::ignored_extract_script(cwd, &target_archive), COPY_TIMEOUT)
        .await
        .map_err(|e| fail(&sel.left, e.message))?;
    if !out.status.success() {
        return Err(fail(&sel.left, format!("extracting on {target} failed: {}", stderr_of(&out))));
    }
    Ok((sel.carry, sel.left))
}
```

**(e)** In the `session_moved` detail JSON add `"carried": carried,` and in the final `MoveReport { .. }` add `carried,`. Because the JSON is built before the report, write `"carried": &carried` in the `json!` and move `carried` into the report afterwards.

**(f)** Rewrite the module docs at the top of `mod.rs` so steps 1–3 describe the carry (preflight no longer requires clean/pushed unless `strict`; add the snapshot/bundle/apply/ignored steps and the cleanup; keep the failure-handling paragraph, adding the three new codes). Point at ADR 0002.

- [ ] **Step 5: Run the full suite**

Run: `cargo test -p fleet-core`
Expected: PASS — all new flow tests, the three updated ones, and everything else. Then:

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings. (`await_holding_lock`: the new code takes no store lock across an await — if clippy flags one, restructure; do not `allow` it.)

- [ ] **Step 6: Commit**

```bash
git -C $WT add crates/fleet-core/src/service/move_session/mod.rs
git -C $WT commit -m "feat(move): carry uncommitted, unpushed and small ignored work to the target"
```

---

### Task 8: API surfaces, ADR and docs

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/params.rs:575-588`, `crates/fleet-core/src/mcp/tools/lifecycle.rs:340-397`
- Modify: `src/lib/moveSession.ts`
- Create: `docs/adr/0002-move-carries-work-as-is.md`
- Modify: `docs/adr/0001-descope-freeze-ship-move.md` (status line only)
- Regenerate: `docs/control-api-reference.md`
- Test: `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Consumes: `MoveSessionArgs.strict` (Task 6), `MoveReport.carried` (Task 7).
- Produces: MCP param `strict` (default `false`); TS `moveSession(sessionId, targetHostAlias, { keepSource?, strict? })` and `CarryReport` types.

- [ ] **Step 1: Write the failing MCP param test** (in `mcp/tools/tests.rs`, next to the other `MoveSessionParams` usage)

```rust
#[test]
fn move_session_strict_defaults_to_false() {
    let p: super::params::MoveSessionParams =
        serde_json::from_value(serde_json::json!({ "session_id": 1, "target_host_alias": "beta" })).unwrap();
    assert!(!p.strict && !p.keep_source);
    let p: super::params::MoveSessionParams =
        serde_json::from_value(serde_json::json!({ "session_id": 1, "target_host_alias": "beta", "strict": true })).unwrap();
    assert!(p.strict);
}
```

(If `tests.rs` reaches `params` through a different path, mirror the import the neighbouring tests use.)

Run: `cargo test -p fleet-core move_session_strict_defaults`
Expected: FAIL — no field `strict`.

- [ ] **Step 2: Implement the MCP side**

`params.rs`, in `MoveSessionParams` after `keep_source`:

```rust
    /// Refuse a dirty worktree (E_MOVE_DIRTY) or an unpushed branch
    /// (E_MOVE_UNPUSHED) instead of carrying them along. Default false.
    #[serde(default)]
    pub strict: bool,
```

`lifecycle.rs`: pass `strict: p.strict` in the `MoveSessionArgs` literal (replacing Task 6's `strict: false`), add `strict={}` to both `audit`/`confirm_gate` format strings, and replace the tool description with:

```rust
    #[tool(description = "Move a work session to another host with its work as \
        it is: copy the Claude transcript, carry the git state through the \
        fleet (unpushed commits, staged, modified and untracked files, plus \
        small git-ignored files such as .env — no origin needed, nothing is \
        pushed, committed or stashed, the source worktree is never modified), \
        create the worktree on the target, start it with --resume so the same \
        conversation continues, and only once the target is confirmed running \
        kill the source (keep_source=true leaves it running). strict=true \
        refuses a dirty worktree (E_MOVE_DIRTY) or an unpushed branch \
        (E_MOVE_UNPUSHED) instead of carrying them. Refused when the source is \
        mid merge/rebase (E_MOVE_MIDOP), when an existing target worktree has \
        its own uncommitted changes (E_MOVE_TARGET_DIRTY), when the transcript \
        is over move.max_transcript_mb or the bundle over move.max_bundle_mb \
        (E_MOVE_TOO_LARGE, details.payload), or when a carry step fails \
        (E_MOVE_CARRY, details.step). Nothing on the source changes before the \
        target is confirmed; a failure after the target started returns \
        E_MOVE_PARTIAL and leaves both sessions. Needs a token allowed on BOTH \
        hosts (in practice the master token). Gated by \
        mcp.confirm_destructive (retry with confirm_nonce). Returns a JSON \
        MoveReport: source_session_id, target_session_id, from_host, to_host, \
        tmux_name, transcript_bytes, source_killed, warnings, carried (commits, \
        bundle_bytes, dirty_entries, ignored_carried, ignored_left_behind, \
        target_seeded), target (the new row, parent_session_id = source).")]
```

- [ ] **Step 3: Regenerate the reference and run the suite**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
cargo test -p fleet-core
```

Expected: PASS; `git -C $WT status --short` shows `docs/control-api-reference.md` modified.

- [ ] **Step 4: Frontend types**

`src/lib/moveSession.ts` — add above `MoveReport`, add the field, and extend the wrapper:

```ts
/** What a move carried besides the transcript (mirrors `carry::CarryReport`). */
export interface CarryReport {
  /** Commits the target lacked (unpushed work included). */
  commits: number;
  bundle_bytes: number;
  /** `git status --porcelain` rows restored on the target. */
  dirty_entries: { status: string; path: string }[];
  ignored_carried: { path: string; bytes: number }[];
  /** `bytes` is null for a deny-listed entry: it is never sized. */
  ignored_left_behind: {
    path: string;
    bytes: number | null;
    reason: 'denylisted' | 'over_cap' | 'unsupported_name';
  }[];
  target_seeded: 'existing' | 'cloned' | 'initialized';
}
```

In `MoveReport`, after `warnings`: `carried: CarryReport;`

Replace the doc comment and signature:

```ts
/** Move a work session to another host with its work as it is: transcript,
 *  unpushed commits, uncommitted and untracked files, small git-ignored files.
 *  The source is killed once the target is confirmed (unless `keepSource`).
 *  `strict` refuses a dirty or unpushed source (`E_MOVE_DIRTY` /
 *  `E_MOVE_UNPUSHED`) instead of carrying it. Other refusals: `E_MOVE_MIDOP`,
 *  `E_MOVE_TARGET_DIRTY`, `E_MOVE_TOO_LARGE`, `E_MOVE_CARRY`; `E_MOVE_PARTIAL`
 *  leaves both sessions. */
export async function moveSession(
  sessionId: number,
  targetHostAlias: string,
  opts: { keepSource?: boolean; strict?: boolean } = {},
): Promise<Result<MoveReport>> {
  const r = await invokeCmd<MoveReport>('move_session', {
    args: {
      session_id: sessionId,
      target_host_alias: targetHostAlias,
      keep_source: opts.keepSource ?? false,
      strict: opts.strict ?? false,
    },
  });
  if (r.ok) mergeSession(r.value.target);
  return r;
}
```

Search the frontend for other constructions of a `MoveReport` (test fixtures, the Move dialog): `grep -rn "transcript_bytes" src/`. Add a `carried` value to each fixture:

```ts
carried: { commits: 0, bundle_bytes: 0, dirty_entries: [], ignored_carried: [], ignored_left_behind: [], target_seeded: 'existing' },
```

If a test asserts the exact `invokeCmd` args of `moveSession`, add `strict: false` to the expectation.

Run: `pnpm install --frozen-lockfile && npx vitest run && npx svelte-check`
Expected: PASS, 0 errors.

- [ ] **Step 5: ADR 0002**

`docs/adr/0002-move-carries-work-as-is.md`:

```markdown
# ADR 0002: `move_session` carries the work as it is

- Status: accepted
- Date: 2026-09-19
- Supersedes: the preflight clause of ADR 0001 (decision 2, first bullet:
  "The worktree must be clean (`E_MOVE_DIRTY`) and the branch pushed with
  nothing unpushed (`E_MOVE_UNPUSHED`)")
- Spec: `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`

## Context

ADR 0001 shipped `move_session` with the rule "the move never makes a git
decision for the user" and implemented it as a refusal: a dirty worktree or
an unpushed branch stopped the move. In practice almost every live session
is dirty or ahead of origin, some branches have no usable remote at all, and
a target host may not be able to reach or authenticate to origin. The move
was correct and rarely usable.

## Decision

The principle stays; the refusal goes.

> The move never pushes, never commits to or moves a user branch on the
> source, never stashes, and never modifies the source working tree or
> index. It carries the work as it is.

- The source worktree is snapshotted through a temporary index into two
  commits parked under `refs/fleet/transfer/<claude_id>/`. A `git bundle` of
  what the target lacks is relayed through the orchestrator (the only path
  that is guaranteed to exist), fetched into the target's main clone, and
  replayed with `git read-tree` so that uncommitted stays uncommitted,
  staged stays staged, untracked stays untracked and unpushed stays
  unpushed. The target's `git status --porcelain` is compared with the
  source's; a mismatch fails the move before the target session starts.
- Origin is never required. A target with no clone gets one by `git clone`,
  or by `git init` plus the bundle when origin is unreachable.
- Small git-ignored files travel in a separate archive under size caps and a
  deny-list; what stays behind is reported.
- What the move writes on the source: unreferenced git objects, the private
  ref namespace and a temp directory. The refs and the directory are removed
  when the move ends, on success and on failure.
- `strict: true` restores the ADR 0001 refusals for callers that want the
  guarantee that origin holds everything.

## Consequences

- A successful move leaves a copy of the uncommitted work in the source
  worktree. It is reported, not cleaned up: deleting user work is a separate,
  explicit action.
- An operation in progress (merge, rebase, cherry-pick, revert, bisect) is
  refused (`E_MOVE_MIDOP`); its state is not carried.
- Submodule contents, LFS objects, stashes, hooks and per-repo config do not
  travel. Untracked nested repositories fail the porcelain check rather than
  being dropped silently.
- Payloads pass through the orchestrator in 8 MiB chunks and a `0600` temp
  file, bounded by `move.max_bundle_mb`.
```

In ADR 0001 change the status line to:

```markdown
- Status: accepted; the preflight clause of decision 2 is superseded by ADR 0002
```

- [ ] **Step 6: Full local CI, unpiped**

Run: `scripts/ci-local.sh`
Expected: every stage green (fmt, clippy, workspace tests, reference doc, frontend tests, type-check, cargo-deny). Read the whole output; do not pipe it. If `cargo fmt --all --check` fails, run `cargo fmt --all` and re-run.

- [ ] **Step 7: Commit**

```bash
git -C $WT branch --show-current
git -C $WT add crates/fleet-core/src/mcp/tools/params.rs crates/fleet-core/src/mcp/tools/lifecycle.rs crates/fleet-core/src/mcp/tools/tests.rs docs/control-api-reference.md src/lib/moveSession.ts docs/adr/0001-descope-freeze-ship-move.md docs/adr/0002-move-carries-work-as-is.md
# plus any frontend fixture files Step 4 touched — add them by name
git -C $WT commit -m "feat(move): strict opt-out on the MCP tool and frontend, ADR 0002"
```

---

## After the plan

Not part of this plan, by design (spec → *Out of scope*): sidechain transcripts and Claude project memory (slice 2); the Transfer button, preflight sheet, progress events, wait-for-idle, retry-from-step, settings UI for the three new keys (slice 3). A manual end-to-end check on two real hosts (dirty + unpushed session, mac → linux) is worth doing before the PR; it needs the user's go-ahead because a dev build of the app terminates the installed one.
