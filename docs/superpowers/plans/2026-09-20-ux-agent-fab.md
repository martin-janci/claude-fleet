# UX Agent (FAB) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A floating button in every view of the desktop app opens an agent that drives the fleet through the app's own MCP control API.

**Architecture:** The agent is one long-lived tmux-backed fleet session per fleet — not one per device — running in `~/.claude-fleet/operator/` with its own revocable client token. The FAB panel is a compact `ConversationPanel` plus a composer over that session; destructive tool calls stop at the existing `McpConfirmDialog`. Two new router tools exist only so the desktop can reach the agent's lifecycle in hub mode.

**Tech Stack:** Rust (fleet-core, Tauri 2), SQLite via rusqlite, rmcp (MCP 2025-11-25), Svelte 5 runes, Vitest, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-20-ux-agent-fab-design.md`

## Global Constraints

- **No FAB action may go through a `LocalOnly` command.** Every command this plan adds gets `Verdict::Routed`. This is what keeps the phone slice free.
- **The agent never holds the master token.** Its identity is a `client_tokens` row named `ux-agent`, mode `full`.
- **Shell-quoting has one implementation:** `crate::shell::quote` (alias `shq`). Every value interpolated into an SSH/bash command string must use it.
- **Never hold the `Store` mutex across an `.await`.** Snapshot under the guard, drop it, then await.
- **Adding a Tauri command** means: a `backend/verdicts.rs` row, a `route` by command name, a non-default row in `backend/tests_routing.rs`, a `generate_handler!` entry, then `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` and `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- **Adding a router tool** means exactly one `TOOL_POLICIES` row; `every_router_tool_has_exactly_one_policy_row` fails with the row to add if you forget.
- **Run the suite whole:** `scripts/ci-local.sh`. Never filter per task, never pipe through `| tail` — a red test hides that way. Run `pnpm install --frozen-lockfile` once before any frontend test.
- **Keyboard chord is ⌘E / Ctrl+Shift+E.** ⌘J is the Session view.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/038_project_system.sql` | The `system` column on `projects`. |
| `crates/fleet-core/src/store/rows.rs` | `ProjectRow.system`, the column list, the row mapper. |
| `crates/fleet-core/src/store/projects.rs` | `upsert_system_project`; the joined-query column offsets. |
| `crates/fleet-core/src/store/schema.rs` | Migration 038 registration + its guard + its test. |
| `crates/fleet-core/src/service/projects.rs` | The stale-rows sweep honours `system`. |
| `crates/fleet-core/src/service/operator.rs` | **New.** The only module that knows the operator is special: identity, birth, status, self-guard. |
| `crates/fleet-core/src/service/sessions/prompt.rs` | `select_targets` excludes the operator. |
| `crates/fleet-core/src/mcp/tools/support.rs` | A confirm-gated tool's deadline absorbs the confirmation window. |
| `crates/fleet-core/src/mcp/guard.rs` | Two `TOOL_POLICIES` rows. |
| `crates/fleet-core/src/mcp/tools/session_ops.rs` | The two router tools. |
| `src-tauri/src/commands/operator.rs` | **New.** Two thin handlers + their `routed` module. |
| `src/lib/agent_context.ts` | **New.** Pure: app state → chip label + prompt prefix. |
| `src/lib/app_views.ts` | `appChord` gains `'agent'`. |
| `src/lib/operator.ts` | **New.** Store: agent row, panel state, readiness. |
| `src/lib/AgentFab.svelte` | **New.** The button. |
| `src/lib/AgentPanel.svelte` | **New.** The sheet. |
| `src/App.svelte` | Mounts both; routes the chord. |

---

### Task 1: A confirm-gated tool's deadline absorbs the confirmation window

Independent of everything else — a standing defect. Do it first so the rest of the plan builds on a sound confirmation path.

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/support.rs:943-958` (`tool_deadline`)
- Test: `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing new. `tool_deadline(&str) -> Duration` keeps its signature and stays a pure function of the tool name.

- [ ] **Step 1: Write the failing test**

Append to `crates/fleet-core/src/mcp/tools/tests.rs`:

```rust
/// A confirmation nonce must never outlive the call that is waiting on it.
/// When it does, you approve inside the TTL and the agent has already been
/// holding an `E_TIMEOUT` for minutes — the one failure mode a confirmation
/// dialog must not have. `CONFIRM_TTL` cannot simply be shortened instead:
/// `set_clipboard` and `cancel_task` are `Deadline::Quick` (60 s), so a TTL
/// under every cap would be under a minute, which is not a window a human
/// can answer in.
#[test]
fn every_confirmed_tool_outlives_its_confirmation_window() {
    let confirmed: Vec<&crate::mcp::guard::ToolPolicy> = crate::mcp::guard::TOOL_POLICIES
        .iter()
        .filter(|p| p.confirm)
        .collect();
    assert!(
        !confirmed.is_empty(),
        "the confirmation gate has no tools — this test would pass vacuously"
    );
    for p in confirmed {
        let deadline = super::support::tool_deadline(p.name);
        assert!(
            deadline > crate::mcp::guard::CONFIRM_TTL,
            "{} is confirm-gated but its deadline ({:?}) does not outlast CONFIRM_TTL ({:?})",
            p.name,
            deadline,
            crate::mcp::guard::CONFIRM_TTL,
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p fleet-core every_confirmed_tool_outlives_its_confirmation_window
```

Expected: FAIL — `kill_session is confirm-gated but its deadline (300s) does not outlast CONFIRM_TTL (600s)`.

- [ ] **Step 3: Make the deadline absorb the window**

Replace the body of `tool_deadline` in `crates/fleet-core/src/mcp/tools/support.rs`:

```rust
pub(super) fn tool_deadline(tool: &str) -> std::time::Duration {
    match guard::policy(tool) {
        Some(p) => {
            let work = match p.deadline {
                guard::Deadline::LongPoll => LONG_POLL_CAP,
                guard::Deadline::Lifecycle => LIFECYCLE_CAP,
                guard::Deadline::Quick => QUICK_CAP,
            };
            // A confirm-gated call may sit blocked on a human. The class cap
            // bounds the WORK; the confirmation window is time the call is
            // MEANT to spend waiting, so it is added rather than competed
            // with. Without this the nonce outlives the call waiting on it
            // and an approval arrives to an already-failed call.
            if p.confirm {
                work + guard::CONFIRM_TTL
            } else {
                work
            }
        }
        None => {
            // Reachable only for a name the router does not serve (rmcp then
            // answers "tool not found") — the exhaustiveness test keeps every
            // served tool with exactly one TOOL_POLICIES row.
            tracing::debug!(tool, "[mcp] unclassified tool name gets the quick cap");
            QUICK_CAP
        }
    }
}
```

- [ ] **Step 4: Run the test and the surrounding module**

```bash
cargo test -p fleet-core mcp::tools
```

Expected: PASS, including `every_router_tool_has_exactly_one_policy_row` and any existing deadline tests. If an existing test asserts an exact deadline for a confirm-gated tool, update it to the new value and say so in the commit — it is asserting the old behaviour.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/mcp/tools/support.rs crates/fleet-core/src/mcp/tools/tests.rs
git commit -m "fix(mcp): a confirmed tool's deadline outlasts its confirmation window"
```

---

### Task 2: The `system` project flag

**Files:**
- Create: `crates/fleet-core/migrations/038_project_system.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (guard fn near `projects_has_adopted`; `MIGRATIONS` tail; tests module)
- Modify: `crates/fleet-core/src/store/rows.rs:7-52` (`ProjectRow`, `PROJECT_COLUMNS`, `map_project_row`)
- Modify: `crates/fleet-core/src/store/projects.rs` (`upsert_project_impl`, new `upsert_system_project`, `list_projects_joined` offsets)
- Modify: `crates/fleet-core/src/service/projects.rs:328` (the sweep)
- Test: `crates/fleet-core/src/store/schema.rs` tests, `crates/fleet-core/src/service/projects.rs` tests

**Interfaces:**
- Consumes: nothing.
- Produces: `ProjectRow.system: bool`; `Store::upsert_system_project(owner: &str, repo: &str, base_path: &str) -> Result<i64, rusqlite::Error>`.

> **Gotcha that will bite you:** `list_projects_joined` selects `PROJECT_COLUMNS` first and reads the LEFT JOINed worktree columns at offsets **6, 7, 8, 9**. Adding a seventh project column shifts those to **7, 8, 9, 10**. Miss this and projects silently grow a worktree named after their `system` flag.

- [ ] **Step 1: Write the failing migration test**

In the `tests` module of `crates/fleet-core/src/store/schema.rs`, after `migration_027_adds_adopted_column_defaulting_to_unset_and_reruns_safely`:

```rust
/// 038 on a database with project rows (stopped at 037): the column is
/// added, defaults to 0 (not a system row) for existing rows, and a re-run
/// (tests roll the recorded version back and migrate again) is a no-op that
/// keeps a row's `system` flag.
#[test]
fn migration_038_adds_system_column_defaulting_to_unset_and_reruns_safely() {
    let old = store_at_version(37);
    old.conn
        .execute_batch(
            "INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');",
        )
        .unwrap();
    old.migrate().expect("038 on an existing DB");
    assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert!(
        !old.list_projects().unwrap()[0].system,
        "a pre-038 row is not a system row"
    );
    let pid = old
        .upsert_system_project("fleet", "operator", "~/.claude-fleet/operator")
        .unwrap();
    old.conn
        .execute_batch("DELETE FROM schema_version WHERE version >= 38;")
        .unwrap();
    old.migrate().expect("re-running 038 is safe");
    assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert!(
        old.list_projects()
            .unwrap()
            .into_iter()
            .find(|p| p.id == pid)
            .unwrap()
            .system,
        "the system flag survives a re-run"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p fleet-core migration_038_adds_system_column
```

Expected: FAIL to compile — `no field 'system' on ProjectRow`, `no method 'upsert_system_project'`.

- [ ] **Step 3: Write the migration**

Create `crates/fleet-core/migrations/038_project_system.sql`:

```sql
-- Marks the project row `service::operator` creates for the UX agent's own
-- working directory (`~/.claude-fleet/operator`). It is not one of the
-- user's repositories: the project picker hides it, and `refresh_projects`'s
-- stale-rows sweep must never delete it for living outside the projects root
-- and not being rediscovered by a scan — that is its normal shape, not
-- evidence of staleness, exactly as with `adopted` (027). 0/1 as INTEGER
-- (SQLite has no native boolean); every other row defaults to 0.
ALTER TABLE projects ADD COLUMN system INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (38);
```

- [ ] **Step 4: Register it**

In `crates/fleet-core/src/store/schema.rs`, beside `projects_has_adopted`:

```rust
/// `already_applied` guard of migration 038: `projects` already has its
/// `system` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn projects_has_system(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'system'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

and at the end of `MIGRATIONS`:

```rust
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 38,
        sql: include_str!("../../migrations/038_project_system.sql"),
        already_applied: Some(projects_has_system),
    },
```

- [ ] **Step 5: Carry the column into the row type**

In `crates/fleet-core/src/store/rows.rs`, add to `ProjectRow` after `adopted`:

```rust
    /// Set by `service::operator` (migration 038): this row is the UX agent's
    /// own working directory, not one of the user's repositories. The project
    /// picker hides it and `refresh_projects`'s stale-rows sweep leaves it
    /// alone — the same bargain `adopted` makes, for a different reason.
    pub system: bool,
```

then:

```rust
pub(super) const PROJECT_COLUMNS: &str =
    "id, owner, repo, base_path, last_session_at, adopted, system";
```

and in `map_project_row`, after the `adopted` line:

```rust
        system: row.get::<_, i64>(6)? != 0,
```

- [ ] **Step 6: Thread it through the store, and fix the joined offsets**

In `crates/fleet-core/src/store/projects.rs`, give `upsert_project_impl` a `system` parameter and add the public entry point:

```rust
    /// Upsert the project row that backs a fleet-internal working directory —
    /// today only the UX agent's (`service::operator`). `adopted` is set too:
    /// the directory is outside the projects root by construction, and the
    /// sweep's outside-root rule must not reach it either.
    pub fn upsert_system_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.upsert_project_impl(owner, repo, base_path, true, true)
    }
```

with `upsert_project` and `upsert_adopted_project` passing `false` / `false` and `true` / `false` respectively, and the impl:

```rust
    fn upsert_project_impl(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
        adopted: bool,
        system: bool,
    ) -> Result<i64, rusqlite::Error> {
        let id: i64 = self.conn.query_row(
            "INSERT INTO projects (owner, repo, base_path, adopted, system) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(owner, repo) DO UPDATE SET base_path=excluded.base_path, adopted=excluded.adopted, system=excluded.system
             RETURNING id",
            rusqlite::params![owner, repo, base_path, adopted as i64, system as i64],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_project(id)? {
            self.bus.project_updated(&row);
        }
        Ok(id)
    }
```

Then in `list_projects_joined`, shift every worktree offset by one:

```rust
        let rows = stmt.query_map([], |row| {
            Ok((
                map_project_row(row)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })?;
```

- [ ] **Step 7: Run the migration test**

```bash
cargo test -p fleet-core store::
```

Expected: PASS. Any other construction site of `ProjectRow` that the compiler names needs `system: false`.

- [ ] **Step 8: Write the failing sweep test**

In the `tests` module of `crates/fleet-core/src/service/projects.rs`, beside `refresh_projects_keeps_an_adopted_row_outside_the_root_but_drops_a_plain_one`:

```rust
/// The operator's project lives outside the projects root and no scan will
/// ever rediscover it, which is exactly the shape the sweep deletes. It must
/// survive, or the agent loses its home on the next Settings save.
#[tokio::test]
async fn refresh_projects_keeps_a_system_row_outside_the_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let sysid = {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        let map = serde_json::json!({ "local": tmp.path().to_string_lossy() }).to_string();
        settings::set(&s, settings::PROJECTS_BASE_PATH, &map).unwrap();
        s.upsert_system_project("fleet", "operator", "/elsewhere/operator")
            .unwrap()
    };
    let rows = refresh_projects(&store).await.unwrap();
    assert!(
        rows.iter().any(|r| r.project.id == sysid),
        "a system row outside the root survives a refresh"
    );
}
```

- [ ] **Step 9: Run it to verify it fails**

```bash
cargo test -p fleet-core refresh_projects_keeps_a_system_row
```

Expected: PASS already — `upsert_system_project` sets `adopted` too. **If it passes, that is correct, not a mistake:** the test pins the behaviour against someone later splitting `system` from `adopted`. Add the explicit guard anyway in the next step so the sweep states its own rule.

- [ ] **Step 10: Make the sweep say it in its own words**

In `crates/fleet-core/src/service/projects.rs`, extend the skip condition:

```rust
            if fresh_ids.contains(&row.id)
                || removed.contains(&row.id)
                || row.adopted
                || row.system
            {
                continue;
            }
```

- [ ] **Step 11: Run the service tests**

```bash
cargo test -p fleet-core service::projects
```

Expected: PASS.

- [ ] **Step 12: Commit**

```bash
git add crates/fleet-core/migrations/038_project_system.sql crates/fleet-core/src/store crates/fleet-core/src/service/projects.rs
git commit -m "feat(store): a system project flag for fleet-internal working directories"
```

---

### Task 3: Operator identity and the self-guard

**Files:**
- Create: `crates/fleet-core/src/service/operator.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (add `pub mod operator;` in alphabetical position, between `onboarding` and `outcome`)
- Test: inline `#[cfg(test)] mod tests` in `operator.rs`

**Interfaces:**
- Consumes: `Store::get_setting` / `Store::set_setting` (`crates/fleet-core/src/store/mod.rs:177,188`).
- Produces:
  - `pub const SETTING_OPERATOR_SESSION: &str = "operator.session";`
  - `pub struct OperatorRef { pub host_alias: String, pub tmux_name: String }`
  - `pub fn parse_ref(raw: &str) -> Option<OperatorRef>` (PURE)
  - `pub fn format_ref(r: &OperatorRef) -> String` (PURE)
  - `pub fn operator_ref(store: &Store) -> Option<OperatorRef>`
  - `pub fn set_operator_ref(store: &Store, r: &OperatorRef) -> Result<(), IpcError>`
  - `pub fn refuse_if_operator(store: &Store, host_alias: &str, tmux_name: &str, what: &str) -> Result<(), IpcError>`

- [ ] **Step 1: Write the failing tests**

Create `crates/fleet-core/src/service/operator.rs` with only the test module and the stubs' signatures absent — write the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn a_reference_round_trips_and_a_malformed_one_is_no_reference() {
        let r = OperatorRef {
            host_alias: "mefistos".into(),
            tmux_name: "fleet-operator".into(),
        };
        assert_eq!(format_ref(&r), "mefistos/fleet-operator");
        assert_eq!(parse_ref("mefistos/fleet-operator"), Some(r));
        // A tmux name may not contain '/', so the first separator is the only
        // one — but a value with no separator, or an empty half, is not a
        // reference and must not resolve to one.
        assert_eq!(parse_ref("mefistos"), None);
        assert_eq!(parse_ref("/fleet-operator"), None);
        assert_eq!(parse_ref("mefistos/"), None);
        assert_eq!(parse_ref(""), None);
    }

    #[test]
    fn an_unrecorded_operator_guards_nothing() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(operator_ref(&s), None);
        refuse_if_operator(&s, "local", "anything", "kill_session")
            .expect("with no operator recorded, nothing is refused");
    }

    #[test]
    fn the_operator_refuses_to_be_acted_on_and_its_neighbours_do_not() {
        let s = Store::open_in_memory().unwrap();
        let r = OperatorRef {
            host_alias: "local".into(),
            tmux_name: "fleet-operator".into(),
        };
        set_operator_ref(&s, &r).unwrap();
        assert_eq!(operator_ref(&s), Some(r));

        let err = refuse_if_operator(&s, "local", "fleet-operator", "kill_session")
            .expect_err("the operator must refuse to be killed");
        assert_eq!(err.code, crate::ipc_error::codes::E_FORBIDDEN);
        assert!(
            err.message.contains("kill_session"),
            "the refusal names what was attempted: {}",
            err.message
        );

        // Same name on another host, and another name on the same host, are
        // ordinary sessions.
        refuse_if_operator(&s, "mefistos", "fleet-operator", "kill_session").unwrap();
        refuse_if_operator(&s, "local", "blue-sirius", "kill_session").unwrap();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p fleet-core service::operator
```

Expected: FAIL to compile — `operator` is not a module yet / the items do not exist.

- [ ] **Step 3: Write the module**

Put this above the test module in `crates/fleet-core/src/service/operator.rs`:

```rust
//! The UX agent's operator session: who it is, and the one rule that keeps it
//! from acting on itself.
//!
//! This is the ONLY module that knows the operator is special. Everything
//! else treats it as an ordinary session — which is the point: it gets the
//! transcript, the conversation view, restart, reboot survival and the
//! sidebar for free, and removing the feature means deleting this file.

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;

/// `settings` key holding `"<host_alias>/<tmux_name>"` for the live operator
/// session. Absent until `ensure_operator` has run once.
pub const SETTING_OPERATOR_SESSION: &str = "operator.session";

/// Where the operator session lives. Identity is `(host, tmux name)` rather
/// than a row id because ids churn on re-discovery, exactly as the quick
/// switcher's MRU key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRef {
    pub host_alias: String,
    pub tmux_name: String,
}

/// PURE: render a reference for storage.
pub fn format_ref(r: &OperatorRef) -> String {
    format!("{}/{}", r.host_alias, r.tmux_name)
}

/// PURE: read a stored reference. Both halves must be non-empty — a
/// half-written value must resolve to "no operator", never to a reference
/// that guards the wrong session (or every session on a host).
pub fn parse_ref(raw: &str) -> Option<OperatorRef> {
    let (host, tmux) = raw.split_once('/')?;
    if host.is_empty() || tmux.is_empty() {
        return None;
    }
    Some(OperatorRef {
        host_alias: host.to_string(),
        tmux_name: tmux.to_string(),
    })
}

/// The recorded operator, if one has ever been created. A store error reads
/// as "none": the guard's job is to refuse acting on a KNOWN operator, and a
/// database that cannot answer has not named one.
pub fn operator_ref(store: &Store) -> Option<OperatorRef> {
    store
        .get_setting(SETTING_OPERATOR_SESSION)
        .ok()
        .flatten()
        .as_deref()
        .and_then(parse_ref)
}

/// Record the operator's whereabouts. Called once by `ensure_operator`.
pub fn set_operator_ref(store: &Store, r: &OperatorRef) -> Result<(), IpcError> {
    store
        .set_setting(SETTING_OPERATOR_SESSION, &format_ref(r))
        .map_err(|e| {
            IpcError::new(
                codes::E_SQLITE,
                format!("record the operator session: {e}"),
            )
        })
}

/// Refuse a session-addressed operation aimed at the operator itself.
///
/// Without this, "tidy up the zombie sessions" ends the conversation that
/// asked for it — mid-sentence, with no one left to say what happened.
pub fn refuse_if_operator(
    store: &Store,
    host_alias: &str,
    tmux_name: &str,
    what: &str,
) -> Result<(), IpcError> {
    match operator_ref(store) {
        Some(r) if r.host_alias == host_alias && r.tmux_name == tmux_name => Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{what} refused: {tmux_name} on {host_alias} is the UX agent's own session. \
                 Close the agent panel and act on it from the sidebar if you mean it."
            ),
        )),
        _ => Ok(()),
    }
}
```

Add to `crates/fleet-core/src/service/mod.rs`, in alphabetical position:

```rust
pub mod operator;
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test -p fleet-core service::operator
```

Expected: PASS, three tests.

- [ ] **Step 5: Wire the guard into the session-addressed operations**

In each of `kill_session`, `safe_kill_session`, `restart_session` and `move_session`, immediately after the existing host/name validation and before any work, take the store guard and call:

```rust
    {
        let s = lock(store)?;
        crate::service::operator::refuse_if_operator(&s, host_alias, tmux_name, "kill_session")?;
    }
```

with `"kill_session"` replaced by that operation's own name. Note the scope braces: the guard is dropped before any `.await`.

- [ ] **Step 6: Run the session tests**

```bash
cargo test -p fleet-core service::sessions
```

Expected: PASS — no existing test records an operator, so `refuse_if_operator` is a no-op for all of them.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/operator.rs crates/fleet-core/src/service/mod.rs crates/fleet-core/src/service/sessions crates/fleet-core/src/service/safe_kill.rs crates/fleet-core/src/service/move_session
git commit -m "feat(operator): the UX agent's identity, and the rule that it may not act on itself"
```

---

### Task 4: A broadcast never fans into the operator

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/prompt.rs:280-306` (`select_targets`), `:335-395` (`broadcast_prompt`)
- Test: the `tests` module of `crates/fleet-core/src/service/sessions/prompt.rs`

**Interfaces:**
- Consumes: `service::operator::{operator_ref, OperatorRef}` from Task 3.
- Produces: `select_targets(sessions, filter, controller, operator)` — one new trailing parameter, `operator: Option<&(String, String)>`.

- [ ] **Step 1: Write the failing test**

In the `tests` module of `prompt.rs`:

```rust
/// A broadcast that reaches the UX agent makes it prompt itself: it answers,
/// which is activity, which is another broadcast candidate. The existing rate
/// limiter makes that loop slow rather than absent — slow enough to look like
/// a mystery and fast enough to eat the agent's context.
#[test]
fn select_targets_excludes_the_operator_as_well_as_the_controller() {
    let sessions = vec![
        work_session(1, "local", "blue-sirius"),
        work_session(2, "local", "fleet-operator"),
        work_session(3, "mefistos", "controller"),
    ];
    let controller = ("mefistos".to_string(), "controller".to_string());
    let operator = ("local".to_string(), "fleet-operator".to_string());
    let ids = select_targets(
        &sessions,
        &BroadcastFilter::default(),
        Some(&controller),
        Some(&operator),
    );
    assert_eq!(ids, vec![1], "only the ordinary work session is a target");

    // Same name on a different host is an ordinary session.
    let elsewhere = ("hetzner".to_string(), "fleet-operator".to_string());
    let ids = select_targets(
        &sessions,
        &BroadcastFilter::default(),
        None,
        Some(&elsewhere),
    );
    assert_eq!(ids, vec![1, 2, 3]);
}
```

If the module has no `work_session` helper, add one beside the test:

```rust
fn work_session(id: i64, host: &str, tmux: &str) -> SessionRow {
    let mut s = SessionRow::default();
    s.id = id;
    s.host_alias = host.to_string();
    s.tmux_name = tmux.to_string();
    s.kind = "work".to_string();
    s
}
```

If `SessionRow` has no `Default`, build it from whatever constructor the neighbouring tests in this module already use — copy their shape rather than deriving `Default` on the row type.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p fleet-core select_targets_excludes_the_operator
```

Expected: FAIL to compile — `select_targets` takes 3 arguments, not 4.

- [ ] **Step 3: Add the parameter**

In `select_targets`, extend the doc comment's rules list with:

```rust
///   - the operator session `(host_alias, tmux_name)`, when recorded, is
///     excluded so a fan-out never prompts the UX agent that may have sent it.
```

extend the signature:

```rust
pub fn select_targets(
    sessions: &[SessionRow],
    f: &BroadcastFilter,
    controller: Option<&(String, String)>,
    operator: Option<&(String, String)>,
) -> Vec<i64> {
```

and add a filter beside the controller's:

```rust
        .filter(|s| match operator {
            Some((host, tmux)) => !(&s.host_alias == host && &s.tmux_name == tmux),
            None => true,
        })
```

- [ ] **Step 4: Pass it from `broadcast_prompt`**

In the snapshot block, alongside `resolve_controller`:

```rust
    let (sessions, controller, operator) = {
        let s = lock(store)?;
        let sessions = s.list_all_sessions().map_err(|e| {
            IpcError::new(codes::E_SQLITE, format!("list sessions for broadcast: {e}"))
        })?;
        let controller = resolve_controller(&s);
        let operator = crate::service::operator::operator_ref(&s)
            .map(|r| (r.host_alias, r.tmux_name));
        (sessions, controller, operator)
    };

    let targets = select_targets(&sessions, &filter, controller.as_ref(), operator.as_ref());
```

Update every other `select_targets` call site the compiler names by passing `None` for the new parameter.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p fleet-core service::sessions::prompt
```

Expected: PASS, including the existing controller-exclusion tests.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/sessions/prompt.rs
git commit -m "fix(broadcast): never fan a prompt into the UX agent's own session"
```

---
### Task 5: `ensure_operator` — the agent is born

**Files:**
- Modify: `crates/fleet-core/src/service/operator.rs` (append above the test module)
- Test: the test module of `crates/fleet-core/src/service/operator.rs`

**Interfaces:**
- Consumes: `Store::upsert_system_project` (Task 2); `set_operator_ref` / `operator_ref` (Task 3); `crate::service::provision::{write_host_file, write_host_file_secret}`; `crate::service::sessions::lifecycle::{new_session, NewSessionArgs}`; `crate::mcp::generate_token`; `crate::mcp::auth::sha256_hex`; `crate::mcp::settings::configured_port`; `Store::insert_client_token`.
- Produces:
  - `pub const OPERATOR_OWNER: &str = "fleet";`
  - `pub const OPERATOR_REPO: &str = "operator";`
  - `pub const OPERATOR_DIR: &str = "~/.claude-fleet/operator";`
  - `pub const OPERATOR_TMUX_NAME: &str = "fleet-operator";`
  - `pub const SETTING_OPERATOR_TOKEN_SHA: &str = "operator.token_sha";`
  - `pub fn claude_md() -> &'static str` (PURE)
  - `pub fn settings_json(endpoint: &str, token: &str) -> String` (PURE)
  - `pub async fn ensure_operator(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, reg: &Arc<CancellationRegistry>) -> Result<SessionRow, IpcError>`

> The endpoint is always `http://127.0.0.1:<configured_port>/mcp`, in **both** modes: the operator runs on the machine that serves the control API — `local` for the desktop, the hub's own host for a hub — so loopback is right and no public URL is ever baked into the file.

- [ ] **Step 1: Write the failing tests for the pure parts**

Add to the test module in `operator.rs`:

```rust
    #[test]
    fn the_settings_file_points_at_the_endpoint_and_carries_the_token() {
        let j = settings_json("http://127.0.0.1:4180/mcp", "deadbeef");
        let v: serde_json::Value = serde_json::from_str(&j).expect("valid JSON");
        let srv = &v["mcpServers"]["claude-fleet"];
        assert_eq!(srv["type"], "http");
        assert_eq!(srv["url"], "http://127.0.0.1:4180/mcp");
        assert_eq!(srv["headers"]["Authorization"], "Bearer deadbeef");
    }

    #[test]
    fn the_operating_instructions_state_the_two_rules_that_matter() {
        let md = claude_md();
        assert!(
            md.contains("propose") || md.contains("confirm"),
            "the operator must be told destructive work is confirmed, not assumed"
        );
        assert!(
            md.contains("not a repository") || md.contains("do not write code"),
            "the operator must be told its directory is not a place to write code"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p fleet-core service::operator
```

Expected: FAIL to compile — `settings_json` and `claude_md` do not exist.

- [ ] **Step 3: Write the pure parts**

```rust
/// Owner/repo of the operator's own project row. Not a real repository —
/// `upsert_system_project` flags it so the picker hides it and the sweep
/// leaves it be.
pub const OPERATOR_OWNER: &str = "fleet";
pub const OPERATOR_REPO: &str = "operator";
/// The operator's working directory. Deliberately NOT one of the user's
/// checkouts: the agent runs sessions, it does not write code.
pub const OPERATOR_DIR: &str = "~/.claude-fleet/operator";
/// Fixed tmux name, so the session is recognisable in the sidebar and in
/// `tmux ls` without consulting the database.
pub const OPERATOR_TMUX_NAME: &str = "fleet-operator";
/// `settings` key holding the SHA-256 of the operator's client token, so
/// `operator_status` can tell a revoked token from a healthy one without
/// reading the secret back off the host.
pub const SETTING_OPERATOR_TOKEN_SHA: &str = "operator.token_sha";

/// PURE: the operator's standing instructions.
pub fn claude_md() -> &'static str {
    "# You are the fleet operator\n\
     \n\
     You drive claude-fleet through its MCP control API on behalf of the\n\
     person at the keyboard. `list_sessions`, `fleet_health` and\n\
     `session_conversation` tell you what is happening; `send_prompt`,\n\
     `new_session` and the rest change it.\n\
     \n\
     Two rules.\n\
     \n\
     1. **Destructive work is proposed, never assumed.** Killing, deleting,\n\
     moving, broadcasting and committing stop for a confirmation you do not\n\
     control. Say plainly what you are about to do and let the dialog do its\n\
     job; do not try to route around a refusal.\n\
     \n\
     2. **This directory is not a repository and you do not write code in\n\
     it.** When work needs doing in a project, start a session on the right\n\
     host and brief it. You are the operator, not the worker.\n\
     \n\
     Answer in the language the person uses. Prefer one short paragraph over\n\
     a report: the sidebar already shows what changed.\n"
}

/// PURE: the operator's `.claude/settings.json`, pointing its MCP client at
/// this fleet with its own bearer token.
pub fn settings_json(endpoint: &str, token: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            "claude-fleet": {
                "type": "http",
                "url": endpoint,
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    })
    .to_string()
}
```

- [ ] **Step 4: Run to verify the pure tests pass**

```bash
cargo test -p fleet-core service::operator
```

Expected: PASS.

- [ ] **Step 5: Write the failing idempotence test**

```rust
    /// `ensure_operator` runs on every press of the button. Twice over it
    /// must leave one project, one session and one token — not three.
    #[tokio::test]
    async fn ensure_operator_is_idempotent() {
        let (store, ssh, reg) = fixture();
        let first = ensure_operator(&store, &ssh, &reg).await.expect("first");
        let second = ensure_operator(&store, &ssh, &reg).await.expect("second");
        assert_eq!(first.id, second.id, "the same session comes back");

        let s = store.lock().unwrap();
        let projects: Vec<_> = s
            .list_projects()
            .unwrap()
            .into_iter()
            .filter(|p| p.system)
            .collect();
        assert_eq!(projects.len(), 1, "one operator project, not two");
        let tokens: Vec<_> = s
            .list_client_tokens(false)
            .unwrap()
            .into_iter()
            .filter(|t| t.name == "ux-agent")
            .collect();
        assert_eq!(tokens.len(), 1, "one operator token, not two");
        assert_eq!(
            operator_ref(&s),
            Some(OperatorRef {
                host_alias: "local".into(),
                tmux_name: OPERATOR_TMUX_NAME.into(),
            })
        );
    }
```

Add `fixture()` to this file's test module, built from whatever in-memory store + fake SSH harness the neighbouring service tests already use — `crates/fleet-core/src/service/sessions/lifecycle_tests.rs` constructs one; copy its shape rather than inventing a new one. It must return `(Arc<Mutex<Store>>, Arc<SshClient>, Arc<CancellationRegistry>)` with a `local` host row present.

- [ ] **Step 6: Run to verify it fails**

```bash
cargo test -p fleet-core ensure_operator_is_idempotent
```

Expected: FAIL to compile — `ensure_operator` does not exist.

- [ ] **Step 7: Write `ensure_operator`**

```rust
/// Make sure the operator session exists, and return its row.
///
/// Idempotent by design — this runs on every press of the FAB. When a live
/// session is already recorded it is a pair of store reads and nothing else.
/// Birth is lazy for exactly this reason: an agent you never open costs
/// nothing.
pub async fn ensure_operator(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    // 1. Already alive? Then there is nothing to do.
    {
        let s = lock(store)?;
        if let Some(r) = operator_ref(&s) {
            if let Some(row) = s
                .find_session(&r.host_alias, &r.tmux_name)
                .map_err(|e| IpcError::new(codes::E_SQLITE, format!("find operator: {e}")))?
            {
                if row.lost_at.is_none() {
                    return Ok(row);
                }
            }
        }
    }

    // 2. The project row and the token, under one guard.
    let (project_id, token) = {
        let s = lock(store)?;
        let project_id = s
            .upsert_system_project(OPERATOR_OWNER, OPERATOR_REPO, OPERATOR_DIR)
            .map_err(|e| {
                IpcError::new(codes::E_SQLITE, format!("operator project row: {e}"))
            })?;
        // Only the hash is ever stored; the plaintext lives in the operator's
        // settings.json on the host and nowhere else.
        let token = crate::mcp::generate_token();
        let sha = crate::mcp::auth::sha256_hex(&token);
        // A previous token by this name may still be live (a half-finished
        // birth). Revoke it rather than colliding on the unique name index.
        if let Ok(rows) = s.list_client_tokens(false) {
            for row in rows.into_iter().filter(|r| r.name == "ux-agent") {
                let _ = s.revoke_client_token(&row.name);
            }
        }
        s.insert_client_token("ux-agent", &sha, "full")?;
        s.set_setting(SETTING_OPERATOR_TOKEN_SHA, &sha).map_err(|e| {
            IpcError::new(codes::E_SQLITE, format!("record the operator token: {e}"))
        })?;
        (project_id, token)
    };

    // 3. The files on the host. `settings.json` carries the bearer token, so
    //    it goes through the secret path: never in an argv, never a truncated
    //    file on a failed write.
    let endpoint = {
        let s = lock(store)?;
        format!(
            "http://127.0.0.1:{}/mcp",
            crate::mcp::settings::configured_port(&s)?
        )
    };
    let claude_dir = format!("{OPERATOR_DIR}/.claude");
    crate::service::provision::write_host_file(
        ssh.as_ref(),
        "local",
        OPERATOR_DIR,
        &format!("{OPERATOR_DIR}/CLAUDE.md"),
        claude_md(),
    )
    .await?;
    crate::service::provision::write_host_file_secret(
        ssh.as_ref(),
        "local",
        &claude_dir,
        &format!("{claude_dir}/settings.json"),
        &settings_json(&endpoint, &token),
    )
    .await?;

    // 4. The session itself — an ordinary work session, which is the whole
    //    point: transcript, conversation view, restart and reboot survival
    //    all come for free.
    let row = crate::service::sessions::lifecycle::new_session(
        crate::service::sessions::lifecycle::NewSessionArgs {
            host_alias: "local".to_string(),
            project_id,
            worktree_id: None,
            name: OPERATOR_TMUX_NAME.to_string(),
            call_id: None,
            new_worktree: None,
            base_branch: None,
            kind: Some("work".to_string()),
            start_command: None,
            friendly_name: Some("fleet operator".to_string()),
        },
        store,
        ssh,
        reg,
    )
    .await?;

    {
        let s = lock(store)?;
        set_operator_ref(
            &s,
            &OperatorRef {
                host_alias: row.host_alias.clone(),
                tmux_name: row.tmux_name.clone(),
            },
        )?;
    }
    Ok(row)
}
```

Add the imports this needs at the top of the file:

```rust
use crate::cancel::CancellationRegistry;
use crate::ipc_error::lock;
use crate::ssh::SshClient;
use crate::store::SessionRow;
use std::sync::{Arc, Mutex};
```

If `Store` has no `find_session(host, tmux)`, use whichever lookup the reconcile path already uses to resolve a `(host, tmux_name)` pair to a row, and keep the `lost_at.is_none()` condition.

- [ ] **Step 8: Run the test**

```bash
cargo test -p fleet-core service::operator
```

Expected: PASS, five tests.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/src/service/operator.rs
git commit -m "feat(operator): bring the UX agent's session into being, idempotently"
```

---

### Task 6: `operator_status` — readiness, and why not

**Files:**
- Modify: `crates/fleet-core/src/service/operator.rs`
- Test: its test module

**Interfaces:**
- Consumes: Task 5's constants; `crate::mcp::settings::McpSettings::read`; `Store::active_client_tokens`.
- Produces:
  - `pub struct OperatorStatus { pub ready: bool, pub session: Option<SessionRow>, pub blocked: Option<String> }`
  - `pub fn operator_status(store: &Mutex<Store>) -> Result<OperatorStatus, IpcError>`
  - Blocked values, exactly these four strings: `"absent"`, `"lost"`, `"no_mcp"`, `"token_revoked"`.

> **No `#[serde(default)]` and no `skip_serializing_if` on this struct.** Both `Option` fields always serialise (as `null` when empty) so a hub-read result deserialises without defaults.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn status_names_why_the_agent_cannot_work() {
        let (store, _ssh, _reg) = fixture();

        // Never born.
        let st = operator_status(&store).unwrap();
        assert!(!st.ready);
        assert_eq!(st.blocked.as_deref(), Some("absent"));
        assert!(st.session.is_none());

        // Born, but the control API is off: an agent with no tools is a
        // chatbot, and the panel must say so rather than let it apologise.
        {
            let s = store.lock().unwrap();
            s.set_setting("mcp.enabled", "false").unwrap();
            set_operator_ref(
                &s,
                &OperatorRef {
                    host_alias: "local".into(),
                    tmux_name: OPERATOR_TMUX_NAME.into(),
                },
            )
            .unwrap();
        }
        assert_eq!(
            operator_status(&store).unwrap().blocked.as_deref(),
            Some("no_mcp")
        );
    }
```

Use whatever settings key `McpSettings::read` actually reads for `enabled` — check `crates/fleet-core/src/mcp/settings.rs` and use that constant rather than the literal above if one exists.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p fleet-core status_names_why_the_agent_cannot_work
```

Expected: FAIL to compile — `operator_status` / `OperatorStatus` do not exist.

- [ ] **Step 3: Implement**

```rust
/// What the FAB needs to know before it opens a panel.
///
/// Deliberately plain fields, always serialised: a hub-read result must
/// deserialise without serde defaults.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperatorStatus {
    pub ready: bool,
    pub session: Option<SessionRow>,
    /// `null`, or one of `"absent"`, `"lost"`, `"no_mcp"`, `"token_revoked"`.
    pub blocked: Option<String>,
}

/// Why the agent can or cannot work right now.
///
/// The control-API check is computed HERE, inside the authoritative backend,
/// rather than from the desktop's `mcp_status` — that command is
/// `LocalOnly`, and a hub's API is always on. Asking the backend that would
/// actually serve the agent is the same question in both modes.
pub fn operator_status(store: &Mutex<Store>) -> Result<OperatorStatus, IpcError> {
    let s = lock(store)?;
    let blocked = |why: &str, session: Option<SessionRow>| OperatorStatus {
        ready: false,
        session,
        blocked: Some(why.to_string()),
    };

    if !crate::mcp::settings::McpSettings::read(&s)?.enabled {
        return Ok(blocked("no_mcp", None));
    }
    let Some(r) = operator_ref(&s) else {
        return Ok(blocked("absent", None));
    };
    let row = s
        .find_session(&r.host_alias, &r.tmux_name)
        .map_err(|e| IpcError::new(codes::E_SQLITE, format!("find operator: {e}")))?;
    let Some(row) = row else {
        return Ok(blocked("absent", None));
    };
    if row.lost_at.is_some() {
        return Ok(blocked("lost", Some(row)));
    }
    // A revoked token is a deliberate act, so nothing re-mints itself — the
    // panel offers a button and the person presses it.
    let sha = s.get_setting(SETTING_OPERATOR_TOKEN_SHA).ok().flatten();
    let live = s
        .active_client_tokens()
        .map(|rows| {
            rows.iter()
                .any(|t| Some(&t.token_sha256) == sha.as_ref())
        })
        .unwrap_or(false);
    if !live {
        return Ok(blocked("token_revoked", Some(row)));
    }
    Ok(OperatorStatus {
        ready: true,
        session: Some(row),
        blocked: None,
    })
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p fleet-core service::operator
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/operator.rs
git commit -m "feat(operator): say why the agent cannot work, rather than letting it apologise"
```

---

### Task 7: Two router tools

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/session_ops.rs`
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES`)
- Test: `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Consumes: `service::operator::{ensure_operator, operator_status, OperatorStatus}` (Tasks 5, 6).
- Produces: router tools `ensure_operator` (returns `SessionRow`) and `operator_status` (returns `OperatorStatus`).

- [ ] **Step 1: Write the failing test**

In `crates/fleet-core/src/mcp/tools/tests.rs`:

```rust
/// The operator's lifecycle must be reachable by the desktop, which pairs as
/// an ORDINARY CLIENT and never holds the master token — so these two are
/// `Access::Client`. They are not confirm-gated (creating the agent is what
/// the person just asked for by pressing the button) and `ensure_operator`
/// spawns a session, so it is `Deadline::Lifecycle`.
#[test]
fn the_operator_tools_are_client_reachable_and_not_admin() {
    for name in ["ensure_operator", "operator_status"] {
        let p = crate::mcp::guard::policy(name)
            .unwrap_or_else(|| panic!("{name} has no TOOL_POLICIES row"));
        assert!(!crate::mcp::guard::is_admin_tool(name), "{name} is not fleet admin");
        assert!(crate::mcp::guard::is_client_tool(name), "{name} is client-reachable");
        assert!(!p.confirm, "{name} is not confirm-gated");
    }
    assert!(
        crate::mcp::guard::is_readonly_tool("operator_status"),
        "reading the agent's readiness observes, it does not change"
    );
    assert!(
        !crate::mcp::guard::is_readonly_tool("ensure_operator"),
        "creating the agent is a write"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p fleet-core the_operator_tools_are_client_reachable
```

Expected: FAIL — `ensure_operator has no TOOL_POLICIES row`.

- [ ] **Step 3: Add the tools**

In `crates/fleet-core/src/mcp/tools/session_ops.rs`, beside `new_bg_session`, following that file's `#[tool(...)]` conventions exactly. Keep the descriptions short — a test caps the total served tool-description budget:

```rust
    #[tool(
        description = "Make sure the UX agent's operator session exists; returns its row. Idempotent."
    )]
    pub(super) async fn ensure_operator(&self) -> Result<CallToolResult, ErrorData> {
        self.guarded("ensure_operator", "", |this| async move {
            crate::service::operator::ensure_operator(&this.store, &this.ssh, &this.reg).await
        })
        .await
    }

    #[tool(
        description = "Whether the UX agent can work, and why not: absent | lost | no_mcp | token_revoked."
    )]
    pub(super) async fn operator_status(&self) -> Result<CallToolResult, ErrorData> {
        self.guarded("operator_status", "", |this| async move {
            crate::service::operator::operator_status(&this.store)
        })
        .await
    }
```

Match the surrounding helpers' actual names and shapes — copy the body structure of the neighbouring `new_bg_session` rather than the sketch above if they differ.

In `crates/fleet-core/src/mcp/guard.rs`, add two `TOOL_POLICIES` rows in the `session_ops.rs` section:

```rust
    ToolPolicy {
        name: "ensure_operator",
        access: Access::Client,
        readonly: false,
        confirm: false,
        deadline: Deadline::Lifecycle,
    },
    ToolPolicy {
        name: "operator_status",
        access: Access::Client,
        readonly: true,
        confirm: false,
        deadline: Deadline::Quick,
    },
```

- [ ] **Step 4: Run the MCP tests**

```bash
cargo test -p fleet-core mcp::
```

Expected: PASS, including `every_router_tool_has_exactly_one_policy_row` and the tool-description budget test. If the budget test fails, shorten the two descriptions — do not raise the cap.

- [ ] **Step 5: Regenerate the reference**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

Then re-run it without the flag to confirm it passes. Never hand-edit `docs/control-api-reference.md`.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/mcp docs/control-api-reference.md
git commit -m "feat(mcp): ensure_operator and operator_status"
```

---

### Task 8: The two Tauri commands

**Files:**
- Create: `src-tauri/src/commands/operator.rs`
- Modify: `src-tauri/src/commands/mod.rs` (declare the module)
- Modify: `src-tauri/src/lib.rs` or wherever `generate_handler!` lives (add both, in the same order as the verdict rows)
- Modify: `src-tauri/src/backend/verdicts.rs` (two rows), `src-tauri/src/backend/tests_routing.rs` (two entries)

**Interfaces:**
- Consumes: the tools from Task 7; `service::operator::{ensure_operator, operator_status, OperatorStatus}`.
- Produces: Tauri commands `ensure_operator -> SessionRow` and `operator_status -> OperatorStatus`.

- [ ] **Step 1: Write the command module**

Create `src-tauri/src/commands/operator.rs`:

```rust
//! Tauri IPC wrappers for the UX agent's lifecycle. Logic lives in
//! `service::operator`; this file only adapts `tauri::State` to plain
//! references.
//!
//! Both ROUTE in remote mode. That is not incidental: the agent panel is the
//! same panel on a hub-backed desktop, and the phone slice inherits these
//! tools unchanged. A `LocalOnly` verdict here would have closed that door.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::operator::{self, OperatorStatus};
use fleet_core::ssh::SshClient;
use fleet_core::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn ensure_operator(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    routed::ensure_operator(&backend, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn operator_status(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OperatorStatus, IpcError> {
    routed::operator_status(&backend, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn ensure_operator(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.ensure_operator().await,
            None => operator::ensure_operator(store, ssh, reg).await,
        }
    }

    pub async fn operator_status(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<OperatorStatus, IpcError> {
        match backend.hub() {
            Some(hub) => hub.operator_status().await,
            None => operator::operator_status(store),
        }
    }
}
```

Add the two hub-client methods next to the existing ones (`hub.list_projects()` and friends), following that file's own pattern for a no-argument tool call.

- [ ] **Step 2: Add the verdict rows**

In `src-tauri/src/backend/verdicts.rs`, in `VERDICTS`, in `generate_handler!` order:

```rust
    (
        "ensure_operator",
        Verdict::Routed {
            tool: "ensure_operator",
        },
    ),
    (
        "operator_status",
        Verdict::Routed {
            tool: "operator_status",
        },
    ),
```

- [ ] **Step 3: Add the routing test entries**

In `src-tauri/src/backend/tests_routing.rs`, in the handler table:

```rust
        (
            "ensure_operator",
            "ensure_operator",
            json!({}),
            "{}",
            Box::new(|b, s, _| {
                block_on(commands::operator::routed::operator_status(b, s)).map(|_| ())
            }),
        ),
```

**Note the trap:** the closure above deliberately shows `operator_status` because `ensure_operator` needs `ssh` and `reg`, which that closure shape does not carry. Use whichever closure arity the table provides for commands taking SSH — `move_session` and `new_session` already do this; copy their entry shape for `ensure_operator` and use the simple shape only for `operator_status`.

- [ ] **Step 4: Run the routing tests**

```bash
cargo test -p claude-fleet --lib backend
```

Expected: PASS. The handler-list test names any command missing from `generate_handler!` or from `VERDICTS`.

- [ ] **Step 5: Regenerate the verdict table and the reference**

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

Re-run both without the flags to confirm they pass. The first regen run may itself report FAILED — that is the regeneration, not a failure; the second run is the verdict.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(commands): ensure_operator and operator_status, both routed"
```

---

### Task 9: `agent_context.ts` — the visible chip

**Files:**
- Create: `src/lib/agent_context.ts`
- Test: `src/lib/agent_context.test.ts`

**Interfaces:**
- Consumes: `SessionRow` from `src/lib/sessions.ts`.
- Produces:
  - `export interface AgentContext { chipLabel: string; prefix: string }`
  - `export interface AgentContextInput { view: 'terminal' | 'hosts' | 'files'; session: SessionRow | null; hostAlias: string | null; branch: string | null }`
  - `export function agentContext(input: AgentContextInput): AgentContext | null`

- [ ] **Step 1: Write the failing test**

Create `src/lib/agent_context.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { agentContext } from './agent_context';
import type { SessionRow } from './sessions';

const session = (over: Partial<SessionRow> = {}) =>
  ({
    id: 7,
    tmux_name: 'blue-sirius',
    host_alias: 'mefistos',
    friendly_name: null,
    kind: 'work',
    ...over,
  }) as SessionRow;

describe('agentContext', () => {
  it('a selected session is host, name and branch — what "this one" means', () => {
    const c = agentContext({
      view: 'terminal',
      session: session(),
      hostAlias: 'mefistos',
      branch: 'feature/x',
    });
    expect(c?.chipLabel).toBe('blue-sirius · mefistos · feature/x');
    expect(c?.prefix).toContain('blue-sirius');
    expect(c?.prefix).toContain('mefistos');
    expect(c?.prefix).toContain('feature/x');
  });

  it('prefers the friendly name, because that is what the person sees', () => {
    const c = agentContext({
      view: 'terminal',
      session: session({ friendly_name: 'the payments one' }),
      hostAlias: 'mefistos',
      branch: null,
    });
    expect(c?.chipLabel).toBe('the payments one · mefistos');
  });

  it('in the Hosts view the context is the host, not a session', () => {
    const c = agentContext({ view: 'hosts', session: null, hostAlias: 'hetzner', branch: null });
    expect(c?.chipLabel).toBe('hetzner');
    expect(c?.prefix).toContain('hetzner');
  });

  it('with nothing selected there is no chip and no prefix to remove', () => {
    expect(agentContext({ view: 'terminal', session: null, hostAlias: null, branch: null })).toBeNull();
    expect(agentContext({ view: 'hosts', session: null, hostAlias: null, branch: null })).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npx vitest run src/lib/agent_context.test.ts
```

Expected: FAIL — cannot resolve `./agent_context`.

- [ ] **Step 3: Implement**

Create `src/lib/agent_context.ts`:

```ts
// What the agent is told about where you are standing, and what the composer
// shows you of it. Pure on purpose: the rules for "what does 'this one' mean
// in this view" will accumulate here, and they are cheaper to get right in a
// test than in a component.
import type { SessionRow } from './sessions';

export interface AgentContext {
  /** What the removable chip reads. */
  chipLabel: string;
  /** What is actually prepended to the prompt. */
  prefix: string;
}

export interface AgentContextInput {
  view: 'terminal' | 'hosts' | 'files';
  session: SessionRow | null;
  hostAlias: string | null;
  branch: string | null;
}

export function agentContext(input: AgentContextInput): AgentContext | null {
  if (input.session) {
    const name = input.session.friendly_name || input.session.tmux_name;
    const parts = [name, input.session.host_alias];
    if (input.branch) parts.push(input.branch);
    return {
      chipLabel: parts.join(' · '),
      prefix:
        `[context] the person is looking at session "${name}" on host ` +
        `${input.session.host_alias}` +
        (input.branch ? ` (branch ${input.branch})` : '') +
        `. "this one" and "here" mean that session unless they say otherwise.`,
    };
  }
  if (input.hostAlias) {
    return {
      chipLabel: input.hostAlias,
      prefix:
        `[context] the person is looking at host ${input.hostAlias} with no ` +
        `session selected. "here" means that host.`,
    };
  }
  return null;
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
npx vitest run src/lib/agent_context.test.ts
```

Expected: PASS, four tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/agent_context.ts src/lib/agent_context.test.ts
git commit -m "feat(ui): the agent's context chip, as a pure function"
```

---

### Task 10: `appChord` gains the agent

**Files:**
- Modify: `src/lib/app_views.ts:60-115` (`AppChord`, `appChord`, a new label fn)
- Test: `src/lib/app_views.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `AppChord` gains `'agent'`; `export function agentChordLabel(isMac: boolean): string`.

- [ ] **Step 1: Write the failing test**

Append to the `appChord` describe block in `src/lib/app_views.test.ts`:

```ts
  it('⌘E opens the agent, and does not collide with ⌘J (Session view)', () => {
    expect(appChord(ev('e', { metaKey: true }), true)).toBe('agent');
    expect(appChord(ev('E', { metaKey: true }), true)).toBe('agent');
    expect(appChord(ev('j', { metaKey: true }), true)).toBe('session-view');
    expect(appChord(ev('e', { ctrlKey: true }), true)).toBeNull();
    expect(appChord(ev('E', { ctrlKey: true, shiftKey: true }), false)).toBe('agent');
    expect(appChord(ev('e', { ctrlKey: true }), false)).toBeNull();
  });

  it('labels the agent chord per platform', () => {
    expect(agentChordLabel(true)).toBe('⌘E');
    expect(agentChordLabel(false)).toBe('Ctrl+Shift+E');
  });
```

and add `agentChordLabel` to the import at the top of the file.

- [ ] **Step 2: Run to verify it fails**

```bash
npx vitest run src/lib/app_views.test.ts
```

Expected: FAIL — `agentChordLabel` is not exported; `appChord` returns null for `e`.

- [ ] **Step 3: Implement**

```ts
export type AppChord = 'hosts' | 'settings' | 'session-view' | 'agent';
```

inside `appChord`, in the meta branch after the `j` line:

```ts
    if (k === 'e') return 'agent';
```

and in the non-mac branch after its `j` line:

```ts
    if (k === 'e') return 'agent';
```

then:

```ts
/** Label for the agent chord, for the FAB's tooltip and the hint. */
export function agentChordLabel(isMac: boolean): string {
  return isMac ? '⌘E' : 'Ctrl+Shift+E';
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
npx vitest run src/lib/app_views.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/app_views.ts src/lib/app_views.test.ts
git commit -m "feat(ui): ⌘E opens the agent"
```

---

### Task 11: `operator.ts` — the store

**Files:**
- Create: `src/lib/operator.ts`
- Test: `src/lib/operator.test.ts`

**Interfaces:**
- Consumes: `invokeCmd` from `src/lib/result.ts`; `SessionRow` from `src/lib/sessions.ts`; the two commands from Task 8.
- Produces:
  - `export type OperatorBlocked = 'absent' | 'lost' | 'no_mcp' | 'token_revoked'`
  - `export interface OperatorStatus { ready: boolean; session: SessionRow | null; blocked: OperatorBlocked | null }`
  - `export const agentPanelOpen: Writable<boolean>`
  - `export const operatorState: Writable<'unknown' | 'waking' | 'ready' | OperatorBlocked>`
  - `export const operatorSession: Writable<SessionRow | null>`
  - `export function blockedCopy(b: OperatorBlocked): { title: string; action: string | null }` (PURE)
  - `export async function openAgent(): Promise<void>`
  - `export async function refreshOperator(): Promise<void>`

- [ ] **Step 1: Write the failing test**

Create `src/lib/operator.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { get } from 'svelte/store';
import {
  agentPanelOpen,
  operatorState,
  operatorSession,
  openAgent,
  blockedCopy,
} from './operator';

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(false);
  operatorState.set('unknown');
  operatorSession.set(null);
});

describe('openAgent', () => {
  it('opens the panel, wakes the agent, and ends ready', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' });
    invoke.mockResolvedValueOnce({ id: 7, tmux_name: 'fleet-operator', host_alias: 'local' });
    await openAgent();
    expect(get(agentPanelOpen)).toBe(true);
    expect(get(operatorState)).toBe('ready');
    expect(get(operatorSession)?.id).toBe(7);
  });

  it('a blocked agent does not get woken, and the reason survives', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'no_mcp' });
    await openAgent();
    expect(get(agentPanelOpen)).toBe(true);
    expect(get(operatorState)).toBe('no_mcp');
    // Only the status call — ensure_operator must not run when the control
    // API is off: it would create a session that cannot reach any tool.
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it('a ready agent is not re-created', async () => {
    invoke.mockResolvedValueOnce({
      ready: true,
      session: { id: 3, tmux_name: 'fleet-operator', host_alias: 'local' },
      blocked: null,
    });
    await openAgent();
    expect(get(operatorState)).toBe('ready');
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe('blockedCopy', () => {
  it('every reason has copy, and only the fixable ones offer a button', () => {
    expect(blockedCopy('no_mcp').action).toBe('Enable the control API');
    expect(blockedCopy('lost').action).toBe('Restart the agent');
    expect(blockedCopy('token_revoked').action).toBe('Mint a new token');
    expect(blockedCopy('absent').action).toBeNull();
    for (const b of ['no_mcp', 'lost', 'token_revoked', 'absent'] as const) {
      expect(blockedCopy(b).title.length).toBeGreaterThan(0);
    }
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npx vitest run src/lib/operator.test.ts
```

Expected: FAIL — cannot resolve `./operator`.

- [ ] **Step 3: Implement**

Create `src/lib/operator.ts`:

```ts
// The UX agent's state, as the panel needs it. Follows `app_views.ts`: the
// FAB and the panel talk through these stores instead of prop-drilling
// through App.svelte.
import { get, writable } from 'svelte/store';
import { invokeCmd } from './result';
import type { SessionRow } from './sessions';

export type OperatorBlocked = 'absent' | 'lost' | 'no_mcp' | 'token_revoked';

export interface OperatorStatus {
  ready: boolean;
  session: SessionRow | null;
  blocked: OperatorBlocked | null;
}

export const agentPanelOpen = writable(false);
export const operatorState = writable<'unknown' | 'waking' | 'ready' | OperatorBlocked>('unknown');
export const operatorSession = writable<SessionRow | null>(null);

/** PURE: what the panel says, and whether there is a button under it. */
export function blockedCopy(b: OperatorBlocked): { title: string; action: string | null } {
  switch (b) {
    case 'no_mcp':
      return {
        title: 'The control API is off, so the agent would have no tools.',
        action: 'Enable the control API',
      };
    case 'lost':
      return {
        title: 'The agent session was lost. It is not brought back silently — it may have stopped mid-sentence.',
        action: 'Restart the agent',
      };
    case 'token_revoked':
      return {
        title: 'The agent’s token was revoked, so it can no longer reach the fleet.',
        action: 'Mint a new token',
      };
    case 'absent':
      return { title: 'Waking the agent…', action: null };
  }
}

/** Read status without changing anything. */
export async function refreshOperator(): Promise<void> {
  const r = await invokeCmd<OperatorStatus>('operator_status');
  if (!r.ok) {
    operatorState.set('absent');
    return;
  }
  operatorSession.set(r.value.session);
  operatorState.set(r.value.ready ? 'ready' : (r.value.blocked ?? 'absent'));
}

/**
 * Open the panel and make sure there is an agent behind it.
 *
 * `absent` is the only reason worth acting on here: every other block is a
 * deliberate state (the API turned off, a token revoked) or a death worth
 * seeing, and creating a session under any of them would produce an agent
 * that cannot work.
 */
export async function openAgent(): Promise<void> {
  agentPanelOpen.set(true);
  await refreshOperator();
  if (get(operatorState) !== 'absent') return;
  operatorState.set('waking');
  const r = await invokeCmd<SessionRow>('ensure_operator');
  if (!r.ok) {
    operatorState.set('absent');
    return;
  }
  operatorSession.set(r.value);
  operatorState.set('ready');
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
npx vitest run src/lib/operator.test.ts
```

Expected: PASS, four tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/operator.ts src/lib/operator.test.ts
git commit -m "feat(ui): the agent's store — open, wake, and say why not"
```

---

### Task 12: `AgentFab.svelte`

**Files:**
- Create: `src/lib/AgentFab.svelte`
- Test: `src/lib/AgentFab.test.ts`

**Interfaces:**
- Consumes: `openAgent`, `operatorState` (Task 11); `agentChordLabel` (Task 10).
- Produces: a component taking no props, mounted once by `App.svelte`.

- [ ] **Step 1: Write the failing test**

Create `src/lib/AgentFab.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import AgentFab from './AgentFab.svelte';
import { agentPanelOpen, operatorState } from './operator';

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(false);
  operatorState.set('unknown');
});

describe('AgentFab', () => {
  it('is a labelled button that opens the agent', async () => {
    invoke.mockResolvedValue({ ready: true, session: null, blocked: null });
    render(AgentFab);
    const btn = screen.getByRole('button', { name: /agent/i });
    await fireEvent.click(btn);
    expect(invoke).toHaveBeenCalledWith('operator_status', undefined);
  });

  it('says why it is unavailable when the control API is off', async () => {
    operatorState.set('no_mcp');
    render(AgentFab);
    const btn = screen.getByRole('button', { name: /agent/i });
    expect(btn).toHaveAttribute('title', expect.stringContaining('control API'));
  });
});
```

If `invokeCmd` passes no second argument, adjust the `toHaveBeenCalledWith` assertion to match what `result.ts` actually sends rather than changing `result.ts`.

- [ ] **Step 2: Run to verify it fails**

```bash
npx vitest run src/lib/AgentFab.test.ts
```

Expected: FAIL — cannot resolve `./AgentFab.svelte`.

- [ ] **Step 3: Implement**

Create `src/lib/AgentFab.svelte`:

```svelte
<script lang="ts">
  // The one visible way in, from every view. Mounted once in App.svelte
  // beside HintLayer and McpConfirmDialog — that is what makes it present
  // over the terminal, Hosts and Files without any of them knowing.
  import { openAgent, operatorState, blockedCopy, type OperatorBlocked } from './operator';
  import { agentChordLabel } from './app_views';

  const isMac = navigator.platform.toLowerCase().includes('mac');
  const blocked = $derived(
    $operatorState === 'no_mcp' || $operatorState === 'token_revoked'
      ? blockedCopy($operatorState as OperatorBlocked)
      : null,
  );
  const title = $derived(blocked ? blocked.title : `Ask the agent (${agentChordLabel(isMac)})`);
</script>

<button class="agent-fab" {title} aria-label="Agent" onclick={() => void openAgent()}>
  <span aria-hidden="true">✦</span>
</button>

<style>
  .agent-fab {
    position: fixed;
    right: 20px;
    bottom: 20px;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    border: 1px solid var(--border);
    background: var(--accent-bg, var(--panel));
    color: var(--fg);
    font-size: 20px;
    cursor: pointer;
    z-index: 40;
    box-shadow: 0 2px 10px rgb(0 0 0 / 30%);
  }
  .agent-fab:hover {
    filter: brightness(1.15);
  }
</style>
```

Use the CSS custom properties this codebase actually defines — check `src/app.css` and replace `--border`, `--panel`, `--fg` with the real names if they differ.

- [ ] **Step 4: Run to verify it passes**

```bash
npx vitest run src/lib/AgentFab.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/AgentFab.svelte src/lib/AgentFab.test.ts
git commit -m "feat(ui): the agent FAB"
```

---

### Task 13: `AgentPanel.svelte`

**Files:**
- Create: `src/lib/AgentPanel.svelte`
- Test: `src/lib/AgentPanel.test.ts`

**Interfaces:**
- Consumes: `agentPanelOpen`, `operatorState`, `operatorSession`, `blockedCopy` (Task 11); `agentContext` (Task 9); `ConversationPanel.svelte`; `PromptComposer.svelte`.
- Produces: a component taking no props, mounted once by `App.svelte`.

- [ ] **Step 1: Write the failing test**

Create `src/lib/AgentPanel.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import AgentPanel from './AgentPanel.svelte';
import { agentPanelOpen, operatorState, operatorSession } from './operator';

const row = (over = {}) =>
  ({
    id: 7,
    tmux_name: 'fleet-operator',
    host_alias: 'local',
    kind: 'work',
    claude_status: 'idle',
    stuck_kind: null,
    ...over,
  }) as never;

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(true);
  operatorState.set('ready');
  operatorSession.set(row());
});

describe('AgentPanel', () => {
  it('will not send while the agent is working — two pastes into one REPL is one mangled prompt', () => {
    operatorSession.set(row({ claude_status: 'working' }));
    render(AgentPanel);
    expect(screen.getByRole('button', { name: /send/i })).toBeDisabled();
    expect(screen.getByText(/working/i)).toBeTruthy();
  });

  it('surfaces stuck_kind, because a stuck agent looks exactly like a slow one', () => {
    operatorSession.set(row({ claude_status: 'working', stuck_kind: 'trust_prompt' }));
    render(AgentPanel);
    expect(screen.getByText(/trust/i)).toBeTruthy();
  });

  it('offers a restart when the agent was lost, and does not resurrect it by itself', () => {
    operatorState.set('lost');
    render(AgentPanel);
    expect(screen.getByRole('button', { name: /restart/i })).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith('ensure_operator', expect.anything());
  });

  it('shows the context chip and drops it when removed', async () => {
    render(AgentPanel);
    // With no session selected elsewhere in the app there is no chip at all.
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npx vitest run src/lib/AgentPanel.test.ts
```

Expected: FAIL — cannot resolve `./AgentPanel.svelte`.

- [ ] **Step 3: Implement**

Create `src/lib/AgentPanel.svelte`. Keep the conversation rendering delegated — this component owns the *frame*, not the transcript:

```svelte
<script lang="ts">
  // The agent's sheet: a compact conversation over the operator session, a
  // composer, and the removable context chip. Everything that renders turns
  // is ConversationPanel's job; what lives here is the frame, the chip, and
  // the three states where the agent cannot simply be talked to.
  import ConversationPanel from './ConversationPanel.svelte';
  import { agentPanelOpen, operatorState, operatorSession, blockedCopy, refreshOperator, type OperatorBlocked } from './operator';
  import { agentContext, type AgentContextInput } from './agent_context';
  import { sendPrompt } from './sessions';
  import { stuckKindLabel } from './attention';

  let { contextInput = null }: { contextInput?: AgentContextInput | null } = $props();

  let draft = $state('');
  let chipDropped = $state(false);
  let sending = $state(false);

  const ctx = $derived(contextInput && !chipDropped ? agentContext(contextInput) : null);
  const session = $derived($operatorSession);
  const working = $derived(session?.claude_status === 'working');
  const stuck = $derived(session?.stuck_kind ?? null);
  const blocked = $derived(
    $operatorState !== 'ready' && $operatorState !== 'waking' && $operatorState !== 'unknown'
      ? blockedCopy($operatorState as OperatorBlocked)
      : null,
  );

  async function send() {
    if (!session || working || sending || !draft.trim()) return;
    sending = true;
    const body = ctx ? `${ctx.prefix}\n\n${draft}` : draft;
    await sendPrompt(session.host_alias, session.tmux_name, body);
    draft = '';
    sending = false;
    void refreshOperator();
  }
</script>

{#if $agentPanelOpen}
  <section class="agent-panel" aria-label="Agent">
    {#if blocked}
      <p class="blocked">{blocked.title}</p>
      {#if blocked.action}
        <button onclick={() => void refreshOperator()}>{blocked.action}</button>
      {/if}
    {:else}
      {#if session}
        <ConversationPanel sessionId={session.id} compact={true} />
      {/if}
      {#if stuck}
        <p class="stuck">{stuckKindLabel(stuck)}</p>
      {/if}
      {#if ctx}
        <button
          class="chip"
          data-testid="agent-context-chip"
          onclick={() => (chipDropped = true)}
          title="Send without this context"
        >
          {ctx.chipLabel} ✕
        </button>
      {/if}
      <div class="composer">
        <textarea bind:value={draft} placeholder="Ask the agent…" rows="2"></textarea>
        <button onclick={() => void send()} disabled={working || sending}>Send</button>
      </div>
      {#if working}
        <p class="busy">The agent is working — wait for it to finish.</p>
      {/if}
    {/if}
  </section>
{/if}
```

Match `ConversationPanel`'s real prop names; if it has no `compact` prop, drop it rather than adding one in this task. Match `sendPrompt`'s real signature in `src/lib/sessions.ts`.

- [ ] **Step 4: Run to verify it passes**

```bash
npx vitest run src/lib/AgentPanel.test.ts
```

Expected: PASS, four tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/AgentPanel.svelte src/lib/AgentPanel.test.ts
git commit -m "feat(ui): the agent panel"
```

---

### Task 14: Wire it into the app

**Files:**
- Modify: `src/App.svelte:523-530` (mount both components), plus its chord handler
- Modify: `src/lib/hints.ts` (one `HintId`)
- Test: `src/lib/hints.test.ts`

**Interfaces:**
- Consumes: everything above.
- Produces: nothing new.

- [ ] **Step 1: Write the failing hint test**

In `src/lib/hints.test.ts`, extend whichever test enumerates `HINTS`:

```ts
  it('has a hint for the agent FAB, or nobody finds the button', () => {
    const h = HINTS.find((x) => x.id === 'agent-fab');
    expect(h).toBeTruthy();
    expect(h!.text.length).toBeGreaterThan(0);
  });
```

- [ ] **Step 2: Run to verify it fails**

```bash
npx vitest run src/lib/hints.test.ts
```

Expected: FAIL — no hint with id `agent-fab`.

- [ ] **Step 3: Add the hint**

In `src/lib/hints.ts`, add `| 'agent-fab'` to `HintId` and a `HintDef` at the end of `HINTS` (last, so it never displaces an onboarding hint):

```ts
  {
    id: 'agent-fab',
    text: 'Ask the agent to drive the fleet — it can do anything this app can.',
    placement: 'left',
  },
```

- [ ] **Step 4: Mount the components and route the chord**

In `src/App.svelte`, beside the existing global overlays:

```svelte
<AgentFab />
<AgentPanel contextInput={agentContextInput} />
```

with the import beside `McpConfirmDialog`'s, and `agentContextInput` derived from the state App already holds:

```ts
  const agentContextInput = $derived({
    view: hostsViewOpen ? ('hosts' as const) : ('terminal' as const),
    session: selectedSession,
    hostAlias: selectedSession?.host_alias ?? selectedHost,
    branch: selectedBranch,
  });
```

using whatever those variables are actually called in `App.svelte`. In the existing `appChord` handler, add the `'agent'` case:

```ts
      case 'agent':
        void openAgent();
        break;
```

- [ ] **Step 5: Run the full frontend suite**

```bash
pnpm install --frozen-lockfile
npx vitest run
npx svelte-check --tsconfig ./tsconfig.json
```

Expected: PASS, no new type errors.

- [ ] **Step 6: Run everything, unfiltered**

```bash
scripts/ci-local.sh
```

Expected: green. Do not pipe it through `| tail` — read the whole output.

- [ ] **Step 7: Commit**

```bash
git add src/App.svelte src/lib/hints.ts src/lib/hints.test.ts
git commit -m "feat(ui): mount the agent FAB and panel over every view"
```

---

## Manual verification (once, after Task 14)

Automated tests cover the guard rails; they cannot tell you the agent is actually born or that the dialog really appears.

1. Start the app with a fresh profile. The FAB is bottom-right over the terminal.
2. Press it. The agent wakes; a session named `fleet-operator` appears in the sidebar.
3. Ask it to list the sessions. It answers from `list_sessions`.
4. Ask it to kill a throwaway session. `McpConfirmDialog` appears. Approve it. The session disappears **from the sidebar on its own** — that repaint, not the agent's reply, is the thing to verify.
5. Ask it to kill `fleet-operator`. It must be refused with `E_FORBIDDEN`.
6. Turn the control API off in Settings, reopen the panel: the FAB explains rather than opening a useless chat.

## Self-review notes

- **Spec coverage.** Every section of the spec maps to a task: *Where the agent lives* → 2, 3, 5; *Components* → 2–14; *Data flow* → 5, 11, 13; *Error handling* → 1, 3, 4, 6, 11, 13; *Testing* → every task's test step; *The phone and voice* → nothing, deliberately (slice 2); *Open question for slice 2* → nothing, deliberately.
- **Two spec claims cannot be met as written and were corrected in the spec before this plan was drafted** (commit `7a3858b`): the "no new MCP tools" claim, and ⌘J. A third, the `CONFIRM_TTL` fix, was replaced by the deadline-absorbs-the-window approach in Task 1.
- **Known soft spots** an implementer will have to resolve against the real code, flagged inline rather than papered over: the test fixture in Task 5, `Store::find_session`'s real name, `ConversationPanel`'s prop names, `sendPrompt`'s signature, the CSS custom-property names, and the `tests_routing.rs` closure arity for a command that needs SSH.
