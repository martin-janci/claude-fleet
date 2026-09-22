# Phone as a pager — hub contract (`send_prompt { keys }`, `pending_input`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two additive hub changes the phone's "Waiting for you" card needs: a way to press Enter / Esc / C-c in a session without typing text, and the numbered options of a permission or question dialog on the session row.

**Architecture:** `keys` is a new optional argument on `send_prompt` that maps to one `tmux send-keys` of a named key, skips the untrusted marker (a key is not text) and is audited but never recorded as a prompt. `pending_input` is a new nullable JSON column on `sessions`, filled by the reconcile pass from the `Dialog` that `pane_intel` already parses, carried on the row and on `session:updated`, with `#[serde(default)]` everywhere so an older desktop or phone never fails a read.

**Tech Stack:** Rust workspace (`crates/fleet-core`, `src-tauri`), rusqlite migrations, serde, rmcp `#[tool]` router. Verify with `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`.

**Spec:** `fleet-mobile/docs/superpowers/specs/2026-09-21-phone-as-pager-design.md`, section 1.6 (checked out beside this repo at `../fleet-mobile`, or on GitHub `martin-janci/fleet-mobile`).

## Global Constraints

- Every value interpolated into an SSH/bash command string is quoted with `crate::shell::quote` (`shq`).
- Never hold the `Store` mutex guard across an `.await`.
- A new wire field on a hub-read row MUST carry `#[serde(default)]` (an absent field must not fail the whole `list_sessions` reply on an older hub — see `docs/hub.md` → *Version skew* and memory `hub-contract-golden-regen`).
- A new tool argument needs `Serialize` on the args struct and a non-default row in `src-tauri/src/backend/tests_routing.rs` (the hub client serialises the whole struct).
- After editing any `#[tool(...)]` description: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`. After any wire change: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (its regen run reports FAILED — re-run without the flag).
- The served tool-definition budget test (`the_served_definition_budget_stays_bounded`, currently 57,700 bytes) will trip on a longer `send_prompt` description; add one short clause and raise the constant only with the measured numbers in its comment.
- Migrations: a new `NNN_<topic>.sql` in `crates/fleet-core/migrations/` plus an entry in `MIGRATIONS` in `crates/fleet-core/src/store/schema.rs` with an `already_applied` guard for `ADD COLUMN`; bump the schema-version assertion in `crates/fleet-core/src/service/health.rs` (`health_from_store_reports_version_db_ready_and_schema`).
- `CARGO_TARGET_DIR` must be exported explicitly in this worktree (the `cargo` shell function otherwise shares a target dir with other worktrees).
- Branch: `feat/pager-hub-contract` off `origin/main`. One PR, merge commit style. Do not push or open the PR from a task.

## File map

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/mcp/tools/params.rs` | `SendPromptParams.keys` |
| `crates/fleet-core/src/mcp/tools/messaging.rs` | `send_prompt` branches on `keys` |
| `crates/fleet-core/src/service/sessions/prompt.rs` | `send_keys(args, store, ssh)` |
| `crates/fleet-core/src/tmux.rs` | `send_named_key(session, key)` command builder |
| `crates/fleet-core/src/service/pane_intel.rs` | `Dialog.options`, `PendingInput` type |
| `crates/fleet-core/migrations/040_pending_input.sql` | `sessions.pending_input TEXT` |
| `crates/fleet-core/src/store/schema.rs`, `store/rows.rs`, `store/sessions.rs` (or wherever `SessionRow` is mapped) | column + row field |
| `crates/fleet-core/src/service/reconcile*.rs` (where `current_activity` is written from pane intel) | write `pending_input` |
| `src/lib/sessions.ts` (TS `SessionRow`) | `pending_input: PendingInput | null` |
| `src-tauri/src/backend/tests_routing.rs`, `hub_contract.golden.json` | routing row, golden |
| `docs/control-api-reference.md`, `docs/control-api.md`, `docs/hub.md` | regenerated / one paragraph each |

---

### Task 1: `send_prompt { keys }`

**Files:**
- Modify: `crates/fleet-core/src/tmux.rs`, `crates/fleet-core/src/service/sessions/prompt.rs`, `crates/fleet-core/src/mcp/tools/params.rs:236-259`, `crates/fleet-core/src/mcp/tools/messaging.rs:8-41`, `src-tauri/src/backend/tests_routing.rs` (send_prompt row), `docs/control-api.md` (blast-radius bullet)
- Test: `crates/fleet-core/src/tmux.rs` (unit), `crates/fleet-core/src/service/sessions/prompt.rs` (unit), `crates/fleet-core/src/mcp/tools/tests.rs` (tool)

**Interfaces:**
- Produces: `pub enum NamedKey { Enter, Escape, CtrlC }` with `NamedKey::parse(&str) -> Option<NamedKey>` accepting exactly `"Enter" | "Escape" | "C-c"` and `fn tmux_name(self) -> &'static str` (`"Enter" | "Escape" | "C-c"`); `pub fn send_named_key(tmux_name: &str, key: NamedKey) -> String` (the tmux command string).
- Produces: `pub async fn send_keys(host_alias: &str, tmux_name: &str, key: NamedKey, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<(), IpcError>` — records a `keys_sent` session event with the key name, never a `prompt_sent`.
- `SendPromptParams.keys: Option<String>` (serde default, schemars-documented: "Press a key instead of typing text: Enter, Escape or C-c. Never marked, never recorded as a prompt. `prompt` must be empty with it.").

- [ ] **Step 1: Write the failing tests**

In `tmux.rs` tests:
```rust
#[test]
fn named_keys_parse_exactly_and_build_a_send_keys_command() {
    assert_eq!(NamedKey::parse("Enter"), Some(NamedKey::Enter));
    assert_eq!(NamedKey::parse("Escape"), Some(NamedKey::Escape));
    assert_eq!(NamedKey::parse("C-c"), Some(NamedKey::CtrlC));
    assert_eq!(NamedKey::parse("enter"), None, "case matters: the value is a tmux key name");
    assert_eq!(NamedKey::parse("rm -rf"), None);
    assert_eq!(
        send_named_key("my session", NamedKey::Escape),
        format!("tmux send-keys -t {} Escape", crate::shell::quote("my session"))
    );
}
```
In `mcp/tools/tests.rs` (use the existing `send_prompt` test fixtures — a store with a session row and the fake SSH that records commands; see `marker_is_applied_unless_master_asks_for_raw` for the shape):
```rust
#[tokio::test]
async fn keys_press_a_key_without_a_marker_and_without_recording_a_prompt() {
    // arrange: session id 1 on host "local" as the neighbouring send_prompt tests do
    let r = tools.send_prompt(Extension(client_caller("phone", TokenMode::Full)), Parameters(SendPromptParams {
        session_id: Some(1), host_alias: None, tmux_name: None, prompt: String::new(), submit: true, raw: false, keys: Some("Escape".into()),
    })).await.expect("keys");
    assert_eq!(result_json(&r)["delivered"], true);
    // the recorded command is a bare send-keys Escape, no marker text anywhere
    let cmds = ssh.commands();
    assert!(cmds.iter().any(|c| c.contains("send-keys") && c.contains(" Escape")), "{cmds:?}");
    assert!(!cmds.iter().any(|c| c.contains("treat as untrusted")), "{cmds:?}");
    // timeline: keys_sent, not prompt_sent, and last_prompt untouched
    let s = store.lock().unwrap();
    let hist = s.session_events(1, 10).unwrap();
    assert!(hist.iter().any(|e| e.kind == "keys_sent" && e.detail.as_deref() == Some("Escape")));
    assert!(!hist.iter().any(|e| e.kind == "prompt_sent"));
}

#[tokio::test]
async fn keys_refuse_an_unknown_key_and_text_alongside_it() {
    let bad = tools.send_prompt(Extension(Caller::master()), Parameters(SendPromptParams { session_id: Some(1), host_alias: None, tmux_name: None, prompt: String::new(), submit: true, raw: false, keys: Some("Delete".into()) })).await.expect_err("unknown key");
    assert!(bad.message.starts_with("E_VALIDATE"), "{}", bad.message);
    let both = tools.send_prompt(Extension(Caller::master()), Parameters(SendPromptParams { session_id: Some(1), host_alias: None, tmux_name: None, prompt: "hi".into(), submit: true, raw: false, keys: Some("Enter".into()) })).await.expect_err("text and keys");
    assert!(both.message.starts_with("E_VALIDATE"), "{}", both.message);
}
```
(The fake SSH's recorded-commands accessor and the events reader are whatever the neighbouring tests use — read `mcp/tools/tests.rs` around the existing `send_prompt` tests and copy their helpers.)

- [ ] **Step 2: Run to verify they fail**

Run: `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/pager-hub && cargo test -p fleet-core --lib -- named_keys keys_press keys_refuse`
Expected: compile error — `NamedKey`, `keys` field missing.

- [ ] **Step 3: Implement**

`tmux.rs`:
```rust
/// A key `send_prompt { keys }` may press. Closed on purpose: tmux's key
/// names are a small language of their own and "press whatever you like"
/// would be a second way to type into a pane, unmarked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey { Enter, Escape, CtrlC }

impl NamedKey {
    pub fn parse(s: &str) -> Option<Self> {
        match s { "Enter" => Some(Self::Enter), "Escape" => Some(Self::Escape), "C-c" => Some(Self::CtrlC), _ => None }
    }
    pub fn tmux_name(self) -> &'static str {
        match self { Self::Enter => "Enter", Self::Escape => "Escape", Self::CtrlC => "C-c" }
    }
}

/// `tmux send-keys -t <session> <Key>` — one named key, no literal text.
pub fn send_named_key(tmux_name: &str, key: NamedKey) -> String {
    format!("tmux send-keys -t {} {}", crate::shell::quote(tmux_name), key.tmux_name())
}
```
`prompt.rs`: `pub async fn send_keys(...)` — resolve the host the way `send_prompt` does (same SSH path, `E_TMUX` on a non-zero exit), run `send_named_key`, then `record_session_event(store, host_alias, tmux_name, "keys_sent", Some(key.tmux_name().to_string()))`. Do not touch `last_prompt`.

`params.rs`: add to `SendPromptParams`:
```rust
    /// Press a key instead of typing text: `Enter`, `Escape` or `C-c`. Never
    /// marked (a key is not text) and never recorded as a prompt. `prompt`
    /// must be empty with it.
    #[serde(default)]
    pub keys: Option<String>,
```
`messaging.rs` `send_prompt`: before `apply_marker`:
```rust
if let Some(k) = p.keys.as_deref() {
    let key = crate::tmux::NamedKey::parse(k).ok_or_else(|| mcp_err(codes::E_VALIDATE, format!("keys must be Enter, Escape or C-c, not {k:?}"), None))?;
    if !p.prompt.is_empty() {
        return Err(mcp_err(codes::E_VALIDATE, "keys and a non-empty prompt cannot be sent together", None));
    }
    sessions::send_keys(&row.host_alias, &row.tmux_name, key, &self.store, &self.ssh).await.map_err(to_mcp_err)?;
    return ok_json(&serde_json::json!({ "delivered": true, "session_id": row.id, "turn_seq_before": row.turn_seq }));
}
```
Append to the tool description one clause: `keys=Enter|Escape|C-c presses a key instead (unmarked).` Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`; if the budget test trips, raise `BUDGET_BYTES` by the measured delta with a sentence in its comment.

`src-tauri/src/backend/tests_routing.rs`: the `send_prompt` args fixture gains `keys: Some("Enter".into())` so the routed call serialises the new field (see the `strict` example from `move_session`).

`docs/control-api.md` blast-radius bullet: add "`keys` presses Enter, Escape or C-c without text; it is never marked."

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: all green, including `reference_is_current` after the regen and the routing test.

- [ ] **Step 5: Commit**

```bash
git add crates src-tauri docs
git commit -m "feat(mcp): send_prompt can press Enter, Escape or C-c, so a phone can answer a dialog without typing"
```

---

### Task 2: `pending_input` on the session row

**Files:**
- Modify: `crates/fleet-core/src/service/pane_intel.rs:460-570` (`Dialog.options`; `PendingInput` DTO), the reconcile site that writes `current_activity` (grep `current_activity` in `crates/fleet-core/src/service/` — it is written where `PaneIntel` is applied to the row), `crates/fleet-core/src/store/rows.rs` (`SessionRow.pending_input`), the `sessions` SELECT/INSERT/UPDATE lists in `crates/fleet-core/src/store/` (grep `current_activity` there), `crates/fleet-core/src/store/schema.rs`, `crates/fleet-core/src/service/health.rs:476`, `src/lib/sessions.ts` (or wherever the TS `SessionRow` lives — grep `current_activity` in `src/lib/*.ts`), `docs/hub.md` (Events section: one sentence), `docs/control-api.md` (list_sessions row fields)
- Create: `crates/fleet-core/migrations/040_pending_input.sql`
- Test: `pane_intel.rs` unit tests, `store` round-trip test, `src-tauri/src/backend/tests_contract.rs` fixture, `src/lib/*.test.ts` for the TS type (a type-only change needs no runtime test; `pnpm check` covers it)

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingOption { pub n: u8, pub label: String, pub selected: bool }
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingInput { pub kind: String /* "permission" | "input" */, pub question: Option<String>, pub options: Vec<PendingOption> }
```
- `SessionRow.pending_input: Option<PendingInput>` with `#[serde(default)]`; stored as JSON text in `sessions.pending_input`; `None` whenever the pane shows no dialog.
- TS: `pending_input: { kind: "permission" | "input"; question: string | null; options: { n: number; label: string; selected: boolean }[] } | null`.

- [ ] **Step 1: Write the failing tests**

`pane_intel.rs`:
```rust
#[test]
fn a_permission_dialog_carries_its_numbered_options() {
    let pane = "\
Do you want to make this edit to src/main.rs?
❯ 1. Yes
  2. Yes, and don't ask again this session
  3. No, and tell Claude what to do differently
";
    let d = detect_dialog(pane).expect("dialog");
    let p = d.pending_input();
    assert_eq!(p.kind, "permission");
    assert_eq!(p.question.as_deref(), Some("Do you want to make this edit to src/main.rs?"));
    assert_eq!(p.options.len(), 3);
    assert_eq!(p.options[0], PendingOption { n: 1, label: "Yes".into(), selected: true });
    assert_eq!(p.options[2].n, 3);
    assert!(!p.options[2].selected);
}

#[test]
fn a_question_dialog_is_input_and_a_boxed_dialog_loses_its_borders() {
    let pane = "│ Keep ghosted sessions for how long before deleting them? │\n│ ❯ 1. 1 hour │\n│   2. 1 day │\nEnter to select\n";
    let p = detect_dialog(pane).expect("dialog").pending_input();
    assert_eq!(p.kind, "input");
    assert_eq!(p.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["1 hour", "1 day"]);
}
```
Store round-trip (next to the other `SessionRow` store tests):
```rust
#[test]
fn pending_input_round_trips_and_defaults_to_none() {
    let s = Store::open_in_memory().unwrap();
    let id = /* insert a session the way neighbouring tests do */;
    let pi = PendingInput { kind: "permission".into(), question: Some("Do it?".into()), options: vec![PendingOption { n: 1, label: "Yes".into(), selected: true }] };
    s.set_pending_input(id, Some(&pi)).unwrap();
    assert_eq!(s.get_session_by_id(id).unwrap().unwrap().pending_input, Some(pi));
    s.set_pending_input(id, None).unwrap();
    assert_eq!(s.get_session_by_id(id).unwrap().unwrap().pending_input, None);
    // An older hub's JSON (no key) still parses.
    let row: SessionRow = serde_json::from_str(r#"{"id":1,"tmux_name":"s","host_alias":"h", …minimal required fields… }"#).unwrap();
    assert!(row.pending_input.is_none());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core --lib -- pending_input numbered_options`
Expected: compile error — `pending_input`, `PendingInput` missing.

- [ ] **Step 3: Implement**

`040_pending_input.sql`:
```sql
-- The permission / question dialog a blocked session is showing, as JSON
-- ({kind, question, options[{n,label,selected}]}), or NULL when the pane shows
-- none. Derived by the reconcile pass from the same pane read that fills
-- current_activity; a phone turns the options into buttons.
ALTER TABLE sessions ADD COLUMN pending_input TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (40);
```
Register in `schema.rs` with `already_applied: Some(sessions_has_pending_input)` (copy the `client_tokens_has_trusted_at` helper shape). Bump `health.rs` to 40.

`pane_intel.rs`: extend `Dialog` with `options: Vec<PendingOption>` built in `detect_dialog` from `choices` (`n` = the parsed digits, `label` = the line after `N. ` / `N) `, `selected` = the `❯` flag) and add `fn pending_input(&self) -> PendingInput { PendingInput { kind: self.kind.as_str().into(), question: self.prompt.clone(), options: self.options.clone() } }`. Where the reconcile pass writes `current_activity` from the intel result, also write `pending_input` (`Some` when a dialog was detected, `None` otherwise) through a new `Store::set_pending_input(id, Option<&PendingInput>)` that stores `serde_json::to_string`. `rows.rs`: `#[serde(default)] pub pending_input: Option<PendingInput>`, mapped from the TEXT column with `serde_json::from_str(...).ok()`.

`src/lib/sessions.ts` (TS row): add the field as `value | null`. `pnpm check`.

Then: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (extend the fixture in `src-tauri/src/backend/tests_contract.rs` so `pending_input` carries a value, not a default), re-run without the flag; `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (the `list_sessions` description names the row fields — add `pending_input` to that list only if it already lists fields; otherwise leave it and document in `control-api.md`).

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check && npx vitest run && npx svelte-check --threshold error`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates src-tauri src docs
git commit -m "feat(intel): the row carries the dialog's numbered options as pending_input, so a client can answer with a tap"
```

---

## Self-review

- Spec 1.6 (1) → Task 2; (2) → Task 1. Both `serde(default)`; both regenerate the reference and the golden.
- `keys` is deliberately not marked: the marker exists for text an agent could be tricked by, and a key press carries no text. Said in the KDoc and the tool description.
- Types: `PendingOption`/`PendingInput` are defined once in `pane_intel.rs` and re-exported through `store::rows` for the row; the TS mirror uses snake_case as every other field does.
