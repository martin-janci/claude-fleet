# Move Carry Slice 2 (Claude-side State) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A moved session arrives with its per-session directory (subagent transcripts, tool results, title) and the project's Claude memory, merged into the target without ever overwriting what is already there.

**Architecture:** A new pure module `service/move_session/claude_state.rs` holds the selection / merge policies, the script builders and the parsers, reusing `carry.rs`'s guards, output marker and pack/extract helpers. `mod.rs` gains one step after the ignored files with two independent halves (`carry_session_state`, `carry_memory`); each can only warn. `CarryReport` gains two wire fields, mirrored in TypeScript and in the hub routing payload.

**Tech Stack:** Rust (fleet-core), bash + tar + git on the hosts, `FakeSsh` flow tests, real-bash/real-tar tests on both CI runners, TypeScript mirror, the `carry_e2e` cross-host example.

**Spec:** `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md` — read it before any task. Slice 1's spec and `docs/adr/0002-move-carries-work-as-is.md` are the background.

## Global Constraints

- **Both halves can only warn.** No error from this step may abort a move; warnings are exactly `session state was not carried: <why>` and `project memory was not carried: <why>`. Every line of the step runs before `hooks.start_target`.
- **Never overwrite on the target.** Session files: move into place iff the target has no such file or a **strictly smaller** one. Memory files: only names the target lacks. `MEMORY.md`: existing lines never rewritten or reordered; only appended to.
- Every value interpolated into a script goes through `crate::shell::quote`. No other quoting helper.
- Scripts run under `bash -lc` on macOS (BSD userland) **and** Linux (GNU): no single-family flags outside an explicit branch. Consumed stdout is preceded by `printf '\n__CF_OUT__\n'` (`carry::OUT_MARKER`) and read only through `carry::payload` / a parser built on it; a failure prints `__CF_CARRY_FAILED__ <word>` on stderr and exits non-zero **without** the marker (documented exception: a streaming list script may fail after the marker — callers check the exit status first).
- Encoded project dir names **begin with `-`**: every path operand is absolute, or follows `--`, or is `./`-prefixed.
- The whole script is ONE argv word (`bash -lc '<script>'`, ≤128 KiB on Linux): nothing unbounded may be interpolated. Bounds (exact): 200 tar excludes, 300 memory files, 32 KiB of index text.
- Exact values: setting `move.max_session_state_mb` default **200**, range 1–4096; memory bounds **1 MiB** per file, **8 MiB** total, **300** files; index read bound **256 KiB**; allowed session path charset `[A-Za-z0-9._/-]` without `..`; allowed memory file name `[A-Za-z0-9._-]+\.md`.
- Wire rule (`service::repo_read`): every new report type derives `Serialize + Deserialize`, **no `#[serde(default)]`**. The routed payload in `src-tauri/src/backend/tests_routing.rs` and the TS mirror change with it.
- Never hold the `Store` mutex guard across an `.await`. `carry.rs`'s existing public signatures do not change (additions and `pub(super)` visibility only).
- Tests that need `bash`/`tar`/`git` use `carry::tests::require(&[..])` (panics under `CI` instead of skipping). If a test the plan gives you fails, fix the **script**, never weaken the assertion; if you believe an assertion is wrong, stop and report with evidence.
- After every task: the whole `cargo test -p fleet-core`, unfiltered, output redirected to a file and read from it — never judged through `| tail`/`| grep`. Frontend (when `src/` is touched): `pnpm install --frozen-lockfile`, `npx vitest run`, `npx svelte-check`. **`svelte-check` is the authority on TypeScript errors**: the editor's diagnostics on `src/lib/SessionDetails.test.ts` have been stale before.
- Known wall-clock tests that fail only under machine load (not yours): `parallel_reconcile_does_not_serialise_on_slow_host`, `sigkill_during_the_request_leaves_no_token_on_disk`, `a_connection_that_never_handshakes_is_hung_up_on`.
- Git: only this worktree, every command `git -C <worktree>`; never pull/push/fetch/rebase/merge/checkout/switch/reset/stash/clean; before each commit confirm `git -C <wt> branch --show-current` prints `feature/move-carry-claude-state`; add by explicit path (`.reticle/` and `.superpowers/` are not ours); no attribution lines.
- Do not run the desktop app or any `cargo tauri` command (a dev build migrates the production database).

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/move_session/claude_state.rs` | **New.** Pure: constants, selection + merge policies, script builders, parsers, tests. |
| `crates/fleet-core/src/service/move_session/carry.rs` | Report types gain two fields; `pack_script` / `extract_keep_existing_script` generalised from the ignored ones; guards + `payload_str` + `parse_err` become `pub(super)`; test helpers `bash`/`git` become `pub(crate)`. |
| `crates/fleet-core/src/service/move_session/mod.rs` | `pub mod claude_state;`, `Snapshot.session_state_cap`, `carry_session_state`, `carry_memory`, the step, flow tests. |
| `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte` | The new setting, its mirrored max, its row. |
| `src/lib/moveSession.ts`, `src-tauri/src/backend/tests_routing.rs`, `src/lib/SessionDetails.test.ts` | Wire mirror, routed payload, fixture. |
| `crates/fleet-core/src/mcp/tools/lifecycle.rs`, `docs/control-api-reference.md`, `docs/adr/0002-move-carries-work-as-is.md`, `CLAUDE.md` | Tool description, regenerated reference, "what travels". |
| `crates/fleet-core/examples/carry_e2e.rs` | Cross-host checks for both halves. |

---

### Task 1: The wire contract — report fields, the setting, the mirrors

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/carry.rs` (types near `CarryReport`; the round-trip test)
- Create: `crates/fleet-core/src/service/move_session/claude_state.rs`
- Modify: `crates/fleet-core/src/service/move_session/mod.rs` (`pub mod claude_state;`)
- Modify: `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte`
- Modify: `src/lib/moveSession.ts`, `src/lib/SessionDetails.test.ts`, `src-tauri/src/backend/tests_routing.rs`

**Interfaces:**
- Produces (in `carry`): `SessionStateReport { carried: Vec<IgnoredEntry>, kept_target: Vec<String>, left_behind: Vec<LeftBehind> }`, `MemoryReport { carried: Vec<IgnoredEntry>, kept_target: Vec<String>, identical: u32, index_lines_added: u32, left_behind: Vec<LeftBehind> }` (both `Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize`), and `CarryReport.session_state` / `CarryReport.memory`.
- Produces (in `claude_state`): `SETTING_MAX_SESSION_STATE_MB: &str = "move.max_session_state_mb"`, `DEFAULT_MAX_SESSION_STATE_MB: u64 = 200`. In `settings`: `MOVE_MAX_SESSION_STATE_MB`, `MOVE_MAX_SESSION_STATE_MB_MAX: u64 = 4096`. In TS: `SETTING_KEYS.moveMaxSessionStateMb`, `MOVE_MAX_SESSION_STATE_MB_MAX = 4096`.

- [ ] **Step 1: Failing tests.** In `carry.rs` `mod tests`, extend `carry_report_round_trips_and_a_missing_field_fails_loudly`: the literal gains

```rust
            session_state: SessionStateReport {
                carried: vec![IgnoredEntry { path: "subagents/agent-ab12.jsonl".into(), bytes: 2048 }],
                kept_target: vec!["custom-title.json".into()],
                left_behind: vec![LeftBehind { path: "subagents/agent-ff00.jsonl".into(), bytes: Some(900_000_000), reason: LeftReason::OverCap }],
            },
            memory: MemoryReport {
                carried: vec![IgnoredEntry { path: "build-notes.md".into(), bytes: 512 }],
                kept_target: vec!["deploy.md".into()],
                identical: 3,
                index_lines_added: 1,
                left_behind: Vec::new(),
            },
```

and two more loud-failure assertions after the existing one:

```rust
        for field in ["session_state", "memory"] {
            let mut missing = serde_json::to_value(&report).unwrap();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<CarryReport>(missing).is_err(), "{field}");
        }
```

In `settings.rs` tests, next to `carry_settings_have_specs_defaults_and_bounds`:

```rust
    #[test]
    fn session_state_cap_has_a_spec_a_default_and_bounds() {
        assert_eq!(spec(MOVE_MAX_SESSION_STATE_MB).unwrap().default, "200");
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "1").is_ok());
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "0").is_err());
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "4096").is_ok());
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "4097").is_err());
        assert_eq!(resolve(MOVE_MAX_SESSION_STATE_MB, Some("50")), "50");
    }
```

Run: `cargo test -p fleet-core carry_report_round_trips session_state_cap` → FAIL to compile.

- [ ] **Step 2: Implement the Rust side.** `carry.rs`, after `LeftBehind`:

```rust
/// What travelled of the per-session directory (`<project dir>/<id>/`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStateReport {
    /// Files merged into place on the target (path inside `<id>/`).
    pub carried: Vec<IgnoredEntry>,
    /// The target already had an equal or larger copy; it was kept.
    pub kept_target: Vec<String>,
    pub left_behind: Vec<LeftBehind>,
}

/// What travelled of the project's Claude memory (`<repo root>/memory/`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReport {
    pub carried: Vec<IgnoredEntry>,
    /// Same name on both hosts, different contents: the target's file stays.
    pub kept_target: Vec<String>,
    pub identical: u32,
    /// Lines appended to the target's `MEMORY.md`.
    pub index_lines_added: u32,
    pub left_behind: Vec<LeftBehind>,
}
```

and in `CarryReport`, after `target_seeded`: `pub session_state: SessionStateReport,` and `pub memory: MemoryReport,`.

New file `claude_state.rs`:

```rust
//! The Claude-side state of a moved session: its per-session directory
//! (subagent transcripts, tool results, title) and the project's memory.
//! Pure, like `carry.rs`: policies, script builders, parsers — `mod.rs` runs
//! the scripts. Both halves only ever ADD to the target.
//! See `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`.

/// `settings` key: largest per-session directory (MiB) a move carries.
pub const SETTING_MAX_SESSION_STATE_MB: &str = "move.max_session_state_mb";
pub const DEFAULT_MAX_SESSION_STATE_MB: u64 = 200;
```

`mod.rs`: `pub mod claude_state;` next to `pub mod carry;`. `settings.rs`: following the `MOVE_MAX_BUNDLE_MB` pattern exactly — `pub const MOVE_MAX_SESSION_STATE_MB_MAX: u64 = 4096;` (doc: "Upper bound for [`MOVE_MAX_SESSION_STATE_MB`]: one transfer's payload."), `pub const MOVE_MAX_SESSION_STATE_MB: &str = crate::service::move_session::claude_state::SETTING_MAX_SESSION_STATE_MB;` (doc: "Largest per-session Claude directory (MiB) `move_session` carries; the biggest files stay behind above it."), and a `Spec { key: MOVE_MAX_SESSION_STATE_MB, default: "200", kind: Kind::Int { min: 1, max: MOVE_MAX_SESSION_STATE_MB_MAX } }` after the `MOVE_IGNORED_TOTAL_MB` entry.

- [ ] **Step 3: The mirrors.** `src/lib/fleet_settings.ts`: `moveMaxSessionStateMb: 'move.max_session_state_mb',` after `moveIgnoredTotalMb`; default `'move.max_session_state_mb': '200',`; and

```ts
/** Mirror of `settings::MOVE_MAX_SESSION_STATE_MB_MAX` (`Kind::Int { min: 1, max }`). */
export const MOVE_MAX_SESSION_STATE_MB_MAX = 4096;
```

`src/lib/SettingsDialog.svelte`: import the constant; after the `limit-move-ignored-total-mb` field add

```svelte
      <div class="mcp-field">
        <label class="lbl" for="limit-move-session-state-mb">carry session</label>
        <input class="port" id="limit-move-session-state-mb" type="number" min="1" max={MOVE_MAX_SESSION_STATE_MB_MAX} step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.moveMaxSessionStateMb)}
          disabled={limitsBusy}
          aria-describedby="limit-move-session-state-desc"
          data-testid="move-session-state-mb"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.moveMaxSessionStateMb, 'Move session state', e)} />
        <span class="hook-desc" id="limit-move-session-state-desc">largest per-session Claude directory (MiB, 1–{MOVE_MAX_SESSION_STATE_MB_MAX}: subagent transcripts, tool results) Move to host… carries; above it the biggest files stay behind</span>
      </div>
```

`src/lib/moveSession.ts`, in `CarryReport` after `target_seeded`:

```ts
  /** The per-session directory (subagent transcripts, tool results, title). */
  session_state: {
    carried: { path: string; bytes: number }[];
    /** The target already had an equal or larger copy. */
    kept_target: string[];
    left_behind: { path: string; bytes: number | null; reason: 'denylisted' | 'over_cap' | 'unsupported_name' }[];
  };
  /** The project's Claude memory; the target's own files are never replaced. */
  memory: {
    carried: { path: string; bytes: number }[];
    kept_target: string[];
    identical: number;
    index_lines_added: number;
    left_behind: { path: string; bytes: number | null; reason: 'denylisted' | 'over_cap' | 'unsupported_name' }[];
  };
```

Add to every `carried: {…}` literal in `src/` (`grep -rn "target_seeded" src/`): `session_state: { carried: [], kept_target: [], left_behind: [] }, memory: { carried: [], kept_target: [], identical: 0, index_lines_added: 0, left_behind: [] },`. In `src-tauri/src/backend/tests_routing.rs` `MOVE_PAYLOAD`, insert after `"target_seeded":"existing"`:

```
,"session_state":{"carried":[{"path":"subagents/agent-ab12.jsonl","bytes":2048}],"kept_target":[],"left_behind":[]},"memory":{"carried":[],"kept_target":["deploy.md"],"identical":3,"index_lines_added":0,"left_behind":[]}
```

- [ ] **Step 4: Verify.** `cargo test -p fleet-core` (whole suite, incl. the settings↔dialog parity test), `cargo test -p claude-fleet` (routing table), `pnpm install --frozen-lockfile && npx vitest run && npx svelte-check`, `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`. All green.

- [ ] **Step 5: Commit.**

```bash
git -C $WT add crates/fleet-core/src/service/move_session/carry.rs crates/fleet-core/src/service/move_session/claude_state.rs crates/fleet-core/src/service/move_session/mod.rs crates/fleet-core/src/service/settings.rs src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte src/lib/moveSession.ts src/lib/SessionDetails.test.ts src-tauri/src/backend/tests_routing.rs
git -C $WT commit -m "feat(move): report fields and the cap for the Claude-side state"
```

---

### Task 2: The pure policies

**Files:** Modify `crates/fleet-core/src/service/move_session/claude_state.rs`.

**Interfaces:**
- Consumes: `carry::{IgnoredEntry, LeftBehind, LeftReason, payload}`.
- Produces (all `pub`):
  - consts `MAX_SESSION_EXCLUDES: usize = 200`, `MEMORY_FILE_MAX_BYTES: u64 = 1 << 20`, `MEMORY_TOTAL_MAX_BYTES: u64 = 8 << 20`, `MEMORY_MAX_FILES: usize = 300`, `INDEX_READ_MAX_BYTES: u64 = 256 * 1024`, `INDEX_APPEND_MAX_BYTES: usize = 32 * 1024`, `INDEX_NAME: &str = "MEMORY.md"`
  - `struct ListedFile { path: String, bytes: u64 }`; `fn parse_file_list(stdout: &[u8]) -> Vec<ListedFile>` — `<bytes>\t<path>\0` records after the marker; no marker → empty
  - `struct SessionSelection { carry: Vec<IgnoredEntry>, exclude: Vec<String>, left: Vec<LeftBehind>, skip: Option<String> }`; `fn select_session_files(listed: Vec<ListedFile>, cap_bytes: u64) -> SessionSelection`
  - `struct ListedMemory { hash: String, bytes: u64, name: String }`; `struct MemoryListing { dir: String, exists: bool, files: Vec<ListedMemory> }`; `fn parse_memory_list(stdout: &[u8]) -> Option<MemoryListing>`
  - `struct MemoryDecision { carry: Vec<IgnoredEntry>, kept_target: Vec<String>, identical: u32, left: Vec<LeftBehind> }`; `fn decide_memory(source: &[ListedMemory], target: &[ListedMemory]) -> MemoryDecision`
  - `struct IndexMerge { append: String, lines: u32 }`; `fn merge_index(source_index: &str, target_index: Option<&str>, carried: &[String]) -> IndexMerge`
  - `fn exclude_pattern(path: &str) -> String` — the path with every char outside `[A-Za-z0-9._/-]` replaced by `?`

- [ ] **Step 1: Failing tests** (new `#[cfg(test)] mod tests` at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::move_session::carry::{LeftReason, OUT_MARKER};

    fn marked(body: &[u8]) -> Vec<u8> {
        let mut v = format!("banner\n\n{OUT_MARKER}\n").into_bytes();
        v.extend_from_slice(body);
        v
    }
    fn f(path: &str, bytes: u64) -> ListedFile {
        ListedFile { path: path.into(), bytes }
    }
    fn m(name: &str, hash: &str, bytes: u64) -> ListedMemory {
        ListedMemory { hash: hash.into(), bytes, name: name.into() }
    }

    #[test]
    fn file_list_parses_after_the_marker_and_is_empty_without_one() {
        let got = parse_file_list(&marked(b"2048\tsubagents/agent-ab.jsonl\x0012\tcustom-title.json\x00garbage\x00"));
        assert_eq!(got.len(), 2, "the record without a tab is dropped");
        assert_eq!((got[0].path.as_str(), got[0].bytes), ("subagents/agent-ab.jsonl", 2048));
        assert!(parse_file_list(b"12\tno-marker\0").is_empty());
    }

    #[test]
    fn under_the_cap_the_whole_session_directory_travels() {
        let sel = select_session_files(vec![f("subagents/a.jsonl", 600), f("tool-results/x.txt", 300), f("custom-title.json", 20)], 1000);
        assert!(sel.exclude.is_empty() && sel.left.is_empty() && sel.skip.is_none());
        assert_eq!(sel.carry.len(), 3);
    }

    #[test]
    fn over_the_cap_the_largest_files_stay_behind_first() {
        let sel = select_session_files(vec![f("subagents/big.jsonl", 900), f("subagents/mid.jsonl", 400), f("subagents/small.jsonl", 100), f("custom-title.json", 20)], 600);
        assert_eq!(sel.exclude, vec!["subagents/big.jsonl"], "dropping the largest is enough: 520 <= 600");
        assert_eq!(sel.left[0].reason, LeftReason::OverCap);
        assert_eq!(sel.left[0].bytes, Some(900));
        let carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(carried.len(), 3);
        assert!(!carried.contains(&"subagents/big.jsonl"));
    }

    #[test]
    fn odd_names_are_excluded_by_a_wildcard_pattern_never_interpolated_raw() {
        let sel = select_session_files(vec![f("ok.json", 1), f("we ird*[x].txt", 1), f("../escape", 1), f("a/../b", 1)], u64::MAX);
        assert_eq!(sel.carry.len(), 1);
        assert_eq!(sel.left.iter().filter(|l| l.reason == LeftReason::UnsupportedName).count(), 3);
        assert!(sel.exclude.contains(&"we?ird??x?.txt".to_string()), "{:?}", sel.exclude);
        assert!(sel.exclude.iter().all(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || "._/-?".contains(c))));
        assert_eq!(exclude_pattern("a b"), "a?b");
    }

    #[test]
    fn too_many_exclusions_skip_the_half_instead_of_building_a_huge_command() {
        let many: Vec<ListedFile> = (0..MAX_SESSION_EXCLUDES + 5).map(|i| f(&format!("subagents/a{i:04}.jsonl"), 1000)).collect();
        let sel = select_session_files(many, 1); // nothing fits → every file would be excluded
        assert!(sel.skip.as_deref().is_some_and(|w| w.contains("200")), "{:?}", sel.skip);
        assert!(sel.carry.is_empty() && sel.exclude.is_empty());
    }

    #[test]
    fn memory_list_needs_its_directory_record() {
        let l = parse_memory_list(&marked(b"dir\t/h/.claude/projects/-r/memory\t1\0aaa\t10\tnote.md\0bbb\tx\tbad.md\0")).unwrap();
        assert_eq!((l.dir.as_str(), l.exists, l.files.len()), ("/h/.claude/projects/-r/memory", true, 1));
        assert!(parse_memory_list(&marked(b"aaa\t10\tnote.md\0")).is_none(), "no dir record");
        assert!(parse_memory_list(&marked(b"dir\trelative\t1\0")).is_none(), "the dir must be absolute");
        assert!(parse_memory_list(b"dir\t/x\t1\0").is_none(), "no marker");
    }

    #[test]
    fn memory_only_ever_adds_and_the_index_never_travels_as_a_file() {
        let d = decide_memory(
            &[m("new.md", "h1", 100), m("same.md", "h2", 50), m("differs.md", "h3", 70), m("MEMORY.md", "h4", 30), m("we ird.md", "h5", 5), m("note.txt", "h6", 5), m("huge.md", "h7", MEMORY_FILE_MAX_BYTES + 1)],
            &[m("same.md", "h2", 50), m("differs.md", "OTHER", 99), m("MEMORY.md", "hX", 10)],
        );
        assert_eq!(d.carry.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["new.md"]);
        assert_eq!(d.kept_target, vec!["differs.md"]);
        assert_eq!(d.identical, 1);
        let reason = |n: &str| d.left.iter().find(|l| l.path == n).map(|l| l.reason);
        assert_eq!(reason("we ird.md"), Some(LeftReason::UnsupportedName));
        assert_eq!(reason("note.txt"), Some(LeftReason::UnsupportedName), "only *.md is memory");
        assert_eq!(reason("huge.md"), Some(LeftReason::OverCap));
        assert!(d.left.iter().all(|l| l.path != "MEMORY.md"));
    }

    #[test]
    fn memory_stops_at_the_total_and_count_bounds() {
        let many: Vec<ListedMemory> = (0..MEMORY_MAX_FILES + 2).map(|i| m(&format!("n{i:04}.md"), "h", 10)).collect();
        let d = decide_memory(&many, &[]);
        assert_eq!(d.carry.len(), MEMORY_MAX_FILES);
        assert_eq!(d.left.len(), 2);
        let big: Vec<ListedMemory> = (0..10).map(|i| m(&format!("b{i}.md"), "h", MEMORY_FILE_MAX_BYTES)).collect();
        assert_eq!(decide_memory(&big, &[]).carry.len(), 8, "8 MiB in total");
    }

    #[test]
    fn an_index_line_travels_only_with_its_carried_file() {
        let src = "# Memory Index\n\n- [New](new.md) — hook\n- [Kept](differs.md) — src view\n- plain line without a link\n- [Two](new.md) and [other](x.md)\n- [Dot](./dotted.md) — dot-slash link\n";
        let carried = vec!["new.md".to_string(), "dotted.md".to_string()];
        let got = merge_index(src, Some("# Memory Index\n\n- [Mine](mine.md) — target\n"), &carried);
        assert_eq!(got.append, "- [New](new.md) — hook\n- [Two](new.md) and [other](x.md)\n- [Dot](./dotted.md) — dot-slash link\n");
        assert_eq!(got.lines, 3);
        // no trailing newline on the target: start on a fresh line
        assert!(merge_index(src, Some("- [Mine](mine.md)"), &carried).append.starts_with("\n- [New]"));
        // no index on the target: create one with a header
        assert!(merge_index(src, None, &carried).append.starts_with("# Memory Index\n\n- [New]"));
        // nothing carried → nothing appended, not even a header
        let none = merge_index(src, None, &[]);
        assert_eq!((none.append.as_str(), none.lines), ("", 0));
    }

    #[test]
    fn the_index_append_is_bounded_and_cannot_close_the_heredoc() {
        let line = format!("- [N](n.md) {}\n", "x".repeat(1000));
        let src = format!("{}CF_INDEX\n", line.repeat(100)); // ~100 KiB, plus a hostile bare delimiter line
        let got = merge_index(&src, Some(""), &["n.md".to_string()]);
        assert!(got.append.len() <= INDEX_APPEND_MAX_BYTES, "{}", got.append.len());
        assert!(got.lines > 0 && got.append.ends_with('\n'));
        assert!(!got.append.lines().any(|l| l == "CF_INDEX"));
    }
}
```

Run: `cargo test -p fleet-core claude_state::tests` → FAIL to compile.

- [ ] **Step 2: Implement.**

```rust
use crate::service::move_session::carry::{payload, IgnoredEntry, LeftBehind, LeftReason};

pub const MAX_SESSION_EXCLUDES: usize = 200;
pub const MEMORY_FILE_MAX_BYTES: u64 = 1 << 20;
pub const MEMORY_TOTAL_MAX_BYTES: u64 = 8 << 20;
pub const MEMORY_MAX_FILES: usize = 300;
pub const INDEX_READ_MAX_BYTES: u64 = 256 * 1024;
pub const INDEX_APPEND_MAX_BYTES: usize = 32 * 1024;
pub const INDEX_NAME: &str = "MEMORY.md";
/// The heredoc delimiter of the index-append script; never allowed as a line.
pub(super) const INDEX_HEREDOC: &str = "CF_INDEX";

fn records(stdout: &[u8]) -> impl Iterator<Item = Vec<&str>> {
    payload(stdout)
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|r| std::str::from_utf8(r).ok())
        .map(|r| r.split('\t').collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedFile {
    pub path: String,
    pub bytes: u64,
}

/// `<bytes>\t<path>\0` records after the marker. No marker, a non-UTF-8 or a
/// malformed record → dropped: a listing can never fail a move.
pub fn parse_file_list(stdout: &[u8]) -> Vec<ListedFile> {
    records(stdout)
        .filter_map(|p| match p.as_slice() {
            [n, path] => Some(ListedFile { path: path.to_string(), bytes: n.trim().parse().ok()? }),
            _ => None,
        })
        .collect()
}

fn safe_session_path(p: &str) -> bool {
    !p.is_empty()
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        && !p.split('/').any(|seg| seg == ".." || seg.is_empty())
}

/// A tar exclude pattern for `path` that cannot carry shell or glob syntax of
/// the caller's making: every char outside the safe set becomes `?` (one-char
/// wildcard on GNU and BSD tar alike).
pub fn exclude_pattern(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() || "._/-".contains(c) { c } else { '?' }).collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSelection {
    pub carry: Vec<IgnoredEntry>,
    /// Exclude patterns, relative to `<id>/`.
    pub exclude: Vec<String>,
    pub left: Vec<LeftBehind>,
    /// Set when the half must be skipped; `carry`/`exclude` are then empty.
    pub skip: Option<String>,
}

/// The whole directory travels unless it is over `cap_bytes`; then the
/// largest files stay behind, one by one, until the rest fits.
pub fn select_session_files(listed: Vec<ListedFile>, cap_bytes: u64) -> SessionSelection {
    let mut sel = SessionSelection::default();
    let mut ok: Vec<ListedFile> = Vec::new();
    for f in listed {
        if safe_session_path(&f.path) {
            ok.push(f);
        } else {
            sel.exclude.push(exclude_pattern(&f.path));
            sel.left.push(LeftBehind { path: f.path, bytes: Some(f.bytes), reason: LeftReason::UnsupportedName });
        }
    }
    ok.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let mut total: u64 = ok.iter().fold(0u64, |t, f| t.saturating_add(f.bytes));
    let mut keep_from = 0;
    while total > cap_bytes && keep_from < ok.len() {
        let f = &ok[keep_from];
        total -= f.bytes;
        sel.exclude.push(f.path.clone());
        sel.left.push(LeftBehind { path: f.path.clone(), bytes: Some(f.bytes), reason: LeftReason::OverCap });
        keep_from += 1;
    }
    if sel.exclude.len() > MAX_SESSION_EXCLUDES {
        return SessionSelection {
            left: sel.left,
            skip: Some(format!("more than {MAX_SESSION_EXCLUDES} files would have to stay behind; raise {SETTING_MAX_SESSION_STATE_MB}")),
            ..Default::default()
        };
    }
    sel.carry = ok[keep_from..].iter().map(|f| IgnoredEntry { path: f.path.clone(), bytes: f.bytes }).collect();
    sel.carry.sort_by(|a, b| a.path.cmp(&b.path));
    sel
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMemory {
    pub hash: String,
    pub bytes: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryListing {
    /// Absolute path of the memory dir (it may not exist yet).
    pub dir: String,
    pub exists: bool,
    pub files: Vec<ListedMemory>,
}

/// First record `dir\t<abs path>\t<0|1>`, then `<hash>\t<bytes>\t<name>`.
pub fn parse_memory_list(stdout: &[u8]) -> Option<MemoryListing> {
    let mut recs = records(stdout);
    let (dir, exists) = match recs.next()?.as_slice() {
        ["dir", d, e] if d.starts_with('/') => (d.to_string(), *e == "1"),
        _ => return None,
    };
    let files = recs
        .filter_map(|p| match p.as_slice() {
            [h, n, name] => Some(ListedMemory { hash: h.to_string(), bytes: n.trim().parse().ok()?, name: name.to_string() }),
            _ => None,
        })
        .collect();
    Some(MemoryListing { dir, exists, files })
}

fn safe_memory_name(n: &str) -> bool {
    n.len() > 3 && n.ends_with(".md") && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryDecision {
    pub carry: Vec<IgnoredEntry>,
    pub kept_target: Vec<String>,
    pub identical: u32,
    pub left: Vec<LeftBehind>,
}

/// Memory only ever adds: a name the target has is never carried, and the
/// index is merged line-wise elsewhere, never copied.
pub fn decide_memory(source: &[ListedMemory], target: &[ListedMemory]) -> MemoryDecision {
    let mut d = MemoryDecision::default();
    let mut src: Vec<&ListedMemory> = source.iter().filter(|f| f.name != INDEX_NAME).collect();
    src.sort_by(|a, b| a.name.cmp(&b.name));
    let mut total = 0u64;
    for f in src {
        let left = |reason| LeftBehind { path: f.name.clone(), bytes: Some(f.bytes), reason };
        if !safe_memory_name(&f.name) {
            d.left.push(left(LeftReason::UnsupportedName));
        } else if let Some(t) = target.iter().find(|t| t.name == f.name) {
            if t.hash == f.hash { d.identical += 1 } else { d.kept_target.push(f.name.clone()) }
        } else if f.bytes > MEMORY_FILE_MAX_BYTES
            || d.carry.len() >= MEMORY_MAX_FILES
            || total.saturating_add(f.bytes) > MEMORY_TOTAL_MAX_BYTES
        {
            d.left.push(left(LeftReason::OverCap));
        } else {
            total += f.bytes;
            d.carry.push(IgnoredEntry { path: f.name.clone(), bytes: f.bytes });
        }
    }
    d
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexMerge {
    /// Exactly the text to append to the target's index ("" → do nothing).
    pub append: String,
    pub lines: u32,
}

/// The file a line's FIRST markdown link points at (`](name.md)`), without
/// a leading `./`.
fn link_target(line: &str) -> Option<&str> {
    let rest = &line[line.find("](")? + 2..];
    let t = &rest[..rest.find(')')?];
    Some(t.strip_prefix("./").unwrap_or(t))
}

/// An index line travels only with its carried file. The target's own lines
/// are never touched — this only produces text to append.
pub fn merge_index(source_index: &str, target_index: Option<&str>, carried: &[String]) -> IndexMerge {
    let mut body = String::new();
    let mut lines = 0u32;
    for line in source_index.lines() {
        let travels = link_target(line).is_some_and(|t| carried.iter().any(|c| c == t));
        if !travels || line == INDEX_HEREDOC {
            continue;
        }
        if body.len() + line.len() + 1 > INDEX_APPEND_MAX_BYTES - 64 {
            break;
        }
        body.push_str(line);
        body.push('\n');
        lines += 1;
    }
    if lines == 0 {
        return IndexMerge::default();
    }
    let prefix = match target_index {
        None => "# Memory Index\n\n",
        Some(t) if !t.is_empty() && !t.ends_with('\n') => "\n",
        Some(_) => "",
    };
    IndexMerge { append: format!("{prefix}{body}"), lines }
}
```

- [ ] **Step 3: Verify.** `cargo test -p fleet-core claude_state::tests`, then the whole `cargo test -p fleet-core`, `cargo fmt --all`, `cargo clippy -p fleet-core --all-targets -- -D warnings`.

- [ ] **Step 4: Commit.** `git -C $WT add crates/fleet-core/src/service/move_session/claude_state.rs` → `feat(move): selection and merge policies for the Claude-side state`

---

### Task 3: Session-directory scripts

**Files:** Modify `claude_state.rs`; modify `carry.rs` (visibility only).

**Interfaces:**
- Consumes: from `carry` — make `id_guard`, `home_guard`, `payload_str`, `parse_err` `pub(super)`; in `carry::tests` make `bash` and `git` `pub(crate)` (next to the already `pub(crate)` `require`). No signature changes.
- Produces (all `pub`, in `claude_state`):
  - `fn session_list_script(project_dir: &str, claude_id: &str) -> String` → `parse_file_list`
  - `fn session_pack_script(project_dir: &str, claude_id: &str, excludes: &[String]) -> String` → `carry::parse_pack` (prints `<bytes>\t<abs archive path>`; archive `state.tgz`)
  - `fn session_merge_script(target_project_dir: &str, claude_id: &str, archive: &str) -> String`
  - `struct MergeResult { carried: Vec<IgnoredEntry>, kept: Vec<String>, failed: Vec<String> }`; `fn parse_merge(stdout: &str) -> Option<MergeResult>` (lines `carried\t<bytes>\t<path>`, `kept\t<path>`, `failed\t<path>`; `None` without the marker)

- [ ] **Step 1: Failing tests** (in `claude_state::tests`; they run REAL bash + tar + find). Required behaviour — write these tests:

```rust
    use crate::service::move_session::carry::tests::{bash, require};
    use crate::shell::quote;
    use std::os::unix::fs::PermissionsExt;

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    /// A source project dir whose NAME begins with `-`, like every real one.
    fn source_project(root: &std::path::Path) -> std::path::PathBuf {
        let p = root.join("-Users-me-r--claude-worktrees-feat");
        for (rel, body) in [
            ("subagents/agent-aa.jsonl", "aaaaaaaaaa\n".repeat(50)),   // 550 B
            ("subagents/agent-aa.meta.json", "{}".to_string()),
            ("subagents/agent-bb.jsonl", "b\n".repeat(2000)),          // 4000 B — the largest
            ("tool-results/out1.txt", "tool output\n".to_string()),
            ("workflows/w1/step.json", "{}".to_string()),
            ("custom-title.json", "{\"title\":\"t\"}".to_string()),
        ] {
            let f = p.join(ID).join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, body).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::os::unix::fs::symlink("/etc/hosts", p.join(ID).join("link-out")).unwrap();
        std::fs::write(p.join(format!("{ID}.jsonl")), "main transcript\n").unwrap(); // must NOT be listed or packed
        p
    }
```

  1. `session_state_is_listed_packed_staged_and_merged` — list → `parse_file_list` yields exactly the six regular files with exact byte sizes (the symlink and the sibling `<id>.jsonl` are absent); `select_session_files(.., u64::MAX)`; pack with no excludes → `carry::parse_pack` → archive exists, bytes match, archive mode `0600`, transfer dir `0700`; merge into a **fresh** target project dir (also `-`-leading) → `parse_merge`: six `carried`, none kept/failed; on disk: contents identical, files `0600`, created dirs `0700`, **no `link-out`**, the staging dir is gone, and nothing was written outside `<target>/<id>/`.
  2. `merge_keeps_an_equal_or_larger_target_copy_and_replaces_a_smaller_one` — target pre-populated with: `subagents/agent-aa.jsonl` **smaller** than the source's, `subagents/agent-bb.jsonl` **larger** (its own extra lines), `custom-title.json` byte-identical, and `subagents/agent-own.jsonl` that only the target has → after the merge: `aa` replaced (equals the source), `bb` and the title untouched and reported `kept`, `agent-own.jsonl` untouched; `carried` lists exactly the files that landed.
  3. `excluded_files_do_not_travel_on_either_tar` — pack with `excludes = ["subagents/agent-bb.jsonl", exclude_pattern("we ird.txt")]` after adding a file literally named `we ird.txt` → `tar -tzf` of the archive lists neither; everything else is there. (This is the GNU-vs-BSD `--exclude` anchoring check: it must pass on macOS and on the Ubuntu runner.)
  4. `a_source_without_a_session_directory_lists_nothing_and_succeeds` — list against a project dir with no `<id>/` → exit 0, marker present, empty list.
  5. `session_scripts_refuse_a_bad_id_fail_cleanly_and_quote_everything` — `""`, `"a/b"`, `".."` → non-zero + `carry::FAILED` for all three scripts, and a sibling `…/transfer/other/keep.txt` survives; a corrupt archive → merge exits non-zero with the sentinel, **no marker**, target untouched; every script contains `quote(evil)` for each interpolated value with `evil = "a b'$(touch /tmp/pwn)\n;x"` and never the raw `touch /tmp/pwn`; a banner-prefixed run of the list and merge scripts (`printf 'Welcome'; ` with no trailing newline) still parses.

Run: `cargo test -p fleet-core claude_state::tests` → FAIL to compile.

- [ ] **Step 2: Implement.**

```rust
use crate::service::move_session::carry::{home_guard, id_guard, payload_str, FAILED, OUT_MARKER};
use crate::shell::quote;

/// Every regular file under `<project dir>/<id>/` as `<bytes>\t<path>\0`,
/// the path relative to `<id>/`. Symlinks and special files are not listed —
/// and the merge moves regular files only, so they never travel. No such
/// directory is not an error. Streaming: a `find` failure is detected after
/// the marker; callers check the exit status first.
pub fn session_list_script(project_dir: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:state-list
set +e
d={d}
id={id}
{id_guard}
cd -- "$d" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
[ -d "./$id" ] || exit 0
cd -- "./$id" || {{ printf '{FAILED} cd-id\n' >&2; exit 5; }}
find . -type f -exec sh -c 'for f; do n=$(wc -c < "$f" | tr -d " "); printf "%s\t%s\0" "${{n:-0}}" "${{f#./}}"; done' _ {{}} +
[ "$?" -eq 0 ] || {{ printf '{FAILED} find\n' >&2; exit 5; }}
"#,
        d = quote(project_dir),
        id = quote(claude_id),
        id_guard = id_guard(),
    )
}

/// One tar of `./<id>` minus `excludes` (patterns relative to `<id>/`, each
/// given in both member-name spellings so GNU and BSD tar agree).
pub fn session_pack_script(project_dir: &str, claude_id: &str, excludes: &[String]) -> String {
    let ex: Vec<String> = excludes
        .iter()
        .flat_map(|p| [format!("--exclude=./{claude_id}/{p}"), format!("--exclude={claude_id}/{p}")])
        .map(|a| quote(&a))
        .collect();
    format!(
        r#"# cf-carry:state-pack
set +e
d={d}
id={id}
{id_guard}
{home_guard}
umask 077
cd -- "$d" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
dir="$HOME/.cache/claude-fleet/transfer/$id"
mkdir -p -- "$dir" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
COPYFILE_DISABLE=1 tar -czf "$dir/state.tgz" {ex} "./$id" >/dev/null 2>&1 || {{ printf '{FAILED} tar\n' >&2; exit 5; }}
n=$(wc -c < "$dir/state.tgz" | tr -d ' ')
[ -n "$n" ] || {{ printf '{FAILED} size\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
printf '%s\t%s\n' "$n" "$dir/state.tgz"
"#,
        d = quote(project_dir),
        id = quote(claude_id),
        ex = ex.join(" "),
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

/// Extract into a staging dir inside the transfer dir — never in place —
/// then move each staged REGULAR file into `<target project dir>/<id>/` iff
/// the target has no such file or a strictly smaller one (these files are
/// append-only: the larger copy is the newer one).
pub fn session_merge_script(target_project_dir: &str, claude_id: &str, archive: &str) -> String {
    format!(
        r#"# cf-carry:state-merge
set +e
d={d}
id={id}
a={a}
{id_guard}
{home_guard}
umask 077
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
stage="$HOME/.cache/claude-fleet/transfer/$id/state-staging"
rm -rf -- "$stage"
mkdir -p -- "$stage" || fail mkdir
tar -tzf "$a" >/dev/null 2>&1 || fail corrupt
tar -xzf "$a" -C "$stage" >/dev/null 2>&1 || fail extract
mkdir -p -- "$d/$id" || fail mkdir-target
printf '\n{OUT_MARKER}\n'
if cd -- "$stage/$id" 2>/dev/null; then
  find . -type f -print0 | while IFS= read -r -d '' f; do
    rel=${{f#./}}
    dst="$d/$id/$rel"
    s=$(wc -c < "$f" | tr -d ' ')
    t=-1
    if [ -f "$dst" ] && [ ! -L "$dst" ]; then t=$(wc -c < "$dst" | tr -d ' '); elif [ -e "$dst" ] || [ -L "$dst" ]; then t=999999999999; fi
    if [ "${{t:--1}}" -lt "${{s:-0}}" ]; then
      if mkdir -p -- "$(dirname -- "$dst")" && mv -f -- "$f" "$dst"; then printf 'carried\t%s\t%s\n' "$s" "$rel"; else printf 'failed\t%s\n' "$rel"; fi
    else
      printf 'kept\t%s\n' "$rel"
    fi
  done
fi
cd / 2>/dev/null
rm -rf -- "$stage"
exit 0
"#,
        d = quote(target_project_dir),
        id = quote(claude_id),
        a = quote(archive),
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeResult {
    pub carried: Vec<IgnoredEntry>,
    pub kept: Vec<String>,
    pub failed: Vec<String>,
}

pub fn parse_merge(stdout: &str) -> Option<MergeResult> {
    let mut r = MergeResult::default();
    for line in payload_str(stdout)?.lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["carried", n, path] => r.carried.push(IgnoredEntry { path: path.to_string(), bytes: n.trim().parse().ok()? }),
            ["kept", path] => r.kept.push(path.to_string()),
            ["failed", path] => r.failed.push(path.to_string()),
            _ => {}
        }
    }
    Some(r)
}
```

A destination that exists but is not a regular file (a directory, a symlink) is treated as "larger" → `kept`: the merge never replaces something it does not understand.

- [ ] **Step 3: Verify** (focused → whole suite → fmt → clippy), zero `skipping:` lines.

- [ ] **Step 4: Commit.** `carry.rs` + `claude_state.rs` → `feat(move): list, pack and merge the per-session Claude directory`

---

### Task 4: Memory scripts

**Files:** Modify `claude_state.rs`; modify `carry.rs` (generalise pack/extract).

**Interfaces:**
- Produces in `carry` (additions; the two `ignored_*` functions keep their signatures and now delegate):
  - `pub fn pack_script(dir: &str, claude_id: &str, archive_name: &str, paths: &[String]) -> String` — `ignored_pack_script(w, id, p)` = `pack_script(w, id, "ignored.tgz", p)`; the marker comment becomes `# cf-carry:pack` for the new function while `ignored_pack_script` keeps emitting `# cf-carry:ignored-pack` (pass the marker name as a private parameter — existing `FakeSsh` rules match on it)
  - `pub fn extract_keep_existing_script(dir: &str, archive: &str, create_dir: bool) -> String` — `ignored_extract_script(c, a)` = the same with `create_dir = false` and its existing `# cf-carry:ignored-extract` marker; with `create_dir = true` the script first does `umask 077; mkdir -p -- "$cwd"`
- Produces in `claude_state` (all `pub`):
  - `fn memory_list_script(repo_dir: &str, fallback_dir: Option<&str>) -> String` → `parse_memory_list`
  - `fn memory_read_index_script(memory_dir: &str) -> String`; `fn parse_index(stdout: &str) -> Option<Option<String>>` — `None`: no marker; `Some(None)`: no index file; `Some(Some(text))`
  - `fn memory_append_index_script(memory_dir: &str, text: &str) -> String`
  - `const MEMORY_ARCHIVE: &str = "memory.tgz"`

- [ ] **Step 1: Failing tests** (real bash, git, tar; `HOME` isolated by the `bash` helper). Required:
  1. `memory_is_found_by_the_repo_root_not_the_worktree` — a repo at `<tmp>/r` with a linked worktree `<tmp>/wt`; memory placed at `$HOME/.claude/projects/<enc(pwd -P of r)>/memory/` with `note.md`, `MEMORY.md`, a `sub/` directory, a symlinked `.md`, and `notes.txt` → `memory_list_script(<wt path>, Some(<wt path>))` reports `exists = true`, that dir, and exactly `note.md` + `MEMORY.md` with hashes equal to `git hash-object --no-filters` of the files. Then remove that memory dir and create one under the **worktree's** encoded name → the fallback is reported. With neither → `exists = false` and `dir` = the repo-root one (the dir a target would create).
  2. `memory_files_are_added_and_never_replaced` — source memory `new.md`, `differs.md`; target memory `differs.md` (other content) and no dir at all in a second case → `carry::pack_script(src_memory_dir, ID, MEMORY_ARCHIVE, &["new.md"])` → `extract_keep_existing_script(target_memory_dir, archive, true)`; assert `new.md` arrived `0600`, the target's `differs.md` is byte-identical to before, a missing target dir is created `0700`. Also pack **both** names and extract over the existing `differs.md`: it still wins.
  3. `the_index_is_only_ever_appended_to` — target `MEMORY.md` = `"# Memory Index\n\n- [Mine](mine.md) — t"` (no trailing newline); `merge_index(..)` → `memory_append_index_script` → file = original bytes + `"\n"` + appended lines, mode unchanged; with no index on the target and `append` starting with the header → the file is created `0600` with exactly `append`; `text = ""` → the script is a no-op and creates nothing; `memory_read_index_script` round-trips content with quotes, `$(…)`, backslashes and a line `__CF_OUT__`-lookalike mid-line, and reports a missing file as `Some(None)`.
  4. `memory_scripts_fail_cleanly_and_quote_everything` — quoting as in Task 3; `memory_list_script` on a path that is not a git repo → non-zero + sentinel; ignored-file behaviour unchanged: the existing `ignored_*` tests still pass **untouched**.

- [ ] **Step 2: Implement.** In `carry.rs` turn the body of `ignored_pack_script` into

```rust
fn pack_script_as(marker: &str, dir: &str, claude_id: &str, archive_name: &str, paths: &[String]) -> String
```

(marker names are binding, because `FakeSsh` rules match on them: the generic functions emit `# cf-carry:pack` and `# cf-carry:extract`; the ignored ones keep `# cf-carry:ignored-pack` / `# cf-carry:ignored-extract`, and neither generic name is a substring of those) — the existing body with `# cf-carry:{marker}`, `wt` → the given dir, and `"$dir/ignored.tgz"` → `"$dir/"{archive}` where `archive = quote(archive_name)` is interpolated as `"$dir"/{archive}`), then

```rust
pub fn ignored_pack_script(worktree: &str, claude_id: &str, paths: &[String]) -> String {
    pack_script_as("ignored-pack", worktree, claude_id, "ignored.tgz", paths)
}
/// [`ignored_pack_script`] for any directory and archive name.
pub fn pack_script(dir: &str, claude_id: &str, archive_name: &str, paths: &[String]) -> String {
    pack_script_as("pack", dir, claude_id, archive_name, paths)
}
```

and the same for extract (`extract_script_as(marker, dir, archive, create_dir)`). In `claude_state.rs`:

```rust
pub const MEMORY_ARCHIVE: &str = "memory.tgz";

/// Where a repo's Claude memory lives and what is in it. Claude Code keys
/// memory by the MAIN checkout (the parent of the git common dir), not by the
/// worktree; `fallback_dir` (the worktree itself) is tried when that has no
/// `memory/`. First record `dir\t<abs>\t<0|1>` — reported even when the dir
/// does not exist, because a target creates it there.
pub fn memory_list_script(repo_dir: &str, fallback_dir: Option<&str>) -> String {
    format!(
        r#"# cf-carry:memory-list
set +e
r={r}
fb={fb}
{home_guard}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
enc() {{ ( cd -- "$1" 2>/dev/null && pwd -P ) | sed 's/[^A-Za-z0-9]/-/g'; }}
cd -- "$r" 2>/dev/null || fail cd
g=$(git rev-parse --git-common-dir 2>/dev/null) || fail not-a-repo
top=$( cd -- "$g" 2>/dev/null && cd .. && pwd -P ) || fail common-dir
[ -n "$top" ] || fail common-dir
m="$HOME/.claude/projects/$(enc "$top")/memory"
if [ ! -d "$m" ] && [ -n "$fb" ]; then
  alt="$HOME/.claude/projects/$(enc "$fb")/memory"
  [ -d "$alt" ] && m="$alt"
fi
printf '\n{OUT_MARKER}\n'
if [ -d "$m" ]; then printf 'dir\t%s\t1\0' "$m"; else printf 'dir\t%s\t0\0' "$m"; exit 0; fi
for f in "$m"/*.md; do
  [ -f "$f" ] && [ ! -L "$f" ] || continue
  h=$(git hash-object --no-filters -- "$f" 2>/dev/null) || continue
  n=$(wc -c < "$f" | tr -d ' ')
  printf '%s\t%s\t%s\0' "$h" "${{n:-0}}" "$(basename -- "$f")"
done
exit 0
"#,
        r = quote(repo_dir),
        fb = quote(fallback_dir.unwrap_or("")),
        home_guard = home_guard(),
    )
}

/// The target's or source's `MEMORY.md`: after the marker, `present` or
/// `absent` on the first line, then at most [`INDEX_READ_MAX_BYTES`] + 1
/// bytes of content (one over, so the caller can tell "too large").
pub fn memory_read_index_script(memory_dir: &str) -> String {
    format!(
        r#"# cf-carry:memory-index
set +e
m={m}
f="$m/{INDEX_NAME}"
printf '\n{OUT_MARKER}\n'
if [ -f "$f" ] && [ ! -L "$f" ]; then printf 'present\n'; head -c {max} -- "$f"; else printf 'absent\n'; fi
exit 0
"#,
        m = quote(memory_dir),
        max = INDEX_READ_MAX_BYTES + 1,
    )
}

pub fn parse_index(stdout: &str) -> Option<Option<String>> {
    let body = payload_str(stdout)?;
    match body.split_once('\n') {
        Some(("present", text)) => Some(Some(text.to_string())),
        Some(("absent", _)) => Some(None),
        None if body == "absent" => Some(None),
        _ => None,
    }
}

/// Append `text` (from [`merge_index`]) to the target's index. The text
/// reaches the file through a QUOTED heredoc, so nothing in it is expanded;
/// `merge_index` never emits a line equal to the delimiter. Empty text → a
/// no-op that creates nothing.
pub fn memory_append_index_script(memory_dir: &str, text: &str) -> String {
    if text.is_empty() {
        return "# cf-carry:memory-append\nexit 0\n".to_string();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    format!(
        r#"# cf-carry:memory-append
set +e
m={m}
umask 077
mkdir -p -- "$m" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
f="$m/{INDEX_NAME}"
if [ -L "$f" ]; then printf '{FAILED} symlink\n' >&2; exit 5; fi
cat >> "$f" <<'{INDEX_HEREDOC}' || {{ printf '{FAILED} append\n' >&2; exit 5; }}
{body}
{INDEX_HEREDOC}
printf 'ok\n'
"#,
        m = quote(memory_dir),
    )
}
```

Mind the heredoc + `||` placement: bash requires the here-document to start on the line after the command line, so `cat >> "$f" <<'CF_INDEX' || { …; }` must be written on ONE line followed by the body. If real bash disagrees with the exact form above, fix the form — the behaviour in test 3 is what matters.

- [ ] **Step 3: Verify** (focused → whole suite, including every pre-existing `ignored_*` test unchanged → fmt → clippy).

- [ ] **Step 4: Commit.** `feat(move): find, list and merge the project's Claude memory`

---

### Task 5: Wire the step into the move

**Files:** Modify `crates/fleet-core/src/service/move_session/mod.rs`.

**Interfaces:**
- Consumes: everything from Tasks 1–4; existing `sh`, `download`, `put_file`, `stderr_of`, `COPY_TIMEOUT`, `Snapshot`, `located.path`, `prep.path`, `project_root`, `state.worktree`, `target_dir`, `id`, `carried`, `warnings`.
- Produces: `Snapshot.session_state_cap: u64` (bytes); `async fn carry_session_state(..) -> Result<carry::SessionStateReport, (carry::SessionStateReport, String)>`; `async fn carry_memory(..) -> Result<carry::MemoryReport, (carry::MemoryReport, String)>`; the step, placed after the ignored-files `match` and before `put(ssh, &target, &prep.path, &bytes)`.

- [ ] **Step 1: Fixture + failing flow tests.** In `fixture()` add default replies so every existing test keeps its exact behaviour (no session directory, no memory):

```rust
            .on_host("alpha", Match::script_contains("# cf-carry:state-list"), Reply::ok(&out("")))
            .on_host("alpha", Match::script_contains("# cf-carry:memory-list"), Reply::Exit { code: 0, stdout: { let mut b = out("").into_bytes(); b.extend_from_slice(b"dir\t/home/a/.claude/projects/-r/memory\t0\0"); b }, stderr: Vec::new() })
```

New tests (names binding; assert behaviour, not the fixture):
  1. `the_claude_side_state_travels_and_is_reported` — alpha lists two session files; pack/chunk replies as for the ignored archive; beta's `state-merge` replies `carried`/`kept` lines → `rep.carried.session_state` has them; alpha lists memory `new.md` + `differs.md` + `MEMORY.md`, beta lists `differs.md` (other hash) → `memory.carried == [new.md]`, `kept_target == [differs.md]`; the index scripts reply a source index with a line for each → exactly one `memory-append` script is sent to beta and its text contains the `new.md` line and **not** the `differs.md` line; `index_lines_added == 1`; **no warnings**; the `session_moved` event detail carries both reports; the pack script for the session dir was given the SOURCE project dir (parent of the located transcript) and the merge script the TARGET one (parent of `prep.path`); the memory list on beta was given `project_root`.
  2. `each_half_failing_is_a_warning_and_the_other_half_still_runs` — table: state-list exit 5; state-pack exit 5; state-merge exit 5 (and: merge reporting a `failed\t…` line); memory-list exit 5 on alpha; on beta; memory pack exit 5; extract exit 5; append exit 5; a transport error (`Reply::Unreachable`) on a state script → in every row the move **succeeds**, exactly the matching warning is present (`session state was not carried:` / `project memory was not carried:`), and the *other* half's report is populated.
  3. `a_source_with_no_claude_state_sends_no_pack_and_warns_nothing` — the default fixture: no `state-pack`, no `# cf-carry:pack`, no `memory-append` script anywhere; `warnings.is_empty()`.
  4. `an_index_over_the_read_bound_is_left_alone_with_a_warning` — beta's index reply is `INDEX_READ_MAX_BYTES + 1` bytes → files are still carried, no append script is sent, one warning mentions the index.
  5. `the_session_state_cap_comes_from_the_setting` — set `move.max_session_state_mb` to `1`; alpha lists a 2 MiB and a 10 KiB file → the pack script contains an `--exclude` for the big one; it is reported `over_cap`.
  6. Extend `carry_failures_leave_the_source_untouched_and_still_clean_up`: none of the new scripts is sent when a slice-1 carry step fails (they come later).

Run → FAIL to compile.

- [ ] **Step 2: Implement.** `Snapshot`: `session_state_cap: u64`, filled with `setting(claude_state::SETTING_MAX_SESSION_STATE_MB, claude_state::DEFAULT_MAX_SESSION_STATE_MB).saturating_mul(1024 * 1024)`. A small private relay helper removes the repetition the two halves would otherwise share with `carry_ignored` (do **not** refactor `carry_ignored` itself in this task):

```rust
/// Pull `archive` (`bytes` long) off `src` and put it at `to` on `target`.
async fn relay(ssh: &dyn SshExec, src: &str, archive: &str, bytes: u64, target: &str, to: &str) -> Result<(), String> {
    let local = download(ssh, src, archive, bytes, "tgz").await.map_err(|e| e.message)?;
    put_file(ssh, target, &local.0, to).await.map_err(|e| e.message)
}

/// Run a script whose failure is only ever a warning: `Err(why)`.
async fn sh_soft(ssh: &dyn SshExec, host: &str, script: &str, what: &str) -> Result<std::process::Output, String> {
    let out = sh(ssh, host, script, COPY_TIMEOUT).await.map_err(|e| format!("{what} on {host}: {}", e.message))?;
    if out.status.success() { Ok(out) } else { Err(format!("{what} on {host} failed: {}", stderr_of(&out))) }
}
```

`carry_session_state(ssh, src, target, src_project_dir, tgt_project_dir, target_dir, id, cap)`: list (`sh_soft`) → `parse_file_list` → empty → `Ok(default)`; `select_session_files` → `skip` → `Err((report with left_behind, why))`; pack with `sel.exclude` → `carry::parse_pack` → `relay` to `{target_dir}/state.tgz` → merge → `parse_merge` (`None` → `Err`) → report `{ carried: merged.carried, kept_target: merged.kept, left_behind: sel.left }`; a non-empty `merged.failed` → `Err((that report, format!("{} file(s) could not be placed on {target}: {}", n, first three names)))`. Every `Err` carries the best report built so far, so `left_behind` is never lost.

`carry_memory(ssh, src, target, src_worktree, tgt_project_root, target_dir, id)`: list source (`memory_list_script(src_worktree, Some(src_worktree))`) → `None` → `Err`; `!exists` or no files → `Ok(default)`; list target (`memory_list_script(tgt_project_root, None)`) → `decide_memory`; if `carry` non-empty: `carry::pack_script(&source.dir, id, claude_state::MEMORY_ARCHIVE, &names)` → `relay` to `{target_dir}/memory.tgz` → `carry::extract_keep_existing_script(&target.dir, .., true)`; then the index: read both (`parse_index`), skip with a warning-worthy `Err` **after** recording the carried files if either is over `INDEX_READ_MAX_BYTES` or unreadable; `merge_index` → if `lines > 0` send `memory_append_index_script(&target.dir, &merge.append)`; report.

The step, after the ignored-files `match`:

```rust
    // 3e. The Claude-side state: the per-session directory and the project's
    //     memory. Both only ever add on the target, and neither can fail the move.
    let parent = |p: &str| std::path::Path::new(p).parent().map(|d| d.to_string_lossy().into_owned());
    match (parent(&located.path), parent(&prep.path)) {
        (Some(src_dir), Some(tgt_dir)) => {
            match carry_session_state(ssh, &src, &target, &src_dir, &tgt_dir, &target_dir, &id, snap.session_state_cap).await {
                Ok(r) => carried.session_state = r,
                Err((r, why)) => {
                    carried.session_state = r;
                    warnings.push(format!("session state was not carried: {why}"));
                }
            }
        }
        _ => warnings.push("session state was not carried: the transcript has no parent directory".into()),
    }
    match carry_memory(ssh, &src, &target, &state.worktree, &project_root, &target_dir, &id).await {
        Ok(r) => carried.memory = r,
        Err((r, why)) => {
            carried.memory = r;
            warnings.push(format!("project memory was not carried: {why}"));
        }
    }
```

Update the module docs (step list: what now travels) — and **no** `?` anywhere in this block.

- [ ] **Step 3: Verify.** `cargo test -p fleet-core move_session::`, the whole suite, `cargo test -p claude-fleet`, fmt, `cargo clippy --workspace --all-targets -- -D warnings`. Re-read the block once more for a stray `?` or an early `return`.

- [ ] **Step 4: Commit.** `feat(move): the session directory and the project memory travel with a move`

---

### Task 6: Docs, the tool description, and the cross-host harness

**Files:** Modify `crates/fleet-core/src/mcp/tools/lifecycle.rs`; regenerate `docs/control-api-reference.md`; modify `docs/adr/0002-move-carries-work-as-is.md`, `CLAUDE.md`, `crates/fleet-core/examples/carry_e2e.rs`.

- [ ] **Step 1: Tool description.** In the `move_session` `#[tool(description = …)]`: after the sentence listing what is carried, add `"It also carries the session's Claude directory (subagent transcripts, tool results, title; up to move.max_session_state_mb, biggest files stay behind above it) and the project's Claude memory — adding files the target lacks and appending their MEMORY.md lines, never replacing anything there; neither can fail the move (warnings)."` and extend the `MoveReport` field list: `carried (…, session_state, memory)`. Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, confirm `docs/control-api-reference.md` changed, and that the plain run passes.

- [ ] **Step 2: ADR 0002.** Under *Decision*, a new bullet: the Claude-side state travels — the per-session directory merged file by file (a file lands only where the target has none or a strictly smaller one: these files are append-only, so the larger copy is the newer one, which keeps a return trip correct) and the project memory (keyed by the repo root, not the worktree; only names the target lacks; index lines travel with their carried files; the target's files and index lines are never rewritten). Under *Consequences*: host-specific notes travel like any other; transcript content is never rewritten, so its absolute `tool-results` paths are stale on the target. `CLAUDE.md`: extend the existing move sentence with "and the session's Claude directory and project memory (slice 2 spec `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`)".

- [ ] **Step 3: `carry_e2e`.** Per scenario, before the move: a source project dir `<lt>/projects/-cf-e2e-src-<name>/` with `<id>/subagents/agent-aa.jsonl` (+ `.meta.json`), `tool-results/o.txt`, `custom-title.json`; on the target a project dir `<rt>/projects/-cf-e2e-tgt-<name>/` pre-populated with a **larger** `subagents/agent-aa.jsonl` in the `cloned` scenario only. Memory: the source's real `$HOME/.claude/projects/<enc(repo root)>/memory/` (the repo root is under the run's temp dir, so the name contains `cf-e2e`) with `new.md`, `differs.md`, `MEMORY.md`; the target's memory dir for its project root with its own `differs.md` + a one-line `MEMORY.md` **without** a trailing newline. Run the real scripts in the flow's order (list → select → pack → chunked relay → merge; list both → decide → pack → relay → extract → read both indexes → merge → append) and check: session files identical on the target, `0600`, dirs `0700`, the larger pre-existing copy kept (cloned scenario) and everything carried (initialized scenario); `new.md` arrived, the target's `differs.md` byte-identical to before, the index = old bytes + newline + exactly the `new.md` line. Teardown removes, on both hosts, only dirs whose name contains `cf-e2e` under `~/.claude/projects/` (guard the `rm -rf` on that substring) — and verifies they are gone. Keep it `cargo fmt` + `cargo clippy -D warnings` clean; do **not** run it against a remote host yourself — the controller does that.

- [ ] **Step 4: Full local CI.** `scripts/ci-local.sh`, whole output to a file, every stage green, zero `skipping:` lines.

- [ ] **Step 5: Commit.** `docs(move): what travels now; carry_e2e covers the Claude-side state` (two commits if docs and harness separate cleanly).

**Controller, after Task 6:** run `cargo run -p fleet-core --example carry_e2e -- mefistos` (the user approved that host for this harness), verify zero leftovers on both hosts, and put the result in the PR body.

---

## After the plan

Re-fetch and check `HEAD..origin/main` before the final merge: this repo gains dozens of commits a day, a textually clean merge has twice hidden a semantic break, and GitHub shows a PR as CONFLICTING when `main` touches a file this branch renamed (merge `origin/main` locally, re-run the full CI, push). Out of scope here and owned by slice 3: showing the two report fields, wait-for-idle, retry / return trip.
