# Smart caching — remembered read cursors: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A reader can re-ask five fetch tools with `fresh_for: <its own session id>` and get only what is new — a delta for append-only streams, a cheap `unchanged` for snapshots — never silently skipping anything.

**Architecture:** Migration 044 adds `read_cursors`, keyed by `(reader_session_id, tool, resource_key)`. A pure module (`service/fresh.rs`) decides, from a stored cursor and the current head, whether to answer a delta, `unchanged`, or the full payload with a stated reset reason. The five MCP tools gain one optional `fresh_for` param each; with it absent every response is byte-for-byte what it is today.

**Tech Stack:** Rust (`crates/fleet-core`), SQLite via `rusqlite` behind `std::sync::Mutex`, `sha2` + `hex` (already dependencies — `mcp/auth.rs:119-122` uses them).

**Spec:** `docs/superpowers/specs/2026-09-23-smart-caching-cursors-design.md`

## Global Constraints

- **`fresh_for` absent ⇒ the response is byte-identical to today.** This is what keeps the hub↔desktop contract golden untouched: the desktop never sends `fresh_for`. Pinned by a test in the shape of the existing `a_view_is_opt_in_and_the_default_answer_is_byte_identical`.
- **Never a silent skip.** A cursor that cannot be trusted returns the full payload *and says why*. Every place the watermark advances must advance only to what was actually returned.
- **Streams page oldest-first by `id`.** With `fresh_for`, `session_history` and `inbox` return rows with `id > watermark ORDER BY id ASC LIMIT n`, and the watermark advances to the largest id returned — never to the head. Newest-first + `limit` + advance-to-head silently drops the rows between. Order by `id`, never by `at` (wall clock; `list_session_events` orders by `at DESC` today and a clock step would reorder it).
- **`fresh_for` goes on MCP-layer param structs only.** `service::repo_read::RepoFileArgs` is shared by `repo_file` and `repo_diff` and is hub-routed (`src-tauri/src/backend/tests_routing.rs:673,691`, `src-tauri/src/commands/files.rs`). It must not change. `repo_diff` gets its own `RepoDiffParams`.
- **Budget:** `BUDGET_BYTES` in `crates/fleet-core/src/mcp/tools/tests.rs` (63,730 at plan time — it moves upstream constantly; **measure, never copy this number**). Raised **once**, in Task 4, documented in the established comment style naming this cycle.
- **Codegen is CI-enforced:** `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` after any tool description or param change, then re-run without the env var. A regen run can itself report FAILED — re-run before concluding.
- **Never hold the `Store` guard across an `.await`, and never re-enter it.** Cycle 1 shipped a deadlock re-entering a held non-reentrant mutex from a helper.
- **Run `cargo test -p fleet-core --lib` UNFILTERED and unpiped** before every commit, and quote its summary line. A filtered run once let a broken test ride three tasks.
- **`export CARGO_TARGET_DIR=/tmp/ft-communication-smart-caching-15485b`** in every shell that runs cargo; scripts bypass the shell's cargo wrapper and would otherwise compile against another worktree's crates.
- Test seeder: `s.upsert_host("local").unwrap(); s.upsert_session(name, "local", None, None, 0, 0, "running", None).unwrap()`. `Store.conn` is private outside `store::*` — tests elsewhere use `s.conn_ref()`.

## Corrections to the spec, found while writing this plan

These are decided here; each task carries the relevant one.

1. **`turn_seq` never resets.** `record_stop_hook_for_row` (`store/sessions.rs:1012`) is its only writer, `turn_seq = turn_seq + 1`. The spec's "`turn_seq` can restart on `/clear`/`/resume`" is wrong. The reachable way a cursor ends up *ahead of the head* is **session-id reuse** (`sessions.id` has no AUTOINCREMENT): a new session starts at `turn_seq = 0` while an old cursor says 50. The rule stays; the reason is `ahead_of_head`.
2. **A `/clear` silently skips turns without a generation.** Because `turn_seq` keeps counting while the transcript *file* changes, a cursor at 10, turns 11–12 in the old conversation, then `/clear`, then 13–14: `turn_seq − watermark = 4`, the new file holds 2 turns, and 11–12 vanish. So the cursor stores a **`generation`**: the id of the latest conversation-boundary event (`conversation_started`, `conversation_ended`, `compact_done`) for the session. Any boundary since the last read ⇒ full payload, reason `conversation_changed`. This covers `/clear`, `/resume` and compaction uniformly, from data the timeline already records.
3. **No 7-day window for cursors.** The spec reused cycle 1's retention window. That window exists so a *sender* can be told about unread mail; a cursor has no consumer once its reader or target is gone. Orphan cursors are deleted on the same GC pass, immediately.
4. **An unknown `fresh_for` answers, and says so.** The spec's "reader no longer exists → answer, store no cursor" is kept, and made explicit: `cursor_reset: "reader_unknown"`. A caller passing a wrong id must not silently get full payloads forever.
5. **The no-SSH claim is provable in a unit test**, not only the e2e the spec named: a row whose transcript cannot be read at all (no host, no file) still answers `unchanged` when the cursor is current. If the unchanged path read anything, that call would error.

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/fleet-core/migrations/044_read_cursors.sql` (create) | the table and its unique key | 1 |
| `crates/fleet-core/src/store/schema.rs` | register 044; `EXPECTED_TABLES` | 1 |
| `crates/fleet-core/src/store/read_cursors.rs` (create) | cursor get / put / orphan sweep | 1 |
| `crates/fleet-core/src/store/mod.rs` | `mod read_cursors;` + re-export | 1 |
| `crates/fleet-core/src/store/timeline.rs` | id-ascending stream reads after a watermark; the conversation-boundary generation | 2 |
| `crates/fleet-core/src/service/fresh.rs` (create) | PURE: the decision, the reset reasons, the snapshot hash, the envelope | 3 |
| `crates/fleet-core/src/service/mod.rs` | `pub mod fresh;` | 3 |
| `crates/fleet-core/src/mcp/tools/params.rs` | `fresh_for` on four structs; new `RepoDiffParams` | 4 |
| `crates/fleet-core/src/mcp/tools/support.rs` | extract the compact-JSON string builder so a snapshot can hash exactly what it returns | 4 |
| `crates/fleet-core/src/mcp/tools/{orchestration,messaging,repo,session_ops}.rs` | descriptions (4), then wiring (5-7) | 4-7 |
| `crates/fleet-core/src/mcp/tools/tests.rs` | budget raise (4); tool-level tests (5-7) | 4-7 |
| `crates/fleet-core/src/service/gc.rs` | call the orphan-cursor sweep | 8 |
| `docs/control-api.md` | the prose the descriptions cannot afford | 8 |

`params.rs`, `tests.rs` and the tool files are touched by more than one task. Tasks run sequentially, so this is ordering, not conflict — but no implementer may assume a file is untouched by its neighbours.

---

### Task 1: Migration 044 and the cursor store

**Files:**
- Create: `crates/fleet-core/migrations/044_read_cursors.sql`, `crates/fleet-core/src/store/read_cursors.rs`
- Modify: `crates/fleet-core/src/store/schema.rs` (after the version-43 entry at ~L372; `EXPECTED_TABLES` at ~L596), `crates/fleet-core/src/store/mod.rs`

**Interfaces:**
- Produces:
  - `pub struct CursorRow { pub watermark: Option<i64>, pub generation: Option<i64>, pub content_hash: Option<String> }`
  - `Store::get_read_cursor(&self, reader: i64, tool: &str, resource_key: &str) -> Result<Option<CursorRow>, IpcError>`
  - `Store::put_stream_cursor(&self, reader: i64, tool: &str, resource_key: &str, target: Option<i64>, watermark: i64, generation: Option<i64>) -> Result<(), IpcError>`
  - `Store::put_snapshot_cursor(&self, reader: i64, tool: &str, resource_key: &str, target: Option<i64>, content_hash: &str) -> Result<(), IpcError>`
  - `Store::sweep_orphan_read_cursors(&self) -> Result<usize, IpcError>`

- [ ] **Step 1: Write the failing tests** in `store/read_cursors.rs`, and add `"read_cursors"` to `EXPECTED_TABLES` in `schema.rs`.

```rust
#[cfg(test)]
mod tests {
    use crate::store::Store;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None).unwrap()
    }

    #[test]
    fn a_missing_cursor_is_none() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        assert!(s.get_read_cursor(r, "session_history", "7").unwrap().is_none());
    }

    #[test]
    fn put_is_an_upsert_per_reader_tool_and_resource() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        s.put_stream_cursor(r, "session_history", "7", Some(7), 10, None).unwrap();
        s.put_stream_cursor(r, "session_history", "7", Some(7), 25, None).unwrap();
        let c = s.get_read_cursor(r, "session_history", "7").unwrap().unwrap();
        assert_eq!(c.watermark, Some(25), "the second put replaces the first");
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM read_cursors", [], |x| x.get(0))
            .unwrap();
        assert_eq!(n, 1, "one row per (reader, tool, resource), never two");
    }

    /// THE regression test for the defect this cycle's design exists to
    /// avoid: a caller label is `host:<alias>`, so every session on a host is
    /// one caller, and a caller-keyed cursor would let them consume each
    /// other's deltas. Two readers of one resource keep independent cursors.
    #[test]
    fn two_readers_of_one_resource_keep_independent_cursors() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "reader-a");
        let b = seed(&s, "reader-b");
        s.put_stream_cursor(a, "session_transcript", "9", Some(9), 3, Some(1)).unwrap();
        s.put_stream_cursor(b, "session_transcript", "9", Some(9), 8, Some(1)).unwrap();
        assert_eq!(s.get_read_cursor(a, "session_transcript", "9").unwrap().unwrap().watermark, Some(3));
        assert_eq!(s.get_read_cursor(b, "session_transcript", "9").unwrap().unwrap().watermark, Some(8));
    }

    #[test]
    fn a_snapshot_cursor_stores_a_hash_and_no_watermark() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        s.put_snapshot_cursor(r, "list_sessions", "all", None, "abc").unwrap();
        let c = s.get_read_cursor(r, "list_sessions", "all").unwrap().unwrap();
        assert_eq!(c.content_hash.as_deref(), Some("abc"));
        assert_eq!(c.watermark, None);
    }

    #[test]
    fn the_sweep_removes_cursors_whose_reader_or_target_is_gone() {
        let s = Store::open_in_memory().unwrap();
        let reader = seed(&s, "reader");
        let target = seed(&s, "target");
        let keep = seed(&s, "keep");
        s.put_stream_cursor(reader, "session_history", &target.to_string(), Some(target), 1, None).unwrap();
        s.put_stream_cursor(keep, "session_history", &keep.to_string(), Some(keep), 1, None).unwrap();
        s.put_snapshot_cursor(keep, "list_sessions", "all", None, "h").unwrap();
        s.delete_session(target).unwrap();
        assert_eq!(s.sweep_orphan_read_cursors().unwrap(), 1, "only the cursor ON the gone target");
        s.delete_session(reader).unwrap();
        assert_eq!(s.sweep_orphan_read_cursors().unwrap(), 0, "its only cursor was already swept");
        assert!(s.get_read_cursor(keep, "session_history", &keep.to_string()).unwrap().is_some());
        assert!(
            s.get_read_cursor(keep, "list_sessions", "all").unwrap().is_some(),
            "a NULL target is not an orphan — list_sessions has no target session"
        );
        s.delete_session(keep).unwrap();
        assert_eq!(s.sweep_orphan_read_cursors().unwrap(), 2, "a gone READER takes all its cursors");
    }
}
```

- [ ] **Step 2: Run to verify RED**

`cargo test -p fleet-core --lib store::` — expected: compile failure (`no method named get_read_cursor`), and `open_in_memory_creates_all_tables` failing on `read_cursors`. Quote the output.

- [ ] **Step 3: Write the migration** — `crates/fleet-core/migrations/044_read_cursors.sql`:

```sql
-- Remembered read cursors (smart caching, cycle 2). A reader re-asking a
-- fetch tool with `fresh_for` gets only what is new. Keyed by the READER'S
-- OWN session id, never the caller: a caller label is `host:<alias>`, so
-- every Claude session on a host is the same caller, and a caller-keyed
-- cursor would let them consume each other's deltas by default.
--
-- A row uses `watermark` (+ `generation`) for an append-only stream, or
-- `content_hash` for a snapshot — never both.
--
-- `generation`: for session_transcript, the id of the latest
-- conversation-boundary event when the cursor was written. turn_seq keeps
-- counting across /clear while the transcript FILE changes, so without it a
-- /clear between two reads would silently skip the old conversation's tail.
--
-- `target_session_id`: the session a cursor is ABOUT (NULL for
-- list_sessions, which has none), so the GC can drop cursors whose target
-- is gone. No foreign keys, matching the rest of this schema: dangling ids
-- are a tolerated state and the sweep is what cleans them.
CREATE TABLE IF NOT EXISTS read_cursors (
  id INTEGER PRIMARY KEY,
  reader_session_id INTEGER NOT NULL,
  tool TEXT NOT NULL,
  resource_key TEXT NOT NULL,
  target_session_id INTEGER,
  watermark INTEGER,
  generation INTEGER,
  content_hash TEXT,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_read_cursors_key
  ON read_cursors(reader_session_id, tool, resource_key);
CREATE INDEX IF NOT EXISTS idx_read_cursors_target
  ON read_cursors(target_session_id) WHERE target_session_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (44);
```

- [ ] **Step 4: Register it** in `schema.rs`, after the version-43 entry. It is `CREATE ... IF NOT EXISTS` only, so it re-runs safely and needs no guard:

```rust
    // `read_cursors` (smart caching, cycle 2): CREATE TABLE / INDEX IF NOT
    // EXISTS only, so re-running it is a no-op — no `already_applied` guard.
    Migration::plain(44, include_str!("../../migrations/044_read_cursors.sql")),
```

- [ ] **Step 5: Write the store** — `crates/fleet-core/src/store/read_cursors.rs`:

```rust
//! Remembered read cursors (migration 044). See the migration for why a
//! cursor is keyed by the reader's own session id rather than the caller.

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;

/// What a reader last saw of one resource through one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRow {
    pub watermark: Option<i64>,
    pub generation: Option<i64>,
    pub content_hash: Option<String>,
}

impl Store {
    pub fn get_read_cursor(
        &self,
        reader: i64,
        tool: &str,
        resource_key: &str,
    ) -> Result<Option<CursorRow>, IpcError> {
        self.conn
            .query_row(
                "SELECT watermark, generation, content_hash FROM read_cursors \
                 WHERE reader_session_id = ?1 AND tool = ?2 AND resource_key = ?3",
                rusqlite::params![reader, tool, resource_key],
                |r| {
                    Ok(CursorRow {
                        watermark: r.get(0)?,
                        generation: r.get(1)?,
                        content_hash: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Upsert a stream cursor. Clears any hash, so a row never carries both.
    pub fn put_stream_cursor(
        &self,
        reader: i64,
        tool: &str,
        resource_key: &str,
        target: Option<i64>,
        watermark: i64,
        generation: Option<i64>,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO read_cursors \
               (reader_session_id, tool, resource_key, target_session_id, \
                watermark, generation, content_hash, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7) \
             ON CONFLICT(reader_session_id, tool, resource_key) DO UPDATE SET \
               target_session_id = excluded.target_session_id, \
               watermark = excluded.watermark, generation = excluded.generation, \
               content_hash = NULL, updated_at = excluded.updated_at",
            rusqlite::params![reader, tool, resource_key, target, watermark, generation, now_unix()],
        )?;
        Ok(())
    }

    /// Upsert a snapshot cursor. Clears any watermark/generation.
    pub fn put_snapshot_cursor(
        &self,
        reader: i64,
        tool: &str,
        resource_key: &str,
        target: Option<i64>,
        content_hash: &str,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO read_cursors \
               (reader_session_id, tool, resource_key, target_session_id, \
                watermark, generation, content_hash, updated_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6) \
             ON CONFLICT(reader_session_id, tool, resource_key) DO UPDATE SET \
               target_session_id = excluded.target_session_id, \
               watermark = NULL, generation = NULL, \
               content_hash = excluded.content_hash, updated_at = excluded.updated_at",
            rusqlite::params![reader, tool, resource_key, target, content_hash, now_unix()],
        )?;
        Ok(())
    }

    /// Delete cursors whose reader or (non-NULL) target session no longer
    /// exists. Immediate, no retention window: unlike undelivered mail, a
    /// cursor has no one to inform once its reader or target is gone.
    pub fn sweep_orphan_read_cursors(&self) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM read_cursors WHERE \
               reader_session_id NOT IN (SELECT id FROM sessions) \
               OR (target_session_id IS NOT NULL \
                   AND target_session_id NOT IN (SELECT id FROM sessions))",
            [],
        )?)
    }
}
```

In `store/mod.rs` add `mod read_cursors;` beside the other `mod` lines and `pub use read_cursors::CursorRow;` beside the other re-exports. `now_unix` is `pub(super)` in `store/rows.rs:926` — if `super::now_unix` does not resolve, import it the way `store/participants.rs` does.

- [ ] **Step 6: GREEN, full suite, clippy**

`cargo test -p fleet-core --lib` (whole suite — quote the summary line), then `cargo clippy -p fleet-core --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 7: Commit** — `feat(store): migration 044 — remembered read cursors`

---

### Task 2: Stream reads after a watermark, and the conversation generation

**Files:** Modify `crates/fleet-core/src/store/timeline.rs`. Test in its existing `mod tests`.

**Interfaces:**
- Consumes: the tables that already exist (`session_events`, `session_messages`, `participants`).
- Produces:
  - `Store::session_events_after(&self, session_id: i64, after_id: i64, limit: i64) -> Result<Vec<SessionEvent>, IpcError>` — `id > after_id ORDER BY id ASC`
  - `Store::max_session_event_id(&self, session_id: i64) -> Result<Option<i64>, IpcError>`
  - `Store::inbox_after(&self, session_id: i64, after_id: i64, unread_only: bool, limit: i64) -> Result<Vec<SessionMessage>, IpcError>` — resolved via participant exactly as `list_inbox` is; `id > after_id ORDER BY id ASC`
  - `Store::max_inbox_id(&self, session_id: i64) -> Result<Option<i64>, IpcError>`
  - `Store::conversation_generation(&self, session_id: i64) -> Result<Option<i64>, IpcError>` — `MAX(id)` of `session_events` with `kind IN ('conversation_started','conversation_ended','compact_done')`

- [ ] **Step 1: Confirm the boundary kinds before writing a test against them.** Run `graft grep "conversation_started|conversation_ended|compact_done"` and confirm all three are real `insert_session_event` kinds. If any is spelled differently, use the real spelling everywhere below and say so in your report.

- [ ] **Step 2: Write the failing tests** (in `timeline.rs`'s `mod tests`; reuse the file's seeder or add the one from Global Constraints):

```rust
    #[test]
    fn events_after_are_oldest_first_by_id_and_exclude_the_watermark() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let e1 = s.insert_session_event(a, "prompt_sent", Some("1")).unwrap();
        let e2 = s.insert_session_event(a, "prompt_sent", Some("2")).unwrap();
        let e3 = s.insert_session_event(a, "prompt_sent", Some("3")).unwrap();
        let got: Vec<i64> = s.session_events_after(a, e1, 50).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(got, vec![e2, e3], "oldest-first, strictly after the watermark");
        assert_eq!(s.max_session_event_id(a).unwrap(), Some(e3));
    }

    /// The paging rule the cursor depends on: with a limit, the OLDEST rows
    /// after the watermark come back, so advancing to the largest id returned
    /// can never step over one that was not returned.
    #[test]
    fn events_after_with_a_limit_return_the_oldest_rows_not_the_newest() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let ids: Vec<i64> = (0..5)
            .map(|i| s.insert_session_event(a, "prompt_sent", Some(&i.to_string())).unwrap())
            .collect();
        let got: Vec<i64> = s.session_events_after(a, 0, 2).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(got, vec![ids[0], ids[1]]);
    }

    #[test]
    fn inbox_after_resolves_by_participant_and_pages_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m1 = s.insert_message(a, b, "one", "message", None).unwrap();
        let m2 = s.insert_message(a, b, "two", "message", None).unwrap();
        let m3 = s.insert_message(a, b, "three", "message", None).unwrap();
        let got: Vec<i64> = s.inbox_after(b, m1, false, 50).unwrap().iter().map(|m| m.id).collect();
        assert_eq!(got, vec![m2, m3]);
        assert_eq!(s.max_inbox_id(b).unwrap(), Some(m3));
        assert!(s.inbox_after(a, 0, false, 50).unwrap().is_empty(), "the sender's inbox is empty");
    }

    #[test]
    fn the_generation_moves_only_on_a_conversation_boundary() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        assert_eq!(s.conversation_generation(a).unwrap(), None);
        let g1 = s.insert_session_event(a, "conversation_started", None).unwrap();
        s.insert_session_event(a, "prompt_sent", None).unwrap();
        s.insert_session_event(a, "turn_done", None).unwrap();
        assert_eq!(s.conversation_generation(a).unwrap(), Some(g1), "ordinary events do not move it");
        let g2 = s.insert_session_event(a, "compact_done", None).unwrap();
        assert_eq!(s.conversation_generation(a).unwrap(), Some(g2), "a compaction does");
    }
```

- [ ] **Step 3: RED.** `cargo test -p fleet-core --lib store::timeline` — expected compile failure naming the missing methods. Quote it.

- [ ] **Step 4: Implement** in `timeline.rs`, inside `impl Store`. `inbox_after` must use the **same participant resolution as `list_inbox`** — read `list_inbox` and copy its WHERE clause shape, changing only the id filter and the order. Model:

```rust
    /// Events for `session_id` strictly after `after_id`, OLDEST first by
    /// id. The cursor path: paging oldest-first and advancing only to the
    /// last id returned is what makes a `limit` unable to skip a row. By id,
    /// never `at` — `at` is wall-clock and a clock step would reorder it.
    pub fn session_events_after(
        &self,
        session_id: i64,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, kind, detail, claude_session_id FROM session_events \
             WHERE session_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, after_id, limit], |row| {
            Ok(SessionEvent {
                id: row.get(0)?,
                session_id: row.get(1)?,
                at: row.get(2)?,
                kind: row.get(3)?,
                detail: row.get(4)?,
                claude_session_id: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn max_session_event_id(
        &self,
        session_id: i64,
    ) -> Result<Option<i64>, crate::ipc_error::IpcError> {
        Ok(self.conn.query_row(
            "SELECT MAX(id) FROM session_events WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?)
    }

    /// The id of the latest conversation boundary for `session_id`. A
    /// transcript cursor stores it: `turn_seq` keeps counting across a
    /// /clear while the transcript FILE changes, so a moved generation means
    /// the old conversation's unread tail is not in the file being read.
    pub fn conversation_generation(
        &self,
        session_id: i64,
    ) -> Result<Option<i64>, crate::ipc_error::IpcError> {
        Ok(self.conn.query_row(
            "SELECT MAX(id) FROM session_events WHERE session_id = ?1 \
             AND kind IN ('conversation_started','conversation_ended','compact_done')",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?)
    }
```

`SessionEvent`'s construction above must match `list_session_events`' mapper exactly (`timeline.rs:129-145`) — copy it from there rather than from this plan if they differ. Write `inbox_after` and `max_inbox_id` against `list_inbox`'s participant-resolved WHERE (`to_participant_id = (SELECT id FROM participants WHERE session_id = ?1)`), with `AND id > ?2 ORDER BY id ASC LIMIT ?n`, and `unread_only` handled exactly as `list_inbox` handles it.

- [ ] **Step 5: GREEN, full suite (quote it), clippy, fmt.**
- [ ] **Step 6: Commit** — `feat(store): id-ordered stream reads after a watermark, and the conversation generation`

---

### Task 3: The pure decision

**Files:** Create `crates/fleet-core/src/service/fresh.rs`; add `pub mod fresh;` to `service/mod.rs`.

**Interfaces:**
- Consumes: `store::CursorRow` (Task 1).
- Produces:
  - `pub enum ResetReason { ConversationChanged, AheadOfHead, ReaderUnknown }` with `pub fn as_str(self) -> &'static str` → `"conversation_changed"` / `"ahead_of_head"` / `"reader_unknown"`
  - `pub enum StreamStart { Full(Option<ResetReason>), After(i64), Unchanged }`
  - `pub fn decide_stream(stored: Option<&CursorRow>, head: Option<i64>, generation: Option<i64>) -> StreamStart`
  - `pub fn snapshot_hash(payload: &str) -> String`
  - `pub fn envelope(unchanged: bool, reset: Option<ResetReason>, more: bool, data: serde_json::Value) -> serde_json::Value`

`head` is `Option` because an empty stream has no max id. `generation` is `None` for tools with no generation (history, inbox) and for a transcript with no boundary yet. `None == None` counts as the same generation — **deliberately**, for exactly those two cases; test it so it is a decision and not an accident.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CursorRow;

    fn cur(w: i64, g: Option<i64>) -> CursorRow {
        CursorRow { watermark: Some(w), generation: g, content_hash: None }
    }

    #[test]
    fn no_cursor_is_a_full_read_with_no_reset() {
        assert_eq!(decide_stream(None, Some(9), None), StreamStart::Full(None));
    }

    #[test]
    fn a_cursor_at_the_head_is_unchanged() {
        assert_eq!(decide_stream(Some(&cur(9, None)), Some(9), None), StreamStart::Unchanged);
    }

    #[test]
    fn a_cursor_behind_the_head_reads_after_it() {
        assert_eq!(decide_stream(Some(&cur(4, None)), Some(9), None), StreamStart::After(4));
    }

    /// Session-id reuse (`sessions.id` has no AUTOINCREMENT): a new session
    /// starts at turn_seq 0 while an old cursor says 50.
    #[test]
    fn a_cursor_ahead_of_the_head_resets() {
        assert_eq!(
            decide_stream(Some(&cur(50, None)), Some(3), None),
            StreamStart::Full(Some(ResetReason::AheadOfHead))
        );
    }

    #[test]
    fn a_cursor_on_an_empty_stream_resets_rather_than_claiming_unchanged() {
        assert_eq!(
            decide_stream(Some(&cur(5, None)), None, None),
            StreamStart::Full(Some(ResetReason::AheadOfHead))
        );
    }

    /// The /clear case: turn_seq kept counting, the transcript file changed.
    #[test]
    fn a_moved_generation_resets_even_when_the_watermark_looks_current() {
        assert_eq!(
            decide_stream(Some(&cur(9, Some(1))), Some(9), Some(2)),
            StreamStart::Full(Some(ResetReason::ConversationChanged))
        );
        assert_eq!(
            decide_stream(Some(&cur(4, Some(1))), Some(9), Some(2)),
            StreamStart::Full(Some(ResetReason::ConversationChanged)),
            "a generation change outranks a readable delta"
        );
    }

    #[test]
    fn a_first_boundary_after_a_cursor_with_none_resets() {
        assert_eq!(
            decide_stream(Some(&cur(4, None)), Some(9), Some(7)),
            StreamStart::Full(Some(ResetReason::ConversationChanged))
        );
    }

    /// Deliberate: tools with no generation pass None on both sides.
    #[test]
    fn none_and_none_is_the_same_generation() {
        assert_eq!(decide_stream(Some(&cur(4, None)), Some(9), None), StreamStart::After(4));
    }

    #[test]
    fn a_snapshot_cursor_is_not_a_stream_cursor() {
        let snap = CursorRow { watermark: None, generation: None, content_hash: Some("h".into()) };
        assert_eq!(decide_stream(Some(&snap), Some(9), None), StreamStart::Full(None));
    }

    #[test]
    fn the_hash_is_stable_and_sensitive() {
        assert_eq!(snapshot_hash("[1,2]"), snapshot_hash("[1,2]"));
        assert_ne!(snapshot_hash("[1,2]"), snapshot_hash("[1,3]"));
        assert_eq!(snapshot_hash("x").len(), 64, "hex sha-256");
    }

    #[test]
    fn the_envelope_carries_every_flag() {
        let v = envelope(false, Some(ResetReason::ReaderUnknown), true, serde_json::json!([1]));
        assert_eq!(v["unchanged"], false);
        assert_eq!(v["cursor_reset"], "reader_unknown");
        assert_eq!(v["more"], true);
        assert_eq!(v["data"], serde_json::json!([1]));
        let quiet = envelope(true, None, false, serde_json::Value::Null);
        assert!(quiet["cursor_reset"].is_null());
    }
}
```

- [ ] **Step 2: RED** — `cargo test -p fleet-core --lib service::fresh`. Quote it.

- [ ] **Step 3: Implement**

```rust
//! The smart-caching decision (cycle 2), kept pure so every ordering of
//! stored cursor, head and generation can be tested without a store.
//!
//! The one rule: a cursor that cannot be trusted yields the FULL payload and
//! a stated reason. Never a silent skip.

use crate::store::CursorRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    /// A conversation boundary happened since the last read.
    ConversationChanged,
    /// The cursor is past the head — in practice a reused session id.
    AheadOfHead,
    /// `fresh_for` names no session. Answered, stated, nothing stored.
    ReaderUnknown,
}

impl ResetReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ResetReason::ConversationChanged => "conversation_changed",
            ResetReason::AheadOfHead => "ahead_of_head",
            ResetReason::ReaderUnknown => "reader_unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStart {
    Full(Option<ResetReason>),
    After(i64),
    Unchanged,
}

pub fn decide_stream(
    stored: Option<&CursorRow>,
    head: Option<i64>,
    generation: Option<i64>,
) -> StreamStart {
    let Some(c) = stored else {
        return StreamStart::Full(None);
    };
    let Some(w) = c.watermark else {
        // A snapshot row under a stream tool: treat as no cursor.
        return StreamStart::Full(None);
    };
    if c.generation != generation {
        return StreamStart::Full(Some(ResetReason::ConversationChanged));
    }
    match head {
        None => StreamStart::Full(Some(ResetReason::AheadOfHead)),
        Some(h) if w > h => StreamStart::Full(Some(ResetReason::AheadOfHead)),
        Some(h) if w == h => StreamStart::Unchanged,
        Some(_) => StreamStart::After(w),
    }
}

pub fn snapshot_hash(payload: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(payload.as_bytes()))
}

pub fn envelope(
    unchanged: bool,
    reset: Option<ResetReason>,
    more: bool,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "unchanged": unchanged,
        "cursor_reset": reset.map(ResetReason::as_str),
        "more": more,
        "data": data,
    })
}
```

- [ ] **Step 4: GREEN, full suite (quote), clippy, fmt.**
- [ ] **Step 5: Commit** — `feat(service): the pure fresh-read decision`

---

### Task 4: The surface — five params, one budget raise, one regen

All schema and description changes happen here, once, so the budget is measured and raised **once**. The params are inert until Tasks 5-7 wire them.

**Files:** Modify `mcp/tools/params.rs`, `mcp/tools/support.rs`, the four tool files' `#[tool(description = …)]` text only, `mcp/tools/tests.rs`, `docs/control-api-reference.md` (generated).

**Interfaces:**
- Produces:
  - `pub fresh_for: Option<i64>` (with `#[serde(default)]`) on `SessionTranscriptParams`, `SessionHistoryParams`, `InboxParams`, `ListSessionsParams`
  - `pub struct RepoDiffParams { pub session_id: i64, pub path: String, #[serde(default)] pub fresh_for: Option<i64> }` with `impl From<&RepoDiffParams> for repo_read::RepoFileArgs`
  - `pub(super) fn compact_json_string<T: serde::Serialize>(value: &T, view: Option<&[&str]>) -> Result<String, McpError>` in `support.rs`, with `ok_json_compact_view` rewritten to call it

- [ ] **Step 1: Write the failing test** in `tests.rs` — the byte-identity guarantee, modelled on `a_view_is_opt_in_and_the_default_answer_is_byte_identical` (find it with `graft grep "a_view_is_opt_in"` and follow its fixture). For each of the five tools, call once with `fresh_for` omitted and once with `fresh_for: None` explicitly deserialized, and assert the two responses are byte-identical and equal to the response before this cycle. For `repo_diff`, assert its schema is now named `RepoDiffParams` while `repo_file`'s is still `RepoPathParams` — proof the shared hub-routed struct did not change.

- [ ] **Step 2: RED.** Quote it.

- [ ] **Step 3: Add the params.** Each field gets exactly this doc comment (one clause — the prose goes in `docs/control-api.md`, Task 8):

```rust
    /// Your own session id: return only what is new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
```

`RepoDiffParams` in `params.rs`:

```rust
/// `repo_diff`'s own params. Deliberately NOT `repo_read::RepoFileArgs`:
/// that struct is shared with `repo_file` and routed desktop→hub, so a new
/// field there would leak onto `repo_file` and change a wire struct.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoDiffParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
    /// Your own session id: return only what is new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
}

impl From<&RepoDiffParams> for crate::service::repo_read::RepoFileArgs {
    fn from(p: &RepoDiffParams) -> Self {
        Self { session_id: p.session_id, path: p.path.clone() }
    }
}
```

Change `repo_diff`'s signature in `repo.rs` to `Parameters(p): Parameters<RepoDiffParams>` and call `repo_read::repo_diff((&p).into(), …)` — behaviour unchanged.

- [ ] **Step 4: Extract the string builder** in `support.rs`, so a snapshot can hash exactly the bytes it returns:

```rust
/// The compact JSON string [`ok_json_compact_view`] returns — split out so a
/// snapshot tool can hash exactly the bytes it would send.
pub(super) fn compact_json_string<T: serde::Serialize>(
    value: &T,
    view: Option<&[&str]>,
) -> Result<String, McpError> {
    let mut v = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    if let Some(fields) = view {
        super::views::project_rows(&mut v, fields);
    }
    strip_nulls(&mut v);
    serde_json::to_string(&v)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))
}

pub(super) fn ok_json_compact_view<T: serde::Serialize>(
    value: &T,
    view: Option<&[&str]>,
) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![text_content(compact_json_string(value, view)?)]))
}
```

Keep `ok_json_compact_view`'s existing doc comment.

- [ ] **Step 5: Descriptions.** Append one sentence to each of the five tools' `description`: `fresh_for returns only what is new since your last read.` For `session_transcript` add: `unchanged costs no transcript read.` Nothing more — every byte is paid on every request by every caller.

- [ ] **Step 6: Measure and raise the budget once.** Run `cargo test -p fleet-core --lib the_served_definition_budget_stays_bounded -- --nocapture` and read the measured bytes it prints. Set `BUDGET_BYTES` to that plus the customary 100, and append a comment in the established style: raised for smart caching (cycle 2) — one `fresh_for` on each of five tools plus `RepoDiffParams`; descriptions cut to one clause first; measured N, set to N+100. **Do not raise it by a guessed amount.**

- [ ] **Step 7: Regenerate the reference.** `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, then without the env var; it must pass.

- [ ] **Step 8: Full suite (quote), clippy, fmt.**
- [ ] **Step 9: Commit** — `feat(mcp): fresh_for on five fetch tools (inert), one budget raise`

---

### Task 5: Wire `session_transcript`

**Files:** Modify `mcp/tools/orchestration.rs` (`session_transcript`, ~L54). Tests in `mcp/tools/tests.rs`.

**Interfaces:** Consumes `fresh::{decide_stream, StreamStart, ResetReason}`, `Store::{get_read_cursor, put_stream_cursor, conversation_generation, get_session_by_id}`, the existing `self.transcript_for(&row, since_turn, max_chars)`.

The tool returns **text**, so the flags are text lines, not an envelope:

- `Unchanged` → `(unchanged since your last read at turn {w})` — and **no call to `transcript_for`**.
- `After(w)` → `transcript_for(&row, Some(w), p.max_chars)`, cursor advanced to `row.turn_seq`.
- `Full(reason)` → `transcript_for(&row, None, p.max_chars)` (today's default: the last turn), prefixed with `[cursor reset: {reason} — earlier turns may not be shown; see session_conversations]` when `reason` is `Some`. Cursor set to `row.turn_seq` unless the reason is `ReaderUnknown`.

Resolve everything that needs the store — reader existence, the stored cursor, the generation — in **one** lock scope that ends before `transcript_for` is awaited. Write the cursor in a second short scope *after* the transcript read succeeds; if the read errors, do not advance the cursor.

`max_chars` trims the **front** of a long delta (`render_tail`, `service/transcript.rs`, pinned by `render_tail_keeps_the_last_turns_and_trims_from_the_front`, whose marker starts `[session_transcript: N chars dropped`). In the `After` branch, if the returned text starts with `[session_transcript:`, append on a new line: `[cursor: the oldest new turns were cut by max_chars; re-read with since_turn={w} and a larger max_chars]`. The cursor still advances — the loss is visible and recoverable, and not advancing would loop forever on the same truncated delta.

- [ ] **Step 1: Failing tests.** The decisive one proves the unchanged path reads nothing remote:

```rust
#[tokio::test]
async fn an_unchanged_transcript_read_touches_no_transcript_at_all() {
    // A target whose transcript can NOT be read: host with no reachable
    // ssh, no transcript path. Any read attempt would error.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("nowhere").unwrap();
    let reader = s.upsert_session("reader", "nowhere", None, None, 0, 0, "running", None).unwrap();
    let target = s.upsert_session("target", "nowhere", None, None, 0, 0, "running", None).unwrap();
    s.set_claude_session_id(target, "conv-1").unwrap();
    for _ in 0..3 { s.record_stop_hook_for_row(target).unwrap(); } // turn_seq = 3
    s.put_stream_cursor(reader, "session_transcript", &target.to_string(), Some(target), 3, None).unwrap();
    let t = test_tools(s);
    let out = t
        .session_transcript(
            Extension(Caller::master()),
            Parameters(SessionTranscriptParams {
                session_id: target, since_turn: None, max_chars: None, fresh_for: Some(reader),
            }),
        )
        .await
        .expect("unchanged must answer without reading the transcript");
    assert!(result_text(&out).starts_with("(unchanged since your last read at turn 3)"));
}
```

Use the file's existing helpers for building tools and reading a result's text (`test_tools`, and whatever it uses to extract text — find them with `graft grep "fn test_tools"`). Confirm `set_claude_session_id` and `record_stop_hook_for_row` signatures with `graft grep` before relying on them; adapt calls to the real ones and say so in your report. Also test: an unknown `fresh_for` yields the `reader_unknown` reset line and stores no cursor; a moved generation yields the `conversation_changed` line. Those two need a readable transcript — if the fixture cannot provide one, assert them at the decision level (Task 3 already does) plus a store assertion that no cursor was written, and say which you chose.

- [ ] **Step 2: RED** (quote). **Step 3: implement.** **Step 4: full suite (quote), clippy, fmt.**
- [ ] **Step 5: Commit** — `feat(mcp): session_transcript answers only what is new`

---

### Task 6: Wire `session_history` and `inbox`

**Files:** Modify `mcp/tools/messaging.rs` (`session_history` ~L189, `inbox` ~L385). Tests in `mcp/tools/tests.rs`.

**Interfaces:** Consumes `fresh::{decide_stream, envelope, StreamStart, ResetReason}`, Task 1's cursor store, Task 2's `session_events_after`, `max_session_event_id`, `inbox_after`, `max_inbox_id`.

With `fresh_for` present both tools return `fresh::envelope(unchanged, reset, more, data)` with `data` the row array, **oldest-first**:

- `Unchanged` → `envelope(true, None, false, [])`.
- `After(w)` → rows = `*_after(target, w, limit + 1)`; `more = rows.len() > limit`; truncate to `limit`; advance the cursor to the **last returned row's id** (not the head).
- `Full(reason)` → rows = `*_after(target, 0, limit + 1)`, same `more` rule, same advance rule, `reset = reason`.
- No generation for either tool: pass `None` as both stored and current generation.
- `ReaderUnknown` → answer as `Full`, write no cursor.

`session_history` has **no `caller` parameter** today — do not add a host gate as part of this task; it is not in scope. `inbox`'s existing host gate and `mark_read` semantics are unchanged: `mark_read` applies to the rows actually returned, exactly as today.

Note for the report: `inbox` already has a consuming "only new" mechanism (`unread_only` + `mark_read`). `fresh_for` adds a **non-consuming, per-reader** delta — e.g. a controller watching a worker's inbox without marking it read. Say whether the implementation made that distinction observable in a test.

- [ ] **Step 1: Failing tests** — the silent-skip regression first:

```rust
/// Newest-first + limit + advance-to-head would skip rows. Oldest-first,
/// advancing only to what was returned, cannot.
#[tokio::test]
async fn a_history_cursor_that_falls_behind_pages_through_everything() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let reader = s.upsert_session("reader", "local", None, None, 0, 0, "running", None).unwrap();
    let target = s.upsert_session("target", "local", None, None, 0, 0, "running", None).unwrap();
    let ids: Vec<i64> = (0..5)
        .map(|i| s.insert_session_event(target, "prompt_sent", Some(&i.to_string())).unwrap())
        .collect();
    let t = test_tools(s);
    let mut seen = Vec::new();
    for _ in 0..4 {
        let out = t.session_history(Parameters(SessionHistoryParams {
            session_id: target, limit: Some(2), fresh_for: Some(reader),
        })).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&result_text(&out)).unwrap();
        for e in v["data"].as_array().unwrap() { seen.push(e["id"].as_i64().unwrap()); }
        if v["unchanged"] == true { break; }
    }
    assert_eq!(seen, ids, "every event exactly once, in order, none skipped");
}
```

Plus: a second call right after catching up returns `unchanged: true` with an empty `data`; two different readers of the same target see the same full sequence independently; `inbox` with `fresh_for` and `mark_read: false` leaves `read_at` untouched.

- [ ] **Step 2: RED** (quote). **Step 3: implement.** **Step 4: full suite (quote), clippy, fmt.**
- [ ] **Step 5: Commit** — `feat(mcp): session_history and inbox page only what is new`

---

### Task 7: Wire the snapshots — `repo_diff` and `list_sessions`

**Files:** Modify `mcp/tools/repo.rs` (`repo_diff`), `mcp/tools/session_ops.rs` (`list_sessions`, the final `match (view, p.summary)` at ~L112). Tests in `mcp/tools/tests.rs`.

**Interfaces:** Consumes `support::compact_json_string` (Task 4), `fresh::{snapshot_hash, envelope, ResetReason}`, `Store::{get_read_cursor, put_snapshot_cursor}`.

- Build the response string exactly as today (`compact_json_string` with the same `view` argument the branch would have used; `repo_diff` today uses `ok_json`, so hash `serde_json::to_string(&v)` there — whatever the tool would return, byte for byte).
- `fresh_for` absent → return it unchanged (Task 4's byte-identity test must stay green).
- Present → hash it; equal to the stored hash → `envelope(true, None, false, null)`; else store the new hash and return `envelope(false, reset, false, <the payload parsed back to a Value>)`.
- `ReaderUnknown` → `envelope(false, Some(ReaderUnknown), false, payload)`, no cursor.
- `resource_key`: `repo_diff` → `format!("{session_id}:{path}")`, target `Some(session_id)`. `list_sessions` → a fingerprint of every filter param (`host_alias`, `project_id`, `status`, `claude_status`, `include_lost`, `summary`, `limit`, `tag`, `view`, `needs_attention` — **not** `force` and **not** `fresh_for`), target `None`. Two different filters must never share a cursor.

State this in the `list_sessions` doc comment: the default slim shape drops `last_activity_at` and `current_activity` (per the comment already at `session_ops.rs:~106`), so `unchanged` fires usefully there; `summary: false` includes them, so it will rarely fire in full mode — the hash is over exactly what is returned, never a curated subset.

For `repo_diff`, state in the doc comment that the diff is still computed to hash it: the saving is context and transfer, not server work.

- [ ] **Step 1: Failing tests:** two identical `list_sessions` calls with the same `fresh_for` → the second is `unchanged: true`; a status change between them → `unchanged: false` with the payload; the same reader with two different filters keeps two cursors (the second filter's first call is never `unchanged`).
- [ ] **Step 2: RED** (quote). **Step 3: implement.** **Step 4: full suite (quote), clippy, fmt.**
- [ ] **Step 5: Commit** — `feat(mcp): repo_diff and list_sessions answer unchanged`

---

### Task 8: Retention, docs, and the full gate

**Files:** Modify `service/gc.rs` (`sweep_with`, beside the `sweep_retired_participants` call at ~L339), `docs/control-api.md`.

- [ ] **Step 1: Failing test** in `gc.rs`: with `gc.enabled = false`, a sweep removes a cursor whose reader was deleted and keeps a live one — the retention path must not be gated on the opt-in session killer, which cycle 1 got wrong once.
- [ ] **Step 2: RED** (quote).
- [ ] **Step 3: Implement.** `sweep_with` returns `GcReport`, not a `Result`: follow its existing best-effort pattern (no `?`; a failed sweep contributes 0). Add `swept_read_cursors: usize` to `GcReport` with `#[serde(default)]`.
- [ ] **Step 4: Prose.** In `docs/control-api.md`, a *Remembered read cursors* section: what `fresh_for` is (your own session id, not the caller's, and why); the three answers (delta / `unchanged` / full with `cursor_reset`) and the three reasons; oldest-first paging with `more`; that `repo_diff`'s `unchanged` still computes the diff; that `list_sessions` `unchanged` is useful in the default slim shape and rare in full mode.
- [ ] **Step 5: The full gate, in order, output read directly — never through `| tail`:**
  `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo test --workspace` → `npx vitest run` → `npx svelte-check` → `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` then without → `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` then without (it must produce **no diff**: `fresh_for` absent ⇒ no wire change) → `scripts/ci-local.sh --hub-e2e`.
- [ ] **Step 6: Commit** — `feat(gc): sweep orphan read cursors; document fresh_for`

## Self-Review

**Spec coverage.** Migration 044 → 1. Reader-keyed cursors and the two-readers regression → 1. Per-tool watermarks → 2, 5, 6. Untrusted-cursor rules → 3 (decision), 5-6 (wiring). Snapshot hashes → 3, 7. One param per tool → 4. Budget measured once → 4. Retention → 1 (sweep), 8 (called). Testing → every task, plus the no-read proof in 5.

**Deviations from the spec, all stated above:** the `generation` column (a `/clear` silent-skip the spec missed); `ahead_of_head` caused by id reuse, not a `turn_seq` restart; no retention window for cursors; `reader_unknown` made explicit; the no-SSH claim proven by a unit test as well; `repo_diff` on its own params struct; streams paging oldest-first. The spec's budget figure (62,640) is stale — it was 63,730 when this plan was written and will move again.

**Placeholder scan.** Clean. Where a helper's exact name is unknown from the plan (`test_tools`, `result_text`, `set_claude_session_id`), the step says to confirm it with a named `graft grep` and adapt — an instruction with a target, not a gap.

**Type consistency.** `CursorRow` (1) → consumed by `decide_stream` (3). `StreamStart` / `ResetReason` / `envelope` / `snapshot_hash` (3) → 5, 6, 7. `compact_json_string` (4) → 7. `RepoDiffParams` (4) → 7. `session_events_after` / `inbox_after` / `max_*` / `conversation_generation` (2) → 5, 6.
