# Fleet Observability & Self-Heal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close claude-fleet's observability and self-heal gaps — give operators per-session activity, status, context-pressure, stuck-detection, controller safety, and a fleet-wide health view — while fixing the four concrete UX/correctness bugs found in live use.

**Architecture:** A single per-host reconcile pane-tail read is the keystone: one extra `tmux capture-pane` per work session, parsed into `current_activity`, a derived `claude_status`, a `StuckState`, and a `context_pct`. A background tokio tick makes reconcile proactive rather than pull-only. Foundational shell-quote consolidation lands first so all SSH command construction sits on one tested primitive. Controller identity + guardrails prevent the fleet from killing itself. Wave 2 features (fleet_health roll-up, event timeline, broadcast) consume the Wave 1 data.

**Tech Stack:** Rust (Tauri 2 backend, tokio, rusqlite), Svelte 5 frontend (runes), SQLite migrations, `rmcp` MCP server.

**Verification baseline (per CLAUDE.md):**
- Backend: `cd src-tauri && cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. Cargo needs Tauri system libs; on a box without them the build script fails — that's an environment gap, not a code error.
- Frontend: `pnpm test`, `pnpm check`. Some tests (`session_ui.test.ts`, `App.test.ts`) fail pre-existingly with `localStorage is undefined` — verify against `main` before blaming a change.

**Wave ordering (strict):** Wave 0 → merge to `main` → branch Wave 1 worktrees → merge Wave 1 → branch Wave 2 worktrees. Within a wave, tasks are independent and parallelizable across worktrees.

---

## WAVE 0 — Foundational (solo, must merge before Wave 1)

### Task 0: Consolidate the four shell-quote copies

**Why first:** `shell_quote` / `shq` / `shell_quote_str` / `shell_escape` are duplicated (CLAUDE.md). Issue 1 changes quoting-adjacent code and every SSH path depends on it. One tested primitive must exist before parallel agents touch `sessions.rs`/`tmux.rs`.

**Files:**
- Keep: `src-tauri/src/shell.rs` (canonical `quote`)
- Modify: every call site of `shq`/`shell_quote`/`shell_quote_str`/`shell_escape` (grep below)
- Test: `src-tauri/src/shell.rs` (add property tests)

- [ ] **Step 1: Inventory the copies**

Run: `cd src-tauri && grep -rn "fn shq\|fn shell_quote\|fn shell_quote_str\|fn shell_escape\|shell_quote\|shell_escape\|\bshq\b" src/`
Record every definition and call site.

- [ ] **Step 2: Add property test against the canonical `quote`**

In `src-tauri/src/shell.rs` `mod tests`, add a round-trip test that proves `quote` survives `bash -c`:

```rust
#[test]
fn quote_round_trips_through_bash() {
    for raw in [
        "plain", "with space", "single'quote", "double\"quote",
        "new\nline", "$(cmd)", "`backtick`", "semi;colon", "a && b",
        "glob*", "tab\tchar", "emoji 🦀",
    ] {
        let cmd = format!("printf %s {}", quote(raw));
        let out = std::process::Command::new("bash").args(["-c", &cmd]).output().unwrap();
        assert!(out.status.success(), "bash failed for {raw:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), raw, "mismatch for {raw:?}");
    }
}
```

- [ ] **Step 3: Run it, confirm it passes for the canonical impl**

Run: `cd src-tauri && cargo test --lib shell::tests::quote_round_trips_through_bash -- --nocapture`
Expected: PASS (proves `quote` is the correct primitive to keep).

- [ ] **Step 4: Replace each duplicate with `crate::shell::quote`**

For each call site found in Step 1, switch to `use crate::shell::quote;` and call `quote(...)`. Delete the duplicate `fn` definitions. Keep one short alias `pub use crate::shell::quote as shq;` ONLY if removing the name churns too many lines — prefer full removal.

- [ ] **Step 5: Verify whole crate**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all pass; no `dead_code`/`unused` warnings for removed fns.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor: consolidate shell quoting into shell::quote with property tests"
```

---

## WAVE 1 — Parallel (each task = its own worktree, branched from post-Wave-0 main)

### Task A: Silence the Stop-hook curl (Issue 5)

**Files:**
- Modify: `src-tauri/src/commands/mcp.rs` (hook command template, ~line 264)
- Test: `src-tauri/src/commands/mcp.rs` `mod tests`

- [ ] **Step 1: Locate the hook command string**

Run: `cd src-tauri && grep -n "127.0.0.1\|/hook\|curl" src/commands/mcp.rs`
The template writes `http://127.0.0.1:{port}/hook?token={token}` into `~/.claude/settings.json`.

- [ ] **Step 2: Extract the command into a pure, testable builder**

Add a function so the shape is unit-testable:

```rust
/// Build the fail-silent hook curl. A down/refused server must produce no
/// stdout/stderr (no pane noise) and exit 0 so Claude doesn't surface it.
fn hook_curl_command(port: u16, token: &str) -> String {
    format!(
        "curl --connect-timeout 1 --max-time 2 -s -o /dev/null \
         'http://127.0.0.1:{port}/hook?token={token}' 2>/dev/null || true"
    )
}
```

- [ ] **Step 3: Write the test**

```rust
#[test]
fn hook_curl_is_fail_silent() {
    let c = hook_curl_command(4180, "tok");
    assert!(c.contains("--connect-timeout 1"));
    assert!(c.contains("-s -o /dev/null"));
    assert!(c.ends_with("|| true"));
    assert!(c.contains("2>/dev/null"));
}
```

- [ ] **Step 4: Run test (fails: fn not defined), then wire the builder into `install_fleet_hook` and rerun**

Run: `cd src-tauri && cargo test --lib commands::mcp`
Expected: FAIL → after wiring → PASS.

- [ ] **Step 5: Verify + commit**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "fix: make Stop-hook curl fail-silent to stop ECONNREFUSED pane noise"
```

---

### Task B: send_prompt submits reliably + explicit `submit` flag (Issue 1)

**Files:**
- Modify: `src-tauri/src/service/sessions.rs` (`build_send_commands` ~830, `send_prompt_inner` ~844)
- Modify: `src-tauri/src/mcp/tools.rs` (`SendPromptParams` ~157)
- Test: `src-tauri/src/service/sessions.rs` `mod tests` (~1232)

- [ ] **Step 1: Write failing tests for the new command shape**

```rust
#[test]
fn send_commands_strip_trailing_newline_and_submit_once() {
    let cmds = build_send_commands("dev-x", "line1\nline2\n", true);
    // body preserves the internal newline, trailing newline stripped
    assert!(cmds.iter().any(|c| c.contains("-l") && c.contains("line1") && c.contains("line2")));
    // exactly one Enter/submit, with a settle before it
    let enters = cmds.iter().filter(|c| c.ends_with("Enter")).count();
    assert_eq!(enters, 1);
    assert!(cmds.iter().any(|c| c.contains("sleep")));
}

#[test]
fn send_commands_no_submit_when_submit_false() {
    let cmds = build_send_commands("dev-x", "stage me", false);
    assert!(cmds.iter().all(|c| !c.ends_with("Enter")));
}
```

- [ ] **Step 2: Run, confirm fail (signature mismatch)**

Run: `cd src-tauri && cargo test --lib service::sessions`
Expected: FAIL (arity/behavior).

- [ ] **Step 3: Implement**

```rust
pub fn build_send_commands(tmux_name: &str, prompt: &str, submit: bool) -> Vec<String> {
    let body = prompt.strip_suffix('\n').unwrap_or(prompt);
    let mut cmds = vec![format!("tmux send-keys -t {} -l {}", quote(tmux_name), quote(body))];
    if submit {
        // settle so the REPL flushes the literal paste before the submit key
        cmds.push("sleep 0.15".to_string());
        cmds.push(format!("tmux send-keys -t {} Enter", quote(tmux_name)));
    }
    cmds
}
```
Update `send_prompt_inner` to thread `submit` through. Default `submit = true`.

- [ ] **Step 4: Add `submit` to MCP params (optional, default true)**

In `tools.rs` `SendPromptParams`:
```rust
#[serde(default = "default_true")]
pub submit: bool,
```
with `fn default_true() -> bool { true }`, and pass it into the service call. Update the tool description: "Send and SUBMIT a prompt to a session's REPL. Set submit=false to stage text without submitting."

- [ ] **Step 5: Run tests, verify, commit**

Run: `cd src-tauri && cargo test --lib service::sessions && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "fix: send_prompt strips trailing newline and submits once; add submit flag"
```

---

### Task C: Reconcile pane-tail enrichment — activity, derived status, stuck, context (Issues 3, 4, 6, 7)

**This is the keystone task.** One pane-tail read per work session feeds four outputs. Keep parsers pure and heavily unit-tested; the reconcile wiring is thin.

**Files:**
- Create: `src-tauri/src/service/pane_intel.rs` (pure parsers: activity line, stuck detection, context %, derived status)
- Create: `src-tauri/migrations/012_session_context_pressure.sql`
- Modify: `src-tauri/src/store.rs` (`SessionRow` + `ReconcileSession` add `context_pct: Option<f64>`, `stuck_kind: Option<String>`; reconcile upsert COALESCE; list query columns)
- Modify: `src-tauri/src/service/sessions.rs` (`reconcile_sessions` / per-host probe: capture tail, call `pane_intel`, fill fields)
- Modify: `src-tauri/src/service.rs` or `mod.rs` to register `pane_intel`
- Test: `src-tauri/src/service/pane_intel.rs` `mod tests`

- [ ] **Step 1: Migration 012**

`src-tauri/migrations/012_session_context_pressure.sql`:
```sql
ALTER TABLE sessions ADD COLUMN context_pct REAL;
ALTER TABLE sessions ADD COLUMN stuck_kind TEXT;
```
(`current_activity` and `claude_status` already exist from migration 010.)

- [ ] **Step 2: Write pure parsers with tests FIRST**

`pane_intel.rs` exposes:
```rust
pub struct PaneIntel {
    pub activity: Option<String>,     // last meaningful non-empty line, ANSI-stripped, capped
    pub stuck: Option<StuckKind>,     // auth menu / reconnect / trust prompt / oom / press-enter
    pub context_pct: Option<f64>,     // 0..100 from REPL footer, if present
    pub derived_status: Option<&'static str>, // "working" | "idle" | "blocked" | None
}
pub enum StuckKind { AuthMenu, Reconnect, TrustPrompt, Oom, PressEnter }
pub fn analyze(pane_tail: &str) -> PaneIntel;
fn strip_ansi(s: &str) -> String;
```
Tests cover: ANSI stripping; a "Reconnecting…" tail → `Reconnect` + `blocked`; a login/auth menu → `AuthMenu` + `blocked`; "Press Enter to continue" → `PressEnter`; a footer like "Context left until auto-compact: 17%" → `context_pct ≈ 83.0`; a token figure variant; a normal tool line ("⏺ Bash(cargo test)") → `activity` set, `working`; an empty/whitespace tail → all `None`.

Run: `cd src-tauri && cargo test --lib service::pane_intel` → iterate until green.

> NOTE for implementer: the exact REPL footer wording/regex must be confirmed against a live pane (`tmux capture-pane -p` on a real session). Make the context regex tolerant (match both a "% left" and a "N/Mk tokens" shape) and return `None` when nothing matches rather than guessing.

- [ ] **Step 3: Add fields to `store.rs`**

Add `context_pct: Option<f64>` and `stuck_kind: Option<String>` to `SessionRow` and `ReconcileSession`. Extend the list-query SELECT and the reconcile upsert with `context_pct=COALESCE(excluded.context_pct, context_pct)` and same for `stuck_kind`, `current_activity`, `claude_status`. Run `cargo test --lib store`.

- [ ] **Step 4: Wire into reconcile**

In the per-host probe in `reconcile_sessions`, for each `kind == "work"` session, capture the tail (reuse `tmux capture-pane -p -S -8` via the existing tmux exec). Call `pane_intel::analyze`. Populate `current_activity = intel.activity`, `context_pct`, `stuck_kind`. For `claude_status`: prefer the authoritative agent status from `find_for_session`; if that's `None`, fall back to `intel.derived_status` (mark inferred — store as-is but Wave-2 UI distinguishes). Batch the captures so one host round-trip covers all its sessions where feasible.

- [ ] **Step 5: Integration check + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: reconcile pane-tail intel (activity, derived status, stuck, context %)"
```

---

### Task D: Controller identity + self-target guardrails (Issue 8)

**Files:**
- Modify: `src-tauri/src/mcp/tools.rs` (new `whoami`/`register_self` tool)
- Modify: `src-tauri/src/service/sessions.rs` (`kill_session`, `recreate_session`, `restart_session` guard)
- Modify: `src-tauri/src/store.rs` (settings get/set for controller `(host, tmux_name)`)
- Test: `store.rs` + `sessions.rs` `mod tests`

- [ ] **Step 1: Store helpers + test**

Add `set_controller(&self, host: &str, tmux_name: &str)` and `get_controller(&self) -> Option<(String,String)>` backed by the existing `settings` table keys `controller.host` / `controller.tmux`. Test set→get round-trip and `None` when unset.

- [ ] **Step 2: Guard helper + tests (pure)**

```rust
/// Err(E_SELF_TARGET) if (host,name) is the registered controller and !force.
pub fn guard_not_controller(
    controller: Option<&(String, String)>, host: &str, name: &str, force: bool,
) -> Result<(), IpcError>;
```
Tests: target == controller, force=false → Err `E_SELF_TARGET`; force=true → Ok; target != controller → Ok; controller None → Ok.

- [ ] **Step 3: Apply guard**

Call `guard_not_controller` at the top of `kill_session`, `recreate_session`, `restart_session` service fns (read controller from store). Add `force: bool` (default false) to their args + the MCP params.

- [ ] **Step 4: `register_self` MCP tool**

`register_self { host_alias, tmux_name }` → `store.set_controller`. Description: "Mark the calling session as the fleet controller; kill/recreate/restart will refuse to target it without force." Also surface `is_controller` in `list_sessions` output by comparing each row to the stored controller.

- [ ] **Step 5: Verify + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: controller identity + self-target guardrails on kill/recreate/restart"
```

---

### Task E: Background sessions surface in list_sessions (Issue 2)

**Files:**
- Modify: `src-tauri/src/claude_cli.rs` (`parse_session_id_from_bg_output` robustness + `claude_bg` warn-on-null)
- Modify: `src-tauri/src/service/bg_sessions.rs` (`NewBgSessionResult` add `warning: Option<String>`)
- Modify: `src-tauri/src/service/sessions.rs` (reconcile: second pass for agent rows with no tmux match → upsert `kind="bg"`)
- Test: `claude_cli.rs` + `sessions.rs` `mod tests`

- [ ] **Step 1: Harden id parsing + tests**

Extend `parse_session_id_from_bg_output` to also: scan for a bare UUID (regex `[0-9a-f]{8}-[0-9a-f]{4}-...`) anywhere as last resort, and accept the existing prefixes. Add tests for a boxed/banner output containing only a UUID, and confirm existing prefix tests still pass.

- [ ] **Step 2: Warn on null**

`claude_bg` returns `(Option<String>, Option<warning>)` or set `NewBgSessionResult.warning = Some("could not parse session id from claude --bg output")` when `None`. Test the warning is populated on unparseable output.

- [ ] **Step 3: Reconcile bg pass + test**

After the tmux-backed enrichment loop, iterate `ClaudeAgentRow`s whose name/cwd matched NO tmux session; upsert a synthetic row with `kind="bg"`, `tmux_name` = a sentinel (e.g. `bg:<sessionId>`), `claude_session_id`, `claude_status`, project derived from `cwd`. Add a test feeding agent rows + empty tmux list → expect a `bg` SessionRow present. Ensure ghost-cleanup does NOT reap `kind="bg"` rows on the basis of missing tmux (they're never in tmux).

- [ ] **Step 4: Verify + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: bg sessions appear in list_sessions; robust bg session-id parse"
```

---

## WAVE 2 — Parallel (branched from post-Wave-1 main; consumes Wave 1 data)

### Task F: fleet_health roll-up (Improvement A)

**Files:**
- Modify: `src-tauri/src/service/health.rs` (extend `Health` + `health_check`)
- Modify: `src-tauri/src/mcp/tools.rs` (`fleet_health` already calls `health::health_check`)
- Test: `health.rs` `mod tests`

- [ ] **Step 1: Extend the struct + pure aggregator with tests**

Add to `Health`: `hosts_reachable`, `hosts_total`, `sessions_total`, and `by_status: BTreeMap<String,u32>` (working/idle/blocked/stopped/unknown — null → "unknown"), `ghosts`, `context_red` (count `context_pct >= 85`), `stuck` (count `stuck_kind` set). Write a pure `fn summarize(sessions: &[SessionRow], hosts: &[HostRow]) -> FleetSummary` and unit-test it on a fixture vec (null status → unknown bucket; one ghost; one red).

- [ ] **Step 2: Wire into `health_check`**

`health_check` reads sessions+hosts from store (cached reconcile state, no network) and folds in `summarize`. Keep existing `version`/`db_ready`/`schema_version`.

- [ ] **Step 3: Verify + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: fleet_health reports per-host/per-status fleet roll-up"
```

---

### Task G: Persistent session event timeline (Improvement C)

**Files:**
- Create: `src-tauri/migrations/013_session_events.sql`
- Modify: `src-tauri/src/store.rs` (`insert_session_event`, `list_session_events`)
- Modify: `src-tauri/src/events.rs` (emit → also persist) and call sites at status transitions / prompt sent / stuck / context-threshold / kill-recreate
- Modify: `src-tauri/src/mcp/tools.rs` (`session_history` tool)
- Test: `store.rs` `mod tests`

- [ ] **Step 1: Migration 013**

```sql
CREATE TABLE session_events (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL,
  at INTEGER NOT NULL,
  kind TEXT NOT NULL,      -- status_change | prompt_sent | stuck | context_threshold | killed | recreated
  detail TEXT
);
CREATE INDEX idx_session_events_session ON session_events(session_id, at DESC);
```

- [ ] **Step 2: Store fns + test**

`insert_session_event(session_id, kind, detail)` and `list_session_events(session_id, limit)`. Test insert→list ordering (newest first) + limit.

- [ ] **Step 3: Emit at transitions**

In reconcile, when a session's `claude_status` or `stuck_kind` changes vs the prior row, insert a `status_change`/`stuck` event. In `send_prompt_inner` insert `prompt_sent` (truncated). In kill/recreate paths insert those. Append-only; never block the mutation on a failed insert (log + continue).

- [ ] **Step 4: `session_history` MCP tool**

`session_history { session_id, limit? }` → `list_session_events`. Description: "Return the recorded event timeline for a session (status changes, prompts, stuck, kills)."

- [ ] **Step 5: Verify + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: persistent session event timeline + session_history tool"
```

---

### Task H: Background reconcile tick (Improvement D)

**Files:**
- Modify: `src-tauri/src/lib.rs` / app setup (spawn tokio interval task)
- Modify: `src-tauri/src/store.rs` + migration `014` (add `last_reconciled_at` if not derivable)
- Modify: `src-tauri/src/service/sessions.rs` (extract the reconcile body so the tick can call it without a Tauri command)
- Test: behavior is timing/integration — assert the extracted reconcile fn is callable headless + a unit test on the interval guard (skip if unreachable hosts).

- [ ] **Step 1: Make reconcile callable from a background task**

Ensure `reconcile_sessions(store, ssh)` (already pure-ish) is invokable without a Tauri `AppHandle`. If event emission needs the handle, pass an `Emitter` trait object or a channel.

- [ ] **Step 2: Spawn the tick**

In app setup, `tokio::spawn` an interval (default 20s, read from settings key `reconcile.interval_secs`, 0 = disabled). Each tick calls reconcile and emits row events on change. Guard against overlap (skip if a tick is still running).

- [ ] **Step 3: `last_reconciled_at`**

Migration `014`: `ALTER TABLE sessions ADD COLUMN last_reconciled_at INTEGER;` set on each reconcile. Frontend can gray out stale rows (Wave-2 UI, optional).

- [ ] **Step 4: Verify + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: background reconcile tick makes fleet state proactive"
```

---

### Task I: broadcast_prompt fan-out (Improvement E)

**Files:**
- Modify: `src-tauri/src/service/sessions.rs` (`broadcast_prompt(filter, prompt)`)
- Modify: `src-tauri/src/mcp/tools.rs` (`broadcast_prompt` tool + params)
- Test: `sessions.rs` `mod tests` (pure filter logic)

- [ ] **Step 1: Pure filter + test**

```rust
pub struct BroadcastFilter { pub host: Option<String>, pub project_id: Option<i64>, pub status: Option<String> }
pub fn select_targets(sessions: &[SessionRow], f: &BroadcastFilter) -> Vec<i64>;
```
Test filtering by host, by status, by project, and combined; `kind != "work"` excluded; controller excluded by default.

- [ ] **Step 2: broadcast impl**

`broadcast_prompt` resolves targets via `select_targets`, then calls the existing `send_prompt` per target, collecting `Vec<(session_id, Result)>`. Return a JSON summary (sent/failed counts + per-session results).

- [ ] **Step 3: MCP tool**

`broadcast_prompt { host?, project_id?, status?, prompt, submit? }`. Description: "Send the same prompt to every matching work session (excludes the controller). Returns per-session results."

- [ ] **Step 4: Verify + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add -A && git commit -m "feat: broadcast_prompt fan-out to matching sessions"
```

---

## Self-review notes

- **Spec coverage:** Issue 1→B, 2→E, 3→C, 4→C, 5→A, 6→C(+stuck remedies deferred to follow-up: detection lands here; auto-keystroke remedies are a guarded follow-up once detection is proven in the field), 7→C, 8→D; Improvements A→F, B→Task 0, C→G, D→H, E→I.
- **Deferred deliberately:** auto-remedy keystrokes for stuck states (Issue 6) — detection + `blocked` surfacing + paging event ship now; sending `Enter`/`1` automatically is gated behind a per-host opt-in and added only after the detector has proven low false-positive in live use. This avoids auto-acting on a misread pane.
- **Migration numbering:** 012 (context/stuck), 013 (events), 014 (last_reconciled_at). Confirm latest on disk before adding (currently 011 is highest).
- **Cross-task field consistency:** `context_pct: Option<f64>`, `stuck_kind: Option<String>`, `current_activity: Option<String>`, `claude_status: Option<String>` used identically in store + reconcile + health.
- **Live-confirm required:** the REPL footer regex (Task C Step 2) and the `claude --bg` output shape (Task E Step 1) must be checked against a real session/CLI — both are marked in-task.
