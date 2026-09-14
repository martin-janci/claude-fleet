# Agent Rows Outside tmux + Conversation Tab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Classify Claude sessions that run outside tmux correctly (`bg` vs `external`), retire dead bg agents, and replace the broken `claude logs` view with a transcript-backed Conversation tab.

**Architecture:** `claude agents --json` rows keep their `kind`, short job `id` and `startedAt`; reconcile writes `kind='external'` for interactive rows, marks bg agents whose transcript is idle for 24 h as `stopped`, and honours a `dismissed_agents` table. The transcript reader gains a structured parser exposed as the `session_conversation` Tauri command, rendered by a new `ConversationPanel`. `claude logs` code is deleted.

**Tech Stack:** Tauri 2, Rust (rusqlite, tokio, serde), Svelte 5 runes, Vitest.

**Spec:** `docs/superpowers/specs/2026-09-13-agent-rows-and-conversation-design.md` — read it before starting any task.

## Global Constraints

- Every value interpolated into a shell string goes through `crate::shell::quote`.
- Never hold the `Store` mutex guard across an `.await`.
- Migrations: add `src-tauri/migrations/029_dismissed_agents.sql` and register it in `MIGRATIONS` (`src-tauri/src/store/schema.rs`) the way `028_account_nickname.sql` is registered.
- Any new or removed Tauri command, or a changed `#[tool]` description, requires `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`.
- Sentinel `tmux_name` for rows without a pane stays `bg:<sessionId>` for both kinds.
- `AGENT_INACTIVE_SECS = 86_400`; `CONV_TURNS = 10`; `CONV_MAX_CHARS = 64_000`; Conversation poll = 5 000 ms; scroll-pin threshold 40 px.
- `claude_status` vocabulary is unchanged (inactive bg ⇒ `stopped`).
- UI copy is English: "Outside fleet (N)", "Conversation", "inactive", "Remove from list", "No conversation yet", "No Claude session id yet", "Runs outside tmux — no terminal", "Older turns not shown", "this Claude session runs outside fleet; close it where it runs".
- Frontend tooling: `npx vitest run`, `npx svelte-check --tsconfig ./tsconfig.json` (not `pnpm test`/`pnpm check`).
- Rust: `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`. Set `CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet` (fall back to the session scratchpad if that volume is unmounted).
- Tests never touch the network, real `claude`, real ssh, or real credentials. Use `FakeSsh` / fake `TmuxExec` doubles.
- Git: commit only in the given worktree with `git -C <worktree>`; never run `git pull/push/fetch/rebase/checkout/stash/reset`. No attribution lines in commit messages.

---

### Task 1: Agent row kind, job id and start time

**Files:**
- Modify: `src-tauri/src/claude_agents.rs`
- Modify: every constructor of `ClaudeAgentRow` found by `grep -rn "ClaudeAgentRow {" src-tauri/src` (test helpers) — add the new fields.

**Interfaces:**
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
  pub enum AgentKind { Interactive, #[default] Background }
  pub struct ClaudeAgentRow {
      pub session_id: Option<String>,
      pub name: Option<String>,
      pub status: Option<String>,
      pub cwd: Option<String>,
      pub kind: AgentKind,
      pub job_id: Option<String>,     // validated ^[0-9a-f]{8,36}$ else None
      pub started_at: Option<i64>,    // unix seconds
  }
  pub fn find_by_session_id<'a>(rows: &'a [ClaudeAgentRow], session_id: &str) -> Option<&'a ClaudeAgentRow>;
  ```

- [ ] **Step 1: Write failing tests** in `claude_agents.rs` `mod tests`:

```rust
#[test]
fn kind_job_id_and_started_at_are_parsed() {
    let json = r#"[
      {"id":"44366faf","kind":"background","sessionId":"44366faf-ae97-426a-91cd-beaf3c74f1d7","startedAt":1779471359317,"state":"blocked","name":"Test","cwd":"/a"},
      {"pid":1,"kind":"interactive","sessionId":"3f01a60c-6d6c-4186-888e-5696f03d2197","startedAt":1789165229915,"name":"x-f2","cwd":"/b"},
      {"sessionId":"00000000-0000-0000-0000-000000000001","status":"working"},
      {"id":"NOT A JOB; rm","kind":"weird","sessionId":"00000000-0000-0000-0000-000000000002"}
    ]"#;
    let rows = parse_claude_agents_json(json);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].kind, AgentKind::Background);
    assert_eq!(rows[0].job_id.as_deref(), Some("44366faf"));
    assert_eq!(rows[0].started_at, Some(1_779_471_359));
    assert_eq!(rows[1].kind, AgentKind::Interactive);
    assert_eq!(rows[1].job_id, None);
    assert_eq!(rows[2].kind, AgentKind::Background, "missing kind = legacy bg");
    assert_eq!(rows[3].kind, AgentKind::Background, "unknown kind = bg");
    assert_eq!(rows[3].job_id, None, "invalid job id shape is dropped");
}

#[test]
fn find_by_session_id_matches_exactly() {
    let rows = parse_claude_agents_json(
        r#"[{"id":"aaaaaaaa","sessionId":"aaaaaaaa-0000-0000-0000-000000000000"}]"#,
    );
    assert!(find_by_session_id(&rows, "aaaaaaaa-0000-0000-0000-000000000000").is_some());
    assert!(find_by_session_id(&rows, "aaaaaaaa").is_none());
}
```

- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml claude_agents` — expect compile failure (no `AgentKind`).

- [ ] **Step 3: Implement.** In `RawAgentRow` add:

```rust
#[serde(default)]
id: Option<Value>,
#[serde(rename = "startedAt", default)]
started_at: Option<Value>,
```

In `From<RawAgentRow>`:

```rust
let kind = match value_str(raw.kind.as_ref()).as_deref() {
    Some("interactive") => AgentKind::Interactive,
    _ => AgentKind::Background,
};
let job_id = value_str(raw.id.as_ref()).filter(|s| is_job_id(s));
let started_at = raw.started_at.as_ref().and_then(Value::as_i64).map(|ms| ms / 1000);
```

with

```rust
/// `claude stop` / `claude attach` take the short job id (`44366faf`).
/// Only lowercase hex, 8–36 chars (hyphens allowed for a full id) is accepted,
/// so the value is safe to pass as an argv word even before quoting.
pub fn is_job_id(s: &str) -> bool {
    (8..=36).contains(&s.len()) && s.chars().all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}
```

`value_str` lowercases; `is_job_id` must be applied to the lowercased value (a CLI never emits uppercase ids, and `value_str` already lowercases). Keep the existing `kind` debug log. Add `find_by_session_id`. Update the test helper `row(...)` and any other `ClaudeAgentRow { .. }` literal to set `kind: AgentKind::Background, job_id: None, started_at: None`.

- [ ] **Step 4: Run** the same test command — PASS; then the full `cargo test` for compile of other modules.

- [ ] **Step 5: Commit** `feat(agents): keep kind, job id and start time from claude agents`.

---

### Task 2: Store — agent kind on upsert, cleanup of both kinds, dismissals

**Files:**
- Create: `src-tauri/migrations/029_dismissed_agents.sql`
- Modify: `src-tauri/src/store/schema.rs` (register 029 with an `already_applied` guard like 028)
- Modify: `src-tauri/src/store/sessions.rs` (`upsert_bg_session`, `ghost_and_clean_bg_sessions`, new dismissal fns)
- Modify: callers of `upsert_bg_session` (reconcile.rs + tests) to pass the kind

**Interfaces:**
- Produces:
  ```rust
  pub fn upsert_bg_session(&self, host_alias: &str, tmux_name: &str, project_id: Option<i64>,
      claude_session_id: &str, claude_status: Option<&str>, last_activity_at: i64,
      kind: &str /* "bg" | "external" */) -> Result<i64, rusqlite::Error>;
  pub fn dismiss_agent(&self, host_alias: &str, claude_session_id: &str, now: i64) -> Result<(), rusqlite::Error>; // upsert + delete session row (emits session:removed)
  pub fn dismissed_agents(&self, host_alias: &str) -> Result<std::collections::HashMap<String, i64>, rusqlite::Error>;
  pub fn clear_agent_dismissal(&self, host_alias: &str, claude_session_id: &str) -> Result<(), rusqlite::Error>;
  ```

- [ ] **Step 1: Migration file**

```sql
-- Agents (claude agents --json rows outside tmux) the user removed from the
-- list. Reconcile skips an agent while dismissed_at >= its last activity.
CREATE TABLE IF NOT EXISTS dismissed_agents (
  host_alias TEXT NOT NULL,
  claude_session_id TEXT NOT NULL,
  dismissed_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, claude_session_id)
);
```

- [ ] **Step 2: Failing tests** in `store/sessions.rs` tests (use the existing in-memory store helper the neighbouring tests use):

```rust
#[test]
fn upsert_bg_session_writes_kind_and_flips_a_misfiled_row() {
    let s = store();
    s.upsert_bg_session("local", "bg:u1", None, "u1", Some("idle"), 1, "bg").unwrap();
    s.upsert_bg_session("local", "bg:u1", None, "u1", Some("idle"), 2, "external").unwrap();
    assert_eq!(s.get_session("bg:u1", "local").unwrap().unwrap().kind, "external");
}

#[test]
fn cleanup_ghosts_external_rows_too() {
    let s = store();
    s.upsert_bg_session("local", "bg:e1", None, "e1", Some("idle"), 1, "external").unwrap();
    s.ghost_and_clean_bg_sessions("local", &[], 10).unwrap();
    assert_eq!(s.get_session("bg:e1", "local").unwrap().unwrap().status, "ghost");
    s.ghost_and_clean_bg_sessions("local", &[], 20).unwrap();
    assert!(s.get_session("bg:e1", "local").unwrap().is_none());
}

#[test]
fn dismiss_agent_records_and_deletes_the_row() {
    let s = store();
    s.upsert_bg_session("local", "bg:u2", None, "u2", Some("stopped"), 1, "bg").unwrap();
    s.dismiss_agent("local", "u2", 100).unwrap();
    assert!(s.get_session("bg:u2", "local").unwrap().is_none());
    assert_eq!(s.dismissed_agents("local").unwrap().get("u2"), Some(&100));
    assert!(s.dismissed_agents("other").unwrap().is_empty());
    s.clear_agent_dismissal("local", "u2").unwrap();
    assert!(s.dismissed_agents("local").unwrap().is_empty());
}

#[test]
fn migration_029_is_idempotent() {
    // Re-run the migration runner on an already-migrated store; must not fail.
}
```

For the last test follow the pattern used by the 028 idempotency test in `store/schema.rs` (search `028`), and put it there instead of in sessions.rs.

- [ ] **Step 3: Run** `cargo test --manifest-path src-tauri/Cargo.toml store::` — FAIL (arity / missing fns).

- [ ] **Step 4: Implement.**
  - `upsert_bg_session`: add `kind: &str` param; `debug_assert!(kind == "bg" || kind == "external")`; in the INSERT replace the literal `'bg'` with a bound param, and `kind='bg'` in `DO UPDATE` with `kind=excluded.kind`.
  - `ghost_and_clean_bg_sessions`: every `kind='bg'` in its SQL becomes `kind IN ('bg','external')`. Update its doc comment.
  - `dismiss_agent`: `INSERT INTO dismissed_agents(...) VALUES(?1,?2,?3) ON CONFLICT(host_alias, claude_session_id) DO UPDATE SET dismissed_at=excluded.dismissed_at`; then look up `SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2` with `bg:<id>` and call the existing `delete_session(id)` (it emits the removal event; verify by reading `delete_session` at `store/sessions.rs:640`).
  - `dismissed_agents`, `clear_agent_dismissal`: straightforward.
  - Update every caller of `upsert_bg_session` to pass `"bg"` for now (Task 3 passes the real kind).

- [ ] **Step 5: Run** full `cargo test` — PASS. **Commit** `feat(store): agent kind on bg upserts, dismissed_agents table`.

---

### Task 3: Reconcile — external rows, inactive bg agents, dismissals

**Files:**
- Modify: `src-tauri/src/tmux.rs` (trait `TmuxExec` + `LocalTmux` + remote impl)
- Modify: `src-tauri/src/service/sessions/reconcile.rs` (`HostProbe`, `probe_with_timeout`, `reconcile_bg_agents` → `reconcile_agent_rows`)
- Test: `src-tauri/src/service/reconcile_tests.rs` or `service/sessions/tests.rs` (whichever holds `reconcile_bg_agents` tests today — `grep -rn reconcile_bg_agents src-tauri/src`)

**Interfaces:**
- Consumes: `AgentKind`, `ClaudeAgentRow.{kind,started_at}` (Task 1); `upsert_bg_session(.., kind)`, `dismissed_agents`, `clear_agent_dismissal` (Task 2).
- Produces:
  ```rust
  // tmux.rs, TmuxExec
  /// `sessionId → transcript mtime (unix s)` for the given ids; ids that are not
  /// valid Claude session ids are skipped; any failure yields an empty map.
  async fn transcript_mtimes(&self, ids: &[String]) -> std::collections::HashMap<String, i64> {
      let _ = ids; std::collections::HashMap::new()   // default impl
  }
  pub fn transcript_mtimes_script(ids: &[String]) -> Option<String>; // None when no valid id
  pub fn parse_mtimes(stdout: &str) -> HashMap<String, i64>;
  // reconcile.rs
  pub(super) const AGENT_INACTIVE_SECS: i64 = 86_400;
  pub(super) fn agent_is_inactive(status: Option<&str>, last_activity: Option<i64>, now: i64) -> bool;
  // HostProbe gains: pub(super) agent_mtimes: HashMap<String, i64>
  ```

- [ ] **Step 1: Failing unit tests** (pure functions first):

```rust
#[test]
fn mtimes_script_quotes_ids_and_skips_invalid() {
    let ids = vec![
        "44366faf-ae97-426a-91cd-beaf3c74f1d7".to_string(),
        "'; rm -rf / #".to_string(),
    ];
    let s = crate::tmux::transcript_mtimes_script(&ids).unwrap();
    assert!(s.contains("'44366faf-ae97-426a-91cd-beaf3c74f1d7'"));
    assert!(!s.contains("rm -rf"));
    assert!(s.contains("date -r"));
    assert!(crate::tmux::transcript_mtimes_script(&["bad".into()]).is_none());
}

#[test]
fn parse_mtimes_reads_tab_lines_and_ignores_noise() {
    let m = crate::tmux::parse_mtimes("motd\n44366faf-ae97-426a-91cd-beaf3c74f1d7\t1779999999\nx\tnotanumber\n");
    assert_eq!(m.len(), 1);
    assert_eq!(m["44366faf-ae97-426a-91cd-beaf3c74f1d7"], 1_779_999_999);
}

#[test]
fn inactive_rule() {
    let now = 1_000_000;
    let old = now - AGENT_INACTIVE_SECS - 1;
    assert!(agent_is_inactive(Some("blocked"), Some(old), now));
    assert!(agent_is_inactive(None, Some(old), now));
    assert!(!agent_is_inactive(Some("working"), Some(old), now));
    assert!(!agent_is_inactive(Some("blocked"), Some(now - 10), now));
    assert!(!agent_is_inactive(Some("blocked"), None, now), "unknown time = active");
}
```

The script:

```rust
pub fn transcript_mtimes_script(ids: &[String]) -> Option<String> {
    let valid: Vec<String> = ids
        .iter()
        .filter(|id| crate::validate::claude_session_id(id).is_ok())
        .map(|id| crate::shell::quote(id))
        .collect();
    if valid.is_empty() {
        return None;
    }
    Some(format!(
        "for id in {}; do for f in \"$HOME\"/.claude/projects/*/\"$id\".jsonl; do \
         if [ -f \"$f\" ]; then printf '%s\\t%s\\n' \"$id\" \"$(date -r \"$f\" +%s)\"; break; fi; \
         done; done",
        valid.join(" ")
    ))
}
```

`parse_mtimes`: split lines on `'\t'` into exactly two parts, id must pass `validate::claude_session_id`, mtime parses as `i64`.

- [ ] **Step 2: Implement trait method.** Remote impl: `self.remote_bash(&script)` and `parse_mtimes` of stdout when status success, else empty. `LocalTmux`: run the same script through `tokio::process::Command::new("bash").args(["-c", &script])` (no login shell needed; `$HOME` is set). Both return empty on any error.

- [ ] **Step 3: Probe.** In `probe_with_timeout`, inside the timed `probe` future, after `list_claude_agents`:

```rust
let bg_ids: Vec<String> = agent_rows
    .iter()
    .filter(|a| a.kind == crate::claude_agents::AgentKind::Background)
    .filter_map(|a| a.session_id.clone())
    .collect();
let agent_mtimes = if bg_ids.is_empty() {
    std::collections::HashMap::new()
} else {
    tmux.transcript_mtimes(&bg_ids).await
};
```

Add `agent_mtimes` to `HostProbe` (empty map in the timeout and error arms) and thread it to the writer call at `reconcile.rs:502`.

- [ ] **Step 4: Failing reconcile tests** for `reconcile_agent_rows(s, host, live, projects, agents, mtimes, now)` — build a store, no live tmux sessions, agents from JSON:

```rust
#[test]
fn interactive_agents_land_as_external_and_background_as_bg() { /* interactive → kind external; background → kind bg */ }
#[test]
fn idle_background_agent_is_stored_stopped() { /* state blocked, mtime now-2d → claude_status "stopped" */ }
#[test]
fn working_background_agent_is_never_stopped() { /* state working, old mtime → "working" */ }
#[test]
fn started_at_stands_in_when_no_transcript() { /* no mtime, startedAt 2 days ago → stopped */ }
#[test]
fn dismissed_agent_is_skipped_until_newer_activity() {
    // dismiss at t=100; mtime 90 → no row; mtime 150 → row exists and dismissal cleared
}
```

Write each body fully, asserting via `s.get_session("bg:<id>", "local")` (`kind`, `claude_status`) and `s.dismissed_agents("local")`.

- [ ] **Step 5: Implement `reconcile_agent_rows`** (rename `reconcile_bg_agents`, update its call site and doc comment):

```rust
let dismissed = s.dismissed_agents(host_alias).unwrap_or_default();
for agent in unmatched_bg_agents(live, agents, host_alias == "local") {
    let Some(session_id) = agent.session_id.as_deref() else { continue };
    let last_activity = mtimes.get(session_id).copied().or(agent.started_at);
    if let Some(&at) = dismissed.get(session_id) {
        match last_activity {
            Some(t) if t > at => { let _ = s.clear_agent_dismissal(host_alias, session_id); }
            _ => continue, // still dismissed; not in keep ⇒ nothing to prune (row was deleted)
        }
    }
    let tmux_name = format!("bg:{session_id}");
    keep.push(tmux_name.clone());
    let kind = match agent.kind {
        crate::claude_agents::AgentKind::Interactive => "external",
        crate::claude_agents::AgentKind::Background => "bg",
    };
    let mut status = known_agent_status(&tmux_name, agent.status.as_deref());
    if kind == "bg" && agent_is_inactive(status.as_deref(), last_activity, now) {
        status = Some("stopped".to_string());
    }
    // project_id lookup and upsert exactly as before, passing `kind`
}
```

`agent_is_inactive`: `status != Some("working")` and `last_activity.is_some_and(|t| now - t >= AGENT_INACTIVE_SECS)`. Match the real type of `known_agent_status`'s return (read it at `reconcile.rs:1035`).

- [ ] **Step 6: Run** full `cargo test`, clippy, fmt — PASS. **Commit** `fix(reconcile): external rows for interactive agents, retire idle bg agents`.

---

### Task 4: Stop by job id, launch lookup by name, dismiss command, health

**Files:**
- Modify: `src-tauri/src/claude_cli.rs` (`stop_script`, `claude_stop`)
- Modify: `src-tauri/src/service/sessions/lifecycle.rs` (`kill_session` bg branch, `bg_claude_session_id`)
- Modify: `src-tauri/src/service/bg_sessions.rs` (`new_bg_session_tracked`, new `dismiss_agent_session`)
- Modify: `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs` (register `dismiss_agent_session`)
- Modify: `src-tauri/src/service/health.rs` (skip `kind='external'`)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: `find_by_session_id`, `find_by_name`, `ClaudeAgentRow.job_id`, `is_job_id` (Task 1); `Store::dismiss_agent` (Task 2).
- Produces:
  ```rust
  pub fn stop_script(job_id: &str) -> Result<String, IpcError>; // validates is_job_id
  pub async fn claude_stop(ssh: &Arc<SshClient>, host_alias: &str, job_id: &str) -> Result<bool, IpcError>;
  #[derive(Deserialize)] pub struct DismissAgentArgs { pub session_id: i64 }
  pub fn dismiss_agent_session(args: DismissAgentArgs, store: &Mutex<Store>) -> Result<(), IpcError>;
  // Tauri command: dismiss_agent_session(args: DismissAgentArgs) -> Result<(), IpcError>
  ```

- [ ] **Step 1: Failing tests.**
  - `claude_cli.rs`: `stop_script("44366faf")` == `"claude stop '44366faf'"` (match however `quote` renders — assert `contains("44366faf")` and starts with `claude stop `); `stop_script("'; rm")` is `Err`.
  - `bg_sessions.rs`: `dismiss_agent_session` refuses an `external` row (`E_INVALID_STATE`), refuses a `bg` row with `claude_status = "working"`, accepts a `bg` `stopped` row and afterwards `get_session` is `None` and `dismissed_agents` has the id; unknown id → `E_NOTFOUND`.
  - `lifecycle.rs` pure helper: extract the decision into `fn bg_stop_target(kind: &str, agents: &[ClaudeAgentRow], claude_session_id: &str) -> Result<Option<String>, IpcError>` returning `Err(E_INVALID_STATE)` for `external`, `Ok(None)` when the agent is absent, `Ok(Some(job_id))` when present with a job id, `Err(E_INVALID_STATE, "…use Remove from list")` when present without one. Test all four.
  - `bg_sessions.rs` pure helper: `fn pick_launched_id(parsed: Option<String>, agents: &[ClaudeAgentRow], name: &str) -> Option<String>` — parsed id wins; else `find_by_name(..).session_id`; else `None`. Test the three cases.
  - `health.rs`: a store with one `external` blocked row and one tmux blocked row yields counts that include only the tmux row (mirror the existing health test style).

- [ ] **Step 2: Run** the tests — FAIL.

- [ ] **Step 3: Implement.**
  - `stop_script(job_id)`: `if !crate::claude_agents::is_job_id(job_id) { return Err(IpcError::new("E_INVALID", "invalid background job id")) }` then `format!("claude stop {}", quote(job_id))`. Update the doc comment (short id, no `--`).
  - `kill_session` bg branch: read `kind` alongside `id`/`claude_session_id` from the row; build an executor for the host and call `list_claude_agents()` (use the same `TmuxExec` constructor reconcile uses for this host — find it with `grep -rn "fn tmux_for_host\|Box<dyn TmuxExec>" src-tauri/src/service`), then `bg_stop_target`; `Some(job)` → `claude_stop(ssh, host, &job)`; `None` → skip the stop. Keep the `killed` event and the reconcile. Delete `bg_claude_session_id` if it becomes unused (and its test).
  - `new_bg_session_tracked`: when `res.claude_session_id` is `None`, poll `list_claude_agents` up to 3 times with `tokio::time::sleep(Duration::from_secs(1))` between tries and use `pick_launched_id`; set `res.claude_session_id` and clear `res.warning` when found.
  - `dismiss_agent_session`: lock store, `get_session_by_id`, check `kind == "bg"` (else `E_INVALID_STATE` "only background agents can be removed from the list"), `claude_status.as_deref() != Some("working")` (else `E_INVALID_STATE` "stop the agent first"), `claude_session_id` present, then `dismiss_agent(host, cid, now)`.
  - Tauri command wrapper in `commands/sessions.rs` + register in `lib.rs` `generate_handler!`.
  - `health.rs`: skip rows with `kind == "external"` wherever it iterates sessions for counts.

- [ ] **Step 4: Regenerate docs** `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`; run full `cargo test`, clippy, fmt — PASS.

- [ ] **Step 5: Commit** `fix(bg): stop by job id, find launched agents by name, remove from list`.

---

### Task 5: Structured transcript + `session_conversation`; retire `claude logs`

**Files:**
- Modify: `src-tauri/src/service/transcript.rs`
- Modify: `src-tauri/src/mcp/tools/support.rs` (`transcript_for` uses `resolve_args`)
- Modify: `src-tauri/src/mcp/tools/session_ops.rs` (`peek_session` → transcript, deprecated description)
- Modify: `src-tauri/src/service/bg_sessions.rs` (delete `peek_session`, `PeekSessionArgs` if only used there — keep `resolve_peek_target` if MCP still uses it)
- Modify: `src-tauri/src/claude_cli.rs` (delete `claude_logs`, `logs_script`, `NO_BG_LOGS_MSG`, their tests; keep `is_no_running_job` for `claude_stop`)
- Modify: `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs` (remove Tauri `peek_session`; add `session_conversation`)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Produces:
  ```rust
  pub const CONV_TURNS: usize = 10;
  pub const CONV_MAX_CHARS: usize = 64_000;
  #[derive(Serialize, Clone, Debug, PartialEq)] pub struct ConvTurn { pub prompt: Option<String>, pub at: Option<String>, pub items: Vec<ConvItem> }
  #[derive(Serialize, Clone, Debug, PartialEq)] #[serde(tag = "kind", rename_all = "snake_case")]
  pub enum ConvItem { Text { text: String }, Tool { summary: String } }
  #[derive(Serialize, Clone, Debug, PartialEq)] pub struct Conversation { pub turns: Vec<ConvTurn>, pub truncated: bool }
  pub fn parse_conversation(jsonl: &str) -> Vec<ConvTurn>;
  pub fn trim_conversation(turns: Vec<ConvTurn>, max_turns: usize, max_chars: usize) -> Conversation;
  pub fn resolve_args(store: &Mutex<Store>, row: &SessionRow, turns: usize, max_chars: usize) -> Result<TranscriptArgs, IpcError>;
  pub async fn fetch_conversation(args: TranscriptArgs, ssh: &Arc<SshClient>) -> Result<Conversation, IpcError>;
  // Tauri: session_conversation(args: { session_id: i64 }) -> Result<Conversation, IpcError>
  ```
  JSON shape seen by the frontend: `{ "turns": [{ "prompt": "…"|null, "at": "2026-…Z"|null, "items": [{"kind":"text","text":"…"},{"kind":"tool","summary":"Bash(command=ls)"}] }], "truncated": false }`. Note: `Tool.summary` is the one-liner **without** the `[tool_use] ` prefix; `parse_turns` adds the prefix back.

- [ ] **Step 1: Failing tests** in `transcript.rs`:

```rust
#[test]
fn parse_conversation_keeps_prompts_text_and_tool_lines() {
    let jsonl = [
        line(serde_json::json!({"type":"user","timestamp":"2026-09-13T10:00:00Z","message":{"role":"user","content":"first"}})),
        line(serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"thinking","thinking":"secret"},
            {"type":"text","text":"Let me look."},
            {"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}})),
        line(serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"a"}]}})),
        line(serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":"second"},{"type":"text","text":"part"}]}})),
        line(serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"noise"}]}})),
        line(serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}})),
    ].join("\n");
    let turns = parse_conversation(&jsonl);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].prompt.as_deref(), Some("first"));
    assert_eq!(turns[0].at.as_deref(), Some("2026-09-13T10:00:00Z"));
    assert_eq!(turns[0].items, vec![
        ConvItem::Text { text: "Let me look.".into() },
        ConvItem::Tool { summary: "Bash(command=ls -la)".into() },
    ]);
    assert_eq!(turns[1].prompt.as_deref(), Some("second\npart"));
    assert_eq!(turns[1].items, vec![ConvItem::Text { text: "Done.".into() }]);
}

#[test]
fn a_prompt_without_reply_is_kept_in_conversation_but_not_in_turns() {
    let jsonl = line(serde_json::json!({"type":"user","message":{"content":"waiting"}}));
    assert_eq!(parse_conversation(&jsonl).len(), 1);
    assert!(parse_turns(&jsonl).is_empty());
}

#[test]
fn trim_conversation_drops_oldest_first() {
    let t = |p: &str, n: usize| ConvTurn { prompt: Some(p.into()), at: None,
        items: vec![ConvItem::Text { text: "x".repeat(n) }] };
    let c = trim_conversation(vec![t("a", 10), t("b", 10), t("c", 10)], 2, 1_000);
    assert_eq!(c.turns.len(), 2);
    assert!(c.truncated);
    assert_eq!(c.turns[0].prompt.as_deref(), Some("b"));
    let c = trim_conversation(vec![t("a", 50), t("b", 50)], 10, 60);
    assert!(c.truncated);
    assert_eq!(c.turns.last().unwrap().prompt.as_deref(), Some("b"));
    let c = trim_conversation(vec![t("a", 5)], 10, 1_000);
    assert!(!c.truncated);
}
```

The existing `parse_turns_*` tests must stay unchanged and pass.

- [ ] **Step 2: Run** — FAIL.

- [ ] **Step 3: Implement.**
  - `parse_conversation`: the loop of today's `parse_turns`, but a human prompt starts a new `ConvTurn` with `prompt` = the string body, or text blocks joined with `"\n"` (trimmed), and `at` = the entry's `timestamp` string. Assistant text → `ConvItem::Text` (trim_end, skip blank); `tool_use` → `ConvItem::Tool { summary }` where `summarize_tool_use` is split so a new `tool_summary(block) -> String` returns `Name(input…)` (truncated to `TOOL_SUMMARY_CHARS - "[tool_use] ".len()` chars + `"…)"`), and assistant entries before any prompt go into a turn with `prompt: None`. Turns with no prompt and no items are dropped.
  - `parse_turns(jsonl)`: `parse_conversation(jsonl).into_iter().filter(|t| !t.items.is_empty()).map(|t| t.items.iter().map(|i| match i { Text{text} => text.clone(), Tool{summary} => format!("[tool_use] {summary}") }).collect::<Vec<_>>().join("\n").trim().to_string()).collect()`. If any existing `parse_turns` or `summarize_tool_use` truncation test breaks, adjust `tool_summary` so the prefixed form equals the previous output exactly; do not edit the old tests.
  - `trim_conversation`: keep the last `max_turns`; then while total chars (prompt + item texts/summaries) > `max_chars`, remove the first item of the first turn (drop the turn when it has no items left, keeping its prompt only if items remain); `truncated` = anything removed.
  - `resolve_args`: move the body of `transcript_for` (cwd from worktree/project, stored path, `is_bg` = `row.tmux_name.starts_with("bg:")` ⇒ `tmux_name: None`) into `transcript.rs`, returning `TranscriptArgs`; `E_INVALID_STATE` when `claude_session_id` is None. `transcript_for` calls it.
  - `fetch_conversation`: same as `fetch_transcript` up to `stdout`, then `trim_conversation(parse_conversation(&text), CONV_TURNS, CONV_MAX_CHARS)`. Factor the shared read/validation/error-mapping into a private `read_tail(args, ssh) -> Result<String, IpcError>` used by both.
  - Tauri `session_conversation`: lock → `get_session_by_id` (E_NOTFOUND) → `resolve_args(.., CONV_TURNS, CONV_MAX_CHARS)` → drop lock → `fetch_conversation`.
  - MCP `peek_session`: keep params and target resolution; for a fleet row call `self.transcript_for(&row, None, None)`; for an untracked `claude_session_id + host_alias` call `fetch_transcript` with `tmux_name: None, transcript_path: None, cwd: None, turns: 1, max_chars: DEFAULT_MAX_CHARS`. New description: `"Deprecated: use session_transcript. Returns the session's last assistant turn from its transcript. Address it with session_id OR claude_session_id (+ host_alias while the fleet row does not exist yet)."`
  - Delete the `claude logs` code listed under Files, the Tauri `peek_session` command and its registration.

- [ ] **Step 4: Regenerate docs**, run full `cargo test`, clippy, fmt — PASS.

- [ ] **Step 5: Commit** `feat(transcript): session_conversation; replace claude logs with the transcript`.

---

### Task 6: Frontend — no-pane helper, Outside fleet group, inactive rows, attention

**Files:**
- Modify: `src/lib/sessions.ts` (+ `sessions.test.ts`)
- Modify: `src/lib/sidebar_index.ts` (+ its test file)
- Modify: `src/lib/attention.ts` (+ test)
- Modify: `src/lib/Sidebar.svelte`, `src/lib/SessionRowItem.svelte` (+ `Sidebar.test.ts`)
- Modify: `src/App.svelte:163`, `src/lib/OnboardingCard.svelte:45`, `src/lib/hints.ts:174`, `src/lib/TerminalView.svelte:294`, `src/lib/SessionDetails.svelte:596`
- Delete: `src/lib/PeekPanel.svelte` and any `PeekPanel` test

**Interfaces:**
- Consumes: Tauri `dismiss_agent_session` (Task 4); rows now carry `kind: 'bg' | 'external' | …`.
- Produces:
  ```ts
  // sessions.ts
  export function hasNoPane(s: Pick<SessionRow, 'kind'>): boolean; // kind === 'bg' || kind === 'external'
  export function isInactiveAgent(s: Pick<SessionRow, 'kind' | 'claude_status'>): boolean; // kind==='bg' && claude_status==='stopped'
  export async function dismissAgentSession(sessionId: number): Promise<Result<null>>; // invokeCmd('dismiss_agent_session', { args: { session_id } })
  // sidebar_index.ts
  export function buildOutsideFleet(sessions: readonly SessionRow[], hostFilter: string): SessionRow[];
  ```

- [ ] **Step 1: Failing tests.**
  - `sessions.test.ts`: `hasNoPane` true for bg/external, false for work/review/shell; `isInactiveAgent` only bg+stopped; `dismissAgentSession(7)` invokes `dismiss_agent_session` with `{ args: { session_id: 7 } }` (mock `invokeCmd` the way the file already does). Remove `peekSession` tests.
  - sidebar_index tests: `buildSessionsByProject` excludes `external` rows even with a `project_id`; `buildOutsideFleet` returns only external rows, respects `hostFilter`, ignores the bg toggle, sorted by `last_activity_at` desc.
  - attention tests: an `external` row with `claude_status: 'blocked'` or a `stuck_kind` has severity below every other row's (return `-1` / whatever the "rest" bucket is) and is absent from `stuckMap`/new-stuck detection and attention counts.
  - `Sidebar.test.ts`: with one external row, `outside-fleet` header shows `Outside fleet (1)`, the row is hidden until the header is clicked, clicking again hides it, the open state is written with `writePref('outside-fleet-open', true)`; the external row renders no `edit-label`, restart or kill buttons; there is no `peek-session` button on any row; a bg row with `claude_status: 'stopped'` shows an `inactive` chip and a `remove-from-list` button that calls `dismissAgentSession(row.id)`.

- [ ] **Step 2: Run** `npx vitest run src/lib/sessions.test.ts src/lib/sidebar_index src/lib/attention src/lib/Sidebar.test.ts` — FAIL.

- [ ] **Step 3: Implement.**
  - Helpers in `sessions.ts`; delete `peekSession`.
  - `sessionVisible` unchanged; `buildSessionsByProject` adds `if (s.kind === 'external') continue;` before the visibility check. Sidebar's `orphanSessions` filter adds `s.kind !== 'external'`.
  - `buildOutsideFleet`: filter `kind === 'external'` and host filter, sort copy by `last_activity_at` desc.
  - `attention.ts`: early-return the lowest bucket for `external` in the severity function; skip `external` in the stuck-map / new-stuck / counts builders.
  - Sidebar: `let outsideOpen = $state(readPref('outside-fleet-open', false, isBool))` with an effect writing it back; section after the orphan section:

    ```svelte
    {#if outsideFleet.length > 0}
      <div class="orphan-section" data-testid="outside-fleet-section">
        <button class="section-header section-toggle" data-testid="outside-fleet"
          aria-expanded={outsideOpen} onclick={() => (outsideOpen = !outsideOpen)}>
          <span class="caret" class:collapsed={!outsideOpen}>▾</span>
          Outside fleet ({outsideFleet.length})
        </button>
        {#if outsideOpen}
          {#each outsideFleet as sess (sess.id)}
            <!-- same SessionRowItem props as orphan rows, plus readOnly -->
          {/each}
        {/if}
      </div>
    {/if}
    ```
    `readPref`/`writePref`/`isBool` come from where `sessions.ts` imports them.
  - `SessionRowItem`: new prop `readOnly = false`; when true, render the name + status chip only (no `row-actions`). Remove the 📋 button, `peek` prop, and the `PeekPanel` block; remove `peekState`/`doPeek`/`closePeek` from Sidebar. When `isInactiveAgent(sess)`, show `<span class="claude-chip inactive-chip" data-testid="inactive-chip">inactive</span>` instead of the status chip, and inside `row-actions` a `remove-from-list` icon button (`title="Remove from list"`, `aria-label="Remove from list"`) calling `dismissAgentSession(sess.id)`; on error push an error toast the way other row actions do. The `🤖` badge stays for bg; external rows get no badge.
  - Replace `kind !== 'bg'` / `kind === 'bg'` "no pane" checks in the listed files with `hasNoPane`. (`App.svelte:240,489-493,511` are handled in Task 8; leave them.)
  - Delete `PeekPanel.svelte`.

- [ ] **Step 4: Run** `npx vitest run` and `npx svelte-check --tsconfig ./tsconfig.json` — PASS, 0 errors.

- [ ] **Step 5: Commit** `feat(sidebar): Outside fleet group, inactive agents, drop log peek`.

---

### Task 7: Frontend — `conversation.ts` and `ConversationPanel.svelte`

**Files:**
- Create: `src/lib/conversation.ts`, `src/lib/conversation.test.ts`
- Create: `src/lib/ConversationPanel.svelte`, `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: Tauri `session_conversation` (Task 5), `SessionRow` from `sessions.ts`.
- Produces:
  ```ts
  export type ConvItem = { kind: 'text'; text: string } | { kind: 'tool'; summary: string };
  export interface ConvTurn { prompt: string | null; at: string | null; items: ConvItem[] }
  export interface Conversation { turns: ConvTurn[]; truncated: boolean }
  export const CONVERSATION_POLL_MS = 5_000;
  export const PIN_THRESHOLD_PX = 40;
  export function sessionConversation(sessionId: number): Promise<Result<Conversation>>;
  export function sameConversation(a: Conversation | null, b: Conversation): boolean; // JSON-equal
  export function isPinned(scrollTop: number, clientHeight: number, scrollHeight: number): boolean; // scrollHeight - scrollTop - clientHeight <= PIN_THRESHOLD_PX
  export function emptyStateText(code: string | null, hasId: boolean): string | null;
  // !hasId → 'No Claude session id yet'; code 'E_NO_TRANSCRIPT' → 'No conversation yet'; else null
  ```
  Component: `<ConversationPanel session={SessionRow} visible={boolean} />`.

- [ ] **Step 1: Failing tests.**
  - `conversation.test.ts`: `sessionConversation(5)` → `invokeCmd('session_conversation', { args: { session_id: 5 } })`; `sameConversation` true for deep-equal, false when an item differs, false vs null; `isPinned` at bottom / 40 px / 41 px; `emptyStateText` three cases.
  - `ConversationPanel.test.ts` (mock `./conversation`'s `sessionConversation`, fake timers, `document.visibilityState` stub):
    1. renders prompt blocks (`data-testid="conv-prompt"`), text items (`conv-text`), tool items (`conv-tool`), and `Older turns not shown` when `truncated`;
    2. `E_NO_TRANSCRIPT` → `No conversation yet`; session without `claude_session_id` → `No Claude session id yet` and no fetch;
    3. other error → message + `conv-retry` button that refetches; earlier good turns stay rendered;
    4. polls every 5 s while `visible` is true; no calls while `visible` is false; no calls while `document.visibilityState === 'hidden'`;
    5. changing `session` drops an in-flight response for the old session (resolve the old promise after switching; old content never appears);
    6. identical poll result does not replace DOM nodes (capture a node reference, advance timers, assert same node).

- [ ] **Step 2: Run** `npx vitest run src/lib/conversation.test.ts src/lib/ConversationPanel.test.ts` — FAIL.

- [ ] **Step 3: Implement** `conversation.ts` per the interface (import `invokeCmd`, `Result` from `./result`).

  `ConversationPanel.svelte`:

  ```svelte
  <script lang="ts">
    import { untrack, tick } from 'svelte';
    import type { SessionRow } from './sessions';
    import { formatRelative } from './format'; // use the relative-time helper SessionDetails uses; check its import path
    import {
      sessionConversation, sameConversation, isPinned, emptyStateText,
      CONVERSATION_POLL_MS, type Conversation,
    } from './conversation';

    let { session, visible }: { session: SessionRow; visible: boolean } = $props();

    let conv = $state<Conversation | null>(null);
    let errorCode = $state<string | null>(null);
    let errorMsg = $state<string | null>(null);
    let loading = $state(false);
    let scroller: HTMLDivElement | undefined = $state();
    let seq = 0;

    async function load() {
      const id = session.id;
      if (!session.claude_session_id) return;
      const mine = ++seq;
      loading = conv === null;
      const pinned = scroller ? isPinned(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight) : true;
      const r = await sessionConversation(id);
      if (mine !== seq || session.id !== id) return;
      loading = false;
      if (r.ok) {
        errorCode = null; errorMsg = null;
        if (!sameConversation(conv, r.value)) {
          conv = r.value;
          if (pinned) { await tick(); scroller?.scrollTo({ top: scroller.scrollHeight }); }
        }
      } else {
        errorCode = r.error.code; errorMsg = r.error.message;
      }
    }

    // Reset + immediate fetch on session change.
    $effect(() => {
      void session.id;
      untrack(() => { seq++; conv = null; errorCode = null; errorMsg = null; void load(); });
    });

    // Poll while shown.
    $effect(() => {
      if (!visible) return;
      const t = setInterval(() => {
        if (document.visibilityState === 'visible') void untrack(load);
      }, CONVERSATION_POLL_MS);
      return () => clearInterval(t);
    });

    const empty = $derived(emptyStateText(errorCode, !!session.claude_session_id));
  </script>
  ```

  Markup: container `data-testid="conversation-panel"`; `{#if empty}` muted paragraph; `{:else if loading}` "Loading…"; otherwise optional error row (`conv-error` + `conv-retry`), `Older turns not shown` when `conv.truncated`, then `{#each conv.turns as turn, i (i)}` → `<blockquote data-testid="conv-prompt">` with `turn.prompt` and `<time>` from `turn.at` (only when non-null), then items: text → `<p class="text" data-testid="conv-text">` with `white-space: pre-wrap`; tool → `<div class="tool" data-testid="conv-tool">` monospace, muted, `text-overflow: ellipsis`, `title` = full summary. If `IpcError` has no `code` field, read `src/lib/result.ts` and use its actual field name. Styles use existing tokens (`--fg`, `--fg-muted`, `--border`, `--mono`); the scroller fills the height with `overflow: auto`.

- [ ] **Step 4: Run** both test files + `npx svelte-check` — PASS.

- [ ] **Step 5: Commit** `feat(conversation): transcript-backed Conversation panel`.

---

### Task 8: App integration — Conversation tab, no-pane rows, remove BgSessionPanel

**Files:**
- Modify: `src/App.svelte` (+ `src/App.test.ts`)
- Modify: `src/lib/SessionDetails.svelte` (Remove from list action for inactive bg)
- Delete: `src/lib/BgSessionPanel.svelte`, `src/lib/BgSessionPanel.test.ts`

**Interfaces:**
- Consumes: `hasNoPane`, `isInactiveAgent`, `dismissAgentSession` (Task 6); `ConversationPanel` (Task 7).

- [ ] **Step 1: Failing tests** in `App.test.ts` (follow the file's existing tab tests for Files/Hosts):
  1. a tab `tab-conversation` exists; disabled with title `No Claude session id yet` when the selected session has no `claude_session_id`; enabled otherwise;
  2. clicking it shows `conversation-panel`, and Files/Hosts become inactive; clicking Terminal hides it; opening Hosts hides it;
  3. selecting a `bg` row and an `external` row: `conversation-panel` shown immediately, `tab-terminal` and `tab-files` disabled with title `Runs outside tmux — no terminal`, `TerminalView` not mounted, no `bg-panel` anywhere;
  4. selecting a tmux row after a bg row returns to the terminal (conversation mode off);
  5. SessionDetails for a `bg` + `stopped` row shows `remove-from-list-details` calling `dismissAgentSession`; for a live bg row the button is absent.

- [ ] **Step 2: Run** `npx vitest run src/App.test.ts src/lib/SessionDetails` — FAIL.

- [ ] **Step 3: Implement.**
  - `let conversationMode = $state(false);`. An effect on `$selectedSession?.id`: if the row `hasNoPane` → `conversationMode = true; filesMode = false;` else if previous selection had no pane → `conversationMode = false`. If the selection has no `claude_session_id` and `!hasNoPane` → `conversationMode = false`.
  - `showConversation()`: requires a selected session with `claude_session_id`; `closeHosts(false); filesMode = false; conversationMode = true`. `showTerminal()` / `showFiles()` / `openHosts()` set `conversationMode = false` (for no-pane rows Terminal/Files are disabled, so they never fire).
  - Tab button between Files and Hosts:

    ```svelte
    <button class="view-tab" class:active={conversationMode && !hostsMode} role="tab"
      aria-selected={conversationMode && !hostsMode}
      disabled={!$selectedSession?.claude_session_id}
      title={!$selectedSession?.claude_session_id ? 'No Claude session id yet' : 'Claude conversation from the transcript'}
      onclick={showConversation} data-testid="tab-conversation">Conversation</button>
    ```
    Terminal and Files: `disabled` also when `$selectedSession && hasNoPane($selectedSession)`, title `Runs outside tmux — no terminal`. Terminal's `active` becomes `!filesMode && !hostsMode && !conversationMode`.
  - Right body: replace the `kind === 'bg'` branch. For no-pane rows render `<div class="view-slot"><ConversationPanel session={$selectedSession} visible={!hostsMode} /></div>`; for other rows keep `TerminalView` + Files overlay and add `{#if conversationMode && $selectedSession}<div class="view-slot overlay"><ConversationPanel session={$selectedSession} visible={!hostsMode} /></div>{/if}`. Include `conversationMode` wherever `filesMode || hostsMode` decides the wide layout (`App.svelte:381`, `:443`) only if Files does so for the same reason — Conversation keeps the center pane like Terminal, so do NOT collapse the center for it.
  - Remove the `BgSessionPanel` import and delete the component + test.
  - SessionDetails: in the actions block, `{#if isInactiveAgent(session)}` a ghost button `data-testid="remove-from-list-details"` "Remove from list" calling `dismissAgentSession(session.id)`; on error show the file's existing error-toast pattern.

- [ ] **Step 4: Run** `npx vitest run`, `npx svelte-check --tsconfig ./tsconfig.json`, `pnpm run build` — PASS.

- [ ] **Step 5: Commit** `feat(app): Conversation tab; no-pane rows open it by default`.

---

### Task 9: Verify, release 0.2.16, install

**Files:** version files via the established manual bump (`package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `Cargo.lock`).

- [ ] **Step 1:** `scripts/ci-local.sh` (or its steps individually) — all green; record test counts.
- [ ] **Step 2:** Bump patch to `0.2.16` in the three version files, refresh `Cargo.lock` (`cargo check --manifest-path src-tauri/Cargo.toml`), commit `chore(release): bump version to 0.2.16`.
- [ ] **Step 3:** `CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet pnpm tauri build --bundles app`.
- [ ] **Step 4:** Move `/Applications/claude-fleet.app` to `/Applications/claude-fleet.app.prev-0.2.15`, copy the new bundle in, relaunch, check the log for version `0.2.16` and schema 29 with no errors.
- [ ] **Step 5 (controller, not a subagent):** confirm with `list_sessions` that local interactive Desktop sessions now report `kind: external` and the three May–July agents report `claude_status: stopped`; then push `HEAD:main`.
