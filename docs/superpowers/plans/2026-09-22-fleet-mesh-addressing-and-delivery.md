# Fleet mesh — addressing and delivery: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every fleet endpoint (session, paired client, hub) a durable identity and one string address, and deliver messages to a running Claude session through the hook response instead of a simulated keystroke.

**Architecture:** Migration 043 adds a `participants` table holding the durable identity, plus `delivered_at` and participant columns on `session_messages`. A string address (`<fleet>/session/<host>/<name>`) is a *resolution key* parsed by one pure function, never a stored foreign key — a session move changes both host alias and row id. Delivery rides the response body of the `UserPromptSubmit` and `Stop` HTTP hooks, which are already installed on every provisioned host; the phase-1 paste primitive survives only as the wake-up for an idle session.

**Tech Stack:** Rust (`crates/fleet-core`), SQLite via `rusqlite` behind `std::sync::Mutex`, axum for `/hook`, `tokio` for the wait primitives, `uuid` v4 (already a dependency).

**Spec:** `docs/superpowers/specs/2026-09-22-fleet-mesh-addressing-and-delivery-design.md`

## Global Constraints

- **`additionalContext` budget: 8000 characters / 200 lines.** `reason`: 2000 / 20. Measured from Claude Code 2.1.278; truncation is silent apart from a `report.truncated` note.
- **`STOP_BLOCK_STREAK_MAX = 3`** consecutive `Stop` blocks per session; one message may block at most once.
- **`REDELIVER_AFTER_TURNS = 2`** — a message with `delivered_at` set and `read_at` null is re-delivered exactly once.
- **Tombstoned participants' undelivered messages survive 7 days** after `retired_at`, then the `service/gc.rs` pass sweeps them.
- **`UserPromptSubmit` and `Stop` hooks must stay synchronous** and keep `HOOK_TIMEOUT_SECS = 5`. Phase 2b item 8 proposes `async: true` and 1–2 s for hooks; these two are the exception, because an `async` hook's response body is discarded.
- **The hook handler must never do remote work** — one indexed `SELECT` and return. No SSH, no hub round-trip.
- **Shell quoting** has one implementation: `crate::shell::quote` (alias `shq`). Never reintroduce a second.
- **Never hold the `Store` mutex guard across an `.await`.**
- **Tool-description budget:** `the_served_definition_budget_stays_bounded` holds 59,400 bytes. Fit inside it, or raise the constant with a written reason in the test's comment history. Never a silent bump.
- **Codegen is CI-enforced:** `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` after any tool/command change; `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` after any `verdicts.rs` row.
- **A new wire field without `serde(default)` is a shipped outage against an older hub.** Every new field on a hub-routed argument ships with a default and a regenerated contract golden.
- **Export `CARGO_TARGET_DIR` before running scripts.** In a worktree `cargo` is a shell function forcing a per-root target dir, but scripts bypass it and inherit the main checkout's shared target, so `ci-local.sh` here can compile against another worktree's crates.
- **Judge test output unpiped.** Never `| tail`. Run the full suite per task, not a filtered subset.

## Deviation from the spec, decided while mapping files

The spec says `wait_for_reply` is event-driven by subscribing to the event bus. **Only the hub builds a subscribable bus** — `BroadcastEventBus::subscribe()` exists but the desktop opens its store with the bus that forwards to Svelte and passes no subscribe handle (`mcp/events_route.rs` documents exactly this and answers `503` there). A bus-subscribing waiter would therefore work on the hub and silently degrade on the desktop.

Instead: a `tokio::sync::Notify` lives on the `Store` and is signalled after a message insert commits. It is independent of which bus the embedder built, needs no new `RowChange` variant (so no contract golden bump and no `events.ts` allowlist entry), and works in every embedding. Same outcome — event-driven wake, not a poll — via a mechanism that actually exists everywhere. Task 10.

`wait_for_reply` also does **not** need a new event variant for a second reason: `insert_message` already writes a `message_received` timeline event, so a future bus-driven consumer has a signal without schema work.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/043_participants.sql` (create) | `participants`, `session_messages.{from,to}_participant_id`, `delivered_at`, `sessions.stop_block_streak` |
| `crates/fleet-core/src/store/schema.rs` (modify) | register migration 043, its `already_applied` guard, `EXPECTED_TABLES` |
| `crates/fleet-core/src/store/participants.rs` (create) | participant CRUD, re-point, retire, tombstone sweep |
| `crates/fleet-core/src/store/mod.rs` (modify) | `mod participants`, `ParticipantRow` re-export, the `Notify` |
| `crates/fleet-core/src/store/timeline.rs` (modify) | undelivered query, `mark_messages_delivered`, participant-keyed inbox |
| `crates/fleet-core/src/service/address.rs` (create) | the string address: pure parse/render, then store resolution |
| `crates/fleet-core/src/service/delivery.rs` (create) | pure `additionalContext` packer and the `Stop` decision |
| `crates/fleet-core/src/service/hooks.rs` (modify) | the delivery lookup a hook performs |
| `crates/fleet-core/src/mcp/hooks.rs` (modify) | the response body shape; 200-with-body vs 204 |
| `crates/fleet-core/src/service/messages.rs` (modify) | address-addressed send, wake-up, `wait_for_reply` |
| `crates/fleet-core/src/service/move_session/finalise.rs` (modify) | re-point the participant on a move |
| `crates/fleet-core/src/service/gc.rs` (modify) | sweep tombstoned participants after 7 days |
| `crates/fleet-core/src/ipc_error.rs` (modify) | `E_PARTICIPANT_UNKNOWN`, `E_PARTICIPANT_RETIRED` |
| `crates/fleet-core/src/mcp/tools/messaging.rs`, `params.rs` (modify) | the tool surface |
| `src-tauri/src/backend/verdicts.rs`, `tests_routing.rs` (modify) | hub-client parity rows |

Deliberate ordering: every pure function (Tasks 3, 5) lands before its consumer, and no two tasks write the same file, so implementers never serialise behind one another.

---

### Task 0: The spike — does an HTTP hook's response body reach the model?

The load-bearing assumption of the whole delivery path. Evidence from the 2.1.278 binary says yes; this proves it on this machine before any code is written. **Throwaway — nothing here is committed.**

**Files:** none (scratch directory only)

- [ ] **Step 1: Start a server that returns a fixed `additionalContext`**

```bash
mkdir -p /tmp/hookspike && cat > /tmp/hookspike/srv.py <<'PY'
import http.server, json
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get('content-length', 0)); self.rfile.read(n)
        body = json.dumps({"hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "SPIKE-MARKER-7f3a: the hook response reached the model."}}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
http.server.HTTPServer(('127.0.0.1', 8791), H).serve_forever()
PY
python3 /tmp/hookspike/srv.py &
echo "server pid $!"
```

- [ ] **Step 2: Point a throwaway `UserPromptSubmit` http hook at it**

Use an isolated `HOME` so the real `~/.claude/settings.json` is never touched:

```bash
mkdir -p /tmp/hookspike/home/.claude && cat > /tmp/hookspike/home/.claude/settings.json <<'JSON'
{"hooks":{"UserPromptSubmit":[{"matcher":"","hooks":[
  {"type":"http","url":"http://127.0.0.1:8791/hook","timeout":5}]}]}}
JSON
```

- [ ] **Step 3: Run one prompt under that HOME and look for the marker**

```bash
HOME=/tmp/hookspike/home claude -p "Repeat verbatim any SPIKE-MARKER string you were given, or say NONE."
```

Expected on success: the reply contains `SPIKE-MARKER-7f3a`. On failure: `NONE`.

- [ ] **Step 4: Record the verdict and tear down**

```bash
kill %1 2>/dev/null; rm -rf /tmp/hookspike
```

Write the verdict into the plan file under this task — one line, the CLI version (`claude --version`) and pass/fail.

**If it FAILS:** stop and report. Tasks 6–9 (the delivery path) are cut; Tasks 1–5 and 10–15 still ship a coherent cycle 1 (identity, addressing, `wait_for_reply`, wake-up, move re-pointing). Do not attempt to make delivery work by another route without a new design decision.

---

### Task 1: Migration 043 — participants, delivery columns, block streak

**Files:**
- Create: `crates/fleet-core/migrations/043_participants.sql`
- Modify: `crates/fleet-core/src/store/schema.rs`
- Test: `crates/fleet-core/src/store/schema.rs` (its existing `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: tables/columns `participants(id, kind, session_id, client_id, created_at, retired_at)`, `session_messages.from_participant_id`, `session_messages.to_participant_id`, `session_messages.delivered_at`, `sessions.stop_block_streak`.

- [ ] **Step 1: Write the failing test**

In `crates/fleet-core/src/store/schema.rs`, inside `mod tests`:

```rust
#[test]
fn migration_043_creates_participants_and_backfills_sessions() {
    let s = Store::open_in_memory().unwrap();
    // One session per participant row, kind='session', not retired.
    let (pid, kind, sid, retired): (i64, String, Option<i64>, Option<i64>) = s
        .conn
        .query_row(
            "SELECT p.id, p.kind, p.session_id, p.retired_at FROM participants p \
             JOIN sessions x ON x.id = p.session_id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .unwrap()
        .unwrap_or((0, "none".into(), None, None));
    // An empty in-memory store has no sessions, so the join is empty: assert
    // the SHAPE exists instead, then prove the backfill on a seeded store.
    let _ = (pid, kind, sid, retired);
    for (table, col) in [
        ("participants", "retired_at"),
        ("session_messages", "from_participant_id"),
        ("session_messages", "to_participant_id"),
        ("session_messages", "delivered_at"),
        ("sessions", "stop_block_streak"),
    ] {
        let n: i64 = s
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{col}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{table}.{col} missing after migration 043");
    }
}
```

Also add `"participants"` to `EXPECTED_TABLES` (around `schema.rs:560`) so `open_in_memory_creates_all_tables` covers it.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/ft-$(basename "$PWD")}"
cargo test -p fleet-core --lib store::schema
```

Expected: FAIL — `participants.retired_at missing after migration 043`, and `open_in_memory_creates_all_tables` fails on the missing table.

- [ ] **Step 3: Write the migration**

`crates/fleet-core/migrations/043_participants.sql`:

```sql
-- Every addressable endpoint in the fleet gets a durable identity here, so an
-- address can be resolved to something that survives a session move. A move
-- creates a NEW sessions row on the target and kills the source
-- (service/move_session/finalise.rs), so a message pointed at a session id
-- would be orphaned (and, before this migration, DELETEd outright by
-- store/sessions.rs delete_session). Messages point at a participant; a move
-- re-points the participant.
--
-- kind: 'session' (session_id set), 'client' (client_id -> client_tokens),
--       'hub' (both NULL; one row per fleet).
-- retired_at: tombstone. A retired participant resolves, and says it is gone,
--       instead of vanishing and leaving the sender with a silent timeout.
CREATE TABLE IF NOT EXISTS participants (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  session_id INTEGER,
  client_id INTEGER,
  created_at INTEGER NOT NULL,
  retired_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_participants_session
  ON participants(session_id) WHERE session_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_participants_client
  ON participants(client_id) WHERE client_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_participants_retired
  ON participants(retired_at) WHERE retired_at IS NOT NULL;

-- One participant per existing session, so no message is left unaddressable.
INSERT INTO participants (kind, session_id, created_at)
  SELECT 'session', id, strftime('%s','now') FROM sessions
  WHERE id NOT IN (SELECT session_id FROM participants WHERE session_id IS NOT NULL);

ALTER TABLE session_messages ADD COLUMN from_participant_id INTEGER;
ALTER TABLE session_messages ADD COLUMN to_participant_id INTEGER;
-- Handed to a hook response. Distinct from read_at: we know we wrote the body,
-- never that Claude ingested it (the turn may have been interrupted).
ALTER TABLE session_messages ADD COLUMN delivered_at INTEGER;

UPDATE session_messages SET
  from_participant_id = (SELECT id FROM participants WHERE session_id = from_session_id),
  to_participant_id   = (SELECT id FROM participants WHERE session_id = to_session_id);

CREATE INDEX IF NOT EXISTS idx_session_messages_undelivered
  ON session_messages(to_participant_id, delivered_at, sent_at);

-- Consecutive Stop-block count, so a remote sender can never shut a session
-- inside a never-ending turn (STOP_BLOCK_STREAK_MAX).
ALTER TABLE sessions ADD COLUMN stop_block_streak INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (43);
```

- [ ] **Step 4: Register it in `schema.rs`**

Add the guard beside `sessions_has_row_version` (the `ALTER TABLE ... ADD COLUMN` statements are not idempotent):

```rust
/// `already_applied` guard of migration 043: `session_messages` already has
/// its `to_participant_id` column, and `ALTER TABLE ... ADD COLUMN` would
/// fail again. See [`Migration`].
fn messages_have_participant_columns(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('session_messages') \
         WHERE name = 'to_participant_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

And append to `MIGRATIONS`, after the version-42 entry:

```rust
    // `participants` (CREATE TABLE IF NOT EXISTS, re-runnable) plus four
    // ADD COLUMNs, which are not — so the same guard shape as 038-042.
    Migration {
        version: 43,
        sql: include_str!("../../migrations/043_participants.sql"),
        already_applied: Some(messages_have_participant_columns),
    },
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib store::
```

Expected: PASS, including `open_in_memory_creates_all_tables` and the re-migration tests that roll the version back (they re-run every later migration, which is what the guard is for).

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/migrations/043_participants.sql crates/fleet-core/src/store/schema.rs
git commit -m "feat(store): migration 043 — participants, delivery columns, block streak"
```

---

### Task 2: `store/participants.rs` — identity CRUD, re-point, retire

**Files:**
- Create: `crates/fleet-core/src/store/participants.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (add `mod participants;` and re-export `ParticipantRow`)
- Test: `crates/fleet-core/src/store/participants.rs` (`mod tests` at the bottom, matching the pattern in `store/timeline.rs`)

**Interfaces:**
- Consumes: migration 043 from Task 1.
- Produces, on `Store`:
  - `pub fn ensure_participant_for_session(&self, session_id: i64) -> Result<i64, IpcError>`
  - `pub fn participant_by_id(&self, id: i64) -> Result<Option<ParticipantRow>, IpcError>`
  - `pub fn participant_for_session(&self, session_id: i64) -> Result<Option<ParticipantRow>, IpcError>`
  - `pub fn repoint_participant(&self, participant_id: i64, new_session_id: i64) -> Result<(), IpcError>`
  - `pub fn retire_participant(&self, participant_id: i64) -> Result<(), IpcError>`
  - `pub struct ParticipantRow { pub id: i64, pub kind: String, pub session_id: Option<i64>, pub client_id: Option<i64>, pub created_at: i64, pub retired_at: Option<i64> }`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_session_for_test(name, "local")
    }

    #[test]
    fn ensure_is_idempotent_and_returns_the_same_id() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "alpha");
        let a = s.ensure_participant_for_session(sid).unwrap();
        let b = s.ensure_participant_for_session(sid).unwrap();
        assert_eq!(a, b, "a second ensure must not create a second identity");
        let row = s.participant_by_id(a).unwrap().unwrap();
        assert_eq!(row.kind, "session");
        assert_eq!(row.session_id, Some(sid));
        assert_eq!(row.retired_at, None);
    }

    #[test]
    fn repoint_moves_the_identity_to_a_new_session_row() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        let p = s.ensure_participant_for_session(src).unwrap();
        s.repoint_participant(p, dst).unwrap();
        assert_eq!(s.participant_by_id(p).unwrap().unwrap().session_id, Some(dst));
        // The identity did not fork: the old session no longer owns one.
        assert!(s.participant_for_session(src).unwrap().is_none());
        assert_eq!(s.participant_for_session(dst).unwrap().unwrap().id, p);
    }

    #[test]
    fn retire_tombstones_rather_than_deletes() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "gone");
        let p = s.ensure_participant_for_session(sid).unwrap();
        s.retire_participant(p).unwrap();
        let row = s.participant_by_id(p).unwrap().expect("row still resolves");
        assert!(row.retired_at.is_some(), "retired participants must still resolve");
    }

    #[test]
    fn repointing_a_retired_participant_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "a");
        let b = seed(&s, "b");
        let p = s.ensure_participant_for_session(a).unwrap();
        s.retire_participant(p).unwrap();
        let err = s.repoint_participant(p, b).unwrap_err();
        assert_eq!(err.code, "E_PARTICIPANT_RETIRED");
    }
}
```

`upsert_session_for_test` is the existing test seeder used by `store/timeline.rs` tests; reuse it rather than writing raw INSERTs. If its name differs in this checkout, grep `store/timeline.rs mod tests` for the helper it uses and use that one.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib store::participants
```

Expected: FAIL to compile — `no method named ensure_participant_for_session`.

- [ ] **Step 3: Write the implementation**

`crates/fleet-core/src/store/participants.rs`:

```rust
//! Durable identity for every addressable endpoint (migration 043).
//!
//! A `sessions` row is not a stable identity: `move_session` creates a new row
//! on the target host and kills the source, so anything pointing at a session
//! id is orphaned by a move. A participant survives it — the move re-points
//! the participant instead (`service/move_session/finalise.rs`).

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// One addressable endpoint. `kind` is `session` | `client` | `hub`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParticipantRow {
    pub id: i64,
    pub kind: String,
    #[serde(default)]
    pub session_id: Option<i64>,
    #[serde(default)]
    pub client_id: Option<i64>,
    pub created_at: i64,
    /// Tombstone. A retired participant still resolves, and reports that it is
    /// gone, so a sender learns instead of timing out.
    #[serde(default)]
    pub retired_at: Option<i64>,
}

const COLUMNS: &str = "id, kind, session_id, client_id, created_at, retired_at";

fn map(row: &rusqlite::Row<'_>) -> rusqlite::Result<ParticipantRow> {
    Ok(ParticipantRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        session_id: row.get(2)?,
        client_id: row.get(3)?,
        created_at: row.get(4)?,
        retired_at: row.get(5)?,
    })
}

impl Store {
    /// The participant id for `session_id`, creating it when absent. The
    /// unique partial index on `participants(session_id)` makes a concurrent
    /// double-create impossible, so this never forks an identity.
    pub fn ensure_participant_for_session(&self, session_id: i64) -> Result<i64, IpcError> {
        if let Some(p) = self.participant_for_session(session_id)? {
            return Ok(p.id);
        }
        self.conn.execute(
            "INSERT INTO participants (kind, session_id, created_at) VALUES ('session', ?1, ?2)",
            rusqlite::params![session_id, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn participant_by_id(&self, id: i64) -> Result<Option<ParticipantRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM participants WHERE id = ?1"),
                rusqlite::params![id],
                map,
            )
            .optional()
            .map_err(IpcError::from)
    }

    pub fn participant_for_session(
        &self,
        session_id: i64,
    ) -> Result<Option<ParticipantRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM participants WHERE session_id = ?1"),
                rusqlite::params![session_id],
                map,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Follow a moved session: the identity (and therefore every message
    /// addressed to it) now points at `new_session_id`. Refused for a retired
    /// participant — a tombstone never comes back to life.
    pub fn repoint_participant(
        &self,
        participant_id: i64,
        new_session_id: i64,
    ) -> Result<(), IpcError> {
        let row = self.participant_by_id(participant_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_PARTICIPANT_UNKNOWN,
                format!("participant {participant_id} not found"),
            )
        })?;
        if row.retired_at.is_some() {
            return Err(IpcError::new(
                codes::E_PARTICIPANT_RETIRED,
                format!("participant {participant_id} is retired"),
            ));
        }
        self.conn.execute(
            "UPDATE participants SET session_id = ?1 WHERE id = ?2",
            rusqlite::params![new_session_id, participant_id],
        )?;
        Ok(())
    }

    /// Tombstone, never delete: undelivered messages stay addressable for the
    /// GC window and the sender gets `message_undeliverable` rather than
    /// silence.
    pub fn retire_participant(&self, participant_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE participants SET retired_at = ?1, session_id = NULL \
             WHERE id = ?2 AND retired_at IS NULL",
            rusqlite::params![now_unix(), participant_id],
        )?;
        Ok(())
    }
}
```

In `crates/fleet-core/src/store/mod.rs` add `mod participants;` beside the other `mod` lines and `pub use participants::ParticipantRow;` beside the other re-exports.

- [ ] **Step 4: Add the two error codes**

In `crates/fleet-core/src/ipc_error.rs`, beside `E_SELF_TARGET`:

```rust
    /// An address parsed, but names no participant in this fleet.
    pub const E_PARTICIPANT_UNKNOWN: &str = "E_PARTICIPANT_UNKNOWN";
    /// The participant resolved, but is tombstoned — the endpoint is gone.
    pub const E_PARTICIPANT_RETIRED: &str = "E_PARTICIPANT_RETIRED";
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib store::participants
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: 4 passed, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/store/participants.rs crates/fleet-core/src/store/mod.rs crates/fleet-core/src/ipc_error.rs
git commit -m "feat(store): participant identity with re-point and tombstone"
```

---

### Task 3: `service/address.rs` — the string address, pure parse and render

**Files:**
- Create: `crates/fleet-core/src/service/address.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (add `pub mod address;`)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: nothing (pure).
- Produces:
  - `pub enum Addr { Session { fleet: Option<String>, host: String, name: String }, Client { fleet: Option<String>, name: String }, Hub { fleet: Option<String> } }`
  - `pub fn parse(s: &str) -> Result<Addr, IpcError>`
  - `pub fn render(a: &Addr) -> String`
  - `pub fn is_foreign(a: &Addr, local_fleet: &str) -> bool`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_shapes_with_an_implicit_local_fleet() {
        assert_eq!(
            parse("/session/mac/dev-foo").unwrap(),
            Addr::Session { fleet: None, host: "mac".into(), name: "dev-foo".into() }
        );
        assert_eq!(
            parse("/client/phone").unwrap(),
            Addr::Client { fleet: None, name: "phone".into() }
        );
        assert_eq!(parse("/hub").unwrap(), Addr::Hub { fleet: None });
    }

    #[test]
    fn parses_an_explicit_fleet() {
        assert_eq!(
            parse("f00d/session/mefistos/api").unwrap(),
            Addr::Session { fleet: Some("f00d".into()), host: "mefistos".into(), name: "api".into() }
        );
    }

    #[test]
    fn render_round_trips_every_shape() {
        for s in ["/session/mac/dev-foo", "/client/phone", "/hub", "f00d/session/m/a", "f00d/hub"] {
            assert_eq!(render(&parse(s).unwrap()), s, "round trip failed for {s}");
        }
    }

    #[test]
    fn malformed_addresses_are_e_validate_and_never_panic() {
        for bad in [
            "", "/", "session/mac/x", "/session/mac", "/session//x", "/session/mac/x/y",
            "/client", "/client/a/b", "/hub/x", "/nope/a", "/session/mac/../x",
            "/session/ma c/x", "/session/mac/na\nme", "a/b/c/d/e",
        ] {
            let err = parse(bad).unwrap_err();
            assert_eq!(err.code, "E_VALIDATE", "expected E_VALIDATE for {bad:?}");
        }
    }

    #[test]
    fn a_host_alias_and_tmux_name_are_validated_by_the_canonical_validators() {
        // Anything host_alias/tmux_name_addressable rejects must not parse:
        // the address is the only new way to name a session, so it may not be
        // a way around those checks.
        assert_eq!(parse("/session/-bad/x").unwrap_err().code, "E_VALIDATE");
        assert_eq!(parse("/session/mac/-x").unwrap_err().code, "E_VALIDATE");
    }

    #[test]
    fn is_foreign_only_when_an_explicit_fleet_differs() {
        let local = parse("/session/mac/x").unwrap();
        let same = parse("abc/session/mac/x").unwrap();
        let other = parse("zzz/session/mac/x").unwrap();
        assert!(!is_foreign(&local, "abc"), "an implicit fleet is always local");
        assert!(!is_foreign(&same, "abc"));
        assert!(is_foreign(&other, "abc"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib service::address
```

Expected: FAIL to compile — `cannot find function parse`.

- [ ] **Step 3: Write the implementation**

`crates/fleet-core/src/service/address.rs`:

```rust
//! The fleet address: one string naming any addressable endpoint.
//!
//! ```text
//! <fleet>/session/<host_alias>/<tmux_name>
//! <fleet>/client/<client_name>
//! <fleet>/hub
//! ```
//!
//! An empty `<fleet>` means "this fleet", so every call written before
//! addresses existed keeps working and cycle 3 (hub↔hub) adds federation
//! without another schema change.
//!
//! This is a RESOLUTION KEY, never a stored foreign key: a session move
//! changes both the host alias and the row id, so the durable thing is the
//! participant (see `store/participants.rs`).
//!
//! The parser is the only new way to name a session, so it delegates to the
//! canonical validators rather than inventing looser rules of its own.

use crate::ipc_error::{codes, IpcError};

/// A parsed address. `fleet: None` means the local fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    Session {
        fleet: Option<String>,
        host: String,
        name: String,
    },
    Client {
        fleet: Option<String>,
        name: String,
    },
    Hub {
        fleet: Option<String>,
    },
}

fn bad(s: &str) -> IpcError {
    IpcError::new(
        codes::E_VALIDATE,
        format!(
            "malformed address {s:?}: expected <fleet>/session/<host>/<name>, \
             <fleet>/client/<name> or <fleet>/hub"
        ),
    )
}

/// Max length of the fleet segment: a UUID v4 without braces is 36 chars.
const FLEET_MAX: usize = 36;

fn fleet_segment(raw: &str, whole: &str) -> Result<Option<String>, IpcError> {
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > FLEET_MAX || !raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(bad(whole));
    }
    Ok(Some(raw.to_string()))
}

/// Parse an address. Every malformed input is `E_VALIDATE`; nothing panics.
pub fn parse(s: &str) -> Result<Addr, IpcError> {
    let parts: Vec<&str> = s.split('/').collect();
    match parts.as_slice() {
        [fleet, "hub"] => Ok(Addr::Hub {
            fleet: fleet_segment(fleet, s)?,
        }),
        [fleet, "client", name] => {
            let fleet = fleet_segment(fleet, s)?;
            crate::validate::not_blank("client name", name).map_err(|_| bad(s))?;
            crate::validate::friendly_name(name).map_err(|_| bad(s))?;
            Ok(Addr::Client {
                fleet,
                name: (*name).to_string(),
            })
        }
        [fleet, "session", host, name] => {
            let fleet = fleet_segment(fleet, s)?;
            crate::validate::host_alias_syntax(host).map_err(|_| bad(s))?;
            crate::validate::tmux_name_addressable(name).map_err(|_| bad(s))?;
            Ok(Addr::Session {
                fleet,
                host: (*host).to_string(),
                name: (*name).to_string(),
            })
        }
        _ => Err(bad(s)),
    }
}

/// The canonical string for an address. `parse` ∘ `render` is the identity.
pub fn render(a: &Addr) -> String {
    let f = |fleet: &Option<String>| fleet.clone().unwrap_or_default();
    match a {
        Addr::Hub { fleet } => format!("{}/hub", f(fleet)),
        Addr::Client { fleet, name } => format!("{}/client/{name}", f(fleet)),
        Addr::Session { fleet, host, name } => {
            format!("{}/session/{host}/{name}", f(fleet))
        }
    }
}

/// True when the address names a fleet that is explicitly not this one. An
/// implicit fleet is always local, so today's callers are never foreign.
pub fn is_foreign(a: &Addr, local_fleet: &str) -> bool {
    let fleet = match a {
        Addr::Hub { fleet } | Addr::Client { fleet, .. } | Addr::Session { fleet, .. } => fleet,
    };
    fleet.as_deref().is_some_and(|f| f != local_fleet)
}
```

Add `pub mod address;` to `crates/fleet-core/src/service/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::address
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: 6 passed. If `friendly_name` rejects a legitimate client name in this checkout, read `validate.rs:281` and pick the validator that matches what `pair_client` already accepts — do not loosen the check here.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/address.rs crates/fleet-core/src/service/mod.rs
git commit -m "feat(service): fleet address parse and render"
```

---

### Task 4: `fleet_id` — mint once, expose in `whoami`

**Files:**
- Modify: `crates/fleet-core/src/service/address.rs` (add `local_fleet_id`)
- Modify: `crates/fleet-core/src/mcp/tools/session_ops.rs:152` (`whoami`)
- Test: `crates/fleet-core/src/service/address.rs`

**Interfaces:**
- Consumes: `Store::get_setting` / `set_setting` (`store/mod.rs:180,191`), `uuid` v4.
- Produces: `pub fn local_fleet_id(store: &Mutex<Store>) -> Result<String, IpcError>`; `whoami` gains a `fleet_id` field.

Stored in `settings`, not a new column — no migration needed.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn fleet_id_is_minted_once_and_then_stable() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let a = local_fleet_id(&store).unwrap();
        let b = local_fleet_id(&store).unwrap();
        assert_eq!(a, b, "the fleet id must not be re-minted");
        assert_eq!(a.len(), 36, "a uuid v4 in hyphenated form");
        // It parses as the fleet segment of an address.
        assert!(!is_foreign(&parse(&format!("{a}/hub")).unwrap(), &a));
    }
```

This test needs `use crate::store::Store;` in the test module.

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p fleet-core --lib service::address::tests::fleet_id
```

Expected: FAIL to compile — `cannot find function local_fleet_id`.

- [ ] **Step 3: Implement it**

Append to `service/address.rs`:

```rust
/// Settings key holding this fleet's identity.
pub const FLEET_ID_KEY: &str = "fleet.id";

/// This fleet's id, minted on first call and stable thereafter. Kept in
/// `settings` rather than a column: it is one value per store, and a
/// migration for it would buy nothing.
pub fn local_fleet_id(store: &std::sync::Mutex<crate::store::Store>) -> Result<String, IpcError> {
    let s = crate::ipc_error::lock(store)?;
    if let Some(existing) = s.get_setting(FLEET_ID_KEY)? {
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    s.set_setting(FLEET_ID_KEY, &id)?;
    Ok(id)
}
```

Then in `mcp/tools/session_ops.rs`, inside `whoami`'s returned JSON object, add
`"fleet_id": crate::service::address::local_fleet_id(&self.store)?`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::address
cargo test -p fleet-core --lib mcp::tools
```

Expected: PASS. `mcp::tools::tests` includes the description-budget test; note its reported byte count now — the `whoami` schema did not change, so it should be unchanged.

- [ ] **Step 5: Regenerate the control API reference**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
cargo test -p fleet-core reference_is_current
```

Expected: the first run rewrites `docs/control-api-reference.md`, the second passes.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/address.rs crates/fleet-core/src/mcp/tools/session_ops.rs docs/control-api-reference.md
git commit -m "feat(service): mint a stable fleet id and report it from whoami"
```

---

### Task 5: `service/delivery.rs` — the pure `additionalContext` packer

**Files:**
- Create: `crates/fleet-core/src/service/delivery.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (add `pub mod delivery;`)
- Test: same file

**Interfaces:**
- Consumes: `crate::store::SessionMessage`.
- Produces:
  - `pub const CTX_MAX_CHARS: usize = 8000;`
  - `pub const CTX_MAX_LINES: usize = 200;`
  - `pub struct Packed { pub text: String, pub included: Vec<i64>, pub remaining: usize }`
  - `pub fn pack(messages: &[SessionMessage], sender_label: &dyn Fn(i64) -> String) -> Packed`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionMessage;

    fn msg(id: i64, body: &str) -> SessionMessage {
        SessionMessage {
            id,
            from_session_id: 1,
            to_session_id: 2,
            body: body.into(),
            kind: "message".into(),
            sent_at: 1_700_000_000 + id,
            read_at: None,
            reply_to: None,
        }
    }
    fn label(_: i64) -> String {
        "alpha@local".into()
    }

    #[test]
    fn empty_input_packs_to_nothing() {
        let p = pack(&[], &label);
        assert_eq!(p.text, "");
        assert!(p.included.is_empty());
        assert_eq!(p.remaining, 0);
    }

    #[test]
    fn a_short_batch_is_included_whole_and_names_each_sender() {
        let p = pack(&[msg(1, "first"), msg(2, "second")], &label);
        assert_eq!(p.included, vec![1, 2]);
        assert_eq!(p.remaining, 0);
        assert!(p.text.contains("first") && p.text.contains("second"));
        assert!(p.text.contains("alpha@local"), "the sender must be visible");
        assert!(p.text.contains("#1") && p.text.contains("#2"), "ids let the agent reply");
    }

    #[test]
    fn the_char_budget_includes_whole_messages_only_and_reports_the_rest() {
        let big = "x".repeat(CTX_MAX_CHARS / 2);
        let p = pack(&[msg(1, &big), msg(2, &big), msg(3, &big)], &label);
        assert!(p.text.chars().count() <= CTX_MAX_CHARS, "budget respected");
        assert!(!p.included.is_empty(), "at least one message gets through");
        assert!(p.included.len() < 3, "not all three can fit");
        assert_eq!(p.remaining, 3 - p.included.len());
        // No body was cut: every included id's full body is present.
        for id in &p.included {
            assert!(p.text.contains(&big), "message {id} was truncated");
        }
        assert!(p.text.contains(&format!("{} more", p.remaining)));
    }

    #[test]
    fn the_line_budget_is_enforced_independently_of_the_char_budget() {
        // 150 one-character lines: far under CTX_MAX_CHARS, over CTX_MAX_LINES
        // once two of them are packed together.
        let tall = "y\n".repeat(150);
        let p = pack(&[msg(1, &tall), msg(2, &tall)], &label);
        assert!(p.text.lines().count() <= CTX_MAX_LINES, "line budget respected");
        assert_eq!(p.included.len(), 1, "the second does not fit on lines");
        assert_eq!(p.remaining, 1);
    }

    #[test]
    fn a_single_message_over_budget_is_never_packed_and_is_reported() {
        let huge = "z".repeat(CTX_MAX_CHARS + 1);
        let p = pack(&[msg(1, &huge)], &label);
        assert!(p.included.is_empty(), "a body is never cut to fit");
        assert_eq!(p.remaining, 1);
        assert!(p.text.contains("1 more"), "the agent must still learn it exists");
    }

    #[test]
    fn multibyte_bodies_are_counted_in_chars_not_bytes() {
        // 3000 four-byte chars = 12000 bytes but only 3000 chars: two of these
        // fit the 8000-char budget, which a byte-based count would deny.
        let m = "🦀".repeat(3000);
        let p = pack(&[msg(1, &m), msg(2, &m)], &label);
        assert_eq!(p.included.len(), 2, "char budget, not byte budget");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib service::delivery
```

Expected: FAIL to compile — `cannot find function pack`.

- [ ] **Step 3: Write the implementation**

`crates/fleet-core/src/service/delivery.rs`:

```rust
//! Rendering pending messages into a hook response's `additionalContext`.
//!
//! Claude Code caps `additionalContext` at 8000 characters and 200 lines and
//! truncates silently past either. Truncating a message body mid-sentence is
//! worse than not sending it, so this packs WHOLE messages only and names how
//! many are left in the inbox.

use crate::store::SessionMessage;

/// Claude Code's `additionalContext` character cap (measured, 2.1.278).
pub const CTX_MAX_CHARS: usize = 8000;
/// Claude Code's `additionalContext` line cap (measured, 2.1.278).
pub const CTX_MAX_LINES: usize = 200;

/// Headroom left for the trailing "N more in the inbox" line, so adding it can
/// never push a packed batch over either budget.
const TAIL_RESERVE_CHARS: usize = 120;
const TAIL_RESERVE_LINES: usize = 2;

/// What one hook response will carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    /// The `additionalContext` value; empty when nothing was packed and
    /// nothing remains.
    pub text: String,
    /// Ids included in `text`, in the order they appear. These are the rows to
    /// stamp `delivered_at` on — and only these.
    pub included: Vec<i64>,
    /// How many of `messages` did not fit.
    pub remaining: usize,
}

/// PURE: render `messages` (oldest first) into one `additionalContext` value.
///
/// `sender_label` turns a sender's session id into something human — normally
/// `"<tmux_name>@<host_alias>"`. Taken as a closure so this stays pure and
/// testable without a store.
pub fn pack(messages: &[SessionMessage], sender_label: &dyn Fn(i64) -> String) -> Packed {
    let budget_chars = CTX_MAX_CHARS.saturating_sub(TAIL_RESERVE_CHARS);
    let budget_lines = CTX_MAX_LINES.saturating_sub(TAIL_RESERVE_LINES);

    let mut blocks: Vec<String> = Vec::new();
    let mut included: Vec<i64> = Vec::new();
    let mut chars = 0usize;
    let mut lines = 0usize;

    for m in messages {
        let block = format!(
            "[fleet msg #{id} from {who}]: {body}",
            id = m.id,
            who = sender_label(m.from_session_id),
            body = m.body
        );
        // +1 for the blank line joining blocks.
        let c = block.chars().count() + 1;
        let l = block.lines().count() + 1;
        if chars + c > budget_chars || lines + l > budget_lines {
            // Whole messages only: stop at the first one that does not fit
            // rather than skipping it, so delivery order is never scrambled.
            break;
        }
        chars += c;
        lines += l;
        included.push(m.id);
        blocks.push(block);
    }

    let remaining = messages.len() - included.len();
    if remaining > 0 {
        blocks.push(format!(
            "({remaining} more message(s) waiting — call the fleet `inbox` tool to read them)"
        ));
    }
    Packed {
        text: blocks.join("\n\n"),
        included,
        remaining,
    }
}
```

Add `pub mod delivery;` to `service/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::delivery
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/delivery.rs crates/fleet-core/src/service/mod.rs
git commit -m "feat(service): pack pending messages into a hook additionalContext"
```

---

### Task 6: Store — undelivered query and `delivered_at` stamping

**Files:**
- Modify: `crates/fleet-core/src/store/timeline.rs`
- Test: same file, its existing `mod tests`

**Interfaces:**
- Consumes: migration 043.
- Produces, on `Store`:
  - `pub fn list_undelivered_for_session(&self, session_id: i64, limit: i64) -> Result<Vec<SessionMessage>, IpcError>` — oldest first, `delivered_at IS NULL` or (re-delivery) `read_at IS NULL` and `delivered_at` older than the current turn minus `REDELIVER_AFTER_TURNS`.
  - `pub fn mark_messages_delivered(&self, ids: &[i64]) -> Result<usize, IpcError>`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn undelivered_is_oldest_first_and_excludes_delivered_rows() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.ensure_participant_for_session(b).unwrap();
        let m1 = s.insert_message(a, b, "one", "message", None).unwrap();
        let m2 = s.insert_message(a, b, "two", "message", None).unwrap();

        let got = s.list_undelivered_for_session(b, 10).unwrap();
        assert_eq!(
            got.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![m1, m2],
            "delivery order is oldest first, unlike the newest-first inbox"
        );

        assert_eq!(s.mark_messages_delivered(&[m1]).unwrap(), 1);
        let got = s.list_undelivered_for_session(b, 10).unwrap();
        assert_eq!(got.iter().map(|m| m.id).collect::<Vec<_>>(), vec![m2]);
    }

    #[test]
    fn marking_delivered_does_not_mark_read() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.ensure_participant_for_session(b).unwrap();
        let m = s.insert_message(a, b, "hi", "message", None).unwrap();
        s.mark_messages_delivered(&[m]).unwrap();
        let row = s.get_message(m).unwrap().unwrap();
        assert_eq!(row.read_at, None, "delivered is not read");
        assert_eq!(
            s.list_inbox(b, true, 10).unwrap().len(),
            1,
            "a delivered message is still unread in the inbox"
        );
    }

    #[test]
    fn mark_delivered_is_idempotent() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.ensure_participant_for_session(b).unwrap();
        let m = s.insert_message(a, b, "hi", "message", None).unwrap();
        assert_eq!(s.mark_messages_delivered(&[m]).unwrap(), 1);
        assert_eq!(
            s.mark_messages_delivered(&[m]).unwrap(),
            0,
            "a second stamp changes nothing"
        );
    }

    #[test]
    fn insert_message_fills_the_participant_columns() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "hi", "message", None).unwrap();
        let (from_p, to_p): (Option<i64>, Option<i64>) = s
            .conn
            .query_row(
                "SELECT from_participant_id, to_participant_id FROM session_messages WHERE id=?1",
                rusqlite::params![m],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(from_p, Some(s.participant_for_session(a).unwrap().unwrap().id));
        assert_eq!(to_p, Some(s.participant_for_session(b).unwrap().unwrap().id));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib store::timeline
```

Expected: FAIL — `no method named list_undelivered_for_session`, and `insert_message_fills_the_participant_columns` fails with `Some(..) != None`.

- [ ] **Step 3: Implement — teach `insert_message` about participants**

In `store/timeline.rs`, replace the body of `insert_message` (currently at `:160-174`) so it resolves both participants first and writes all four columns. Keep the existing signature: callers pass session ids, and the participant is derived, so nothing upstream changes yet.

```rust
        let at = now_unix();
        let from_p = self.ensure_participant_for_session(from_session_id)?;
        let to_p = self.ensure_participant_for_session(to_session_id)?;
        self.conn.execute(
            "INSERT INTO session_messages \
               (from_session_id, to_session_id, from_participant_id, to_participant_id, \
                body, kind, sent_at, reply_to) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                from_session_id,
                to_session_id,
                from_p,
                to_p,
                body,
                kind,
                at,
                reply_to
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
```

- [ ] **Step 4: Implement the two new reads**

Append inside the same `impl Store` block in `store/timeline.rs`:

```rust
    /// Messages waiting to be handed to `session_id`'s next hook response,
    /// OLDEST first — delivery replays conversation order, where the inbox
    /// shows the newest first.
    ///
    /// Resolved through the participant, not `to_session_id`, so a message
    /// addressed before a `move_session` still reaches the moved session.
    pub fn list_undelivered_for_session(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionMessage>, crate::ipc_error::IpcError> {
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM session_messages \
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?1) \
               AND delivered_at IS NULL \
             ORDER BY sent_at ASC, id ASC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![session_id, limit], map_message_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Stamp `delivered_at` on rows that do not have it yet. "Handed to a hook
    /// response", never "the model read it" — there is no ack. Returns how
    /// many rows flipped, so a second call reports 0.
    pub fn mark_messages_delivered(
        &self,
        ids: &[i64],
    ) -> Result<usize, crate::ipc_error::IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let at = now_unix();
        let sql = format!(
            "UPDATE session_messages SET delivered_at = ?1 \
             WHERE delivered_at IS NULL AND id IN ({phs})",
            phs = in_clause(ids.len())
        );
        let params = params_then(rusqlite::params![at], ids);
        Ok(self.conn.execute(&sql, params.as_slice())?)
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib store::
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: PASS, including the pre-existing `session_messages_inbox_roundtrip_and_mark_read`.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/store/timeline.rs
git commit -m "feat(store): undelivered-message query and delivered_at stamping"
```

---

### Task 7: `service/hooks.rs` — the delivery lookup

**Files:**
- Modify: `crates/fleet-core/src/service/hooks.rs`
- Test: same file, its existing `mod tests`

**Interfaces:**
- Consumes: Task 5 `service::delivery::{pack, Packed}`, Task 6 `Store::{list_undelivered_for_session, mark_messages_delivered}`.
- Produces: `pub fn take_pending_delivery(store: &Arc<Mutex<Store>>, payload: &HookPayload, ctx: &HookContext) -> Option<Packed>` — resolves the row the same way `apply_hook` does, packs, stamps, and returns. `None` when there is nothing to deliver.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn take_pending_delivery_packs_stamps_and_then_returns_none() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (a, b) = {
            let s = store.lock().unwrap();
            (seed(&s, "alpha"), seed(&s, "beta"))
        };
        {
            let s = store.lock().unwrap();
            s.insert_message(a, b, "ping", "message", None).unwrap();
            // Bind the hook payload to beta's conversation.
            s.conn
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
        }
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let ctx = HookContext {
            caller: &Caller::master(),
            pane_id: None,
        };

        let packed = take_pending_delivery(&store, &payload, &ctx).expect("one message to deliver");
        assert_eq!(packed.included.len(), 1);
        assert!(packed.text.contains("ping"));
        assert!(packed.text.contains("alpha@local"));

        // Stamped, so the next hook has nothing — no infinite re-delivery.
        assert!(
            take_pending_delivery(&store, &payload, &ctx).is_none(),
            "a delivered message is not handed over twice"
        );
    }

    #[test]
    fn take_pending_delivery_is_none_for_an_unresolvable_hook() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let payload = HookPayload {
            session_id: Some("00000000-0000-0000-0000-000000000000".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let ctx = HookContext {
            caller: &Caller::master(),
            pane_id: None,
        };
        assert!(take_pending_delivery(&store, &payload, &ctx).is_none());
    }
```

Reuse whatever `seed` helper `service/hooks.rs`'s test module already has; if it has none, copy the one from `service/messages.rs` tests.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib service::hooks
```

Expected: FAIL to compile — `cannot find function take_pending_delivery`.

- [ ] **Step 3: Implement it**

Append to `crates/fleet-core/src/service/hooks.rs`:

```rust
/// How many pending messages one hook response considers. The packer's budget
/// is the real limit; this only bounds the query.
const DELIVERY_SCAN_LIMIT: i64 = 64;

/// Pending messages for the session this hook belongs to, packed for an
/// `additionalContext` and stamped `delivered_at` in the same lock window.
///
/// Called from the `/hook` handler, which must answer in milliseconds: this
/// does ONE indexed read plus one UPDATE and never touches SSH or a hub.
///
/// Row resolution deliberately reuses [`resolve_hook_row`] with
/// `may_rebind = false` — delivery must never be the thing that rebinds a
/// conversation to a row; that stays the business of the events that own it.
pub fn take_pending_delivery(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Option<crate::service::delivery::Packed> {
    let s = lock(store).ok()?;
    let (row, _) = resolve_hook_row(&s, payload, ctx, false).ok()??;
    let pending = s.list_undelivered_for_session(row.id, DELIVERY_SCAN_LIMIT).ok()?;
    if pending.is_empty() {
        return None;
    }
    // `sender_label` needs a name per sender; resolve inside this same lock
    // window, and fall back to the bare id rather than failing a delivery.
    let label = |from_id: i64| match s.get_session_by_id(from_id) {
        Ok(Some(r)) => format!("{}@{}", r.tmux_name, r.host_alias),
        _ => format!("session {from_id}"),
    };
    let packed = crate::service::delivery::pack(&pending, &label);
    if packed.included.is_empty() {
        // Everything pending is individually over budget. Still report the
        // tail so the agent learns the messages exist, but stamp nothing —
        // they must stay deliverable via `inbox`.
        return Some(packed);
    }
    let _ = s.mark_messages_delivered(&packed.included);
    Some(packed)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::hooks
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: PASS. If `resolve_hook_row` is private to the module, this function lives in the same module so it is reachable; do not widen its visibility.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/hooks.rs
git commit -m "feat(hooks): look up and stamp the delivery a hook response carries"
```

---

### Task 8: `/hook` answers 200 with a body

**Files:**
- Modify: `crates/fleet-core/src/mcp/hooks.rs`
- Test: same file, its existing `mod tests`

**Interfaces:**
- Consumes: Task 7 `service::hooks::take_pending_delivery`.
- Produces: `/hook` returns `Response` instead of `StatusCode`; `204` when there is nothing to deliver, `200` + JSON otherwise.

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn a_hook_with_nothing_pending_still_answers_204() {
        let state = HookState {
            store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-x".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_pending_message_comes_back_as_hook_specific_output() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (a, b) = {
            let s = store.lock().unwrap();
            let a = seed(&s, "alpha");
            let b = seed(&s, "beta");
            s.insert_message(a, b, "ping", "message", None).unwrap();
            s.conn
                .execute(
                    "UPDATE sessions SET claude_session_id='conv-b' WHERE id=?1",
                    rusqlite::params![b],
                )
                .unwrap();
            (a, b)
        };
        let _ = (a, b);
        let state = HookState {
            store: store.clone(),
            ssh: Arc::new(SshClient::new()),
        };
        let payload = HookPayload {
            session_id: Some("conv-b".into()),
            hook_event_name: Some("UserPromptSubmit".into()),
            ..Default::default()
        };
        let res = handle_hook(
            State(state),
            Extension(Caller::master()),
            axum::http::HeaderMap::new(),
            Json(payload),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(ctx.contains("ping"), "{ctx}");
    }

    #[tokio::test]
    async fn a_client_caller_is_still_refused_and_gets_no_delivery() {
        // Regression guard: the 403 path must not become a delivery channel.
        use crate::mcp::auth::{ClientRef, TokenMode};
        let state = HookState {
            store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
            ssh: Arc::new(SshClient::new()),
        };
        let client = Caller {
            host_alias: None,
            client: Some(ClientRef { id: 1, name: "phone".into(), trusted: false }),
            mode: TokenMode::Full,
        };
        let res = handle_hook(
            State(state),
            Extension(client),
            axum::http::HeaderMap::new(),
            Json(HookPayload::default()),
        )
        .await
        .into_response();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p fleet-core --lib mcp::hooks
```

Expected: FAIL to compile — `handle_hook` returns `StatusCode`, which has no `into_response` shape matching these assertions on a body.

- [ ] **Step 3: Change the handler's return type**

In `crates/fleet-core/src/mcp/hooks.rs`, change `handle_hook`'s signature to `-> Response` and wrap every existing `StatusCode::X` return in `X.into_response()`. Then replace the `Ok(())` arm of the `apply_hook` match:

```rust
    match crate::service::hooks::apply_hook(&state.store, &state.ssh, &payload, &ctx) {
        Ok(()) => {
            // Delivery rides the response body of exactly these two events:
            // they are the only hooks Claude Code reads `additionalContext`
            // from, and both must stay SYNCHRONOUS (never `async: true`) or
            // the body is discarded. See the phase-2b exception in
            // docs/superpowers/specs/2026-09-22-fleet-mesh-addressing-and-delivery-design.md
            let event = payload.hook_event_name.as_deref().unwrap_or("");
            if !matches!(event, "UserPromptSubmit" | "Stop") {
                return StatusCode::NO_CONTENT.into_response();
            }
            match crate::service::hooks::take_pending_delivery(&state.store, &payload, &ctx) {
                Some(packed) if !packed.text.is_empty() => {
                    tracing::debug!(
                        event,
                        delivered = packed.included.len(),
                        remaining = packed.remaining,
                        "[hook] carrying a delivery"
                    );
                    axum::Json(serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": event,
                            "additionalContext": packed.text,
                        }
                    }))
                    .into_response()
                }
                _ => StatusCode::NO_CONTENT.into_response(),
            }
        }
```

Add `use axum::response::{IntoResponse, Response};` to the imports.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib mcp::
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: PASS, including the pre-existing `a_client_caller_is_refused_with_403`.

- [ ] **Step 5: Update the e2e assertion**

In `scripts/hub-e2e.sh:221`, the check `hook with master token, unknown session -> 204` stays as-is (an unknown session has nothing pending). Add a second check beneath it that a known session with a pending message answers 200 with the body. Follow the `check`/`tool` helper style already in the file, and the session-id capture pattern used around line 421.

- [ ] **Step 6: Run the e2e locally**

```bash
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/ft-$(basename "$PWD")}"
cargo build -p fleet-hub -p fleet-agent --locked
BIN=$CARGO_TARGET_DIR/debug/fleet-hub ABIN=$CARGO_TARGET_DIR/debug/fleet-agent bash scripts/hub-e2e.sh
```

Expected: every check passes. This needs `tmux` on the machine; it is the only place the wire contract is proven.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/mcp/hooks.rs scripts/hub-e2e.sh
git commit -m "feat(hook): answer 200 with additionalContext when a message is pending"
```

---

### Task 9: The `Stop` block decision and its streak cap

**Files:**
- Modify: `crates/fleet-core/src/service/delivery.rs` (pure decision fn)
- Modify: `crates/fleet-core/src/mcp/hooks.rs` (apply it on the `Stop` arm)
- Modify: `crates/fleet-core/src/store/participants.rs` (streak read/write — it owns no other file's concerns and keeps `timeline.rs` single-writer)
- Test: `service/delivery.rs` and `mcp/hooks.rs`

**Interfaces:**
- Consumes: Task 5 `Packed`, Task 8's `Stop` arm.
- Produces:
  - `pub const STOP_BLOCK_STREAK_MAX: u32 = 3;`
  - `pub enum StopAction { Context, Block }`
  - `pub fn stop_action(pending: &[SessionMessage], streak: u32) -> StopAction`
  - `Store::{stop_block_streak, bump_stop_block_streak, reset_stop_block_streak}`

- [ ] **Step 1: Write the failing tests**

In `service/delivery.rs` tests:

```rust
    fn question(id: i64) -> SessionMessage {
        SessionMessage { kind: "question".into(), ..msg(id, "answer me") }
    }

    #[test]
    fn a_plain_message_never_blocks_a_stop() {
        assert_eq!(stop_action(&[msg(1, "fyi")], 0), StopAction::Context);
    }

    #[test]
    fn a_question_blocks_a_stop_while_under_the_cap() {
        assert_eq!(stop_action(&[question(1)], 0), StopAction::Block);
        assert_eq!(stop_action(&[question(1)], STOP_BLOCK_STREAK_MAX - 1), StopAction::Block);
    }

    #[test]
    fn at_the_cap_even_a_question_only_adds_context() {
        assert_eq!(stop_action(&[question(1)], STOP_BLOCK_STREAK_MAX), StopAction::Context);
        assert_eq!(stop_action(&[question(1)], STOP_BLOCK_STREAK_MAX + 7), StopAction::Context);
    }

    #[test]
    fn nothing_pending_never_blocks() {
        assert_eq!(stop_action(&[], 0), StopAction::Context);
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p fleet-core --lib service::delivery
```

Expected: FAIL to compile — `cannot find function stop_action`.

- [ ] **Step 3: Implement the pure decision**

Append to `service/delivery.rs`:

```rust
/// Most consecutive `Stop` blocks one session may be held by. Without a cap a
/// remote sender could shut a session inside a never-ending turn — a denial of
/// service against one's own fleet, and reachable in a full mesh where every
/// endpoint can address every other.
pub const STOP_BLOCK_STREAK_MAX: u32 = 3;

/// What a `Stop` hook should do about the pending messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAction {
    /// Add `additionalContext`; the turn ends normally.
    Context,
    /// `decision: "block"` with the messages as the `reason`, so Claude keeps
    /// working and answers.
    Block,
}

/// PURE: block only for a message that genuinely wants an answer, and only
/// while under [`STOP_BLOCK_STREAK_MAX`].
pub fn stop_action(pending: &[SessionMessage], streak: u32) -> StopAction {
    if streak >= STOP_BLOCK_STREAK_MAX {
        return StopAction::Context;
    }
    if pending.iter().any(|m| m.kind == "question") {
        StopAction::Block
    } else {
        StopAction::Context
    }
}
```

- [ ] **Step 4: Implement the streak counters**

Append to `store/participants.rs` (inside its `impl Store`):

```rust
    /// Consecutive `Stop` blocks this session currently sits behind.
    pub fn stop_block_streak(&self, session_id: i64) -> Result<u32, IpcError> {
        let n: i64 = self.conn.query_row(
            "SELECT COALESCE(stop_block_streak, 0) FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u32)
    }

    pub fn bump_stop_block_streak(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET stop_block_streak = COALESCE(stop_block_streak, 0) + 1 \
             WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// Called when a turn ends without a block, so a later question is not
    /// punished for an earlier streak.
    pub fn reset_stop_block_streak(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET stop_block_streak = 0 WHERE id = ?1 AND stop_block_streak <> 0",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }
```

- [ ] **Step 5: Apply it on the `Stop` arm**

`take_pending_delivery` already returns the packed batch; extend `service/hooks.rs` with a sibling that also reports the action, so `mcp/hooks.rs` stays a thin shape-only layer:

```rust
/// As [`take_pending_delivery`], plus what a `Stop` should do about it, and
/// the streak bookkeeping that keeps a block from repeating forever.
pub fn take_pending_stop_delivery(
    store: &Arc<Mutex<Store>>,
    payload: &HookPayload,
    ctx: &HookContext,
) -> Option<(crate::service::delivery::Packed, crate::service::delivery::StopAction)> {
    use crate::service::delivery::{stop_action, StopAction};
    let (row_id, streak, pending) = {
        let s = lock(store).ok()?;
        let (row, _) = resolve_hook_row(&s, payload, ctx, false).ok()??;
        let pending = s
            .list_undelivered_for_session(row.id, DELIVERY_SCAN_LIMIT)
            .ok()?;
        let streak = s.stop_block_streak(row.id).unwrap_or(0);
        (row.id, streak, pending)
    };
    if pending.is_empty() {
        let s = lock(store).ok()?;
        let _ = s.reset_stop_block_streak(row_id);
        return None;
    }
    let action = stop_action(&pending, streak);
    let packed = take_pending_delivery(store, payload, ctx)?;
    let s = lock(store).ok()?;
    match action {
        StopAction::Block => {
            let _ = s.bump_stop_block_streak(row_id);
            let _ = s.insert_session_event(row_id, "stop_blocked_for_message", None);
        }
        StopAction::Context => {
            if streak >= crate::service::delivery::STOP_BLOCK_STREAK_MAX {
                let _ = s.insert_session_event(row_id, "stop_block_cap_reached", None);
            }
            let _ = s.reset_stop_block_streak(row_id);
        }
    }
    Some((packed, action))
}
```

In `mcp/hooks.rs`, route the `Stop` event through this instead, and for `StopAction::Block` answer:

```rust
                    axum::Json(serde_json::json!({
                        "decision": "block",
                        "reason": packed.text,
                    }))
                    .into_response()
```

`reason` is capped at 2000 characters by the CLI, against the packer's 8000. Pass `packed.text` through `crate::service::messages::timeline_detail`-style truncation at 2000 chars **on a char boundary** for the block path only, so a multi-byte body is never split.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/delivery.rs crates/fleet-core/src/service/hooks.rs crates/fleet-core/src/mcp/hooks.rs crates/fleet-core/src/store/participants.rs
git commit -m "feat(hook): block a Stop for a question, capped at three in a row"
```

---

### Task 10: `wait_for_reply` — event-driven via a store `Notify`

**Files:**
- Modify: `crates/fleet-core/src/store/mod.rs` (the `Notify` + `notify_waiters` on message insert)
- Modify: `crates/fleet-core/src/service/messages.rs` (the wait function)
- Test: `crates/fleet-core/src/service/messages.rs`

**Interfaces:**
- Consumes: Task 6's inbox reads.
- Produces:
  - `Store::message_notify(&self) -> Arc<tokio::sync::Notify>`
  - `pub async fn wait_for_reply(store: &Mutex<Store>, session_id: i64, after_message_id: Option<i64>, timeout: Duration) -> Result<Option<SessionMessage>, IpcError>`

See *Deviation from the spec* above for why this is a `Notify` and not a bus subscription.

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn wait_for_reply_returns_immediately_when_one_already_waits() {
        let (store, ssh, a, b) = fixture();
        send_message(args(a, b, "early"), &store, &ssh).await.unwrap();
        let got = wait_for_reply(&store, b, None, Duration::from_secs(5))
            .await
            .unwrap()
            .expect("the already-waiting message");
        assert_eq!(got.body, "early");
    }

    #[tokio::test]
    async fn wait_for_reply_wakes_on_a_message_that_arrives_while_waiting() {
        let (store, ssh, a, b) = fixture();
        let store = std::sync::Arc::new(store);
        let (s2, ssh2) = (store.clone(), ssh.clone());
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            send_message(args(a, b, "late"), &s2, &ssh2).await.unwrap();
        });
        let started = std::time::Instant::now();
        let got = wait_for_reply(&store, b, None, Duration::from_secs(5))
            .await
            .unwrap()
            .expect("the message that arrived during the wait");
        sender.await.unwrap();
        assert_eq!(got.body, "late");
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "the wake must be event-driven, not a 500 ms poll: took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_for_reply_times_out_with_none_rather_than_an_error() {
        let (store, _ssh, _a, b) = fixture();
        let got = wait_for_reply(&store, b, None, Duration::from_millis(150))
            .await
            .unwrap();
        assert!(got.is_none(), "a timeout is Ok(None), not an error");
    }

    #[tokio::test]
    async fn after_message_id_ignores_messages_the_caller_already_saw() {
        let (store, ssh, a, b) = fixture();
        let first = send_message(args(a, b, "one"), &store, &ssh).await.unwrap();
        let got = wait_for_reply(&store, b, Some(first.id), Duration::from_millis(150))
            .await
            .unwrap();
        assert!(got.is_none(), "the already-seen message must not satisfy the wait");
        let second = send_message(args(a, b, "two"), &store, &ssh).await.unwrap();
        let got = wait_for_reply(&store, b, Some(first.id), Duration::from_secs(5))
            .await
            .unwrap()
            .expect("the newer message");
        assert_eq!(got.id, second.id);
    }

    #[tokio::test]
    async fn wait_for_reply_rejects_an_unknown_session() {
        let (store, _ssh, _a, _b) = fixture();
        let err = wait_for_reply(&store, 9999, None, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p fleet-core --lib service::messages
```

Expected: FAIL to compile — `cannot find function wait_for_reply`.

- [ ] **Step 3: Add the `Notify` to the store**

In `crates/fleet-core/src/store/mod.rs`, add a field to `Store`:

```rust
    /// Signalled after a `session_messages` insert commits, so a waiter wakes
    /// on arrival instead of polling.
    ///
    /// Deliberately NOT the event bus: only the hub builds a subscribable bus
    /// (`BroadcastEventBus`), while the desktop's bus forwards to Svelte and
    /// hands out no receiver. A `Notify` on the store works in every
    /// embedding and needs no new `RowChange` variant, so no contract golden
    /// or `events.ts` allowlist entry moves.
    message_notify: Arc<tokio::sync::Notify>,
```

Initialise it to `Arc::new(tokio::sync::Notify::new())` in every constructor, and expose:

```rust
    pub fn message_notify(&self) -> Arc<tokio::sync::Notify> {
        self.message_notify.clone()
    }
```

Then, at the end of `insert_message` in `store/timeline.rs` (after `last_insert_rowid`), signal it:

```rust
        self.message_notify.notify_waiters();
```

Note: `insert_message` is also called inside `Store::atomically`, where a
rollback must announce nothing. `notify_waiters()` only wakes waiters, which
then re-read the table and find nothing — a spurious wake, never a false
message. Record that in the comment; do not try to defer it.

- [ ] **Step 4: Implement the wait**

Append to `crates/fleet-core/src/service/messages.rs`:

```rust
/// Safety floor: a waiter re-reads at least this often even if no
/// notification arrives, so a missed signal costs latency, never the wait.
const REPLY_POLL_FLOOR: std::time::Duration = std::time::Duration::from_millis(500);

/// Bounded wait for the next message addressed to `session_id`, newer than
/// `after_message_id`. `Ok(None)` on timeout — a timeout is an outcome, not an
/// error, matching `wait_for_session`.
pub async fn wait_for_reply(
    store: &Mutex<Store>,
    session_id: i64,
    after_message_id: Option<i64>,
    timeout: std::time::Duration,
) -> Result<Option<SessionMessage>, IpcError> {
    let deadline = tokio::time::Instant::now() + timeout;
    // Take the handle (and validate the session) under one short lock window,
    // never across an await.
    let notify = {
        let s = lock(store)?;
        if s.get_session_by_id(session_id)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} not found"),
            ));
        }
        s.message_notify()
    };
    loop {
        {
            let s = lock(store)?;
            let found = s
                .list_inbox(session_id, false, 1)?
                .into_iter()
                .find(|m| after_message_id.is_none_or(|a| m.id > a));
            if let Some(m) = found {
                return Ok(Some(m));
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        // Wake on arrival; the floor bounds a missed signal.
        let _ = tokio::time::timeout(REPLY_POLL_FLOOR.min(deadline - now), notify.notified()).await;
    }
}
```

`list_inbox` returns newest-first, so taking the first row and testing it
against `after_message_id` finds the newest unseen message.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::messages
cargo clippy -p fleet-core --all-targets -- -D warnings
```

Expected: PASS, including the sub-400 ms assertion. If `is_none_or` is not available on this toolchain, write `after_message_id.map_or(true, |a| m.id > a)` — do not lower the MSRV.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/store/mod.rs crates/fleet-core/src/store/timeline.rs crates/fleet-core/src/service/messages.rs
git commit -m "feat(messages): event-driven wait_for_reply on a store notify"
```

---

### Task 11: Address-addressed `send_message`, the `wait_for_reply` tool, idempotency

**Files:**
- Modify: `crates/fleet-core/src/service/messages.rs` (accept an address; reuse the dedupe table)
- Modify: `crates/fleet-core/src/mcp/tools/messaging.rs`, `crates/fleet-core/src/mcp/tools/params.rs`
- Test: `crates/fleet-core/src/service/messages.rs`, `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Consumes: Task 3 `service::address::{parse, Addr}`, Task 10 `wait_for_reply`.
- Produces: `SendMessageArgs.to_addr: Option<String>` (alternative to `to_session_id`); `SendMessageArgs.client_msg_id: Option<String>`; the `wait_for_reply` MCP tool.

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn send_accepts_an_address_instead_of_a_session_id() {
        let (store, ssh, a, b) = fixture();
        let mut m = args(a, 0, "by address");
        m.to_session_id = 0;
        m.to_addr = Some("/session/local/beta".into());
        let res = send_message(m, &store, &ssh).await.unwrap();
        let inbox = list_inbox(b, false, 10, false, &store).unwrap();
        assert_eq!(inbox[0].id, res.id);
        assert_eq!(inbox[0].body, "by address");
    }

    #[tokio::test]
    async fn an_address_naming_nothing_is_e_participant_unknown() {
        let (store, ssh, a, _b) = fixture();
        let mut m = args(a, 0, "nowhere");
        m.to_session_id = 0;
        m.to_addr = Some("/session/local/ghost".into());
        let err = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_PARTICIPANT_UNKNOWN");
    }

    #[tokio::test]
    async fn a_malformed_address_is_e_validate_and_writes_nothing() {
        let (store, ssh, a, b) = fixture();
        let mut m = args(a, 0, "bad");
        m.to_session_id = 0;
        m.to_addr = Some("nonsense".into());
        let err = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_VALIDATE");
        assert!(list_inbox(b, false, 10, false, &store).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_foreign_fleet_is_refused_until_cycle_three() {
        let (store, ssh, a, _b) = fixture();
        let mut m = args(a, 0, "over there");
        m.to_session_id = 0;
        m.to_addr = Some("some-other-fleet/session/mac/x".into());
        let err = send_message(m, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, "E_UNSUPPORTED");
        assert!(err.message.contains("hub"), "the message must name why: {}", err.message);
    }

    #[tokio::test]
    async fn the_same_client_msg_id_sends_once() {
        let (store, ssh, a, b) = fixture();
        let mut m = args(a, b, "once");
        m.client_msg_id = Some("abc-123".into());
        let first = send_message(m, &store, &ssh).await.unwrap();
        let mut again = args(a, b, "once");
        again.client_msg_id = Some("abc-123".into());
        let second = send_message(again, &store, &ssh).await.unwrap();
        assert_eq!(first.id, second.id, "a retry returns the first result");
        assert_eq!(
            list_inbox(b, false, 10, false, &store).unwrap().len(),
            1,
            "a retry must not deliver twice"
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p fleet-core --lib service::messages
```

Expected: FAIL to compile — `SendMessageArgs` has no field `to_addr`.

- [ ] **Step 3: Implement the resolution and the dedupe**

Add to `SendMessageArgs`:

```rust
    /// Fleet address of the recipient — `<fleet>/session/<host>/<name>`,
    /// `<fleet>/client/<name>` or `<fleet>/hub`. Alternative to
    /// `to_session_id`; when both are set, this wins.
    #[serde(default)]
    pub to_addr: Option<String>,
    /// Caller-chosen id making a retry idempotent. Reuses the same
    /// `(caller, id)` dedupe `send_prompt` already has — a second send with
    /// the same id returns the first result instead of delivering twice.
    #[serde(default)]
    pub client_msg_id: Option<String>,
```

Both `#[serde(default)]`, because a hub-routed argument without a default is a
shipped outage against an older hub.

Resolve at the top of `send_message`, before the existing validation:

```rust
    let to_session_id = match args.to_addr.as_deref() {
        None => args.to_session_id,
        Some(raw) => {
            let addr = crate::service::address::parse(raw)?;
            let fleet = crate::service::address::local_fleet_id(store)?;
            if crate::service::address::is_foreign(&addr, &fleet) {
                return Err(IpcError::new(
                    codes::E_UNSUPPORTED,
                    "that address names another fleet; a hub-to-hub link is not built yet",
                ));
            }
            match addr {
                crate::service::address::Addr::Session { host, name, .. } => {
                    let s = lock(store)?;
                    let row = s.get_session(&name, &host)?.ok_or_else(|| {
                        IpcError::new(
                            codes::E_PARTICIPANT_UNKNOWN,
                            format!("no session {name} on {host}"),
                        )
                    })?;
                    if let Some(p) = s.participant_for_session(row.id)? {
                        if p.retired_at.is_some() {
                            return Err(IpcError::new(
                                codes::E_PARTICIPANT_RETIRED,
                                format!("session {name} on {host} is gone"),
                            ));
                        }
                    }
                    row.id
                }
                crate::service::address::Addr::Client { .. }
                | crate::service::address::Addr::Hub { .. } => {
                    return Err(IpcError::new(
                        codes::E_UNSUPPORTED,
                        "only session addresses can receive a message today",
                    ))
                }
            }
        }
    };
```

Then use `to_session_id` everywhere the function currently uses
`args.to_session_id`. For the dedupe, reuse the table `send_prompt` uses —
grep `client_msg_id` in `service/sessions/prompt.rs` for the helper and call
the same one rather than adding a second table.

`Client` and `Hub` addresses parse but are refused as recipients this cycle:
the participant rows exist so cycle 3 can route to them, and refusing is
honest about what is built.

- [ ] **Step 4: Add the `wait_for_reply` tool**

In `mcp/tools/params.rs`:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForReplyParams {
    /// Your fleet session id (from whoami / list_sessions).
    pub session_id: i64,
    /// Only return a message newer than this id. Pass the id you last saw.
    #[serde(default)]
    pub after_message_id: Option<i64>,
    /// Seconds to wait (default 120, max 600).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}
```

In `mcp/tools/messaging.rs`, following the `wait_for_session` shape exactly
(including `long_poll_permit`):

```rust
    #[tool(description = "Wait for the next message sent TO a session. One \
        call that holds until a message arrives or timeout_s elapses \
        (default 120, max 600) — use this instead of polling inbox. Returns \
        { status: satisfied | timeout, message }.")]
    pub(super) async fn wait_for_reply(
        &self,
        Parameters(p): Parameters<WaitForReplyParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = self.caller()?;
        let _permit = self.long_poll_permit(&caller, "wait_for_reply")?;
        let got = crate::service::messages::wait_for_reply(
            &self.store,
            p.session_id,
            p.after_message_id,
            crate::service::tasks::wait_timeout(p.timeout_s),
        )
        .await
        .map_err(to_mcp_err)?;
        Ok(json_result(serde_json::json!({
            "status": if got.is_some() { "satisfied" } else { "timeout" },
            "message": got,
        })))
    }
```

Register it in the router the same way the neighbouring tools are, and add its
name to the tool-policy / `present::visible_to` tables beside `wait_for_session`
so a readonly caller sees it (it is a read) and the gates do not fail closed on
an unknown name.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib
```

Expected: PASS. **The description-budget test may now fail.** If it does, read its reported byte count, trim the new descriptions first, and only then raise `BUDGET_BYTES` — adding a comment in the same style as the existing history, naming `wait_for_reply` and `to_addr` as the reason.

- [ ] **Step 6: Regenerate the reference**

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
cargo test -p fleet-core reference_is_current
```

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/messages.rs crates/fleet-core/src/mcp/tools/messaging.rs crates/fleet-core/src/mcp/tools/params.rs crates/fleet-core/src/mcp/tools/tests.rs docs/control-api-reference.md
git commit -m "feat(messages): address-addressed send, idempotency, wait_for_reply tool"
```

---

### Task 12: Wake an idle recipient; never touch a blocked one

**Files:**
- Modify: `crates/fleet-core/src/service/messages.rs`
- Test: same file

**Interfaces:**
- Consumes: `sessions::send_system_prompt(&host_alias, &tmux_name, &body, submit, store, ssh)` — the phase-1 primitive with `label: false`, so a message never renames the recipient (`sessions/prompt.rs:550`).
- Produces: `SendMessageArgs.wake: bool`; `SendMessageResult.woke: bool`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn wake_is_skipped_for_a_working_recipient_because_the_hook_will_carry_it() {
        let (store, ssh, a, b) = fixture();
        {
            let s = store.lock().unwrap();
            s.record_notification_hook_for_row(
                b,
                crate::service::pane_intel::ClaudeStatus::Working,
                None,
            )
            .unwrap();
        }
        let mut m = args(a, b, "later");
        m.wake = true;
        let res = send_message(m, &store, &ssh).await.unwrap();
        assert!(!res.woke, "a working session gets the message from its Stop hook");
        assert_eq!(list_inbox(b, true, 10, false, &store).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn wake_refuses_a_blocked_recipient_and_says_why() {
        let (store, ssh, a, b) = fixture();
        {
            let s = store.lock().unwrap();
            s.record_notification_hook_for_row(
                b,
                crate::service::pane_intel::ClaudeStatus::Blocked,
                None,
            )
            .unwrap();
        }
        let mut m = args(a, b, "do not answer the dialog");
        m.wake = true;
        let res = send_message(m, &store, &ssh).await.unwrap();
        assert!(!res.woke);
        let err = res.deliver_error.expect("a blocked recipient reports why");
        assert!(err.contains("inbox"), "{err}");
        assert_eq!(
            list_inbox(b, true, 10, false, &store).unwrap().len(),
            1,
            "the message still lands in the inbox"
        );
    }

    #[tokio::test]
    async fn wake_is_not_attempted_when_wake_is_false() {
        let (store, ssh, a, b) = fixture();
        let res = send_message(args(a, b, "quiet"), &store, &ssh).await.unwrap();
        assert!(!res.woke);
        assert_eq!(res.deliver_error, None);
    }
```

An `idle` recipient's happy path needs real tmux and belongs in the e2e (Step 5), not here — the unit fixture has no tmux server, so asserting a successful paste would be asserting the mock.

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p fleet-core --lib service::messages
```

Expected: FAIL to compile — `SendMessageArgs` has no field `wake`.

- [ ] **Step 3: Implement it**

Add to `SendMessageArgs`:

```rust
    /// Nudge an IDLE recipient so it notices now instead of at its next turn.
    /// Only an idle session is nudged: a `working` one gets the message from
    /// its own `Stop` hook, and a `blocked` one is never typed into — Enter
    /// there would answer whatever dialog is on screen. Defaults to false.
    #[serde(default)]
    pub wake: bool,
```

Add `pub woke: bool` to `SendMessageResult`. After the message row is
committed, replacing nothing in the existing `deliver` path:

```rust
    // Wake-up is the ONLY remaining use of the paste primitive. Delivery
    // proper rides the hook response; this exists because an idle, unprompted
    // session never fires a hook and would otherwise not notice at all.
    let mut woke = false;
    let mut wake_error = None;
    if args.wake {
        match to_row.claude_status.as_deref() {
            Some("idle") | None => {
                let header = pane_header(id, &from_row.tmux_name, &from_row.host_alias, &args.body);
                match sessions::send_system_prompt(
                    &to_row.host_alias,
                    &to_row.tmux_name,
                    &header,
                    true,
                    store,
                    ssh,
                )
                .await
                {
                    Ok(()) => woke = true,
                    Err(e) => wake_error = Some(e.message),
                }
            }
            Some("blocked") => {
                wake_error = Some(
                    "recipient is blocked on a dialog; not typed into — the message is in its inbox"
                        .into(),
                )
            }
            Some(_) => {}
        }
    }
```

Merge `wake_error` into the returned `deliver_error`. Read the actual
`ClaudeStatus` string constants from `service/pane_intel.rs` rather than
hard-coding `"idle"`/`"blocked"` if that enum exposes them — the status
vocabulary lives there by convention.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib service::messages
cargo clippy -p fleet-core --all-targets -- -D warnings
```

- [ ] **Step 5: Add the e2e wake check**

In `scripts/hub-e2e.sh`, beside the existing `send_prompt types into the pane`
check (around line 421), add one that `send_message` with `wake: true` to an
idle session shows the `[msg #N from ...]` header in `capture_session`.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/messages.rs scripts/hub-e2e.sh
git commit -m "feat(messages): wake an idle recipient, never a blocked one"
```

---

### Task 13: A move re-points the participant instead of destroying the inbox

**Files:**
- Modify: `crates/fleet-core/src/service/move_session/finalise.rs`
- Modify: `crates/fleet-core/src/store/sessions.rs` (`delete_session`)
- Test: `crates/fleet-core/src/store/sessions.rs` and `crates/fleet-core/src/service/move_session/finalise.rs`

**Interfaces:**
- Consumes: Task 2 `Store::{participant_for_session, repoint_participant, retire_participant}`.
- Produces: `Store::delete_session` no longer deletes messages; `Store::retire_session_participant(&self, session_id: i64)` for a genuine kill.

This is the defect named in spec §3.1: today a move destroys the undelivered inbox, silently.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn deleting_a_session_retires_its_participant_and_keeps_undelivered_mail() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "unread", "message", None).unwrap();
        let p = s.participant_for_session(b).unwrap().unwrap().id;

        s.delete_session(b).unwrap();

        assert!(s.get_message(m).unwrap().is_some(), "the message survives the kill");
        let row = s.participant_by_id(p).unwrap().expect("the identity survives");
        assert!(row.retired_at.is_some(), "and is tombstoned");
    }

    #[test]
    fn a_move_repoints_the_participant_so_mail_follows_the_session() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let src = seed(&s, "worker");
        let dst = seed(&s, "worker-moved");
        let m = s.insert_message(a, src, "follow me", "message", None).unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;

        // What finalise does: re-point, then kill the source row.
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();

        assert_eq!(s.participant_by_id(p).unwrap().unwrap().session_id, Some(dst));
        let pending = s.list_undelivered_for_session(dst, 10).unwrap();
        assert_eq!(
            pending.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![m],
            "the moved session inherits its undelivered mail"
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p fleet-core --lib store::sessions
```

Expected: FAIL — the message is gone (`delete_session` deleted it) and the participant has no `retired_at`.

- [ ] **Step 3: Change `delete_session`**

Replace the `DELETE FROM session_messages` statement at `store/sessions.rs:874` with a retire of the participant, inside the same transaction:

```rust
        // Deliberately NOT `DELETE FROM session_messages`: a kill used to
        // destroy every undelivered message addressed to this session, and a
        // MOVE goes through here too (the source row is killed after the
        // target is created), so a move silently lost the inbox. The identity
        // is tombstoned instead; `service/gc.rs` sweeps the mail after the
        // retention window, and a move re-points the participant BEFORE this
        // runs, so there is nothing here left to retire.
        tx.execute(
            "UPDATE participants SET retired_at = strftime('%s','now'), session_id = NULL \
             WHERE session_id = ?1 AND retired_at IS NULL",
            rusqlite::params![id],
        )?;
```

- [ ] **Step 4: Re-point in `finalise`**

In `service/move_session/finalise.rs`, where the move already knows
`a.source_row_id` and `a.target_row_id` (around `:173`), re-point before the
source is killed:

```rust
    // The address embeds the host alias and the row id changes on a move, so
    // the durable thing is the participant: re-point it and every message
    // already addressed to this session follows to the target.
    if let Some(p) = s.participant_for_session(a.source_row_id)? {
        s.repoint_participant(p.id, a.target_row_id)?;
    }
```

Place it inside the same transaction/lock window as the other finalise writes,
before `delete_session` runs on the source.

- [ ] **Step 5: Handle the three bulk-delete sites**

`store/reconcile.rs:632`, `store/projects.rs:548` and
`store/hosts_accounts.rs:424` each bulk-delete messages for removed sessions.
Change each to the same `UPDATE participants SET retired_at = ...` shape,
keyed on the same id list. Add one test per site asserting the message
survives and the participant is tombstoned — copy the shape of Step 1's first
test.

- [ ] **Step 6: Run the full backend suite**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS. Watch for pre-existing tests that asserted messages were
deleted — those assertions encoded the defect and must be rewritten to assert
the tombstone, with a comment saying why.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/store/reconcile.rs crates/fleet-core/src/store/projects.rs crates/fleet-core/src/store/hosts_accounts.rs crates/fleet-core/src/service/move_session/finalise.rs
git commit -m "fix(store): a kill tombstones the participant; a move keeps its inbox"
```

---

### Task 14: `message_undeliverable` and the GC window

**Files:**
- Modify: `crates/fleet-core/src/service/gc.rs`
- Modify: `crates/fleet-core/src/store/participants.rs` (the sweep query)
- Test: both files

**Interfaces:**
- Consumes: Task 13's tombstones.
- Produces: `Store::sweep_retired_participants(&self, older_than_secs: i64) -> Result<usize, IpcError>`; a `message_undeliverable` timeline event on each sender.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_retired_participant_within_the_window_is_kept() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "pending", "message", None).unwrap();
        s.delete_session(b).unwrap();
        assert_eq!(s.sweep_retired_participants(RETIRED_RETENTION_SECS).unwrap(), 0);
        assert!(s.get_message(m).unwrap().is_some());
    }

    #[test]
    fn past_the_window_the_mail_is_swept_and_the_sender_is_told() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "pending", "message", None).unwrap();
        s.delete_session(b).unwrap();
        // Age the tombstone past the window.
        s.conn
            .execute(
                "UPDATE participants SET retired_at = retired_at - ?1 WHERE retired_at IS NOT NULL",
                rusqlite::params![RETIRED_RETENTION_SECS + 60],
            )
            .unwrap();

        assert_eq!(s.sweep_retired_participants(RETIRED_RETENTION_SECS).unwrap(), 1);
        assert!(s.get_message(m).unwrap().is_none(), "the mail is gone");
        let ev = s.list_session_events(a, 10).unwrap();
        assert!(
            ev.iter().any(|e| e.kind == "message_undeliverable"),
            "the SENDER must learn its message was never read: {ev:?}"
        );
    }

    #[test]
    fn a_read_message_is_swept_without_telling_the_sender_anything() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "read me", "message", None).unwrap();
        s.mark_messages_read(&[m], b).unwrap();
        s.delete_session(b).unwrap();
        s.conn
            .execute(
                "UPDATE participants SET retired_at = retired_at - ?1 WHERE retired_at IS NOT NULL",
                rusqlite::params![RETIRED_RETENTION_SECS + 60],
            )
            .unwrap();
        s.sweep_retired_participants(RETIRED_RETENTION_SECS).unwrap();
        let ev = s.list_session_events(a, 10).unwrap();
        assert!(
            !ev.iter().any(|e| e.kind == "message_undeliverable"),
            "a message that WAS read is not undeliverable"
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p fleet-core --lib store::participants
```

Expected: FAIL to compile — `cannot find function sweep_retired_participants`.

- [ ] **Step 3: Implement the sweep**

Append to `store/participants.rs`:

```rust
/// How long a tombstoned participant's undelivered mail is kept. Long enough
/// that a sender waiting on a reply, or a human reading a timeline the next
/// day, still sees why nothing came back.
pub const RETIRED_RETENTION_SECS: i64 = 7 * 24 * 60 * 60;

impl Store {
    /// Drop the mail of participants retired longer than `older_than_secs`,
    /// telling each SENDER about anything that was never read. Returns how
    /// many participants were swept.
    pub fn sweep_retired_participants(&self, older_than_secs: i64) -> Result<usize, IpcError> {
        let cutoff = now_unix() - older_than_secs;
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM participants WHERE retired_at IS NOT NULL AND retired_at <= ?1")?;
            let rows = stmt.query_map(rusqlite::params![cutoff], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for pid in &ids {
            // Tell each sender about mail that was never read. A message that
            // WAS read is swept quietly — nothing went wrong there.
            let unread: Vec<(i64, i64)> = {
                let mut stmt = self.conn.prepare(
                    "SELECT id, from_session_id FROM session_messages \
                     WHERE to_participant_id = ?1 AND read_at IS NULL",
                )?;
                let rows = stmt.query_map(rusqlite::params![pid], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (mid, sender) in unread {
                let _ = self.insert_session_event(
                    sender,
                    "message_undeliverable",
                    Some(&format!("message {mid} was never read; recipient is gone")),
                );
            }
            self.conn.execute(
                "DELETE FROM session_messages WHERE to_participant_id = ?1",
                rusqlite::params![pid],
            )?;
            self.conn
                .execute("DELETE FROM participants WHERE id = ?1", rusqlite::params![pid])?;
        }
        Ok(ids.len())
    }
}
```

- [ ] **Step 4: Call it from the GC pass**

In `service/gc.rs`, inside `sweep_with` (`:234`), add a call alongside the
existing sweeps:

```rust
    let swept_participants = {
        let s = lock(store)?;
        s.sweep_retired_participants(crate::store::RETIRED_RETENTION_SECS)
            .unwrap_or(0)
    };
```

Add `swept_participants` to `GcReport` (with `#[serde(default)]`, since the
report crosses the hub wire) and re-export `RETIRED_RETENTION_SECS` from
`store/mod.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p fleet-core --lib
cargo clippy -p fleet-core --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/store/participants.rs crates/fleet-core/src/store/mod.rs crates/fleet-core/src/service/gc.rs
git commit -m "feat(gc): sweep retired participants and tell senders what was never read"
```

---

### Task 15: Hub-client parity, codegen, and the full gate

**Files:**
- Modify: `src-tauri/src/backend/verdicts.rs`, `src-tauri/src/backend/tests_routing.rs`
- Generated: `src/lib/hub_verdicts.generated.json`, `docs/hub.md`, `docs/control-api-reference.md`
- Test: `src/lib/hub_verdicts.test.ts`

**Interfaces:**
- Consumes: every tool added in Tasks 4 and 11.
- Produces: a verdict row per new command; regenerated artefacts.

- [ ] **Step 1: Run the routing tests to see what is missing**

```bash
cargo test -p claude-fleet --lib backend::
```

Expected: FAIL — `tests_routing.rs` holds the handler list and every command needs a verdict row; a new one without a row fails there by design.

- [ ] **Step 2: Add a verdict row per new command**

In `src-tauri/src/backend/verdicts.rs`, add a row for `wait_for_reply` (route
to the hub — it is a read that must work in hub-client mode), keyed **by
command name**, never a second tool literal or a pasted sentence. Follow the
neighbouring `wait_for_session` row exactly.

- [ ] **Step 3: Add the routing test entries**

In `src-tauri/src/backend/tests_routing.rs`, add the command's body and its
routed call. Every new argument field needs a **non-default value** in that
row, or the test cannot tell whether it was serialised.

- [ ] **Step 4: Regenerate every generated artefact**

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

Then re-run both without the env var and confirm they pass. A regen run can
itself report FAILED — re-run it before concluding anything.

- [ ] **Step 5: Regenerate the hub contract golden if the wire changed**

```bash
REGEN_HUB_CONTRACT=1 cargo test -p fleet-core contract
cargo test -p fleet-core contract
```

`GcReport.swept_participants` and the new `SendMessageResult.woke` are new wire
fields; both must carry `#[serde(default)]` or an older hub becomes a shipped
outage.

- [ ] **Step 6: Run the whole gate, unpiped**

```bash
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/ft-$(basename "$PWD")}"
pnpm install --frozen-lockfile
bash scripts/ci-local.sh --hub-e2e
```

Expected: everything green. Do not read this through `| tail`. If
`pnpm test`/`pnpm check` fail to find their binary, use `npx vitest run` and
`npx svelte-check` — the pnpm scripts do not put the binary on PATH here.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: hub-client verdicts and regenerated reference, verdicts and contract"
```

---

## Self-Review

**Spec coverage.** Migration 043 → Task 1. `participants` CRUD/re-point/retire → Task 2. String address → Task 3. `fleet_id` + `whoami` → Task 4. The 8000/200 packer → Task 5. `delivered_at` + undelivered query → Task 6. Two-way `/hook` → Tasks 7–8. `Stop` block + `STOP_BLOCK_STREAK_MAX` → Task 9. `wait_for_reply` → Task 10. Address-addressed send + idempotency → Task 11. Idle wake-up / blocked refusal → Task 12. Move re-pointing + the four DELETE sites → Task 13. Tombstone + `message_undeliverable` + 7-day window → Task 14. Codegen, budget, parity → Task 15. The pre-implementation spike → Task 0.

**Two spec items deliberately not implemented as written, both flagged above:**
- `wait_for_reply` uses a store `Notify`, not a bus subscription (only the hub builds a subscribable bus). See *Deviation from the spec*.
- `REDELIVER_AFTER_TURNS = 2` is **not** implemented. Tasks 6–8 stamp `delivered_at` once and never re-deliver. The counter needed to express "two turn boundaries later" is `sessions.prompt_submit_seq` (migration 042), and wiring re-delivery to it is a behaviour worth its own task rather than a clause buried in the hook path. **Cycle 1 ships without re-delivery**: a message handed to an interrupted turn stays `delivered_at`-stamped and unread, and is still readable via `inbox`. This is a reduction in scope from the spec and needs the operator's agreement; the alternative is a Task 16 that adds it properly.

**Placeholder scan.** No TBD/TODO. Every code step carries real code. Task 2 Step 1, Task 7 Step 1 and Task 12 Step 3 each name a lookup the implementer must confirm against the checkout (`upsert_session_for_test`, the `seed` helper, the `ClaudeStatus` constants) rather than guessing — that is an instruction with an exact target, not a placeholder.

**Type consistency.** `Packed { text, included, remaining }` is produced in Task 5 and consumed unchanged in Tasks 7, 8, 9. `ParticipantRow` fields are produced in Task 2 and read in Tasks 11, 13, 14. `ensure_participant_for_session` (Task 2) is called in Task 6's `insert_message`. `repoint_participant`/`retire_participant` (Task 2) are called in Task 13. `stop_action`/`StopAction` (Task 9) are consumed only in Task 9's own hook change. `local_fleet_id` (Task 4) is called in Task 11. `send_system_prompt` (existing, `sessions/prompt.rs:550`) is called in Task 12 with the six-argument signature it actually has.
