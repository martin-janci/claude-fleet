# Hook events (SessionEnd, StopFailure, Notification) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install three more Claude Code http hooks on every host and turn them into authoritative `stopped` / `idle`+`turn_seq` / `blocked` state plus timeline events.

**Architecture:** The hook block is a table in `commands/mcp.rs`; the payload struct lives in `mcp/hooks.rs`; `service/hooks.rs` dispatches by `hook_event_name` and calls one `Store::record_*` method per event, then appends a timeline event. Every write stamps `last_hook_at` so the reconcile pass does not clobber it.

**Tech Stack:** Rust (rusqlite, serde), Claude Code hooks contract. Tests: `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-15-hook-events-design.md`

## Global Constraints

- Backend build on `claude-fleet-trn`: `cd src-tauri && source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` before any `cargo` command.
- No tool-description change (`reference_is_current` must keep passing); no `eprintln!` (use `tracing`); never hold the store guard across an `.await`; no new migration.
- Status / stuck strings come from `ClaudeStatus::as_str()` / `StuckKind::as_str()` in `service/pane_intel.rs`, never literals in new code.
- Commits: Conventional Commits, last line `Claude-Session: https://claude.ai/code/session_01LnFcQ2QC6QszexSkn2embn`.
- Branch `feat/hook-events` (already checked out, based on `origin/main` at `7f041eb`).

---

### Task 1: Hook block — six events

**Files:**
- Modify: `src-tauri/src/commands/mcp.rs:400-416` (`FLEET_HOOK_EVENTS` + doc), tests near line 740.

**Interfaces:**
- Produces: `pub(crate) const NOTIFICATION_MATCHER: &str`, `pub(crate) const SESSION_END_MATCHER: &str`; `FLEET_HOOK_EVENTS` has six entries.

- [ ] **Step 1: Failing test** — add to the `tests` module of `commands/mcp.rs`, after `build_hook_config_produces_valid_json`:

```rust
    #[test]
    fn hook_block_installs_all_six_events_with_their_matchers() {
        let merged = merge_hook_into_settings_json("", 4180, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let expect = [
            ("Stop", ""),
            ("UserPromptSubmit", ""),
            ("PostToolUse", WORKTREE_TOOL_MATCHER),
            ("SessionEnd", SESSION_END_MATCHER),
            ("StopFailure", ""),
            ("Notification", NOTIFICATION_MATCHER),
        ];
        assert_eq!(FLEET_HOOK_EVENTS.len(), expect.len());
        for (event, matcher) in expect {
            let arr = v["hooks"][event].as_array().unwrap_or_else(|| panic!("{event} missing"));
            assert_eq!(arr.len(), 1, "{event}: {arr:?}");
            assert_eq!(arr[0]["matcher"], matcher, "{event}");
            assert_eq!(arr[0]["hooks"][0]["type"], "http", "{event}");
            assert_eq!(arr[0]["hooks"][0]["url"], "http://127.0.0.1:4180/hook");
        }
        assert_eq!(SESSION_END_MATCHER, "logout|prompt_input_exit|other");
        assert_eq!(
            NOTIFICATION_MATCHER,
            "permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired"
        );
        // Re-running is idempotent for the new events too.
        let again = merge_hook_into_settings_json(&merged, 4180, "tok").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&again).unwrap();
        for (event, _) in expect {
            assert_eq!(v2["hooks"][event].as_array().unwrap().len(), 1, "{event}");
        }
    }
```

- [ ] **Step 2: Run** `cargo test hook_block_installs_all_six` → compile error (missing constants).

- [ ] **Step 3: Implement** — in `commands/mcp.rs` replace the `FLEET_HOOK_EVENTS` block (keep `WORKTREE_TOOL_MATCHER` above it):

```rust
/// `SessionEnd` reasons that mean the Claude process is gone. `clear` and
/// `resume` are deliberately absent: the process lives on under a new
/// session id, and marking the row `stopped` would be wrong.
pub(crate) const SESSION_END_MATCHER: &str = "logout|prompt_input_exit|other";

/// `Notification` types fleet turns into state (see
/// `service::hooks::notification_effect`). `idle_prompt`, `auth_success`,
/// the `elicitation_complete/response` pair and the agent-view-only
/// `agent_*` types carry nothing fleet needs.
pub(crate) const NOTIFICATION_MATCHER: &str = "permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired";

/// The hook events fleet installs, with their matcher. `Stop` is the
/// completion signal (turn over → idle, `turn_seq` bump), `UserPromptSubmit`
/// the busy signal (turn starting → working), `PostToolUse(EnterWorktree|
/// ExitWorktree)` the worktree registration and removal, `SessionEnd` the
/// exit signal (→ stopped), `StopFailure` the API-error completion (→ idle,
/// `turn_seq` bump, `stop_failure` timeline event) and `Notification` the
/// waiting-on-a-human signal (→ blocked). `SessionStart` cannot be an http
/// hook (Claude Code allows only command / mcp_tool there) and is not used.
pub(crate) const FLEET_HOOK_EVENTS: &[(&str, &str)] = &[
    ("Stop", ""),
    ("UserPromptSubmit", ""),
    ("PostToolUse", WORKTREE_TOOL_MATCHER),
    ("SessionEnd", SESSION_END_MATCHER),
    ("StopFailure", ""),
    ("Notification", NOTIFICATION_MATCHER),
];
```

- [ ] **Step 4: Run** `cargo test commands::mcp` → all pass (existing tests only assert the old three keys exist, which they still do).

- [ ] **Step 5: Commit** `feat(hooks): install SessionEnd, StopFailure and Notification http hooks`.

---

### Task 2: Payload fields

**Files:**
- Modify: `src-tauri/src/mcp/hooks.rs:26-48` (`HookPayload`), its tests; `src-tauri/src/service/hooks.rs` test helper `make_payload` (~line 386) and every `HookPayload { … }` literal in that file (add `..Default::default()`).

**Interfaces:**
- Produces: `HookPayload` fields `reason`, `notification_type`, `message`, `title`, `error`, `error_details: Option<String>`; `#[derive(Default)]`.

- [ ] **Step 1: Failing test** — in `mcp/hooks.rs` tests:

```rust
    #[test]
    fn session_end_stop_failure_and_notification_fields_deserialize() {
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"SessionEnd","reason":"logout","unknown":1}"#,
        )
        .unwrap();
        assert_eq!(p.reason.as_deref(), Some("logout"));
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"StopFailure","error":"rate_limit","error_details":"429","last_assistant_message":"API Error"}"#,
        )
        .unwrap();
        assert_eq!(p.error.as_deref(), Some("rate_limit"));
        assert_eq!(p.error_details.as_deref(), Some("429"));
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission","title":"Permission needed"}"#,
        )
        .unwrap();
        assert_eq!(p.notification_type.as_deref(), Some("permission_prompt"));
        assert_eq!(p.message.as_deref(), Some("Claude needs your permission"));
        assert_eq!(p.title.as_deref(), Some("Permission needed"));
        let d = HookPayload::default();
        assert!(d.session_id.is_none() && d.reason.is_none());
    }
```

- [ ] **Step 2: Run** `cargo test session_end_stop_failure_and_notification_fields` → compile error.

- [ ] **Step 3: Implement** — change the derive to `#[derive(Debug, Default, Deserialize)]` and append to the struct:

```rust
    /// `SessionEnd`: why the session ended (`logout`, `prompt_input_exit`,
    /// `other`, `clear`, `resume`).
    pub reason: Option<String>,
    /// `Notification`: which type fired (see `NOTIFICATION_MATCHER`).
    pub notification_type: Option<String>,
    /// `Notification`: human text. Read for nothing; never stored.
    pub message: Option<String>,
    /// `Notification`: optional title. Never stored.
    pub title: Option<String>,
    /// `StopFailure`: the API error type (`rate_limit`, `overloaded`, …).
    pub error: Option<String>,
    /// `StopFailure`: free-text detail, when Claude Code has one.
    pub error_details: Option<String>,
```

In `service/hooks.rs` tests, add `..Default::default()` as the last line of every `HookPayload { … }` literal (there are seven; `make_payload` is one of them). Run `grep -nF 'HookPayload {' src-tauri/src/service/hooks.rs` to find them.

- [ ] **Step 4: Run** `cargo test hooks` → all pass.

- [ ] **Step 5: Commit** `feat(hooks): accept SessionEnd, StopFailure and Notification payload fields`.

---

### Task 3: Store writes

**Files:**
- Modify: `src-tauri/src/store/sessions.rs` after `record_prompt_submit_hook` (~line 850); tests at the end of that file's `tests` module.

**Interfaces:**
- Produces on `Store`:
  - `pub fn record_session_end_hook(&self, claude_session_id: &str) -> Result<Option<SessionRow>, IpcError>`
  - `pub fn record_stop_failure_hook(&self, claude_session_id: &str) -> Result<Option<SessionRow>, IpcError>`
  - `pub fn record_notification_hook(&self, claude_session_id: &str, status: crate::service::pane_intel::ClaudeStatus, stuck: Option<Option<crate::service::pane_intel::StuckKind>>) -> Result<Option<SessionRow>, IpcError>`

- [ ] **Step 1: Failing tests** — append to the `tests` module of `store/sessions.rs`:

```rust
    fn hooked_session(s: &Store) -> i64 {
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-h").unwrap();
        id
    }

    #[test]
    fn record_session_end_hook_marks_stopped_and_clears_stuck() {
        use crate::service::pane_intel::{ClaudeStatus, StuckKind};
        let s = Store::open_in_memory().unwrap();
        let id = hooked_session(&s);
        s.record_notification_hook("uuid-h", ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter)))
            .unwrap();
        let row = s.record_session_end_hook("uuid-h").unwrap().expect("matched");
        assert_eq!(row.claude_status.as_deref(), Some("stopped"));
        assert!(row.idle_since.is_some());
        assert!(row.stuck_kind.is_none() && row.stuck_since.is_none());
        assert!(row.last_hook_at.is_some());
        assert_eq!(row.id, id);
        assert!(s.record_session_end_hook("nope").unwrap().is_none());
    }

    #[test]
    fn record_stop_failure_hook_writes_like_stop() {
        let s = Store::open_in_memory().unwrap();
        hooked_session(&s);
        let a = s.record_stop_failure_hook("uuid-h").unwrap().unwrap();
        assert_eq!(a.claude_status.as_deref(), Some("idle"));
        assert_eq!(a.turn_seq, 1);
        assert_eq!(a.last_stop_at, a.last_turn_at);
        let b = s.record_stop_failure_hook("uuid-h").unwrap().unwrap();
        assert_eq!(b.turn_seq, 2);
    }

    #[test]
    fn record_notification_hook_maps_status_and_stuck_episodes() {
        use crate::service::pane_intel::{ClaudeStatus, StuckKind};
        let s = Store::open_in_memory().unwrap();
        hooked_session(&s);
        // blocked, stuck untouched (None) → stays NULL
        let r = s.record_notification_hook("uuid-h", ClaudeStatus::Blocked, None).unwrap().unwrap();
        assert_eq!(r.claude_status.as_deref(), Some("blocked"));
        assert!(r.stuck_kind.is_none());
        assert!(r.idle_since.is_none(), "blocked is not idle");
        // set press_enter → episode starts
        let r = s.record_notification_hook("uuid-h", ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))).unwrap().unwrap();
        assert_eq!(r.stuck_kind.as_deref(), Some("press_enter"));
        let since = r.stuck_since.expect("episode start");
        // same kind again → episode start kept
        let r = s.record_notification_hook("uuid-h", ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))).unwrap().unwrap();
        assert_eq!(r.stuck_since, Some(since));
        // None → untouched
        let r = s.record_notification_hook("uuid-h", ClaudeStatus::Blocked, None).unwrap().unwrap();
        assert_eq!(r.stuck_kind.as_deref(), Some("press_enter"));
        // working + clear
        let r = s.record_notification_hook("uuid-h", ClaudeStatus::Working, Some(None)).unwrap().unwrap();
        assert_eq!(r.claude_status.as_deref(), Some("working"));
        assert!(r.stuck_kind.is_none() && r.stuck_since.is_none());
        assert!(r.last_hook_at.is_some());
        assert!(s.record_notification_hook("nope", ClaudeStatus::Blocked, None).unwrap().is_none());
    }
```

`SessionRow` must expose `last_hook_at` for these asserts; check `grep -n last_hook_at src-tauri/src/store/rows.rs`. If it is not a row field, replace those two asserts with a direct query: `s.conn.query_row("SELECT last_hook_at FROM sessions WHERE claude_session_id='uuid-h'", [], |r| r.get::<_, Option<i64>>(0)).unwrap().is_some()` (the reconcile tests already do this).

- [ ] **Step 2: Run** `cargo test record_session_end_hook` → compile error.

- [ ] **Step 3: Implement** — after `record_prompt_submit_hook`:

```rust
    /// The SessionEnd hook's write: the Claude process is gone. Sets
    /// `claude_status = stopped`, starts `idle_since` if not already idle,
    /// clears any stuck episode and stamps `last_hook_at` so the reconcile
    /// guard keeps the verdict until a later pass observes the pane afresh.
    pub fn record_session_end_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET claude_status = 'stopped', last_turn_at = ?2, \
                 last_hook_at = ?2, idle_since = COALESCE(idle_since, ?2), \
                 stuck_kind = NULL, stuck_since = NULL \
                 WHERE claude_session_id = ?1",
                rusqlite::params![claude_session_id, now],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        if changed == 0 {
            return Ok(None);
        }
        let row = self.fetch_session_by_claude_id(claude_session_id)?;
        self.bus.session_updated(&row);
        Ok(Some(row))
    }

    /// The StopFailure hook's write: the turn ended in an API error. The row
    /// effect is exactly `Stop`'s (idle, `turn_seq` bump, stamps) so waiters
    /// return and read the error from the transcript; the handler records
    /// the `stop_failure` timeline event that tells the two apart.
    pub fn record_stop_failure_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        self.record_stop_hook(claude_session_id)
    }

    /// The Notification hook's write. `status` is the mapped status;
    /// `stuck` is `Some(Some(kind))` to set (restarting `stuck_since` when
    /// the kind changes), `Some(None)` to clear, `None` to leave the stuck
    /// fields untouched. Stamps `last_hook_at`; `idle_since` follows the
    /// status. Returns the row (`None` when unmatched). Emits
    /// `session_updated`.
    pub fn record_notification_hook(
        &self,
        claude_session_id: &str,
        status: crate::service::pane_intel::ClaudeStatus,
        stuck: Option<Option<crate::service::pane_intel::StuckKind>>,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let status = status.as_str();
        let stuck_sql = match stuck {
            None => String::new(),
            Some(None) => ", stuck_kind = NULL, stuck_since = NULL".to_string(),
            // ?3 is bound below only in this arm.
            Some(Some(_)) => ", stuck_since = CASE WHEN ?3 IS stuck_kind \
                              THEN COALESCE(stuck_since, ?2) ELSE ?2 END, \
                              stuck_kind = ?3"
                .to_string(),
        };
        let sql = format!(
            "UPDATE sessions SET claude_status = ?4, last_hook_at = ?2, \
             idle_since = {idle}{stuck_sql} WHERE claude_session_id = ?1",
            idle = super::rows::idle_since_sql("?4", "?2"),
        );
        let kind = match stuck {
            Some(Some(k)) => Some(k.as_str()),
            _ => None,
        };
        let changed = self
            .conn
            .execute(
                &sql,
                rusqlite::params![claude_session_id, now, kind, status],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        if changed == 0 {
            return Ok(None);
        }
        let row = self.fetch_session_by_claude_id(claude_session_id)?;
        self.bus.session_updated(&row);
        Ok(Some(row))
    }
```

Note the SQL: `stuck_since` is assigned BEFORE `stuck_kind` in the same
`UPDATE` — SQLite evaluates the right-hand sides against the row's OLD
values regardless of assignment order, so the order is only for readability.
`idle_since_sql` lives in `store/rows.rs` as `pub(super)`; if it is not
reachable as `super::rows::idle_since_sql` from `store/sessions.rs`, use the
path the existing `set_claude_status_by_session_id` uses (same file).

- [ ] **Step 4: Run** `cargo test store::sessions` → all pass.

- [ ] **Step 5: Commit** `feat(store): session_end, stop_failure and notification hook writes`.

---

### Task 4: Handlers

**Files:**
- Modify: `src-tauri/src/service/hooks.rs` (`apply_hook` dispatch ~line 20; new fns after `apply_prompt_submit_hook` ~line 190; tests).

**Interfaces:**
- Produces: `pub(crate) fn notification_effect(notification_type: &str) -> Option<(ClaudeStatus, Option<Option<StuckKind>>)>` (pure, tested); `fn apply_session_end_hook`, `fn apply_stop_failure_hook`, `fn apply_notification_hook` (private).
- Consumes: Task 3's `record_*` methods, `Store::insert_session_event(id, kind, detail)`.

- [ ] **Step 1: Failing tests** — in the `tests` module of `service/hooks.rs` (uses the existing `make_store`, `make_ssh`, `make_payload`, `host_caller` helpers; add a local `hooked(store) -> i64` that upserts host `local`, session `sess`, and sets `claude_session_id = "uuid-1"`, exactly like `stop_hook_bumps_turn_seq_and_stamps_last_stop_at` does):

```rust
    fn hooked(store: &Arc<Mutex<Store>>) -> i64 {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-1").unwrap();
        id
    }

    fn events(store: &Arc<Mutex<Store>>, id: i64) -> Vec<(String, Option<String>)> {
        store
            .lock()
            .unwrap()
            .list_session_events(id, 50)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.detail))
            .collect()
    }

    #[test]
    fn session_end_marks_stopped_and_records_the_reason() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("SessionEnd", "uuid-1");
        p.reason = Some("prompt_input_exit".into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("stopped"));
        assert!(events(&store, id)
            .contains(&("session_end".to_string(), Some("prompt_input_exit".to_string()))));
    }

    #[test]
    fn session_end_clear_and_resume_are_noops() {
        let store = make_store();
        let id = hooked(&store);
        for reason in ["clear", "resume"] {
            let mut p = make_payload("SessionEnd", "uuid-1");
            p.reason = Some(reason.into());
            apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
            let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
            assert_ne!(row.claude_status.as_deref(), Some("stopped"), "{reason}");
        }
        assert!(events(&store, id).is_empty());
    }

    #[test]
    fn stop_failure_ends_the_turn_and_records_the_error() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("StopFailure", "uuid-1");
        p.error = Some("rate_limit".into());
        p.error_details = Some("429 Too Many Requests".into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        assert_eq!(row.turn_seq, 1);
        assert!(row.last_stop_at.is_some());
        assert!(events(&store, id).contains(&(
            "stop_failure".to_string(),
            Some("rate_limit: 429 Too Many Requests".to_string())
        )));
    }

    #[test]
    fn notification_effect_maps_the_installed_types() {
        use crate::service::pane_intel::{ClaudeStatus, StuckKind};
        assert_eq!(
            notification_effect("permission_prompt"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("elicitation_dialog"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("elicitation_url_dialog"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("quota_auto_resume_stale"),
            Some((ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))))
        );
        assert_eq!(
            notification_effect("quota_auto_resume_disabled"),
            Some((ClaudeStatus::Blocked, None))
        );
        assert_eq!(
            notification_effect("quota_auto_resume_fired"),
            Some((ClaudeStatus::Working, Some(None)))
        );
        assert_eq!(notification_effect("idle_prompt"), None);
        assert_eq!(notification_effect("agent_needs_input"), None);
    }

    #[test]
    fn notification_hook_blocks_then_resumes() {
        let store = make_store();
        let id = hooked(&store);
        let mut p = make_payload("Notification", "uuid-1");
        p.notification_type = Some("quota_auto_resume_stale".into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("blocked"));
        assert_eq!(row.stuck_kind.as_deref(), Some("press_enter"));

        p.notification_type = Some("quota_auto_resume_fired".into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert!(row.stuck_kind.is_none());

        // An unmapped type (hand-posted; the matcher never sends it) is a no-op.
        p.notification_type = Some("idle_prompt".into());
        apply_hook(&store, &make_ssh(), &p, &Caller::master()).unwrap();
        let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        let ev = events(&store, id);
        assert_eq!(
            ev.iter().filter(|(k, _)| k == "notification").count(),
            2,
            "{ev:?}"
        );
    }

    #[test]
    fn new_events_respect_the_caller_host_binding() {
        let store = make_store();
        hooked(&store);
        let other = host_caller("hostb", TokenMode::Full);
        for (event, field) in [("SessionEnd", "reason"), ("StopFailure", "error"), ("Notification", "notification_type")] {
            let mut p = make_payload(event, "uuid-1");
            match field {
                "reason" => p.reason = Some("other".into()),
                "error" => p.error = Some("unknown".into()),
                _ => p.notification_type = Some("permission_prompt".into()),
            }
            let e = apply_hook(&store, &make_ssh(), &p, &other).unwrap_err();
            assert_eq!(e.code, "E_FORBIDDEN", "{event}");
        }
    }
```

Check the existing helper names first: `grep -n "fn host_caller\|TokenMode::" src-tauri/src/service/hooks.rs`; if the tests module builds callers differently (e.g. `Caller { host_alias: Some("hostb".into()), mode: TokenMode::Full }`), use that form. Check `list_session_events` signature in `store/timeline.rs` and the event row's field names (`kind`, `detail`).

- [ ] **Step 2: Run** `cargo test service::hooks` → compile errors for the new fns.

- [ ] **Step 3: Implement** — dispatch arms in `apply_hook` (before `_ => Ok(())`):

```rust
        Some("SessionEnd") => apply_session_end_hook(store, payload, caller),
        Some("StopFailure") => apply_stop_failure_hook(store, payload, caller),
        Some("Notification") => apply_notification_hook(store, payload, caller),
```

and the functions after `apply_prompt_submit_hook`:

```rust
/// Reasons on which `SessionEnd` means the process is gone. Mirrors
/// `commands::mcp::SESSION_END_MATCHER`; `clear` / `resume` continue under a
/// new session id and must not stop the row.
const SESSION_END_REASONS: &[&str] = &["logout", "prompt_input_exit", "other"];

/// The SessionEnd hook: the Claude process exited. Marks the row `stopped`
/// and records `session_end` with the reason. Defensive against the
/// matcher: an unlisted reason is a no-op.
fn apply_session_end_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    let (Some(session_id), Some(reason)) = (&payload.session_id, payload.reason.as_deref())
    else {
        return Ok(());
    };
    if !SESSION_END_REASONS.contains(&reason) {
        return Ok(());
    }
    let s = store.lock().map_err(|_| IpcError::lock())?;
    if host_checked_row(&s, session_id, caller)?.is_none() {
        return Ok(());
    }
    remember_transcript_path(&s, payload, session_id);
    if let Some(row) = s.record_session_end_hook(session_id)? {
        best_effort_event(&s, row.id, "session_end", Some(reason));
    }
    Ok(())
}

/// The StopFailure hook: the turn ended in an API error (rate limit, auth,
/// overloaded, …). Ends the turn exactly like `Stop` — waiters return and
/// read the error from the transcript — and records `stop_failure` with
/// the error type (and detail when present).
fn apply_stop_failure_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    let Some(session_id) = &payload.session_id else {
        return Ok(());
    };
    let s = store.lock().map_err(|_| IpcError::lock())?;
    if host_checked_row(&s, session_id, caller)?.is_none() {
        return Ok(());
    }
    remember_transcript_path(&s, payload, session_id);
    if let Some(row) = s.record_stop_failure_hook(session_id)? {
        let error = payload.error.as_deref().unwrap_or("unknown");
        let detail = match payload.error_details.as_deref() {
            Some(d) if !d.trim().is_empty() => format!("{error}: {}", d.trim()),
            _ => error.to_string(),
        };
        best_effort_event(&s, row.id, "stop_failure", Some(&detail));
    }
    Ok(())
}

/// What a `Notification` type means for the row: the mapped status and
/// `Some(Some(kind))` to set / `Some(None)` to clear / `None` to leave the
/// stuck fields alone. `None` overall = not a type fleet acts on (the
/// installed matcher never sends one, but a hand-posted body might).
pub(crate) fn notification_effect(
    notification_type: &str,
) -> Option<(ClaudeStatus, Option<Option<StuckKind>>)> {
    Some(match notification_type {
        "permission_prompt" | "elicitation_dialog" | "elicitation_url_dialog" => {
            (ClaudeStatus::Blocked, None)
        }
        // Claude Code waits for Enter after a long sleep: the existing
        // press_enter playbook resolves it.
        "quota_auto_resume_stale" => (ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))),
        "quota_auto_resume_disabled" => (ClaudeStatus::Blocked, None),
        "quota_auto_resume_fired" => (ClaudeStatus::Working, Some(None)),
        _ => return None,
    })
}

/// The Notification hook: Claude is waiting on a human (or just stopped
/// waiting). Applies [`notification_effect`] and records `notification`
/// with the type. `message` / `title` are never stored.
fn apply_notification_hook(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    caller: &Caller,
) -> Result<(), IpcError> {
    let (Some(session_id), Some(kind)) = (&payload.session_id, payload.notification_type.as_deref())
    else {
        return Ok(());
    };
    let Some((status, stuck)) = notification_effect(kind) else {
        return Ok(());
    };
    let s = store.lock().map_err(|_| IpcError::lock())?;
    if host_checked_row(&s, session_id, caller)?.is_none() {
        return Ok(());
    }
    remember_transcript_path(&s, payload, session_id);
    if let Some(row) = s.record_notification_hook(session_id, status, stuck)? {
        best_effort_event(&s, row.id, "notification", Some(kind));
    }
    Ok(())
}

/// Timeline writes never fail the hook that produced them.
fn best_effort_event(s: &Store, session_id: i64, kind: &str, detail: Option<&str>) {
    if let Err(e) = s.insert_session_event(session_id, kind, detail) {
        tracing::warn!(session_id, kind, error = %e, "[hook] session_event insert failed");
    }
}
```

Add `use crate::service::pane_intel::{ClaudeStatus, StuckKind};` to the file's imports. `ClaudeStatus` / `StuckKind` need `PartialEq + Debug` for the `assert_eq!` in the effect test — check their derives in `pane_intel.rs` (they are `Copy` enums with `as_str`; add `PartialEq, Eq` to the derive list if missing).

- [ ] **Step 4: Run** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test hooks` → clean, all pass.

- [ ] **Step 5: Commit** `feat(hooks): SessionEnd → stopped, StopFailure → turn over, Notification → blocked`.

---

### Task 5: Docs, skill, full local CI, PR

**Files:**
- Modify: `docs/control-api.md` (step 5 ~line 269; *Orchestration* ~line 200; new *Hook contract* subsection before `## Security`), `skills/claude-fleet-control/SKILL.md` (~line 138).

- [ ] **Step 1: control-api.md** — replace, in step 5, the phrase
  ``merges fleet's `Stop`, `UserPromptSubmit` and `PostToolUse(EnterWorktree)` hooks``
  with
  ``merges fleet's `Stop`, `UserPromptSubmit`, `PostToolUse(EnterWorktree|ExitWorktree)`, `SessionEnd(logout|prompt_input_exit|other)`, `StopFailure` and `Notification(permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired)` hooks``
  and append to that step: ``**Hosts provisioned before the `SessionEnd` / `StopFailure` / `Notification` hooks existed must be re-provisioned** to get `stopped`, API-error turn completion and hook-driven `blocked`.``

  In *Orchestration*, after ```UserPromptSubmit` marks it `working`, so …`` add the sentence: ``A turn that ends in an API error fires `StopFailure` instead of `Stop`; fleet ends the turn the same way (idle, `turn_seq` bump) and records a `stop_failure` timeline event with the error type, so `run_prompt` / `wait_for_session` return and `session_history` shows why.``

  Insert before `## Security`:

```markdown
## Hook contract

Claude Code on each host POSTs its hook events to `http://127.0.0.1:<port>/hook`
(the reverse tunnel's loopback end on a remote host) as `type: "http"` hooks with
`Authorization: Bearer <host-token>` and a 5 s timeout. The body is Claude Code's
hook input JSON; fleet reads only the fields below and ignores the rest. Answers:
`204` applied (or a no-op for an unknown session), `400` a body that fails
validation (`E_VALIDATE` / `E_INVALID`, e.g. a worktree path outside a project),
`403` a host token reporting about another host's session, `401` no valid token,
`500` a store failure. A non-2xx answer is a non-blocking error on the Claude side:
the session continues.

| Event | Matcher | Fields read | Effect on the session row | Timeline |
|---|---|---|---|---|
| `UserPromptSubmit` | all | `session_id`, `transcript_path` | `claude_status = working`, `idle_since` cleared | — |
| `Stop` | all | `session_id`, `transcript_path`, `cwd` | `idle`, `turn_seq + 1`, `last_stop_at`; triggers safe-kill and task-marker checks | — |
| `StopFailure` | all | `session_id`, `error`, `error_details` | same as `Stop` | `stop_failure` — `<error>[: <details>]` |
| `SessionEnd` | `logout\|prompt_input_exit\|other` | `session_id`, `reason` | `stopped`, `idle_since` started, stuck cleared | `session_end` — reason |
| `Notification` | `permission_prompt\|elicitation_dialog\|elicitation_url_dialog` | `session_id`, `notification_type` | `blocked` | `notification` — type |
| `Notification` | `quota_auto_resume_stale` | same | `blocked`, `stuck_kind = press_enter` | `notification` — type |
| `Notification` | `quota_auto_resume_disabled` | same | `blocked` | `notification` — type |
| `Notification` | `quota_auto_resume_fired` | same | `working`, stuck cleared | `notification` — type |
| `PostToolUse` | `EnterWorktree\|ExitWorktree` | `tool_name`, `tool_input`, `tool_response` | worktree row registered / removed | — |

Every hook write stamps `last_hook_at`; a reconcile pass that started before that
stamp never overwrites the hook's status with its pane heuristic. `SessionEnd`
with reason `clear` or `resume` is not installed (the process continues under a
new session id). `SessionStart` is not used: Claude Code accepts only `command` /
`mcp_tool` hooks there. Fleet never writes `allowedHttpHookUrls` — defining it
at user level would block every other http hook on the host.
```

- [ ] **Step 2: SKILL.md** — after the paragraph starting ``Sessions on hosts provisioned before the `UserPromptSubmit` hook`` add: ``On re-provisioned hosts `blocked` (permission prompt, elicitation, usage-limit wait) and `stopped` (Claude exited) are hook-driven and immediate; a turn that ended in an API error shows as `idle` with a `stop_failure` event in `session_history` — read it before re-sending.``

- [ ] **Step 3: Full local CI**

```bash
cd src-tauri && source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu && export PATH=$CARGO_HOME/bin:$PATH
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | grep -E "^test result|FAILED|panicked" && cargo deny check 2>&1 | tail -1
cd .. && pnpm install --frozen-lockfile && pnpm run check && pnpm run test 2>&1 | grep -E "Test Files|Tests " && pnpm run build 2>&1 | tail -1 && pnpm audit --audit-level=high 2>&1 | tail -1
```

- [ ] **Step 4: Commit docs** `docs(control-api): hook contract; SessionEnd/StopFailure/Notification`, push with the `martin-janci` token (see memory `gh-account-switch-on-trn`), `gh pr create`, `gh pr merge --merge --admin`, delete the branch.
