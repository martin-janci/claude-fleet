# Device Communication Phase 1 — Correct Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make prompt delivery, the optimistic merge, and the desktop→hub call path correct: a prompt can no longer be split, mangled, typed into a dialog or the wrong pane; a command's return value can no longer regress a fresher row; a hub-routed mutation can no longer be reported failed while the hub completes it.

**Architecture:** Four independent fixes on existing seams, no new wire protocol. (1) A new send primitive in `service/sessions/prompt.rs` ships the body base64-encoded through `tmux load-buffer` + `paste-buffer -p` to the row's known pane id. (2) `deliver_prompt` in the MCP tools gates on `claude_status`/`stuck_kind`, waits for the `UserPromptSubmit` hook as the ack via a new monotonic `prompt_submit_seq` column, and dedupes retries by `client_msg_id`. (3) A `row_version` column bumped by an `AFTER UPDATE` trigger drives the frontend `isStale` guard; `new_session` returns the re-read row; the app subscribes to row events before it lists. (4) The hub-client's per-tool timeout is derived from the hub's own deadline table, a connect timeout and a breaker fail fast, and the timeout error says the outcome is unknown.

**Tech Stack:** Rust (tokio, rusqlite, serde, rmcp), Svelte 5 + TypeScript (Vitest), SQLite migrations, tmux 3.x, POSIX sh.

**Spec:** `docs/specs/2026-09-21-device-communication-analysis.md` — section "Roadmap → Phase 1" and the findings tables A, D, E it points at.

## Global Constraints

- Every value interpolated into a shell string is quoted with `crate::shell::quote` (alias `shq`). No second quoter.
- Never hold the `Store` mutex across an `.await`.
- New `SessionRow` wire fields carry `#[serde(default)]` (an older hub omits them); regenerate the contract golden with `REGEN_HUB_CONTRACT=1` (the regen run itself reports FAILED once, re-run to confirm green).
- Any `#[tool(...)]` description edit: keep the added clause to one sentence (the served-definition budget test `the_served_definition_budget_stays_bounded` caps the surface) and regenerate `docs/control-api-reference.md` with `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- A new migration is `crates/fleet-core/migrations/NNN_<topic>.sql` ending in `INSERT OR IGNORE INTO schema_version (version) VALUES (NNN);` plus a `Migration { .. }` row in `store/schema.rs`; an `ALTER TABLE … ADD COLUMN` migration needs an `already_applied` guard like migration 039.
- Prompt body cap: **65 536 bytes** after normalisation, refused with `E_VALIDATE`.
- Ack wait: **1 500 ms**, polled every **100 ms**; one Enter retry, then one more 1 500 ms wait.
- Dedupe cache: keyed `(caller.label(), client_msg_id)`, TTL **10 minutes**, at most **1 024** entries.
- Hub-client connect timeout **5 s** (TCP and TLS each); per-tool call timeout = hub deadline + **10 s**; `move_session` keeps at least 15 min; breaker refuses when the link is `Offline { attempt >= 2 }`.
- Cargo: `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet` before every cargo command in this worktree (the shared target dir; without it scripts compile against another worktree). Frontend: use `npx vitest run` and `npx svelte-check` (pnpm script binaries are not on PATH).
- Git: commit only from this worktree (`git -C <worktree>`); never `git pull/push/checkout/stash/rebase` inside a task.
- Do not edit `docs/control-api-reference.md`, `src/lib/hub_verdicts.generated.json` or `src-tauri/src/backend/hub_contract.golden.json` by hand; they are generated.

## File map

| File | Responsibility in this plan |
|---|---|
| `crates/fleet-core/migrations/040_row_version_and_prompt_ack.sql` | `sessions.row_version`, its trigger, `sessions.prompt_submit_seq` |
| `crates/fleet-core/src/store/schema.rs` | migration row 040 + `already_applied` guard |
| `crates/fleet-core/src/store/rows.rs` | `SessionRow.row_version`, column list, mapper |
| `crates/fleet-core/src/store/sessions.rs` | `prompt_submit_seq` bump, `prompt_ack_state`, `set_started_at` emits |
| `crates/fleet-core/src/service/sessions/prompt.rs` | send primitive, body normaliser, broadcast target filter |
| `crates/fleet-core/src/service/messages.rs` | refuse pane delivery to a blocked recipient |
| `crates/fleet-core/src/service/sessions/lifecycle.rs` | `new_session` returns the re-read row |
| `crates/fleet-core/src/mcp/tools/params.rs` | `SendPromptParams.force`, `.client_msg_id` |
| `crates/fleet-core/src/mcp/tools/support.rs` | delivery gate, ack wait, `deliver_prompt` |
| `crates/fleet-core/src/mcp/tools/messaging.rs` | `send_prompt` tool wiring + dedupe |
| `crates/fleet-core/src/mcp/tools/mod.rs` | `recent_sends` cache on `FleetTools`, `pub tool_deadline` |
| `crates/fleet-core/src/mcp/mod.rs` | re-export `tool_deadline` |
| `crates/fleet-core/src/ipc_error.rs` | `E_HUB_TIMEOUT` |
| `src-tauri/src/backend/remote.rs` | per-tool timeout, connect timeout, breaker, timeout error |
| `src-tauri/src/backend/tests_remote.rs` | tests for the above |
| `src-tauri/src/backend/tests_contract.rs` | contract sample + field list |
| `src/lib/sessions.ts` | `row_version` on the interface, `isStale` |
| `src/lib/sessions.test.ts` | `isStale` tests |
| `src/App.svelte` | subscribe before list |
| `src/App.test.ts` | ordering test |
| `src/lib/moves.ts`, `src/lib/moves.test.ts` | `E_HUB_TIMEOUT` counts as "no answer" |
| `src/lib/result.ts`, `src/App.svelte` | outcome-unknown refresh |
| `docs/control-api.md` | `send_prompt` contract update |

---

### Task 1: Migration 040 — `row_version`, `prompt_submit_seq`, and the store API around them

**Files:**
- Create: `crates/fleet-core/migrations/040_row_version_and_prompt_ack.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS array, after the 039 row; add the guard fn next to `client_tokens_has_trusted_at`)
- Modify: `crates/fleet-core/src/store/rows.rs:115-260` (`SessionRow`, `SESSION_COLUMNS`, `map_session_row`)
- Modify: `crates/fleet-core/src/store/sessions.rs:816-822` (`set_started_at`), `:968-981` (`record_prompt_submit_hook_for_row`), new `prompt_ack_state`
- Modify: `src-tauri/src/backend/tests_contract.rs:55-100` (sample row), `:640-680` (expected field list, count 51 → 52)
- Test: `crates/fleet-core/src/store/sessions.rs` `mod tests` (line ~1278), `crates/fleet-core/src/store/schema.rs` tests

**Interfaces:**
- Produces: `SessionRow.row_version: i64` (wire, `#[serde(default)]`); `Store::prompt_ack_state(&self, row_id: i64) -> Result<Option<PromptAckState>, rusqlite::Error>` with `pub struct PromptAckState { pub prompt_submit_seq: i64, pub hooks_seen: bool }`; `Store::set_started_at` now emits `session_updated`.

- [ ] **Step 1: Write the failing store tests**

Append inside `mod tests` in `crates/fleet-core/src/store/sessions.rs`:

```rust
    #[test]
    fn row_version_bumps_on_every_update_and_rides_the_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let v0 = s.get_session_by_id(id).unwrap().unwrap().row_version;
        s.set_friendly_name("local", "sess", Some("one")).unwrap();
        let v1 = s.get_session_by_id(id).unwrap().unwrap().row_version;
        s.set_started_at(id, 99).unwrap();
        let v2 = s.get_session_by_id(id).unwrap().unwrap().row_version;
        assert!(v1 > v0, "an UPDATE must bump row_version ({v0} -> {v1})");
        assert!(v2 > v1, "every UPDATE bumps it ({v1} -> {v2})");
        // The upsert's DO UPDATE arm is an UPDATE too.
        s.upsert_session("sess", "local", None, None, 1, 2, "running", None)
            .unwrap();
        let v3 = s.get_session_by_id(id).unwrap().unwrap().row_version;
        assert!(v3 > v2, "an upsert of an existing row bumps it ({v2} -> {v3})");
    }

    #[test]
    fn prompt_submit_seq_counts_submits_and_reports_whether_hooks_were_ever_seen() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let st = s.prompt_ack_state(id).unwrap().expect("row exists");
        assert_eq!(st.prompt_submit_seq, 0);
        assert!(!st.hooks_seen, "no hook has stamped this row yet");
        s.record_prompt_submit_hook_for_row(id).unwrap();
        s.record_prompt_submit_hook_for_row(id).unwrap();
        let st = s.prompt_ack_state(id).unwrap().unwrap();
        assert_eq!(st.prompt_submit_seq, 2);
        assert!(st.hooks_seen);
        assert!(s.prompt_ack_state(999_999).unwrap().is_none());
    }

    #[test]
    fn set_started_at_emits_session_updated() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take();
        s.set_started_at(id, 42).unwrap();
        assert!(
            bus.names().contains(&"session:updated"),
            "started_at must reach the UI by event, not only by re-list: {:?}",
            bus.names()
        );
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().started_at, Some(42));
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo test -p fleet-core --lib store::sessions::tests::row_version_bumps 2>&1 | tail -5
```
Expected: compile error — `row_version` is not a field of `SessionRow`, `prompt_ack_state` not found.

- [ ] **Step 3: Write the migration**

`crates/fleet-core/migrations/040_row_version_and_prompt_ack.sql`:

```sql
-- `row_version`: a per-row counter bumped by every UPDATE, so a payload the
-- frontend receives (a command's return value, a row event) can be ordered
-- against the row it already holds. `last_activity_at` cannot do that job:
-- only reconcile writes it, so two writes inside one tick compare equal and
-- whichever arrives last wins. The trigger fires for `INSERT … ON CONFLICT DO
-- UPDATE` too (an upsert's update arm is an UPDATE). Recursive triggers are
-- off by default, and the WHEN clause keeps it a no-op even if they were on.
ALTER TABLE sessions ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0;

CREATE TRIGGER IF NOT EXISTS sessions_row_version_bump
AFTER UPDATE ON sessions
FOR EACH ROW
WHEN NEW.row_version = OLD.row_version
BEGIN
  UPDATE sessions SET row_version = OLD.row_version + 1 WHERE id = NEW.id;
END;

-- `prompt_submit_seq`: how many UserPromptSubmit hooks this row has recorded.
-- A send that wants to know "did the REPL take it?" reads it before the
-- send and waits for it to move. Monotonic, so second-granularity timestamps
-- cannot confuse a hook that lands in the same second as the send. Not on
-- the wire: read through `Store::prompt_ack_state`.
ALTER TABLE sessions ADD COLUMN prompt_submit_seq INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (40);
```

- [ ] **Step 4: Register the migration**

In `crates/fleet-core/src/store/schema.rs`, after the 039 `Migration { .. }` row:

```rust
    // `sessions.row_version` (+ trigger) and `sessions.prompt_submit_seq`;
    // ADD COLUMN, so the same guard as 038/039.
    Migration {
        version: 40,
        sql: include_str!("../../migrations/040_row_version_and_prompt_ack.sql"),
        already_applied: Some(sessions_has_row_version),
    },
```

Next to `client_tokens_has_trusted_at` add:

```rust
fn sessions_has_row_version(conn: &Connection) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for n in names {
        if n? == "row_version" {
            return Ok(true);
        }
    }
    Ok(false)
}
```
(Copy the exact shape of `client_tokens_has_trusted_at` if it differs; the point is a `PRAGMA table_info` probe.)

- [ ] **Step 5: Put `row_version` on the row**

In `crates/fleet-core/src/store/rows.rs`:
- Add to `SessionRow` (after `tags`, before the flattened `usage`):
```rust
    /// Bumped by a trigger on every UPDATE (migration 040). The frontend's
    /// merge guard orders a command's return value against a row event by
    /// it. `#[serde(default)]`: a hub older than the column sends none.
    #[serde(default)]
    pub row_version: i64,
```
- Append `, row_version` to the END of `SESSION_COLUMNS` (after `tmux_pane_id`).
- In `map_session_row` add `row_version: row.get(51)?,` at the top level of the struct literal (index 51; `tmux_pane_id` is 50).
- Every place that constructs a `SessionRow` literal must gain `row_version: 0` — run `cargo check -p fleet-core --tests` and `cargo check -p claude-fleet --tests` and fix each error site (`src-tauri/src/backend/tests_contract.rs::sample_session` gets `row_version: 12`).

- [ ] **Step 6: Store API**

In `crates/fleet-core/src/store/sessions.rs`:

Replace `set_started_at`:
```rust
    /// PROD-5: when fleet created the session. `COALESCE` keeps the first
    /// value. Emits `session_updated` so the sidebar's elapsed label does
    /// not wait for a re-list.
    pub fn set_started_at(&self, id: i64, at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET started_at=COALESCE(started_at, ?1) WHERE id=?2",
            rusqlite::params![at, id],
        )?;
        self.emit_session(id)?;
        Ok(())
    }
```

In `record_prompt_submit_hook_for_row`, change the SQL to also bump the counter:
```rust
                "UPDATE sessions SET claude_status = 'working', idle_since = NULL, \
                     last_hook_at = ?2, prompt_submit_seq = prompt_submit_seq + 1{END_COMPACTING} \
                     WHERE id = ?1"
```

Add, near `record_prompt_submit_hook_for_row`:
```rust
/// What a sender needs to know to wait for the REPL's acknowledgement of a
/// prompt: the submit counter to watch, and whether this row has EVER been
/// stamped by a hook (a host without hooks can never ack, so the sender must
/// not wait for one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptAckState {
    pub prompt_submit_seq: i64,
    pub hooks_seen: bool,
}

impl Store {
    pub fn prompt_ack_state(
        &self,
        row_id: i64,
    ) -> Result<Option<PromptAckState>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT prompt_submit_seq, last_hook_at IS NOT NULL FROM sessions WHERE id = ?1",
                rusqlite::params![row_id],
                |r| {
                    Ok(PromptAckState {
                        prompt_submit_seq: r.get(0)?,
                        hooks_seen: r.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()
    }
}
```
(Put the `impl Store` block wherever the file's other `impl Store` blocks live; the file may already have one big `impl Store` — then add the method inside it. Export `PromptAckState` from `store/mod.rs` the way `SessionRow` is exported.)

- [ ] **Step 7: Run the store and schema tests**

```bash
cargo test -p fleet-core --lib store:: 2>&1 | tail -15
```
Expected: all pass, including the three new tests and the schema tests that assert `LATEST_SCHEMA_VERSION == MIGRATIONS.len()`.

- [ ] **Step 8: Contract golden**

In `src-tauri/src/backend/tests_contract.rs`: add `"row_version",` to the sorted expected list (alphabetically after `"reviews_session_id"`) and change the count assertion `51` → `52`. Then:

```bash
REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib backend::tests_contract 2>&1 | tail -5
cargo test -p claude-fleet --lib backend::tests_contract 2>&1 | tail -5
```
Expected: the first run may report FAILED (it regenerates), the second passes.

- [ ] **Step 9: Full backend suite + fmt/clippy**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
```
Expected: every `test result: ok`.

- [ ] **Step 10: Commit**

```bash
git add crates/fleet-core/migrations/040_row_version_and_prompt_ack.sql crates/fleet-core/src/store src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/hub_contract.golden.json
git commit -m "feat(store): row_version per session and a prompt-submit counter for delivery acks"
```

---

### Task 2: The send primitive — normalised body, base64 via `load-buffer`, bracketed paste to the known pane

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/prompt.rs:8-30` (`build_send_commands` → `build_send_script`), `:49-105` (`send_prompt_inner`)
- Modify: `crates/fleet-core/src/service/sessions/tests.rs:1335-1380` (replace the five `build_send_commands` tests)
- Test: same file

**Interfaces:**
- Consumes: `Store::get_session(tmux_name, host_alias) -> Result<Option<SessionRow>>` (existing), `SessionRow.context.tmux_pane_id: Option<String>` (existing).
- Produces:
  - `pub fn normalize_prompt_body(prompt: &str) -> Result<String, IpcError>`
  - `pub fn build_send_script(tmux_name: &str, pane_id: Option<&str>, body: &str, buffer: &str, submit: bool) -> String`
  - `pub const MAX_PROMPT_BYTES: usize = 65_536;`
  - `send_prompt_inner` unchanged in signature; `sessions::send_prompt(args, store, ssh)` unchanged.

- [ ] **Step 1: Write the failing tests**

Delete the tests `build_send_commands_emits_literal_text_then_enter`, `build_send_commands_escapes_embedded_quotes`, `build_send_commands_quotes_session_name_with_dashes`, `send_commands_strip_trailing_newline_and_submit_once`, `send_commands_no_submit_when_submit_false` in `crates/fleet-core/src/service/sessions/tests.rs` and put these in their place:

```rust
fn b64(s: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

#[test]
fn normalize_prompt_body_folds_crlf_and_lone_cr_into_lf() {
    assert_eq!(normalize_prompt_body("a\r\nb\r\n").unwrap(), "a\nb\n");
    assert_eq!(normalize_prompt_body("a\rb").unwrap(), "a\nb");
    assert_eq!(normalize_prompt_body("tab\there\n").unwrap(), "tab\there\n");
    assert_eq!(normalize_prompt_body("čšž é 🚀").unwrap(), "čšž é 🚀");
}

#[test]
fn normalize_prompt_body_refuses_control_bytes_that_are_keystrokes() {
    for bad in ["\x03", "esc \x1b[200~", "bell\x07", "\x00"] {
        let e = normalize_prompt_body(bad).unwrap_err();
        assert_eq!(e.code, "E_VALIDATE", "{bad:?}");
        assert!(e.message.contains("control"), "{}", e.message);
    }
}

#[test]
fn normalize_prompt_body_caps_the_size_and_names_the_cap() {
    let big = "x".repeat(MAX_PROMPT_BYTES + 1);
    let e = normalize_prompt_body(&big).unwrap_err();
    assert_eq!(e.code, "E_VALIDATE");
    assert!(e.message.contains("65536"), "{}", e.message);
    assert!(normalize_prompt_body(&"x".repeat(MAX_PROMPT_BYTES)).is_ok());
}

#[test]
fn send_script_ships_the_body_base64_through_load_buffer_and_pastes_bracketed() {
    let s = build_send_script("dev-foo", None, "it's a test", "fleet-abc", true);
    assert!(s.contains(&format!("printf %s '{}' | base64 -d | tmux load-buffer -b 'fleet-abc' -", b64("it's a test"))), "{s}");
    assert!(s.contains("tmux paste-buffer -p -d -b 'fleet-abc' -t \"$t\""), "{s}");
    assert!(s.contains("sleep 0.15"), "{s}");
    assert!(s.trim_end().ends_with("tmux send-keys -t \"$t\" Enter"), "{s}");
    // The raw body never appears in the script: no quoting problem can.
    assert!(!s.contains("it's"), "{s}");
}

#[test]
fn send_script_targets_the_known_pane_id_and_falls_back_to_the_exact_session() {
    let s = build_send_script("dev-foo", Some("%17"), "x", "fleet-1", true);
    assert!(s.starts_with("t='=dev-foo:'; if [ \"$(tmux display-message -p -t '%17' '#{session_name}' 2>/dev/null)\" = 'dev-foo' ]; then t='%17'; fi; "), "{s}");
    let s = build_send_script("dev-foo", None, "x", "fleet-1", true);
    assert!(s.starts_with("t='=dev-foo:'; "), "{s}");
    assert!(!s.contains("display-message"), "{s}");
}

#[test]
fn send_script_strips_one_trailing_newline_and_submits_once() {
    let s = build_send_script("dev-x", None, "line1\nline2\n", "fleet-1", true);
    assert!(s.contains(&b64("line1\nline2")), "{s}");
    assert!(!s.contains(&b64("line1\nline2\n")), "{s}");
    assert_eq!(s.matches(" Enter").count(), 1, "{s}");
}

#[test]
fn send_script_without_submit_pastes_but_never_presses_enter() {
    let s = build_send_script("dev-x", None, "stage me", "fleet-1", false);
    assert!(s.contains("paste-buffer"), "{s}");
    assert!(!s.contains("Enter"), "{s}");
    assert!(!s.contains("sleep"), "{s}");
}

#[test]
fn send_script_with_an_empty_body_is_a_bare_enter() {
    let s = build_send_script("dev-x", Some("%3"), "", "fleet-1", true);
    assert!(!s.contains("load-buffer"), "{s}");
    assert!(s.trim_end().ends_with("tmux send-keys -t \"$t\" Enter"), "{s}");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p fleet-core --lib service::sessions::tests::send_script 2>&1 | tail -5
```
Expected: compile error — `build_send_script` / `normalize_prompt_body` / `MAX_PROMPT_BYTES` not found.

- [ ] **Step 3: Implement the primitive**

Replace lines 8–30 of `crates/fleet-core/src/service/sessions/prompt.rs` (the doc comment and `build_send_commands`) with:

```rust
/// Largest prompt body (bytes, after normalisation) a single send carries.
/// The body rides the command line base64-encoded (ssh has no stdin path on
/// an agent-routed host): 64 KiB × 4/3 stays under Linux's 128 KiB per-argv
/// string, with room for the script around it.
pub const MAX_PROMPT_BYTES: usize = 65_536;

/// Make a prompt body safe to type: `\r\n` and a lone `\r` become `\n` (a
/// CRLF client's prompt otherwise reaches the REPL as several submissions,
/// since `\r` is Return there); any other control character except `\n` and
/// `\t` is refused — an ESC or Ctrl-C byte in a body would interrupt or kill
/// the recipient. Caps the size at [`MAX_PROMPT_BYTES`].
pub fn normalize_prompt_body(prompt: &str) -> Result<String, IpcError> {
    let folded = prompt.replace("\r\n", "\n").replace('\r', "\n");
    if let Some(c) = folded
        .chars()
        .find(|c| c.is_control() && *c != '\n' && *c != '\t')
    {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "prompt contains a control character (U+{:04X}); only newline and tab are allowed",
                c as u32
            ),
        ));
    }
    if folded.len() > MAX_PROMPT_BYTES {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "prompt is {} bytes; the limit is {MAX_PROMPT_BYTES} bytes",
                folded.len()
            ),
        ));
    }
    Ok(folded)
}

/// The one shell script that delivers a prompt to a session, in a single
/// round trip:
///
/// 1. pick the target: the row's known pane id when it still belongs to
///    this session (a split window's active pane is the shell, not Claude),
///    else the EXACT session target `=<name>:` (a bare name would let tmux
///    prefix-match another session);
/// 2. `printf … | base64 -d | tmux load-buffer -b <buffer> -` — the body
///    never touches shell quoting, tmux key-name parsing or the 150 ms
///    typing race;
/// 3. `tmux paste-buffer -p -d` — bracketed paste when the pane asked for
///    it (Claude Code does), so the REPL sees one paste with an unambiguous
///    end marker and internal newlines stay soft;
/// 4. when `submit`, a short settle and ONE Enter.
///
/// A single trailing newline is stripped so it cannot pre-submit the body.
/// An empty body is a bare Enter (the Conversation tab's "Press Enter" chip).
pub fn build_send_script(
    tmux_name: &str,
    pane_id: Option<&str>,
    body: &str,
    buffer: &str,
    submit: bool,
) -> String {
    use base64::Engine as _;
    let body = body.strip_suffix('\n').unwrap_or(body);
    let exact = quote(&crate::tmux::exact_pane(tmux_name));
    let mut script = format!("t={exact}; ");
    if let Some(pane) = pane_id {
        let pane_q = quote(pane);
        let name_q = quote(tmux_name);
        script.push_str(&format!(
            "if [ \"$(tmux display-message -p -t {pane_q} '#{{session_name}}' 2>/dev/null)\" = {name_q} ]; then t={pane_q}; fi; "
        ));
    }
    if body.is_empty() {
        script.push_str("tmux send-keys -t \"$t\" Enter");
        return script;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
    let buf_q = quote(buffer);
    script.push_str(&format!(
        "printf %s {} | base64 -d | tmux load-buffer -b {buf_q} - && tmux paste-buffer -p -d -b {buf_q} -t \"$t\"",
        quote(&b64)
    ));
    if submit {
        script.push_str(" && sleep 0.15 && tmux send-keys -t \"$t\" Enter");
    }
    script
}
```

Note: `'#{{session_name}}'` inside `format!` renders as `'#{session_name}'`. The pane id and names go through `quote`, which yields `'%17'` / `'dev-foo'` for plain values, matching the test's expected prefix.

- [ ] **Step 4: Use it in `send_prompt_inner`**

Replace the body of `send_prompt_inner` (lines ~57–105) with:

```rust
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    let body = normalize_prompt_body(prompt)?;
    // The pane Claude runs in, when reconcile or a hook has told us. Lock,
    // read, unlock — never across the send.
    let pane_id = {
        let s = lock(store)?;
        s.get_session(tmux_name, host_alias)?
            .and_then(|r| r.context.tmux_pane_id)
    };
    let buffer = format!("fleet-{}", uuid::Uuid::new_v4().simple());
    let script = build_send_script(tmux_name, pane_id.as_deref(), &body, &buffer, submit);
    let out = if host_alias == "local" {
        crate::service::hub::ensure_local_allowed(host_alias)?;
        tokio::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .await
            .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn bash: {e}")))?
    } else {
        ssh.run(
            host_alias,
            &["bash", "-lc", &quote(&script)],
            std::time::Duration::from_secs(10),
        )
        .await?
    };
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_TMUX,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
```
Keep the rest of the function (the `is_prompt` recording block) exactly as it is, but record from `&body` instead of `prompt`.

Check `get_session`'s return type in `store/sessions.rs:18`: if it returns `Result<Option<SessionRow>, rusqlite::Error>`, map the error with `?` through `IpcError::from` (the file already converts rusqlite errors elsewhere; follow the same pattern as `lock(store)?` callers nearby).

- [ ] **Step 5: Check the other tmux key senders are untouched**

```bash
grep -rn "send-keys" crates/fleet-core/src/service/playbooks.rs crates/fleet-core/src/service/safe_kill.rs crates/fleet-core/src/service/sessions/review.rs
```
Expected: `playbooks.rs` builds its own bare `Enter` (leave it); `safe_kill.rs` and `review.rs` go through `send_prompt` (they get the new primitive for free). No edits.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p fleet-core --lib service::sessions 2>&1 | grep -E "^test result|FAILED|panicked"
```
Expected: `test result: ok`.

- [ ] **Step 7: Real-tmux smoke test (local machine has tmux)**

```bash
tmux new-session -d -s fleet-smoke 'cat' && sleep 0.3
cargo test -p fleet-core --lib service::sessions::tests::send_script 2>&1 | tail -2
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cat > /tmp/fleet-smoke.sh <<'EOF'
t='=fleet-smoke:'; printf %s 'bGluZTEKbGluZTI=' | base64 -d | tmux load-buffer -b 'fleet-smoke-1' - && tmux paste-buffer -p -d -b 'fleet-smoke-1' -t "$t" && sleep 0.15 && tmux send-keys -t "$t" Enter
EOF
bash /tmp/fleet-smoke.sh && sleep 0.3 && tmux capture-pane -p -t '=fleet-smoke:' | head -4; tmux kill-session -t '=fleet-smoke'; rm /tmp/fleet-smoke.sh
```
Expected: the capture shows `line1` and `line2` echoed by `cat` (no bracketed-paste bytes visible, since `cat` did not request the mode). Report the output verbatim in the task summary.

- [ ] **Step 8: fmt, clippy, full suite, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
git add crates/fleet-core/src/service/sessions/prompt.rs crates/fleet-core/src/service/sessions/tests.rs
git commit -m "fix(prompt): deliver through load-buffer/paste-buffer to the known pane; normalise CR and refuse control bytes"
```

---

### Task 3: Gate, ack and dedupe in `deliver_prompt`; refuse blocked recipients in broadcast and messages

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/params.rs:236-258` (`SendPromptParams`)
- Modify: `crates/fleet-core/src/mcp/tools/support.rs:842-862` (`deliver_prompt`) + new helpers next to `run_prompt_ready` (line ~258)
- Modify: `crates/fleet-core/src/mcp/tools/messaging.rs:6-42` (`send_prompt` tool)
- Modify: `crates/fleet-core/src/mcp/tools/mod.rs:62-112` (`FleetTools.recent_sends`)
- Modify: `crates/fleet-core/src/service/sessions/prompt.rs:282-313` (`select_targets`)
- Modify: `crates/fleet-core/src/service/messages.rs:172-191` (`deliver` branch)
- Test: `crates/fleet-core/src/mcp/tools/tests.rs`, `crates/fleet-core/src/service/sessions/tests.rs` (near line 195), `crates/fleet-core/src/service/messages.rs` tests (grep `mod tests` there)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: `Store::prompt_ack_state(row_id) -> Result<Option<PromptAckState>>` (Task 1); `sessions::send_prompt(SendPromptArgs, store, ssh)` (Task 2, unchanged signature).
- Produces (all `pub(super)` in `support.rs`):
  - `fn delivery_gate(row: &SessionRow, force: bool, submit: bool) -> Result<bool /* queued */, McpError>`
  - `async fn await_prompt_ack(store: &Mutex<Store>, row_id: i64, seq_before: i64, wait: Duration) -> Result<bool, McpError>`
  - `fn turn_seq_before(turn_seq: i64, queued: bool, acked: Option<bool>) -> i64`
  - `async fn deliver_prompt(&self, row: &SessionRow, prompt: String, submit: bool, force: bool) -> Result<serde_json::Value, McpError>` returning `{ delivered, session_id, turn_seq_before, queued, acked }`.
  - `SendPromptParams.force: bool` (default false), `SendPromptParams.client_msg_id: Option<String>`.
  - `pub(super) const ACK_WAIT: Duration = 1_500 ms`, `ACK_POLL = 100 ms`.

- [ ] **Step 1: Failing tests for the pure gate and the ack wait**

Add to `crates/fleet-core/src/mcp/tools/tests.rs` (near the `run_prompt_ready` tests; grep `fn run_prompt_ready` to find them, else append at the end of the file):

```rust
fn row_with(status: Option<&str>, stuck: Option<&str>, turn_seq: i64) -> crate::store::SessionRow {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("gate", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let mut row = s.get_session_by_id(id).unwrap().unwrap();
    row.claude_status = status.map(str::to_string);
    row.stuck_kind = stuck.map(str::to_string);
    row.turn_seq = turn_seq;
    row
}

#[test]
fn delivery_gate_refuses_a_blocked_or_stuck_session_unless_forced() {
    let blocked = row_with(Some("blocked"), None, 3);
    let e = delivery_gate(&blocked, false, true).unwrap_err();
    assert!(e.message.starts_with("E_INVALID_STATE"), "{}", e.message);
    assert!(e.message.contains("force"), "{}", e.message);
    let stuck = row_with(Some("idle"), Some("trust_prompt"), 3);
    let e = delivery_gate(&stuck, false, true).unwrap_err();
    assert!(e.message.contains("trust_prompt"), "{}", e.message);
    assert_eq!(delivery_gate(&blocked, true, true).unwrap(), false);
    assert_eq!(delivery_gate(&stuck, true, true).unwrap(), false);
}

#[test]
fn delivery_gate_reports_a_working_session_as_queued_and_idle_as_not() {
    assert_eq!(delivery_gate(&row_with(Some("working"), None, 1), false, true).unwrap(), true);
    assert_eq!(delivery_gate(&row_with(Some("idle"), None, 1), false, true).unwrap(), false);
    assert_eq!(delivery_gate(&row_with(None, None, 1), false, true).unwrap(), false);
    // Staging text (submit=false) never queues a turn.
    assert_eq!(delivery_gate(&row_with(Some("working"), None, 1), false, false).unwrap(), false);
}

#[test]
fn turn_seq_before_points_past_the_current_turn_only_for_an_unacked_queued_prompt() {
    assert_eq!(turn_seq_before(7, false, Some(true)), 7);
    assert_eq!(turn_seq_before(7, false, None), 7);
    assert_eq!(turn_seq_before(7, true, Some(true)), 7, "acked now: the status was stale");
    assert_eq!(turn_seq_before(7, true, Some(false)), 8, "really queued behind the running turn");
    assert_eq!(turn_seq_before(7, true, None), 8);
}

#[tokio::test(start_paused = true)]
async fn await_prompt_ack_returns_true_once_the_submit_counter_moves() {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let id = {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("ack", "local", None, None, 1, 1, "running", None)
            .unwrap()
    };
    let bump = {
        let store = store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(350)).await;
            store.lock().unwrap().record_prompt_submit_hook_for_row(id).unwrap();
        })
    };
    let acked = await_prompt_ack(&store, id, 0, ACK_WAIT).await.unwrap();
    bump.await.unwrap();
    assert!(acked);
}

#[tokio::test(start_paused = true)]
async fn await_prompt_ack_returns_false_when_nothing_moves_before_the_deadline() {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let id = {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("ack", "local", None, None, 1, 1, "running", None)
            .unwrap()
    };
    assert!(!await_prompt_ack(&store, id, 0, ACK_WAIT).await.unwrap());
}
```

Add to `crates/fleet-core/src/service/sessions/tests.rs` after `select_targets_filters_by_project`:

```rust
#[test]
fn select_targets_skips_blocked_and_stuck_sessions_unless_asked_for_them() {
    let mut s = sample_sessions();
    s[0].claude_status = Some("blocked".into());
    s[1].stuck_kind = Some("auth_menu".into());
    let f = BroadcastFilter::default();
    let picked = select_targets(&s, &f, None, None);
    assert!(!picked.contains(&1), "a blocked session would get Enter on its dialog: {picked:?}");
    assert!(!picked.contains(&2), "a stuck session would get Enter on its menu: {picked:?}");
    let f = BroadcastFilter {
        status: Some("blocked".into()),
        ..Default::default()
    };
    assert_eq!(select_targets(&s, &f, None, None), vec![1], "an explicit status filter is the operator's choice");
}
```
(Check `sample_sessions()` a few lines above: sessions 1 and 2 must be `kind == "work"` with no host/project filter interference; adjust the indices to two work sessions if the fixture differs.)

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p fleet-core --lib delivery_gate 2>&1 | tail -3
cargo test -p fleet-core --lib select_targets_skips 2>&1 | tail -3
```
Expected: compile errors (`delivery_gate`, `await_prompt_ack`, `turn_seq_before`, `ACK_WAIT` missing); the selector test fails on the assertion.

- [ ] **Step 3: Params**

In `crates/fleet-core/src/mcp/tools/params.rs`, add to `SendPromptParams` after `raw`:

```rust
    /// Deliver even when the session is blocked on a dialog or stuck. Off by
    /// default: Enter on a permission prompt selects the highlighted answer.
    #[serde(default)]
    pub force: bool,
    /// Caller-chosen id for this send. A repeat with the same id within ten
    /// minutes returns the first result instead of delivering again.
    #[serde(default)]
    pub client_msg_id: Option<String>,
```

- [ ] **Step 4: Gate, ack and the new `deliver_prompt`**

In `crates/fleet-core/src/mcp/tools/support.rs`, after `run_prompt_ready`:

```rust
/// How long a send waits for the REPL's `UserPromptSubmit` hook before
/// reporting `acked: false`, and how often it looks.
pub(super) const ACK_WAIT: std::time::Duration = std::time::Duration::from_millis(1_500);
pub(super) const ACK_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// May a prompt be delivered to this row now? `Err(E_INVALID_STATE)` for a
/// session that is `blocked` or has a `stuck_kind` (Enter would answer its
/// dialog) unless `force`. `Ok(queued)`: true when the session is `working`
/// and the prompt will be submitted, so Claude Code queues it behind the
/// running turn.
pub(super) fn delivery_gate(
    row: &crate::store::SessionRow,
    force: bool,
    submit: bool,
) -> Result<bool, McpError> {
    let blocked_on = match (row.claude_status.as_deref(), row.stuck_kind.as_deref()) {
        (_, Some(kind)) => Some(kind.to_string()),
        (Some("blocked"), None) => Some("a dialog".to_string()),
        _ => None,
    };
    if let (Some(what), false) = (blocked_on, force) {
        return Err(mcp_err(
            "E_INVALID_STATE",
            format!(
                "session {} is waiting on {what}; Enter would answer it — resolve it in the \
                 terminal, or pass force: true to type into it anyway",
                row.id
            ),
            None,
        ));
    }
    Ok(submit && row.claude_status.as_deref() == Some("working"))
}

/// Poll `prompt_submit_seq` until it passes `seq_before` (the REPL took the
/// prompt) or `wait` elapses. Lock, read, unlock — never across the sleep.
pub(super) async fn await_prompt_ack(
    store: &Mutex<crate::store::Store>,
    row_id: i64,
    seq_before: i64,
    wait: std::time::Duration,
) -> Result<bool, McpError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let seq = {
            let s = lock(store).map_err(to_mcp_err)?;
            s.prompt_ack_state(row_id)
                .map_err(|e| to_mcp_err(e.into()))?
                .map(|st| st.prompt_submit_seq)
        };
        match seq {
            None => return Ok(false),
            Some(seq) if seq > seq_before => return Ok(true),
            Some(_) => {}
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(ACK_POLL.min(deadline - now)).await;
    }
}

/// The turn number a caller waits past to collect THIS prompt's reply. A
/// prompt queued behind a running turn is answered by the turn after it;
/// one that was acked now (the `working` status was stale) is the next turn.
pub(super) fn turn_seq_before(turn_seq: i64, queued: bool, acked: Option<bool>) -> i64 {
    if queued && acked != Some(true) {
        turn_seq + 1
    } else {
        turn_seq
    }
}
```

Replace `deliver_prompt` with:

```rust
    /// Deliver a (already marked) prompt to a resolved session and return
    /// `{ delivered, session_id, turn_seq_before, queued, acked }`.
    ///
    /// `acked` is `true` once the session's `UserPromptSubmit` hook stamped
    /// the row after the send, `false` when it did not within [`ACK_WAIT`]
    /// (after one Enter retry for an idle session — the classic "text
    /// arrived, Enter did not"), and `null` when it cannot be known: nothing
    /// was submitted, or no hook has ever reached this row.
    pub(super) async fn deliver_prompt(
        &self,
        row: &crate::store::SessionRow,
        prompt: String,
        submit: bool,
        force: bool,
    ) -> Result<serde_json::Value, McpError> {
        let queued = delivery_gate(row, force, submit)?;
        let before = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.prompt_ack_state(row.id).map_err(|e| to_mcp_err(e.into()))?
        };
        let send = |prompt: String| {
            sessions::send_prompt(
                sessions::SendPromptArgs {
                    host_alias: row.host_alias.clone(),
                    tmux_name: row.tmux_name.clone(),
                    prompt,
                    submit,
                },
                &self.store,
                &self.ssh,
            )
        };
        send(prompt).await.map_err(to_mcp_err)?;
        let acked = match before {
            Some(st) if submit && st.hooks_seen => {
                let mut ok = await_prompt_ack(&self.store, row.id, st.prompt_submit_seq, ACK_WAIT).await?;
                if !ok && !queued {
                    // One more Enter: an empty body is the bare-Enter path.
                    send(String::new()).await.map_err(to_mcp_err)?;
                    ok = await_prompt_ack(&self.store, row.id, st.prompt_submit_seq, ACK_WAIT).await?;
                }
                Some(ok)
            }
            _ => None,
        };
        Ok(serde_json::json!({
            "delivered": true,
            "session_id": row.id,
            "turn_seq_before": turn_seq_before(row.turn_seq, queued, acked),
            "queued": queued,
            "acked": acked,
        }))
    }
```

If `to_mcp_err` takes an `IpcError` and `rusqlite::Error` has no `Into<IpcError>`, wrap with `IpcError::from(e)` or the file's existing conversion (grep `rusqlite::Error` in `support.rs` for the pattern in use). Every other caller of `deliver_prompt` (`broadcast_prompt`, `run_prompt`, `dispatch_task` in `orchestration.rs`, `messaging.rs`) passes `force: false` — `cargo check` lists them.

- [ ] **Step 5: Dedupe cache on `FleetTools`**

In `crates/fleet-core/src/mcp/tools/mod.rs` add the field and init:

```rust
    /// `send_prompt` results by `(caller label, client_msg_id)`, so a
    /// retried call delivers once. Created once in `new`; every per-MCP-
    /// session clone shares it.
    recent_sends: Arc<std::sync::Mutex<RecentSends>>,
```
```rust
            recent_sends: Arc::new(std::sync::Mutex::new(RecentSends::default())),
```
and, in the same file:

```rust
/// Bounded, TTL'd memory of recent `send_prompt` results (S-dedupe).
#[derive(Default)]
pub(super) struct RecentSends {
    entries: std::collections::HashMap<(String, String), (std::time::Instant, serde_json::Value)>,
}

pub(super) const RECENT_SENDS_TTL: std::time::Duration = std::time::Duration::from_secs(600);
pub(super) const RECENT_SENDS_MAX: usize = 1024;

impl RecentSends {
    pub(super) fn get(&mut self, caller: &str, id: &str) -> Option<serde_json::Value> {
        self.sweep();
        self.entries
            .get(&(caller.to_string(), id.to_string()))
            .map(|(_, v)| v.clone())
    }
    pub(super) fn put(&mut self, caller: &str, id: &str, value: serde_json::Value) {
        self.sweep();
        if self.entries.len() >= RECENT_SENDS_MAX {
            // Drop the oldest so the map stays bounded under a chatty client.
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries
            .insert((caller.to_string(), id.to_string()), (std::time::Instant::now(), value));
    }
    fn sweep(&mut self) {
        let now = std::time::Instant::now();
        self.entries.retain(|_, (at, _)| now.duration_since(*at) < RECENT_SENDS_TTL);
    }
}
```

Test, in `crates/fleet-core/src/mcp/tools/tests.rs`:

```rust
#[test]
fn recent_sends_returns_the_first_result_for_a_repeated_id_and_stays_bounded() {
    let mut r = RecentSends::default();
    assert!(r.get("m", "a").is_none());
    r.put("m", "a", serde_json::json!({"n": 1}));
    assert_eq!(r.get("m", "a"), Some(serde_json::json!({"n": 1})));
    assert!(r.get("other-caller", "a").is_none(), "keyed per caller");
    for i in 0..(RECENT_SENDS_MAX + 5) {
        r.put("m", &format!("id-{i}"), serde_json::json!(i));
    }
    assert!(r.entries.len() <= RECENT_SENDS_MAX);
}
```

- [ ] **Step 6: Wire the tool**

In `crates/fleet-core/src/mcp/tools/messaging.rs`, `send_prompt`:
- Add to the `#[tool(description = …)]` string, before the final `)."`: `Refuses a blocked or stuck session (E_INVALID_STATE) unless force=true; a working session queues it (queued=true). acked reports whether the REPL's hook confirmed it. Repeat a client_msg_id to retry without delivering twice.` Keep it to those three sentences.
- Replace the tail of the handler:

```rust
        let label = caller.label();
        if let Some(id) = p.client_msg_id.as_deref() {
            if let Some(hit) = self.recent_sends.lock().unwrap().get(&label, id) {
                audit("send_prompt", &format!("dedupe client_msg_id={id}"));
                return ok_json(&hit);
            }
        }
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let out = self.deliver_prompt(&row, prompt, p.submit, p.force).await?;
        if let Some(id) = p.client_msg_id.as_deref() {
            self.recent_sends.lock().unwrap().put(&label, id, out.clone());
        }
        ok_json(&out)
```
(`Caller::label()` exists at `auth.rs:108`.) Use the poison-tolerant lock pattern if the file has one; otherwise `.lock().unwrap()` matches the surrounding code.

- [ ] **Step 7: Broadcast and messages**

`crates/fleet-core/src/service/sessions/prompt.rs`, `select_targets`: add after the `kind == "work"` filter:

```rust
        // Never fan Enter into a dialog: a blocked or stuck session is
        // skipped unless the operator's status filter asks for exactly that.
        .filter(|s| {
            f.status.as_deref() == Some("blocked")
                || (s.claude_status.as_deref() != Some("blocked") && s.stuck_kind.is_none())
        })
```

`crates/fleet-core/src/service/messages.rs`, inside `if args.deliver {`, before building `header`:

```rust
        if to_row.claude_status.as_deref() == Some("blocked") || to_row.stuck_kind.is_some() {
            deliver_error = Some(format!(
                "session {} is waiting on {}; the message is in its inbox but was not typed into the dialog",
                to_row.id,
                to_row.stuck_kind.as_deref().unwrap_or("a dialog")
            ));
        } else {
            // existing send_prompt match block, unchanged
        }
```
Add a test next to the existing `send_message` tests in `messages.rs` (grep `deliver_error`): a recipient row with `claude_status = "blocked"` (use `set_claude_status_by_session_id` or a direct `UPDATE sessions SET claude_status='blocked'` through `store.conn` if the test module has access) yields `delivered_to_pane == false` and a `deliver_error` mentioning `inbox`, while the inbox row exists.

- [ ] **Step 8: Run tests, regenerate the reference, check the budget**

```bash
cargo test -p fleet-core --lib mcp::tools 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core --lib service:: 2>&1 | grep -E "^test result|FAILED|panicked"
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current 2>&1 | tail -3
cargo test -p fleet-core the_served_definition_budget_stays_bounded 2>&1 | tail -3
```
Expected: all `ok`. If the budget test fails, shorten the description clause (drop the `acked` sentence first).

- [ ] **Step 9: fmt, clippy, full suite, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
git add crates/fleet-core/src/mcp crates/fleet-core/src/service docs/control-api-reference.md
git commit -m "feat(mcp): send_prompt refuses blocked sessions, reports queued/acked, dedupes by client_msg_id"
```

---

### Task 4: `new_session` returns the re-read row

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/lifecycle.rs:618-690`
- Test: `crates/fleet-core/src/service/sessions/lifecycle_tests.rs`

**Interfaces:**
- Consumes: `Store::set_started_at` emitting (Task 1), `Store::get_session_by_id`.
- Produces: `new_session` still returns `Result<SessionRow, IpcError>`, now always the row as of its last write.

- [ ] **Step 1: Extract the tail into a testable function and write its test**

Add to `lifecycle_tests.rs`:

```rust
/// The row `new_session` hands back is the row as of its LAST write — not a
/// snapshot from before `set_started_at` / `set_friendly_name` /
/// `set_claude_session_id` that the frontend would then merge over fresher
/// state (its guard is `row_version`, and a stale snapshot has the lower one).
#[test]
fn finalize_new_session_returns_the_row_as_of_its_last_write() {
    let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
    let s = crate::store::Store::open_with_bus_in_memory(bus.clone()).unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("f4final", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let before = s.get_session_by_id(id).unwrap().unwrap();
    let row = finalize_new_session(&s, id, "local", "f4final", Some("Nice Name"), Some("uuid-9"), false)
        .unwrap();
    assert_eq!(row.started_at.is_some(), true, "started_at is on the returned row");
    assert_eq!(row.friendly_name.as_deref(), Some("Nice Name"));
    assert_eq!(row.claude_session_id.as_deref(), Some("uuid-9"));
    assert!(row.row_version > before.row_version, "the returned row is the fresh one");
    let latest = s.get_session_by_id(id).unwrap().unwrap();
    assert_eq!(row, latest, "returned == stored");
}

#[test]
fn finalize_new_session_tags_a_shell_session() {
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("f4shell", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let row = finalize_new_session(&s, id, "local", "f4shell", None, None, true).unwrap();
    assert_eq!(row.kind, "shell");
    assert!(row.started_at.is_some());
}
```
(`SessionRow` derives `PartialEq`; check `rows.rs:113`. If not, compare `row_version`, `started_at`, `friendly_name`, `claude_session_id`, `kind` field by field.)

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p fleet-core --lib finalize_new_session 2>&1 | tail -3
```
Expected: `finalize_new_session` not found.

- [ ] **Step 3: Implement**

In `lifecycle.rs`, add:

```rust
/// The writes `new_session` makes after the session exists, then ONE re-read
/// so the returned row is the row as of the last write (the frontend merges
/// it optimistically and orders it by `row_version`). Soft-fails the
/// cosmetic writes (`started_at`, friendly name, claude id) with a warning;
/// the `kind` tag and the final read are hard failures.
pub(super) fn finalize_new_session(
    s: &Store,
    row_id: i64,
    host_alias: &str,
    name: &str,
    friendly_name: Option<&str>,
    claude_id: Option<&str>,
    is_shell: bool,
) -> Result<SessionRow, IpcError> {
    // PROD-5: the fleet created this session now.
    if let Err(e) = s.set_started_at(row_id, now_unix()) {
        tracing::warn!(session = %name, error = %e, "[new_session] storing started_at failed");
    }
    if let Some(value) = friendly_name {
        if let Err(e) = s.set_friendly_name(host_alias, name, Some(value)) {
            tracing::warn!(session = %name, error = %e, "[new_session] storing friendly_name failed");
        }
    }
    if is_shell {
        s.set_session_kind(row_id, "shell", None)?;
    } else if let Some(cid) = claude_id {
        // Soft-fail: the session is live; a failed write just means a future
        // recreate falls back to `cl --continue`.
        if let Err(e) = s.set_claude_session_id(row_id, cid) {
            tracing::warn!(session = %name, error = %e, "[new_session] storing claude_session_id failed");
        }
    }
    s.get_session_by_id(row_id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "session vanished after creation"))
}
```

Then replace everything in `new_session` from the `// PROD-5` comment (line ~632) to the final `Ok(row)` with:

```rust
    let derived_friendly = derive_friendly_name(&s, &args, row.worktree_id)?;
    finalize_new_session(
        &s,
        row.id,
        &args.host_alias,
        &args.name,
        derived_friendly.as_deref(),
        claude_id.as_deref(),
        is_shell,
    )
```
(`derive_friendly_name` stays where it is; `claude_id` is the `Option<String>` already in scope. Delete the now-unused `let mut row = row;` and the hand-patching of `context`, `friendly_name`, `claude_session_id`.)

- [ ] **Step 4: Tests, fmt, clippy, commit**

```bash
cargo test -p fleet-core --lib service::sessions 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
git add crates/fleet-core/src/service/sessions/lifecycle.rs crates/fleet-core/src/service/sessions/lifecycle_tests.rs
git commit -m "fix(sessions): new_session returns the row as of its last write"
```

---

### Task 5: Frontend — `isStale` on `row_version`, subscribe before list, merge instead of set

**Files:**
- Modify: `src/lib/sessions.ts:110-115` (interface), `:152-162` (`isStale`), `:193-201` (`loadSessions`)
- Modify: `src/App.svelte:190-236`
- Test: `src/lib/sessions.test.ts`, `src/App.test.ts`

**Interfaces:**
- Consumes: `row_version` on the wire (Task 1).
- Produces: `SessionRow.row_version?: number`; `loadSessions` merges rows in and drops rows the list no longer has.

- [ ] **Step 1: Failing tests**

`src/lib/sessions.test.ts` — add (reuse the fixture row at line ~45 as `base`; copy it into a `const base = {...}` if it is inline):

```ts
describe('optimistic merge guard', () => {
  it('drops a payload whose row_version is older than the row it holds', () => {
    sessions.set([{ ...base, id: 1, friendly_name: 'newer', row_version: 5 }]);
    const { applySessionEvents } = require('./sessions') as typeof import('./sessions');
    applySessionEvents([{ name: 'session:updated', payload: { ...base, id: 1, friendly_name: 'older', row_version: 4 } }]);
    expect(get(sessions)[0].friendly_name).toBe('newer');
  });

  it('applies a payload with an equal or newer row_version, and one without any', () => {
    sessions.set([{ ...base, id: 1, friendly_name: 'v5', row_version: 5 }]);
    const { applySessionEvents } = require('./sessions') as typeof import('./sessions');
    applySessionEvents([{ name: 'session:updated', payload: { ...base, id: 1, friendly_name: 'v5b', row_version: 5 } }]);
    expect(get(sessions)[0].friendly_name).toBe('v5b');
    applySessionEvents([{ name: 'session:updated', payload: { ...base, id: 1, friendly_name: 'v6', row_version: 6 } }]);
    expect(get(sessions)[0].friendly_name).toBe('v6');
    // A row built client-side (no row_version) is never rejected for lacking one.
    sessions.set([{ ...base, id: 2, friendly_name: 'x' }]);
    applySessionEvents([{ name: 'session:updated', payload: { ...base, id: 2, friendly_name: 'y', row_version: 1 } }]);
    expect(get(sessions)[0].friendly_name).toBe('y');
  });

  it('loadSessions keeps an event applied while the list was in flight', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_sessions') {
        // The list was built before the event: an older row_version.
        return [{ ...base, id: 1, friendly_name: 'from-list', row_version: 3 }];
      }
      return null;
    });
    const { applySessionEvents } = require('./sessions') as typeof import('./sessions');
    const p = loadSessions();
    applySessionEvents([{ name: 'session:updated', payload: { ...base, id: 1, friendly_name: 'from-event', row_version: 4 } }]);
    await p;
    expect(get(sessions).find((s) => s.id === 1)?.friendly_name).toBe('from-event');
  });

  it('loadSessions drops rows the list no longer has', async () => {
    sessions.set([{ ...base, id: 1 }, { ...base, id: 2, tmux_name: 'gone' }]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'list_sessions' ? [{ ...base, id: 1, row_version: 1 }] : null,
    );
    await loadSessions();
    expect(get(sessions).map((s) => s.id)).toEqual([1]);
  });
});
```
Check how `applySessionEvents` is exported/imported elsewhere (`src/App.svelte` imports it from `./lib/sessions`); use a normal `import { applySessionEvents } from './sessions'` at the top of the test file instead of `require` if the file already imports from `./sessions` — it does, so extend that import line.

`src/App.test.ts` — add inside `describe('App bootstrap failure', …)` or a new `describe('App startup order')`:

```ts
  it('subscribes to row events before the first list resolves', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const { listen } = await import('@tauri-apps/api/event');
    const inv = invoke as ReturnType<typeof vi.fn>;
    const lis = listen as ReturnType<typeof vi.fn>;
    const original = inv.getMockImplementation() as
      | ((cmd: string, ...rest: unknown[]) => Promise<unknown>)
      | undefined;
    const order: string[] = [];
    lis.mockImplementation(async (name: string) => {
      order.push(`listen:${name}`);
      return () => {};
    });
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_sessions') {
        order.push('list_sessions');
        return [];
      }
      return original ? original(cmd, ...rest) : null;
    });
    try {
      render(App);
      await waitFor(() => expect(order).toContain('list_sessions'));
      const firstListen = order.findIndex((o) => o === 'listen:session:updated');
      const list = order.indexOf('list_sessions');
      expect(firstListen).toBeGreaterThanOrEqual(0);
      expect(firstListen).toBeLessThan(list);
    } finally {
      inv.mockImplementation(original!);
      lis.mockImplementation(async () => () => {});
    }
  });
```

- [ ] **Step 2: Run to verify they fail**

```bash
npx vitest run src/lib/sessions.test.ts src/App.test.ts 2>&1 | tail -20
```
Expected: the four new session tests fail (type error on `row_version` or wrong values); the ordering test fails on `firstListen < list`.

- [ ] **Step 3: Implement**

`src/lib/sessions.ts`:
- Interface, after `tmux_pane_id`:
```ts
  /** Bumped by the backend on every write (migration 040); orders a
   *  command's return value against a row event. Absent on rows built
   *  client-side and on rows from a hub older than the column. */
  row_version?: number;
```
- Guard:
```ts
  // Monotonic guard: a payload carrying a lower row_version than the row we
  // hold is a stale snapshot (a command return value that raced a newer
  // `session:updated`). Equal versions still apply. A payload without one is
  // never rejected for it — only a KNOWN older version is.
  isStale: (incoming, current) =>
    incoming.row_version !== undefined &&
    current.row_version !== undefined &&
    incoming.row_version < current.row_version,
```
- `loadSessions`: replace `sessions.set(r.value)` with a merge that keeps fresher rows and drops vanished ones:
```ts
  if (r.ok) {
    const listed = new Set(r.value.map((s) => s.id));
    sessions.update((cur) => {
      let next = cur.filter((s) => listed.has(s.id));
      for (const row of r.value) next = rows.mergeInto(next, row);
      return next;
    });
    sessionsLoaded.set(true);
  }
```
(`rows.mergeInto` is the pure upsert from `row_store.ts`; the tombstone check inside it is what keeps a just-killed row from coming back via a stale list.)

`src/App.svelte`: move the whole `unlistenEvents = await subscribeToRowEvents({ … })` call (and its comment) to just BEFORE `const [pr, sr, hr, ar] = await Promise.all([` at line ~190. Leave `void loadTasks()` and `loadAccountUsage()` where they are. Update the comment above the moved block: "Subscribed BEFORE the first list: a `session:updated` that lands while the list is in flight would otherwise be emitted to no listener and lost until the row changes again."

- [ ] **Step 4: Run tests and the type-check**

```bash
npx vitest run 2>&1 | tail -8
npx svelte-check 2>&1 | tail -3
```
Expected: all green; svelte-check reports 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/sessions.ts src/lib/sessions.test.ts src/App.svelte src/App.test.ts
git commit -m "fix(ui): order optimistic merges by row_version and subscribe to row events before the first list"
```

---

### Task 6: Hub client — per-tool timeout from the hub's deadline table, connect timeout, breaker, `E_HUB_TIMEOUT`

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/support.rs:950-965` (`tool_deadline` → `pub`), `crates/fleet-core/src/mcp/tools/mod.rs` (re-export), `crates/fleet-core/src/mcp/mod.rs:32-36` (re-export)
- Modify: `crates/fleet-core/src/ipc_error.rs` (after `E_HUB_UNREACHABLE`, line ~224)
- Modify: `src-tauri/src/backend/remote.rs:289-330` (`call_text`), `:1016-1039` (`connect`), `:1044-1063` (timeouts)
- Modify: `src/lib/moves.ts:100,299`, `src/lib/moves.test.ts`, `src/lib/result.ts`, `src/App.svelte` (focus handler)
- Test: `src-tauri/src/backend/tests_remote.rs:920-975`, `src/lib/moves.test.ts`

**Interfaces:**
- Produces: `fleet_core::mcp::tool_deadline(tool: &str) -> Duration` (pub); `codes::E_HUB_TIMEOUT`; `remote::call_timeout(tool)`; `remote::CONNECT_TIMEOUT`.

- [ ] **Step 1: Failing Rust tests**

In `src-tauri/src/backend/tests_remote.rs`, replace `only_move_session_gets_the_long_call_timeout` with:

```rust
/// The client must never give up before the hub does: a lifecycle tool the
/// hub bounds at 300 s answered "did not answer" at 30 s while the hub kept
/// creating the session, and the user's retry made two. Every routed tool's
/// client bound is the hub's own deadline plus a margin.
#[test]
fn the_client_timeout_dominates_the_hub_deadline_for_every_routed_tool() {
    let margin = std::time::Duration::from_secs(10);
    for (cmd, verdict) in super::verdicts::VERDICTS {
        let Some(tool) = verdict.tool() else { continue };
        let hub = fleet_core::mcp::tool_deadline(tool);
        assert!(
            call_timeout(tool) >= hub + margin,
            "{cmd} -> {tool}: client {:?} must be >= hub {:?} + {margin:?}",
            call_timeout(tool),
            hub
        );
    }
    assert!(call_timeout("move_session") >= std::time::Duration::from_secs(15 * 60));
}

/// A timed-out exchange is NOT "unreachable": the hub may well have taken the
/// request. The code and the words say the outcome is unknown.
#[tokio::test(start_paused = true)]
async fn a_timed_out_call_says_the_outcome_is_unknown() {
    struct Silent;
    #[async_trait::async_trait]
    impl HubTransport for Silent {
        async fn post_json(&self, _: &str, _: &str, _: String) -> Result<HubResponse, String> {
            std::future::pending().await
        }
    }
    let b = HubBackend::with_transport(cfg(), Arc::new(Silent));
    let e = b.list_sessions(false).await.expect_err("never answers");
    assert_eq!(e.code, codes::E_HUB_TIMEOUT);
    assert!(e.message.contains("may still complete"), "{}", e.message);
    assert!(!e.message.contains("cl_s3cret-token"), "{}", e.message);
}

/// While the event bridge has already found the hub unreachable twice, a
/// call is refused at once instead of hanging its full bound: the bridge's
/// reconnect is the probe, and the user's click is not.
#[test]
fn a_call_while_the_link_is_known_offline_is_refused_before_the_transport_is_touched() {
    let fake = Fake::answering(Ok(ok("[]")));
    let status = link(HubConnection::Offline {
        attempt: 2,
        retry_in_secs: 7,
        reason: "connection refused".into(),
    });
    let err = block_on(watched(&fake, &status).list_sessions(false)).expect_err("refused");
    assert_eq!(err.code, codes::E_HUB_UNREACHABLE);
    assert!(err.message.contains("retrying in 7"), "{}", err.message);
    nothing_was_sent(&fake);
    // The first failed attempt is not yet a verdict: the call still goes out.
    let status = link(HubConnection::Offline {
        attempt: 1,
        retry_in_secs: 1,
        reason: "connection refused".into(),
    });
    let fake = Fake::answering(Ok(ok("[]")));
    block_on(watched(&fake, &status).list_sessions(false)).expect("still tried");
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}
```
Update the existing test `a_hub_that_accepts_the_request_and_then_says_nothing_still_ends_the_call` to expect `codes::E_HUB_TIMEOUT` and the phrase `no answer within`.

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p claude-fleet --lib backend::tests_remote 2>&1 | grep -E "FAILED|error\[|^test result" | head
```
Expected: compile errors (`fleet_core::mcp::tool_deadline` private, `E_HUB_TIMEOUT` missing).

- [ ] **Step 3: Expose the deadline and add the code**

`crates/fleet-core/src/mcp/tools/support.rs`: change `pub(super) fn tool_deadline` to `pub fn tool_deadline`. In `crates/fleet-core/src/mcp/tools/mod.rs` add `pub use support::tool_deadline;` (next to the other `use support::…` lines; make it `pub`). In `crates/fleet-core/src/mcp/mod.rs` add `pub use tools::tool_deadline;`.

`crates/fleet-core/src/ipc_error.rs`, after `E_HUB_UNREACHABLE`:
```rust
    /// The hub took the request and did not answer within the client's bound.
    /// Unlike `E_HUB_UNREACHABLE`, the operation may have run — a mutation
    /// must not be blindly retried; re-list first.
    pub const E_HUB_TIMEOUT: &str = "E_HUB_TIMEOUT";
```

- [ ] **Step 4: Timeouts, connect timeout, breaker in `remote.rs`**

Replace the `CALL_TIMEOUT` / `MOVE_CALL_TIMEOUT` / `call_timeout` block with:

```rust
/// Added to the hub's own per-tool deadline ([`fleet_core::mcp::tool_deadline`])
/// so the client never gives up before the server: a call reported failed
/// while the hub completes it is how a `new_session` gets clicked twice.
const CALL_MARGIN: std::time::Duration = std::time::Duration::from_secs(10);

/// `move_session` copies a repository, a transcript and the Claude state
/// between hosts: minutes, not seconds. Its bound is the larger of the hub's
/// deadline and this floor.
const MOVE_CALL_FLOOR: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How long `tool` may take to answer: the hub's deadline for it plus a
/// margin. Still bounded: a hub that stops answering mid-call must not leave
/// the window waiting forever.
fn call_timeout(tool: &str) -> std::time::Duration {
    let bound = fleet_core::mcp::tool_deadline(tool) + CALL_MARGIN;
    match tool {
        "move_session" => bound.max(MOVE_CALL_FLOOR),
        _ => bound,
    }
}

/// TCP connect, and separately the TLS handshake, each get this long. A
/// black-holed hub then costs seconds, not the whole call bound.
pub(super) const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
```

In `call_text`, replace the timeout `map_err` so a timeout is `E_HUB_TIMEOUT`:
```rust
                .map_err(|_| {
                    IpcError::new(
                        codes::E_HUB_TIMEOUT,
                        format!(
                            "{} did not answer: no answer within {limit:.0?} — the request may \
                             still complete on the hub; refresh before retrying",
                            self.cfg.base_url
                        ),
                    )
                })?
```
and add the breaker right after the `contract_error` check at the top of `call_text`:
```rust
        if let Some(refused) = self.offline_error(tool) {
            return Err(refused);
        }
```
with, next to `contract_error`:
```rust
    /// Fail fast while the event bridge already knows the hub refuses
    /// connections: from the second failed attempt on, a call is answered
    /// from that knowledge instead of waiting its own bound. Reads the
    /// STATE, unlike [`Self::contract_error`], because this gate only ever
    /// closes — it refuses a call, it never lets one through that the
    /// contract verdict would have stopped.
    fn offline_error(&self, what: &str) -> Option<IpcError> {
        match self.link.as_ref()?.current() {
            HubConnection::Offline {
                attempt,
                retry_in_secs,
                reason,
            } if attempt >= 2 => Some(IpcError::new(
                codes::E_HUB_UNREACHABLE,
                format!(
                    "{what} was not sent: {} has refused {attempt} connection attempts ({}); \
                     retrying in {retry_in_secs}s",
                    self.cfg.base_url,
                    self.redact(&reason)
                ),
            )),
            _ => None,
        }
    }
```

In `connect`, wrap both awaits:
```rust
    let tcp = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((at.host(), at.port())),
    )
    .await
    .map_err(|_| format!("connect {}:{}: timed out after {CONNECT_TIMEOUT:?}", at.host(), at.port()))?
    .map_err(|e| format!("connect {}:{}: {e}", at.host(), at.port()))?;
    let _ = tcp.set_nodelay(true);
```
and
```rust
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
        .await
        .map_err(|_| format!("TLS handshake with {}:{} timed out after {CONNECT_TIMEOUT:?}", at.host(), at.port()))?
        .map_err(|e| format!("TLS handshake with {}:{} failed: {e}", at.host(), at.port()))?;
```

- [ ] **Step 5: Frontend — treat the timeout as "no answer" and refresh**

`src/lib/moves.ts`: replace `const NO_ANSWER = 'E_HUB_UNREACHABLE';` with
```ts
const NO_ANSWER: ReadonlySet<string> = new Set(['E_HUB_UNREACHABLE', 'E_HUB_TIMEOUT']);
```
and `if (r.error.code === NO_ANSWER)` with `if (NO_ANSWER.has(r.error.code))`. In `src/lib/moves.test.ts`, duplicate the existing "hub did not answer" case (line ~404) with `code: 'E_HUB_TIMEOUT'` and the same expectations.

`src/lib/result.ts`, in `invokeCmd`'s catch, before `return`:
```ts
    const error = toIpcError(raw);
    if (error.code === 'E_HUB_TIMEOUT' && typeof window !== 'undefined') {
      // The hub may have done it anyway: whoever owns the lists re-fetches.
      window.dispatchEvent(new CustomEvent('fleet:outcome-unknown', { detail: { cmd } }));
    }
    return { ok: false, error };
```
`src/App.svelte`: next to the focus handler, add a listener registered in `onMount` (and removed in its cleanup) for `fleet:outcome-unknown` that calls `void loadProjects(); void loadSessions();` WITHOUT the 30 s throttle. Follow the file's existing pattern for `window.addEventListener('focus', onFocus)`.

- [ ] **Step 6: Run everything**

```bash
cargo test -p claude-fleet --lib backend:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core 2>&1 | grep -E "^test result|FAILED|panicked" | head
npx vitest run 2>&1 | tail -6 && npx svelte-check 2>&1 | tail -3
grep -rn "30 s\|30s\|within 30" docs/hub.md | grep -i "answer\|timeout" 
```
Expected: all green. If the last grep shows a sentence about the 30 s call bound in `docs/hub.md`, reword it to "the hub's own deadline for the tool plus 10 s (at least 15 min for `move_session`)".

- [ ] **Step 7: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
git add crates/fleet-core/src/mcp crates/fleet-core/src/ipc_error.rs src-tauri/src/backend src/lib/moves.ts src/lib/moves.test.ts src/lib/result.ts src/App.svelte docs/hub.md
git commit -m "fix(hub-client): client timeouts follow the hub's deadlines; connect timeout, offline breaker, E_HUB_TIMEOUT"
```

---

### Task 7: Docs and the full CI mirror

**Files:**
- Modify: `docs/control-api.md:413-420` (`send_prompt` contract)
- Modify: `docs/specs/2026-09-21-device-communication-analysis.md` (mark Phase 1 items landed)

- [ ] **Step 1: Update `docs/control-api.md`**

Replace the sentence starting `` `send_prompt` returns `{ delivered, session_id, turn_seq_before }`; `` with:

```
`send_prompt` returns `{ delivered, session_id, turn_seq_before, queued, acked }`.
It refuses a `blocked` or stuck session with `E_INVALID_STATE` (Enter would
answer its dialog) unless `force: true`; to a `working` session the prompt is
queued behind the running turn (`queued: true`, and `turn_seq_before` already
points past that turn). `acked` is `true` once the session's `UserPromptSubmit`
hook confirmed the prompt, `false` when it did not within 1.5 s (after one
Enter retry), `null` when it cannot be known (nothing submitted, or no hook has
ever reached the row). Pass a `client_msg_id` to make a retry return the first
result instead of delivering twice (10-minute memory). Bodies are limited to
64 KiB; `\r\n` is folded to `\n` and any other control character is refused
(`E_VALIDATE`). The text lands in the pane reconcile last saw Claude in, or
the session's active pane when that is unknown.
```

- [ ] **Step 2: Mark the analysis**

In `docs/specs/2026-09-21-device-communication-analysis.md`, under "### Phase 1", add one line below the heading: `Landed 2026-09-21 on branch feature/device-communication-fa2aec (plan: docs/superpowers/plans/2026-09-21-device-communication-phase-1.md). Deferred from item 4: idempotency keys on every hub-routed mutation — needs a wire change per mutating tool; send_prompt has client_msg_id.`

- [ ] **Step 3: The full CI mirror, unpiped**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo build -p fleet-hub --locked
pnpm install --frozen-lockfile
npx svelte-check
npx vitest run
pnpm run build
cargo test -p fleet-core reference_is_current
cargo test -p claude-fleet --lib verdict_gen
cargo test -p claude-fleet --lib backend::tests_contract
```
Expected: every command exits 0. Read the full output of `cargo test --workspace` and `npx vitest run`, never through `| tail`; a red test elsewhere in the tree is this branch's to explain.

- [ ] **Step 4: Commit**

```bash
git add docs/control-api.md docs/specs/2026-09-21-device-communication-analysis.md docs/superpowers/plans/2026-09-21-device-communication-phase-1.md
git commit -m "docs: send_prompt contract for gated, acked, deduped delivery; phase 1 landed"
```

---

## Self-review

**Spec coverage.** Phase 1 item 1 (send primitive) → Task 2. Item 2 (gate, ack, `client_msg_id`, `select_targets`, `send_message{deliver}`) → Task 3 with Task 1's counter. Item 3 (`row_version` + trigger, `isStale`, re-read row, subscribe-before-list, `sessions.set` → merge) → Tasks 1, 4, 5. Item 4 (timeouts from `TOOL_POLICIES`, connect timeout, `E_HUB_TIMEOUT`, breaker, outcome-unknown refresh) → Task 6; the per-mutation idempotency key is explicitly deferred and recorded in Task 7. The `hosts` table gets no `row_version`: its store has no `isStale` guard today, so there is nothing for it to feed.

**Type consistency.** `PromptAckState { prompt_submit_seq: i64, hooks_seen: bool }` is defined in Task 1 and read in Task 3's `deliver_prompt` and `await_prompt_ack`. `build_send_script(tmux_name, pane_id, body, buffer, submit)` is defined and used only in Task 2. `deliver_prompt(row, prompt, submit, force)` is the four-argument form throughout Task 3. `finalize_new_session(s, row_id, host_alias, name, friendly_name, claude_id, is_shell)` is used with the same order in Task 4's tests and body. `call_timeout(tool)` keeps its name in Task 6 so the routing tests that reference it still compile.

**Placeholders.** None: every step has its code or its exact command; the two "grep to find the spot" instructions name the symbol to grep for and what to do at it.
