# Hub Error Channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The hub collects error-level events from the hub-client desktop, every `fleet-agent` and itself into one bounded, redacted table an operator reads with `fleet-hub reports`.

**Architecture:** A shared `Report` record and a bounded `ReportRing` live in `fleet-proto`; a small `tracing` layer in each binary captures only `ERROR` events into the process-wide ring. The desktop flushes its ring to `POST /report` on a timer; the agent flushes on its heartbeat in a new `AgentFrame::Report`; the hub drains its own ring on the reconcile tick. One `service::reports::ingest` function clamps, redacts, rate-limits and stores everything in `error_reports`, capped by rows on insert and by age on the tick; `GET /reports` and the CLI read it back.

**Tech Stack:** Rust (axum 0.8, tracing-subscriber, rusqlite, tokio, serde), Svelte 5 / TypeScript (Vitest), clap.

**Spec:** `docs/superpowers/specs/2026-09-21-hub-error-channel-design.md`

## Global Constraints

- Caps, copied from the spec and defined once in `fleet_proto::report`: `COMPONENT_MAX = 64`, `MESSAGE_MAX = 2_048`, `CONTEXT_MAX = 4_096`, `HTTP_BATCH_MAX = 50`, `FRAME_BATCH_MAX = 16`, `RING_CAP = 256`, `BODY_MAX = 64 * 1024`, `RATE_PER_MINUTE = 60`.
- Settings: `reports.max_rows` (`Int { min: 100, max: 100_000 }`, default `5000`); `reports.max_age_secs` (`Secs`, default `604800`, `0` disables the age sweep).
- Only `Level::ERROR` events are captured. `level` on the wire is `"error"` or `"warn"`; anything else is `E_VALIDATE`.
- Origin is derived from the caller's token (`Caller::label()`) or the connection's alias, never from a body.
- `fleet-proto` gains no dependency. `fleet-agent` depends on `fleet-proto` only (never `fleet-core`).
- No `PROTO_VERSION` bump, no `wire_contract` bump.
- The print family (`eprintln!`, `println!`, `dbg!`) is forbidden in production code (`no_eprintln_tests.rs`). Use `tracing::{error,warn,info,debug}!`.
- A tracing layer must never log or take a lock that logging could re-enter.
- Never hold the `Store` mutex across an `.await`.
- The desktop off switch is the env var `CLAUDE_FLEET_HUB_REPORTS` (`0` / `false` disables). The agent's is the config field `report_errors` (default `true`).
- Commits: Conventional Commits, no attribution lines. Never edit generated files by hand — regenerate with `REGEN_DOCS=1`, `REGEN_HUB_VERDICTS=1`, `REGEN_HUB_CONTRACT=1`.
- Cargo commands in this worktree: `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/hub-error-channel-debug-b701a6` first (the shared target otherwise mixes worktrees). Frontend: `npx vitest run <file>` and `npx svelte-check` (the `pnpm` scripts are not on PATH).

---

## File map

| File | Responsibility |
|---|---|
| `crates/fleet-proto/src/report.rs` (new) | `Report`, `ReportBatch`, caps, `clamp`, `ReportRing` |
| `crates/fleet-proto/src/lib.rs` | `pub mod report`; `AgentFrame::Report` |
| `crates/fleet-core/migrations/040_error_reports.sql` (new) | the table |
| `crates/fleet-core/src/store/schema.rs` | register migration 040 |
| `crates/fleet-core/src/store/reports.rs` (new) | insert / prune / sweep / list |
| `crates/fleet-core/src/service/settings.rs` | the two settings |
| `crates/fleet-core/src/service/reports.rs` (new) | `ingest`, `RateWindows`, the hub's own drain, the age sweep |
| `crates/fleet-core/src/logging.rs` | `ReportLayer`, the global ring |
| `crates/fleet-core/src/mcp/report_route.rs` (new) | `POST /report`, `GET /reports`, `ReportState` |
| `crates/fleet-core/src/mcp/mod.rs` | mount the routes |
| `crates/fleet-core/src/agent/ws.rs`, `agent/registry.rs` | accept `AgentFrame::Report` |
| `crates/fleet-core/src/service/tick.rs` | age sweep + hub ring drain on the tick |
| `crates/fleet-agent/src/report.rs` (new) | the agent's `ReportLayer` |
| `crates/fleet-agent/src/{main,config,conn}.rs` | install the layer, `report_errors`, heartbeat flush |
| `src-tauri/src/backend/report.rs` (new) | the desktop flusher |
| `src-tauri/src/backend/{startup,mod,verdicts,remote}.rs`, `bootstrap/tasks.rs`, `commands/hub.rs`, `lib.rs` | wiring, the command, its verdict |
| `src/lib/error_report.ts` (new), `src/main.ts`, `src/lib/toasts.ts` | frontend capture |
| `crates/fleet-hub/src/reports.rs` (new), `main.rs`, `pair.rs` | the CLI |
| `docs/hub.md` | documentation |

---

### Task 1: `fleet_proto::report` — the record, the batch, the ring

**Files:**
- Create: `crates/fleet-proto/src/report.rs`
- Modify: `crates/fleet-proto/src/lib.rs` (add `pub mod report;` after `pub mod net;`)

**Interfaces:**
- Produces: `Report { at: i64, level: String, component: String, code: Option<String>, message: String, context: Option<serde_json::Value>, truncated: bool }` with `Report::error(component, message) -> Report`, `Report::clamp(&mut self)`; `ReportBatch { reports: Vec<Report>, dropped: u32 }`; `ReportRing::new() -> ReportRing`, `push(&self, Report)`, `drain(&self, n: usize) -> ReportBatch`, `len(&self) -> usize`; the constants listed in Global Constraints; `pub fn now_unix() -> i64`.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleet-proto/src/report.rs` with only the test module for now:

```rust
//! Error reports: the record a participant sends the hub, the batch it
//! travels in, and the bounded queue every sender keeps.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

#[cfg(test)]
mod tests {
    use super::*;

    fn r(msg: &str) -> Report {
        Report::error("fleet_core::ssh", msg)
    }

    #[test]
    fn clamp_cuts_the_message_at_a_char_boundary_and_marks_it() {
        let mut x = r(&"é".repeat(MESSAGE_MAX + 5));
        x.clamp();
        assert!(x.truncated);
        assert_eq!(x.message.chars().count(), MESSAGE_MAX);
        assert!(x.message.is_char_boundary(x.message.len()));
    }

    #[test]
    fn clamp_drops_an_oversize_context_and_a_long_component() {
        let mut x = r("m");
        x.context = Some(serde_json::json!({ "stack": "x".repeat(CONTEXT_MAX) }));
        x.component = "c".repeat(COMPONENT_MAX + 1);
        x.clamp();
        assert!(x.context.is_none());
        assert!(x.truncated);
        assert_eq!(x.component.len(), COMPONENT_MAX);
    }

    #[test]
    fn clamp_leaves_a_small_report_alone() {
        let mut x = r("fine");
        x.context = Some(serde_json::json!({ "k": 1 }));
        x.clamp();
        assert!(!x.truncated);
        assert_eq!(x.context, Some(serde_json::json!({ "k": 1 })));
    }

    #[test]
    fn the_ring_evicts_the_oldest_and_counts_drops() {
        let ring = ReportRing::new();
        for i in 0..(RING_CAP + 3) {
            ring.push(r(&format!("m{i}")));
        }
        assert_eq!(ring.len(), RING_CAP);
        let b = ring.drain(2);
        assert_eq!(b.dropped, 3);
        assert_eq!(b.reports[0].message, "m3", "the oldest survivor first");
        assert_eq!(b.reports[1].message, "m4");
        assert_eq!(ring.len(), RING_CAP - 2);
        assert_eq!(ring.drain(1).dropped, 0, "the drop count resets");
    }

    #[test]
    fn a_batch_round_trips_as_json() {
        let b = ReportBatch { reports: vec![r("x")], dropped: 2 };
        let text = serde_json::to_string(&b).unwrap();
        let back: ReportBatch = serde_json::from_str(&text).unwrap();
        assert_eq!(back, b);
        let sparse: ReportBatch = serde_json::from_str(r#"{"reports":[]}"#).unwrap();
        assert_eq!(sparse.dropped, 0, "dropped defaults");
    }
}
```

Add `pub mod report;` to `crates/fleet-proto/src/lib.rs` under `pub mod net;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p fleet-proto report::`
Expected: compile error, `Report` not found.

- [ ] **Step 3: Write the implementation**

Above the test module in `report.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const COMPONENT_MAX: usize = 64;
pub const MESSAGE_MAX: usize = 2_048;
/// Bytes of the serialized `context`.
pub const CONTEXT_MAX: usize = 4_096;
/// Reports per `POST /report`.
pub const HTTP_BATCH_MAX: usize = 50;
/// Reports per `AgentFrame::Report`.
pub const FRAME_BATCH_MAX: usize = 16;
/// Reports a sender queues before dropping the oldest.
pub const RING_CAP: usize = 256;
/// Bytes of a `POST /report` body.
pub const BODY_MAX: usize = 64 * 1024;
/// Reports one origin may store per minute.
pub const RATE_PER_MINUTE: u32 = 60;

/// One error a participant reports. Every string is bounded by
/// [`Report::clamp`], which every sender calls before queueing and the hub
/// calls again at ingest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The sender's clock, unix seconds.
    pub at: i64,
    /// `"error"` or `"warn"`.
    pub level: String,
    /// The tracing target, or `frontend` / `frontend:unhandled`.
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub truncated: bool,
}

/// What travels: the reports and how many the sender dropped since its
/// previous batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportBatch {
    #[serde(default)]
    pub reports: Vec<Report>,
    #[serde(default)]
    pub dropped: u32,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Cut `s` to at most `max` chars, on a char boundary. Returns whether it cut.
fn cut(s: &mut String, max: usize) -> bool {
    match s.char_indices().nth(max) {
        Some((i, _)) => {
            s.truncate(i);
            true
        }
        None => false,
    }
}

impl Report {
    pub fn error(component: &str, message: &str) -> Self {
        Report {
            at: now_unix(),
            level: "error".into(),
            component: component.into(),
            code: None,
            message: message.into(),
            context: None,
            truncated: false,
        }
    }

    /// Apply the caps in place, setting `truncated` when anything was cut.
    pub fn clamp(&mut self) {
        let mut cut_any = cut(&mut self.message, MESSAGE_MAX);
        cut_any |= cut(&mut self.component, COMPONENT_MAX);
        if let Some(c) = &mut self.code {
            cut_any |= cut(c, COMPONENT_MAX);
        }
        if let Some(ctx) = &self.context {
            let bytes = serde_json::to_vec(ctx).map(|v| v.len()).unwrap_or(usize::MAX);
            if bytes > CONTEXT_MAX {
                self.context = None;
                cut_any = true;
            }
        }
        self.truncated |= cut_any;
    }
}

/// A bounded queue of reports. The critical section is a push or a pop:
/// nothing here formats, allocates unpredictably or logs, because a tracing
/// layer calls [`ReportRing::push`] from inside an event.
#[derive(Debug, Default)]
pub struct ReportRing {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    queue: VecDeque<Report>,
    dropped: u32,
}

impl ReportRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, report: Report) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.queue.len() >= RING_CAP {
            g.queue.pop_front();
            g.dropped = g.dropped.saturating_add(1);
        }
        g.queue.push_back(report);
    }

    /// Up to `n` oldest reports and the drop count since the last drain.
    pub fn drain(&self, n: usize) -> ReportBatch {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let take = n.min(g.queue.len());
        let reports = g.queue.drain(..take).collect();
        let dropped = std::mem::take(&mut g.dropped);
        ReportBatch { reports, dropped }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p fleet-proto report::`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-proto/src/report.rs crates/fleet-proto/src/lib.rs
git commit -m "feat(proto): error report record, batch and bounded ring"
```

---

### Task 2: `AgentFrame::Report`

**Files:**
- Modify: `crates/fleet-proto/src/lib.rs` (the `AgentFrame` enum at ~line 396)
- Modify: `crates/fleet-core/src/agent/registry.rs:432` (`agent_frame_id`), `crates/fleet-core/src/agent/ws.rs:1003` (`answered_id`)

**Interfaces:**
- Produces: `AgentFrame::Report { reports: Vec<report::Report>, dropped: u32 }`.

- [ ] **Step 1: Write the failing tests** in `lib.rs`'s existing `mod tests`:

```rust
#[test]
fn a_report_frame_round_trips_and_a_full_batch_fits_the_frame_cap() {
    let mut big = report::Report::error(&"c".repeat(report::COMPONENT_MAX), &"m".repeat(report::MESSAGE_MAX));
    big.context = Some(serde_json::json!({ "s": "x".repeat(report::CONTEXT_MAX - 20) }));
    big.code = Some("E_SSH".into());
    let frame = AgentFrame::Report {
        reports: vec![big; report::FRAME_BATCH_MAX],
        dropped: 7,
    };
    let text = encode_agent_frame(&frame).expect("encodes");
    assert!(text.len() < MAX_FRAME_BYTES);
    assert_eq!(decode_agent_frame(&text).unwrap(), frame);
    // Sparse on the wire: both fields default.
    assert_eq!(
        decode_agent_frame(r#"{"kind":"report"}"#).unwrap(),
        AgentFrame::Report { reports: vec![], dropped: 0 }
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleet-proto a_report_frame_round_trips`
Expected: compile error, no variant `Report`.

- [ ] **Step 3: Add the variant** after `Pong` in `AgentFrame`:

```rust
    /// A batch of the agent's own error-level log events, sent on a
    /// heartbeat once welcomed. Answers nothing and is answered by nothing.
    /// A hub older than this variant skips it under the unknown-kind rule
    /// (crate doc), so `PROTO_VERSION` does not move for it.
    Report {
        #[serde(default)]
        reports: Vec<report::Report>,
        #[serde(default)]
        dropped: u32,
    },
```

Then make fleet-core compile: in `registry.rs` `agent_frame_id` change the last arm to `AgentFrame::Hello { .. } | AgentFrame::Report { .. } => None,` and the same in `ws.rs` `answered_id`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p fleet-proto && cargo build -p fleet-core`
Expected: all pass; fleet-core builds (any other `match` on `AgentFrame` the compiler names gets a `Report { .. } =>` arm that does nothing).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-proto/src/lib.rs crates/fleet-core/src/agent/registry.rs crates/fleet-core/src/agent/ws.rs
git commit -m "feat(proto): AgentFrame::Report carries an agent's error batch"
```

---

### Task 3: Migration 040 and `store/reports.rs`

**Files:**
- Create: `crates/fleet-core/migrations/040_error_reports.sql`, `crates/fleet-core/src/store/reports.rs`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS tail ~line 314; `EXPECTED_TABLES` in its tests ~line 513), `crates/fleet-core/src/store/mod.rs` (`mod reports;` + `pub use reports::{ReportFilter, ReportRow};`)

**Interfaces:**
- Produces on `Store`: `insert_reports(&self, origin: &str, reports: &[fleet_proto::report::Report], received_at: i64) -> Result<usize, IpcError>`; `prune_reports_to(&self, max_rows: u64) -> Result<usize, IpcError>`; `sweep_reports_older_than(&self, cutoff: i64) -> Result<usize, IpcError>`; `list_reports(&self, f: &ReportFilter) -> Result<Vec<ReportRow>, IpcError>`.
- `ReportFilter { limit: u32, since: Option<i64>, origin: Option<String>, level: Option<String> }` (`Default`: limit 100).
- `ReportRow { id: i64, received_at: i64, at: i64, origin: String, level: String, component: String, code: Option<String>, message: String, context: Option<serde_json::Value>, truncated: bool }` (`Serialize, Deserialize, Clone, Debug, PartialEq`).

- [ ] **Step 1: The migration**

```sql
-- Error reports collected from every participant (spec
-- 2026-09-21-hub-error-channel-design.md). Bounded by reports.max_rows
-- (pruned on insert) and reports.max_age_secs (swept on the tick).
CREATE TABLE IF NOT EXISTS error_reports (
  id          INTEGER PRIMARY KEY,
  received_at INTEGER NOT NULL,
  at          INTEGER NOT NULL,
  origin      TEXT    NOT NULL,
  level       TEXT    NOT NULL,
  component   TEXT    NOT NULL,
  code        TEXT,
  message     TEXT    NOT NULL,
  context     TEXT,
  truncated   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS error_reports_recent
  ON error_reports(received_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS error_reports_by_origin
  ON error_reports(origin, received_at DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (40);
```

Register it in `schema.rs` after the 039 entry: `Migration::plain(40, include_str!("../../migrations/040_error_reports.sql")),` and add `"error_reports"` to `EXPECTED_TABLES` in the schema tests.

- [ ] **Step 2: Write the failing store tests** — `store/reports.rs`:

```rust
//! The `error_reports` table: what the hub keeps of every participant's
//! error-level events. Bounded on insert (row cap) and on the tick (age).

use super::*;
use fleet_proto::report::Report;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportRow {
    pub id: i64,
    pub received_at: i64,
    pub at: i64,
    pub origin: String,
    pub level: String,
    pub component: String,
    pub code: Option<String>,
    pub message: String,
    pub context: Option<serde_json::Value>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReportFilter {
    pub limit: u32,
    pub since: Option<i64>,
    pub origin: Option<String>,
    pub level: Option<String>,
}

impl Default for ReportFilter {
    fn default() -> Self {
        ReportFilter { limit: 100, since: None, origin: None, level: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn r(msg: &str) -> Report {
        Report::error("fleet_core::ssh", msg)
    }

    #[test]
    fn insert_then_list_newest_first_with_filters() {
        let s = store();
        let mut a = r("first");
        a.code = Some("E_SSH".into());
        a.context = Some(serde_json::json!({ "k": 1 }));
        assert_eq!(s.insert_reports("client:desk", &[a.clone()], 100).unwrap(), 1);
        assert_eq!(s.insert_reports("host:box", &[r("second")], 200).unwrap(), 1);
        let rows = s.list_reports(&ReportFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].message, "second");
        assert_eq!(rows[1].code.as_deref(), Some("E_SSH"));
        assert_eq!(rows[1].context, Some(serde_json::json!({ "k": 1 })));
        assert_eq!(rows[1].origin, "client:desk");
        let since = s.list_reports(&ReportFilter { since: Some(150), ..Default::default() }).unwrap();
        assert_eq!(since.len(), 1);
        let origin = s.list_reports(&ReportFilter { origin: Some("client:desk".into()), ..Default::default() }).unwrap();
        assert_eq!(origin[0].message, "first");
        let limited = s.list_reports(&ReportFilter { limit: 1, ..Default::default() }).unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn prune_keeps_the_newest_rows() {
        let s = store();
        for i in 0..10 {
            s.insert_reports("hub", &[r(&format!("m{i}"))], 1000 + i).unwrap();
        }
        assert_eq!(s.prune_reports_to(3).unwrap(), 7);
        let rows = s.list_reports(&ReportFilter::default()).unwrap();
        assert_eq!(rows.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(), ["m9", "m8", "m7"]);
    }

    #[test]
    fn sweep_deletes_only_older_rows() {
        let s = store();
        s.insert_reports("hub", &[r("old")], 100).unwrap();
        s.insert_reports("hub", &[r("new")], 500).unwrap();
        assert_eq!(s.sweep_reports_older_than(300).unwrap(), 1);
        assert_eq!(s.list_reports(&ReportFilter::default()).unwrap()[0].message, "new");
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p fleet-core store::reports::`
Expected: compile error, no method `insert_reports`.

- [ ] **Step 4: Implement** (between the structs and the tests):

```rust
impl Store {
    /// Append a batch under `origin`. Best-effort caller contract: a failure
    /// is logged and never blocks the sender.
    pub fn insert_reports(
        &self,
        origin: &str,
        reports: &[Report],
        received_at: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for r in reports {
            let context = r
                .context
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .unwrap_or(None);
            tx.execute(
                "INSERT INTO error_reports \
                 (received_at, at, origin, level, component, code, message, context, truncated) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    received_at, r.at, origin, r.level, r.component, r.code, r.message,
                    context, r.truncated as i64
                ],
            )?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Keep the `max_rows` newest rows; returns how many were deleted.
    pub fn prune_reports_to(&self, max_rows: u64) -> Result<usize, crate::ipc_error::IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM error_reports WHERE id NOT IN (\
               SELECT id FROM error_reports ORDER BY received_at DESC, id DESC LIMIT ?1)",
            rusqlite::params![max_rows as i64],
        )?)
    }

    /// Delete rows received before `cutoff`; returns how many.
    pub fn sweep_reports_older_than(&self, cutoff: i64) -> Result<usize, crate::ipc_error::IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM error_reports WHERE received_at < ?1",
            rusqlite::params![cutoff],
        )?)
    }

    /// Newest first, filtered.
    pub fn list_reports(&self, f: &ReportFilter) -> Result<Vec<ReportRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, received_at, at, origin, level, component, code, message, context, truncated \
             FROM error_reports \
             WHERE (?1 IS NULL OR received_at >= ?1) \
               AND (?2 IS NULL OR origin = ?2) \
               AND (?3 IS NULL OR level = ?3) \
             ORDER BY received_at DESC, id DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![f.since, f.origin, f.level, f.limit as i64],
            |row| {
                let context: Option<String> = row.get(8)?;
                Ok(ReportRow {
                    id: row.get(0)?,
                    received_at: row.get(1)?,
                    at: row.get(2)?,
                    origin: row.get(3)?,
                    level: row.get(4)?,
                    component: row.get(5)?,
                    code: row.get(6)?,
                    message: row.get(7)?,
                    context: context.and_then(|c| serde_json::from_str(&c).ok()),
                    truncated: row.get::<_, i64>(9)? != 0,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}
```

Add `mod reports;` and `pub use reports::{ReportFilter, ReportRow};` to `store/mod.rs`.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p fleet-core store::`
Expected: the three new tests pass; the schema tests (`LATEST_SCHEMA_VERSION`, expected tables) still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/migrations/040_error_reports.sql crates/fleet-core/src/store
git commit -m "feat(store): error_reports table with row and age pruning"
```

---

### Task 4: Settings and `service::reports::ingest`

**Files:**
- Modify: `crates/fleet-core/src/service/settings.rs` (constants near line 133, `SPECS` tail)
- Create: `crates/fleet-core/src/service/reports.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (`pub mod reports;`)

**Interfaces:**
- Produces: `settings::REPORTS_MAX_ROWS = "reports.max_rows"`, `settings::REPORTS_MAX_AGE_SECS = "reports.max_age_secs"`; `RateWindows::new()`, `RateWindows::admit(&self, origin: &str, n: u32, now: i64) -> bool`; `Ingested { stored: usize, dropped_by_sender: u32 }`; `ingest(store: &Mutex<Store>, windows: &RateWindows, origin: &str, batch: ReportBatch, batch_max: usize, now: i64) -> Result<Ingested, IpcError>` (errors: `E_VALIDATE` for shape, `E_RATE_LIMITED` when over budget); `drain_own_ring(store: &Mutex<Store>, now: i64) -> usize`; `sweep_by_age(store: &Mutex<Store>, now: i64) -> usize`.
- Consumes: Task 3's store methods, `logging::redact` (existing), `logging::report_ring()` (Task 5 — write `drain_own_ring` in this task; it compiles once Task 5 lands, so order the commits 4 → 5 or stub the call behind Task 5. Simplest: do Task 5 first if working alone. The tasks are independent to review, not to build, so an executor doing Task 4 before 5 leaves `drain_own_ring` for Task 5's commit.)

- [ ] **Step 1: Settings.** In `settings.rs` add near `USAGE_PRICES_JSON`:

```rust
/// Newest `error_reports` rows kept; pruned on every insert.
pub const REPORTS_MAX_ROWS: &str = "reports.max_rows";
/// Rows older than this are swept on the tick; `0` disables the age sweep.
pub const REPORTS_MAX_AGE_SECS: &str = "reports.max_age_secs";
```

and two `Spec` rows at the end of `SPECS`:

```rust
    Spec {
        key: REPORTS_MAX_ROWS,
        default: "5000",
        kind: Kind::Int { min: 100, max: 100_000 },
    },
    Spec {
        key: REPORTS_MAX_AGE_SECS,
        default: "604800",
        kind: Kind::Secs,
    },
```

Run `cargo test -p fleet-core settings::` — any test that snapshots `read_all` output must be updated with the two new keys.

- [ ] **Step 2: Write the failing ingest tests** in `service/reports.rs`:

```rust
//! Ingest for error reports: clamp, redact, rate-limit, store, log.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ReportFilter;
    use fleet_proto::report::Report;

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    fn batch(msgs: &[&str]) -> ReportBatch {
        ReportBatch {
            reports: msgs.iter().map(|m| Report::error("fleet_core::ssh", m)).collect(),
            dropped: 0,
        }
    }

    #[test]
    fn stores_clamped_and_redacted_rows() {
        let s = store();
        let w = RateWindows::new();
        let mut b = batch(&["ssh failed: Authorization: Bearer abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"]);
        b.reports[0].message.push_str(&"x".repeat(MESSAGE_MAX));
        let got = ingest(&s, &w, "client:desk", b, HTTP_BATCH_MAX, 1000).unwrap();
        assert_eq!(got.stored, 1);
        let rows = s.lock().unwrap().list_reports(&ReportFilter::default()).unwrap();
        assert!(!rows[0].message.contains("abcdef0123456789abcdef"), "token redacted");
        assert!(rows[0].truncated);
        assert_eq!(rows[0].received_at, 1000);
        assert_eq!(rows[0].origin, "client:desk");
    }

    #[test]
    fn refuses_a_bad_level_and_an_oversize_batch() {
        let s = store();
        let w = RateWindows::new();
        let mut b = batch(&["m"]);
        b.reports[0].level = "debug".into();
        let e = ingest(&s, &w, "hub", b, HTTP_BATCH_MAX, 1).unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
        let too_many = batch(&vec!["m"; FRAME_BATCH_MAX + 1]);
        let e = ingest(&s, &w, "hub", too_many, FRAME_BATCH_MAX, 1).unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
    }

    #[test]
    fn the_row_cap_is_applied_on_insert() {
        let s = store();
        s.lock().unwrap().set_setting(settings::REPORTS_MAX_ROWS, "100").unwrap();
        let w = RateWindows::new();
        // Three origins so the rate limit never bites.
        for (i, o) in ["a", "b", "c"].iter().enumerate() {
            let b = batch(&vec!["m"; 50]);
            ingest(&s, &w, o, b, HTTP_BATCH_MAX, 10 + i as i64).unwrap();
        }
        let rows = s.lock().unwrap().list_reports(&ReportFilter { limit: 1000, ..Default::default() }).unwrap();
        assert_eq!(rows.len(), 100);
    }

    #[test]
    fn the_rate_limit_refuses_the_batch_that_crosses_the_minute() {
        let s = store();
        let w = RateWindows::new();
        ingest(&s, &w, "host:box", batch(&vec!["m"; 50]), HTTP_BATCH_MAX, 0).unwrap();
        ingest(&s, &w, "host:box", batch(&vec!["m"; 10]), HTTP_BATCH_MAX, 30).unwrap();
        let e = ingest(&s, &w, "host:box", batch(&["m"]), HTTP_BATCH_MAX, 31).unwrap_err();
        assert_eq!(e.code, codes::E_RATE_LIMITED);
        assert_eq!(s.lock().unwrap().list_reports(&ReportFilter { limit: 1000, ..Default::default() }).unwrap().len(), 60);
        // Another origin is unaffected; the next minute admits again.
        ingest(&s, &w, "host:other", batch(&["m"]), HTTP_BATCH_MAX, 31).unwrap();
        ingest(&s, &w, "host:box", batch(&["m"]), HTTP_BATCH_MAX, 61).unwrap();
    }

    #[test]
    fn sweep_by_age_honours_zero_as_never() {
        let s = store();
        let w = RateWindows::new();
        ingest(&s, &w, "hub", batch(&["old"]), HTTP_BATCH_MAX, 0).unwrap();
        s.lock().unwrap().set_setting(settings::REPORTS_MAX_AGE_SECS, "0").unwrap();
        assert_eq!(sweep_by_age(&s, 10_000_000), 0);
        s.lock().unwrap().set_setting(settings::REPORTS_MAX_AGE_SECS, "100").unwrap();
        assert_eq!(sweep_by_age(&s, 10_000_000), 1);
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p fleet-core service::reports::`
Expected: compile errors (`RateWindows`, `ingest` undefined).

- [ ] **Step 4: Implement** above the tests:

```rust
use crate::ipc_error::{codes, lock, IpcError};
use crate::logging;
use crate::service::settings;
use crate::store::Store;
use fleet_proto::report::{Report, ReportBatch, FRAME_BATCH_MAX, HTTP_BATCH_MAX, RATE_PER_MINUTE};
use std::collections::HashMap;
use std::sync::Mutex;

/// Fixed one-minute windows per origin. Pruned of stale windows on every
/// call, so it never holds more entries than there are live origins.
#[derive(Debug, Default)]
pub struct RateWindows {
    inner: Mutex<HashMap<String, (i64, u32)>>,
}

impl RateWindows {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `origin` may store `n` more reports at `now`. Counts them if so.
    pub fn admit(&self, origin: &str, n: u32, now: i64) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|_, (start, _)| now - *start < 60);
        let (start, count) = g.entry(origin.to_string()).or_insert((now, 0));
        if now - *start >= 60 {
            *start = now;
            *count = 0;
        }
        if count.saturating_add(n) > RATE_PER_MINUTE {
            return false;
        }
        *count += n;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingested {
    pub stored: usize,
    pub dropped_by_sender: u32,
}

fn validate(batch: &ReportBatch, batch_max: usize) -> Result<(), IpcError> {
    if batch.reports.len() > batch_max {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("a report batch carries at most {batch_max} reports"),
        ));
    }
    if let Some(bad) = batch.reports.iter().find(|r| r.level != "error" && r.level != "warn") {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("report level must be error or warn, not {:?}", logging::redact(&bad.level)),
        ));
    }
    Ok(())
}

/// Clamp and redact one report in place.
fn sanitise(r: &mut Report) {
    r.clamp();
    r.message = logging::redact(&r.message).into_owned();
    r.component = logging::redact(&r.component).into_owned();
    if let Some(c) = &r.code {
        r.code = Some(logging::redact(c).into_owned());
    }
    if let Some(ctx) = &r.context {
        if let Ok(text) = serde_json::to_string(ctx) {
            let red = logging::redact(&text);
            if red != text {
                r.context = serde_json::from_str(&red).ok();
            }
        }
    }
}

/// Store a batch under `origin`: validate, clamp, redact, rate-limit, insert,
/// prune to `reports.max_rows`, and log one warn line per report.
pub fn ingest(
    store: &Mutex<Store>,
    windows: &RateWindows,
    origin: &str,
    mut batch: ReportBatch,
    batch_max: usize,
    now: i64,
) -> Result<Ingested, IpcError> {
    validate(&batch, batch_max)?;
    if !windows.admit(origin, batch.reports.len() as u32, now) {
        return Err(IpcError::new(
            codes::E_RATE_LIMITED,
            format!("{origin} may store {RATE_PER_MINUTE} reports per minute"),
        ));
    }
    for r in &mut batch.reports {
        sanitise(r);
    }
    let stored = {
        let s = lock(store)?;
        let n = s.insert_reports(origin, &batch.reports, now)?;
        let max_rows = settings::get_string(&s, settings::REPORTS_MAX_ROWS).parse().unwrap_or(5000);
        let _ = s.prune_reports_to(max_rows);
        n
    };
    for r in &batch.reports {
        tracing::warn!(
            target: "fleet_core::report",
            origin, level = %r.level, component = %r.component, code = ?r.code,
            "{}", r.message
        );
    }
    if batch.dropped > 0 {
        tracing::warn!(target: "fleet_core::report", origin, dropped = batch.dropped, "reports dropped by the sender");
    }
    Ok(Ingested { stored, dropped_by_sender: batch.dropped })
}

/// The hub's own errors: drain the process ring into the table, exempt from
/// the rate limit. Returns how many were stored.
pub fn drain_own_ring(store: &Mutex<Store>, now: i64) -> usize {
    let mut batch = logging::report_ring().drain(HTTP_BATCH_MAX);
    if batch.reports.is_empty() {
        return 0;
    }
    for r in &mut batch.reports {
        sanitise(r);
    }
    let Ok(s) = lock(store) else { return 0 };
    let n = s.insert_reports("hub", &batch.reports, now).unwrap_or(0);
    let max_rows = settings::get_string(&s, settings::REPORTS_MAX_ROWS).parse().unwrap_or(5000);
    let _ = s.prune_reports_to(max_rows);
    n
}

/// Delete rows older than `reports.max_age_secs`; `0` means never.
pub fn sweep_by_age(store: &Mutex<Store>, now: i64) -> usize {
    let Ok(s) = lock(store) else { return 0 };
    let max_age = settings::get_secs(&s, settings::REPORTS_MAX_AGE_SECS);
    if max_age == 0 {
        return 0;
    }
    s.sweep_reports_older_than(now - max_age as i64).unwrap_or(0)
}

// Keep the frame cap name in scope for the tests and for `ws.rs`.
pub const AGENT_BATCH_MAX: usize = FRAME_BATCH_MAX;
```

The tests use `MESSAGE_MAX` and `HTTP_BATCH_MAX`/`FRAME_BATCH_MAX`: add `use fleet_proto::report::{MESSAGE_MAX, HTTP_BATCH_MAX, FRAME_BATCH_MAX};` inside the test module (a non-test import of `MESSAGE_MAX` would fail `clippy -D warnings`).

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p fleet-core service::reports:: settings::`
Expected: all pass (the `drain_own_ring` test comes with Task 5).

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service
git commit -m "feat(core): ingest error reports with clamp, redaction, rate limit and retention settings"
```

---

### Task 5: The tracing layer and the global ring (`fleet-core`)

**Files:**
- Modify: `crates/fleet-core/src/logging.rs` (`init_in_with` ~line 332, `init_stderr_fallback` ~line 378, tests ~line 394)

**Interfaces:**
- Produces: `pub fn report_ring() -> &'static ReportRing`; `pub struct ReportLayer;` (`Layer<S>` for any `Subscriber`); `pub fn report_from_event(event: &tracing::Event<'_>) -> Option<Report>` (pure, tested).

- [ ] **Step 1: Write the failing tests** in `logging.rs`'s test module:

```rust
    #[test]
    fn an_error_event_becomes_a_report_and_a_warn_does_not() {
        use tracing_subscriber::layer::SubscriberExt as _;
        let before = report_ring().len();
        let sub = tracing_subscriber::registry().with(ReportLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(target: "fleet_core::ssh", code = "E_SSH", host = "box", "ssh failed: {}", "refused");
            tracing::warn!(target: "fleet_core::ssh", "ignored");
        });
        assert_eq!(report_ring().len(), before + 1);
        let b = report_ring().drain(usize::MAX);
        let r = b.reports.last().unwrap();
        assert_eq!(r.component, "fleet_core::ssh");
        assert_eq!(r.code.as_deref(), Some("E_SSH"));
        assert!(r.message.starts_with("ssh failed: refused"), "{}", r.message);
        assert!(r.message.contains("host=box"), "{}", r.message);
        assert_eq!(r.level, "error");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleet-core logging::tests::an_error_event_becomes_a_report`
Expected: compile error.

- [ ] **Step 3: Implement** (below `redact_secrets`, above `RedactingWriter`):

```rust
/// The process-wide queue of error-level events, fed by [`ReportLayer`] and
/// drained by whoever reports to a hub (the desktop's flusher, the hub's own
/// tick). Bounded at `RING_CAP`; in a process nothing drains it just wraps.
pub fn report_ring() -> &'static fleet_proto::report::ReportRing {
    static RING: LazyLock<fleet_proto::report::ReportRing> =
        LazyLock::new(fleet_proto::report::ReportRing::new);
    &RING
}

/// Flatten one event's fields into a [`Report`]: `message` is the message,
/// `code` is the code, everything else is appended as ` key=value`.
#[derive(Default)]
struct ReportVisitor {
    message: String,
    code: Option<String>,
    rest: String,
}

impl tracing::field::Visit for ReportVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "code" => self.code = Some(format!("{value:?}").trim_matches('"').to_string()),
            name => {
                use std::fmt::Write as _;
                let _ = write!(self.rest, " {name}={value:?}");
            }
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "code" => self.code = Some(value.to_string()),
            name => {
                use std::fmt::Write as _;
                let _ = write!(self.rest, " {name}={value}");
            }
        }
    }
}

/// An `ERROR` event as a clamped report, or `None` for any other level.
pub fn report_from_event(event: &tracing::Event<'_>) -> Option<fleet_proto::report::Report> {
    if *event.metadata().level() != tracing::Level::ERROR {
        return None;
    }
    let mut v = ReportVisitor::default();
    event.record(&mut v);
    let mut r = fleet_proto::report::Report::error(
        event.metadata().target(),
        &format!("{}{}", v.message, v.rest),
    );
    r.code = v.code;
    r.clamp();
    Some(r)
}

/// Pushes every `ERROR` event into [`report_ring`]. Never logs: a layer that
/// logs re-enters the subscriber.
pub struct ReportLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ReportLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(r) = report_from_event(event) {
            report_ring().push(r);
        }
    }
}
```

Then install it: in `init_in_with`, change the registry chain to `.with(filter).with(file_layer).with(stderr_layer).with(ReportLayer)`; in `init_stderr_fallback` add `.with(ReportLayer)` after the stderr layer too. Update the module doc's stack paragraph with one sentence: "`ReportLayer` copies every `ERROR` event into `report_ring()` for the hub error channel (spec 2026-09-21)."

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p fleet-core logging:: service::reports::`
Expected: all pass. Add this test to `service/reports.rs` now that the ring exists:

```rust
    #[test]
    fn drain_own_ring_stores_under_origin_hub() {
        let s = store();
        logging::report_ring().push(Report::error("fleet_core::tick", "boom"));
        assert!(drain_own_ring(&s, 5) >= 1);
        let rows = s.lock().unwrap().list_reports(&ReportFilter { origin: Some("hub".into()), ..Default::default() }).unwrap();
        assert!(rows.iter().any(|r| r.message == "boom"));
    }
```

(The ring is process-global, so other tests' pushes may be present; hence `>= 1` and `any`.)

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/logging.rs crates/fleet-core/src/service/reports.rs
git commit -m "feat(core): ReportLayer captures error events into the process ring"
```

---

### Task 6: `POST /report` and `GET /reports`

**Files:**
- Create: `crates/fleet-core/src/mcp/report_route.rs`
- Modify: `crates/fleet-core/src/mcp/mod.rs` (`pub mod report_route;`; `build_app` signature + body ~line 260; the production call ~line 555; every test call of `build_app` — lines ~555, 682, 1050, 1106, 1216, 1429 — gets `report_route::ReportState::new(Arc::clone(&store))` as a new last argument)

**Interfaces:**
- Produces: `ReportState::new(store: Arc<Mutex<Store>>) -> ReportState` (Clone); handlers `handle_report`, `handle_reports`.
- Consumes: Task 4's `ingest`, `RateWindows`; Task 3's `ReportFilter`/`list_reports`.

- [ ] **Step 1: Write the failing test** in `report_route.rs`. It boots the real `build_app` on a loopback listener, the way `mcp_and_hook_routes_serve_behind_shared_auth` does, and speaks raw HTTP/1.1:

```rust
//! `/report` (any authenticated caller posts its error batch) and `/reports`
//! (the master reads them back). Both sit behind `authorize`, like `/hook`.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{auth, build_app, events_route::EventsState, hooks, pairing, AuthState};
    use crate::mcp::guard::RateLimiter;
    use crate::ssh::SshClient;
    use fleet_proto::report::{Report, ReportBatch, BODY_MAX, HTTP_BATCH_MAX};
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn app() -> (SocketAddr, Arc<Mutex<Store>>) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("box").unwrap();
            s.upsert_host_token("box", "host-tok").unwrap();
            s.insert_client_token("phone", &auth::sha256_hex("ro-tok"), "readonly").unwrap();
        }
        let app = build_app(
            axum::routing::any(|| async { "MCP_OK" }),
            hooks::HookState { store: Arc::clone(&store), ssh: Arc::new(SshClient::new()) },
            AuthState {
                master: Arc::new("s3cret".to_string()),
                store: Arc::clone(&store),
                allowed_hosts: Arc::new(vec![]),
            },
            pairing::PairState::new(
                Arc::clone(&store),
                Arc::new(pairing::PendingPairings::new()),
                Arc::new(RateLimiter::new()),
                "https://fleet.example.com".to_string(),
            ),
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            ReportState::new(Arc::clone(&store)),
        );
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        (addr, store)
    }

    /// One request; returns (status, body).
    async fn http(addr: SocketAddr, method: &str, path: &str, token: &str, body: &str) -> (u16, String) {
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, body.to_string())
    }

    fn batch(n: usize) -> String {
        serde_json::to_string(&ReportBatch {
            reports: vec![Report::error("frontend", "boom"); n],
            dropped: 0,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn a_readonly_client_may_post_and_the_master_reads_it_back() {
        let (addr, _store) = app().await;
        let (st, _) = http(addr, "POST", "/report", "ro-tok", &batch(2)).await;
        assert_eq!(st, 204);
        let (st, _) = http(addr, "POST", "/report", "host-tok", &batch(1)).await;
        assert_eq!(st, 204);
        let (st, body) = http(addr, "GET", "/reports?limit=10", "s3cret", "").await;
        assert_eq!(st, 200);
        let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["origin"], "host:box");
        assert_eq!(rows[1]["origin"], "client:phone");
        let (st, body) = http(addr, "GET", "/reports?origin=host:box", "s3cret", "").await;
        assert_eq!(st, 200);
        assert_eq!(serde_json::from_str::<Vec<serde_json::Value>>(&body).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reading_is_master_only_and_posting_needs_a_token() {
        let (addr, _) = app().await;
        assert_eq!(http(addr, "GET", "/reports", "ro-tok", "").await.0, 403);
        assert_eq!(http(addr, "GET", "/reports", "host-tok", "").await.0, 403);
        assert_eq!(http(addr, "POST", "/report", "nonsense", &batch(1)).await.0, 401);
    }

    #[tokio::test]
    async fn bad_shapes_are_400_and_413_and_the_rate_limit_is_429() {
        let (addr, store) = app().await;
        assert_eq!(http(addr, "POST", "/report", "ro-tok", "{not json").await.0, 400);
        assert_eq!(http(addr, "POST", "/report", "ro-tok", &batch(HTTP_BATCH_MAX + 1)).await.0, 400);
        let huge = format!(r#"{{"reports":[],"dropped":0,"pad":"{}"}}"#, "x".repeat(BODY_MAX));
        assert_eq!(http(addr, "POST", "/report", "ro-tok", &huge).await.0, 413);
        // 50 + 10 fills the minute; the 61st is refused and nothing of it stored.
        assert_eq!(http(addr, "POST", "/report", "ro-tok", &batch(50)).await.0, 204);
        assert_eq!(http(addr, "POST", "/report", "ro-tok", &batch(10)).await.0, 204);
        assert_eq!(http(addr, "POST", "/report", "ro-tok", &batch(1)).await.0, 429);
        let n = store.lock().unwrap().list_reports(&crate::store::ReportFilter { limit: 1000, ..Default::default() }).unwrap().len();
        assert_eq!(n, 60);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleet-core report_route::`
Expected: compile error (`ReportState` undefined; `build_app` arity).

- [ ] **Step 3: Implement** above the tests:

```rust
use super::auth::Caller;
use crate::service::reports::{ingest, RateWindows};
use crate::store::{ReportFilter, Store};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fleet_proto::report::{ReportBatch, HTTP_BATCH_MAX};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct ReportState {
    store: Arc<Mutex<Store>>,
    windows: Arc<RateWindows>,
}

impl ReportState {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        ReportState { store, windows: Arc::new(RateWindows::new()) }
    }
}

/// `POST /report`: any authenticated caller — `readonly` included, since
/// reporting an error changes nothing about the fleet.
pub async fn handle_report(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> StatusCode {
    use crate::ipc_error::codes;
    let batch: ReportBatch = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(caller = %caller.label(), error = %e, "[report] rejected body");
            return StatusCode::BAD_REQUEST;
        }
    };
    let origin = caller.label();
    match ingest(&state.store, &state.windows, &origin, batch, HTTP_BATCH_MAX, fleet_proto::report::now_unix()) {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) if e.code == codes::E_VALIDATE => StatusCode::BAD_REQUEST,
        Err(e) if e.code == codes::E_RATE_LIMITED => StatusCode::TOO_MANY_REQUESTS,
        Err(e) => {
            tracing::error!(code = %e.code, error = %e.message, "[report] store failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// `GET /reports`: master only — the rows hold every client's messages.
pub async fn handle_reports(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Query(mut filter): Query<ReportFilter>,
) -> Response {
    if !caller.is_master() {
        return (StatusCode::FORBIDDEN, "reports are the master token's to read").into_response();
    }
    filter.limit = filter.limit.clamp(1, 1000);
    let rows = match state.store.lock() {
        Ok(s) => s.list_reports(&filter),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!(code = %e.code, error = %e.message, "[reports] list failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

`ReportFilter` must deserialize with a default `limit` when the query omits it: in `store/reports.rs` add `#[serde(default = "default_limit")]` on `limit` with `fn default_limit() -> u32 { 100 }`.

In `mcp/mod.rs`: add `pub mod report_route;`; extend `build_app` with `report_state: report_route::ReportState` and merge, before `.layer(... authorize)`:

```rust
        // `/report` and `/reports` carry their own state and sit behind
        // `authorize` like `/events`: a phone posts its errors with its own
        // token; only the master reads them. The body cap is the spec's.
        .merge(
            axum::Router::new()
                .route("/report", axum::routing::post(report_route::handle_report))
                .route("/reports", axum::routing::get(report_route::handle_reports))
                .layer(axum::extract::DefaultBodyLimit::max(fleet_proto::report::BODY_MAX))
                .with_state(report_state),
        )
```

In the production `start_with_listener`, pass `report_route::ReportState::new(Arc::clone(&store))` — take the clone before `store` is moved into `FleetTools::new` (there is already an `events_store`/`agents_store` clone pattern above it). Update every test call site of `build_app` and `test_app`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p fleet-core report_route:: mcp::`
Expected: all pass. If the `413` assertion fails with `400`, the body limit layer is below the route: it must wrap the two routes (as written above), not the whole merged router.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/mcp crates/fleet-core/src/store/reports.rs
git commit -m "feat(hub): POST /report and GET /reports behind the bearer layer"
```

---

### Task 7: The hub accepts `AgentFrame::Report`

**Files:**
- Modify: `crates/fleet-core/src/agent/ws.rs` (the `Ok(Decoded::Frame(frame))` arm ~line 864; `serve`'s locals; the test module's helpers ~line 1175)

**Interfaces:**
- Consumes: `service::reports::{ingest, RateWindows, AGENT_BATCH_MAX}`, Task 2's variant.

- [ ] **Step 1: Write the failing test** in `ws.rs`'s tests, next to `an_exec_runs...`:

```rust
    #[tokio::test]
    async fn a_report_frame_is_stored_under_the_connection_s_host() {
        let hub = hub().await;
        let mut ws = connected(&hub, HOST_TOKEN, "laptop").await;
        send(&mut ws, &AgentFrame::Report {
            reports: vec![fleet_proto::report::Report::error("fleet_agent::conn", "dial refused")],
            dropped: 2,
        })
        .await;
        wait_until("the report is stored", || {
            hub.store.lock().unwrap()
                .list_reports(&crate::store::ReportFilter::default()).unwrap()
                .iter().any(|r| r.origin == "host:laptop" && r.message == "dial refused")
        })
        .await;
        // Not an answer to anything: the connection is still live.
        assert!(hub.registry.connected("laptop"));
    }
```

Use the existing `HOST_TOKEN`/`hub()`/`connected()`/`send()`/`wait_until()` helpers in that module (read them at ~lines 1087–1230; if the host token constant has a different name, use that name).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleet-core agent::ws::tests::a_report_frame_is_stored`
Expected: times out in `wait_until` (the frame reaches `registry.deliver`, which drops an id-less frame).

- [ ] **Step 3: Implement.** In `serve`, before the loop, add `let report_windows = crate::service::reports::RateWindows::new();`. In the `Ok(Decoded::Frame(frame))` arm, before `answered_id`:

```rust
                            Ok(Decoded::Frame(AgentFrame::Report { reports, dropped })) => {
                                // The agent's own error log, batched on its
                                // heartbeat. Origin is the connection's alias —
                                // from the token, never the body. The store lock
                                // is taken inside `ingest` for the insert only.
                                let origin = format!("host:{alias}");
                                let batch = fleet_proto::report::ReportBatch { reports, dropped };
                                if let Err(e) = crate::service::reports::ingest(
                                    &store,
                                    &report_windows,
                                    &origin,
                                    batch,
                                    crate::service::reports::AGENT_BATCH_MAX,
                                    fleet_proto::report::now_unix(),
                                ) {
                                    tracing::warn!(host = %alias, code = %e.code, error = %e.message, "[agent] report batch refused");
                                }
                            }
```

`store` in `serve` is the `Arc<Mutex<Store>>` destructured from `Session`; pass `&store` (deref to `&Mutex<Store>`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p fleet-core agent::`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/agent/ws.rs
git commit -m "feat(hub): store an agent's Report frames under its host"
```

---

### Task 8: The tick sweeps by age and drains the hub's own ring

**Files:**
- Modify: `crates/fleet-core/src/service/tick.rs` (the tick body after the task sweep ~line 126)

- [ ] **Step 1: Add the two calls** right after the `sweep_open_tasks` block:

```rust
                // Error reports (spec 2026-09-21): the hub's own ERROR events
                // join the table, and rows past `reports.max_age_secs` go.
                {
                    let now = fleet_proto::report::now_unix();
                    let _ = service::reports::drain_own_ring(store, now);
                    let swept = service::reports::sweep_by_age(store, now);
                    if swept > 0 {
                        tracing::info!("reconcile tick: swept {swept} old error report(s)");
                    }
                }
```

Both functions are tested in Task 4; the tick has no unit test of its own body (it is an integration of tested calls, like the task sweep beside it).

- [ ] **Step 2: Build and run the tick tests**

Run: `cargo test -p fleet-core service::tick::`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/fleet-core/src/service/tick.rs
git commit -m "feat(hub): age-sweep error reports and drain the hub's own ring on the tick"
```

---

### Task 9: `fleet-agent` reports on its heartbeat

**Files:**
- Create: `crates/fleet-agent/src/report.rs`
- Modify: `crates/fleet-agent/src/lib.rs` (`pub mod report;`), `main.rs` (`run`, ~line 38), `config.rs` (`Config`), `conn.rs` (the `ticker.tick()` branch ~line 579; tests ~line 1583), `crates/fleet-agent/Cargo.toml` (add `"registry"` to `tracing-subscriber` features)

**Interfaces:**
- Produces: `report::ring() -> &'static ReportRing`; `report::ReportLayer`; `report::report_from_event(&Event) -> Option<Report>`; `Config.report_errors: bool` (serde default `true`, `pub fn default_report_errors() -> bool`).

- [ ] **Step 1: Write the failing tests**

`report.rs`:

```rust
//! The agent's copy of the error-report layer: `fleet-agent` depends on
//! `fleet-proto` only, so the ~40 lines of `fleet_core::logging::ReportLayer`
//! live here too. The record, the ring and the caps are `fleet-proto`'s.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_event_lands_in_the_ring_with_its_code() {
        use tracing_subscriber::layer::SubscriberExt as _;
        let before = ring().len();
        let sub = tracing_subscriber::registry().with(ReportLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(target: "fleet_agent::conn", code = "E_DIAL", "dial failed: {}", "refused");
            tracing::info!("not captured");
        });
        assert_eq!(ring().len(), before + 1);
        let r = ring().drain(usize::MAX).reports.pop().unwrap();
        assert_eq!(r.component, "fleet_agent::conn");
        assert_eq!(r.code.as_deref(), Some("E_DIAL"));
        assert!(r.message.starts_with("dial failed: refused"));
    }
}
```

`config.rs` test:

```rust
    #[test]
    fn report_errors_defaults_on_and_reads_back() {
        let c: Config = serde_json::from_str(r#"{"hub":"https://h","token":"t"}"#).unwrap();
        assert!(c.report_errors);
        let c: Config = serde_json::from_str(r#"{"hub":"https://h","token":"t","report_errors":false}"#).unwrap();
        assert!(!c.report_errors);
    }
```

`conn.rs` test, beside `a_ping_is_answered_with_a_pong_carrying_its_id`:

```rust
    /// An error queued before the handshake is flushed on the first beat
    /// after `welcome` — the case the channel exists for.
    #[tokio::test]
    async fn queued_reports_are_flushed_on_a_beat_once_welcomed() {
        crate::report::ring().push(fleet_proto::report::Report::error("fleet_agent::conn", "earlier dial refused"));
        let mut p = pair().await;
        assert!(p.beat().await);
        match next_frame(&mut p.hub).await.map(|f| f.0) {
            Some(AgentFrame::Report { reports, .. }) => {
                assert!(reports.iter().any(|r| r.message == "earlier dial refused"));
            }
            other => panic!("expected a report frame, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-agent report:: config::tests::report_errors queued_reports`
Expected: compile errors.

- [ ] **Step 3: Implement**

`Cargo.toml`: `tracing-subscriber = { version = "0.3", default-features = false, features = ["fmt", "env-filter", "std", "registry"] }`.

`report.rs` above the tests:

```rust
use fleet_proto::report::{Report, ReportRing};
use std::sync::LazyLock;

pub fn ring() -> &'static ReportRing {
    static RING: LazyLock<ReportRing> = LazyLock::new(ReportRing::new);
    &RING
}

#[derive(Default)]
struct Visitor {
    message: String,
    code: Option<String>,
    rest: String,
}

impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "code" => self.code = Some(format!("{value:?}").trim_matches('"').to_string()),
            name => { let _ = write!(self.rest, " {name}={value:?}"); }
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        match field.name() {
            "message" => self.message = value.to_string(),
            "code" => self.code = Some(value.to_string()),
            name => { let _ = write!(self.rest, " {name}={value}"); }
        }
    }
}

pub fn report_from_event(event: &tracing::Event<'_>) -> Option<Report> {
    if *event.metadata().level() != tracing::Level::ERROR {
        return None;
    }
    let mut v = Visitor::default();
    event.record(&mut v);
    let mut r = Report::error(event.metadata().target(), &format!("{}{}", v.message, v.rest));
    r.code = v.code;
    r.clamp();
    Some(r)
}

/// Never logs from inside: that re-enters the subscriber.
pub struct ReportLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ReportLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(r) = report_from_event(event) {
            ring().push(r);
        }
    }
}
```

`config.rs`: add to `Config`:

```rust
    /// Send this agent's error-level log events to the hub on each heartbeat.
    #[serde(default = "default_report_errors")]
    pub report_errors: bool,
```

with `pub fn default_report_errors() -> bool { true }`. Every literal `Config { .. }` in tests gets `report_errors: true`.

`main.rs` `run`: replace the `tracing_subscriber::fmt()...init()` with

```rust
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    // The config is read after the subscriber is up (it logs its own
    // refusals), so the layer is installed unconditionally and `report_errors`
    // gates the FLUSH instead (`conn::serve`); a disabled agent's ring wraps
    // at RING_CAP and costs nothing.
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt)
        .with(fleet_agent::report::ReportLayer)
        .init();
```

`conn.rs`: `Agent` gains `report_errors: bool` — add a field `pub report_errors: bool` to `Agent` set from `config.report_errors` in `run` (and `true` in `Agent::new`; add `pub fn with_reports(self: Arc<Self>, on: bool)` if `Agent::new` returns an `Arc` — read `Agent::new` at ~line 324 and choose the smallest change that gets the flag to `serve`). In the `ack = ticker.tick()` branch, after the `alive` computation and before `if !alive`:

```rust
                if alive && welcomed && agent.report_errors {
                    let batch = crate::report::ring().drain(fleet_proto::report::FRAME_BATCH_MAX);
                    if !batch.reports.is_empty() || batch.dropped > 0 {
                        let frame = AgentFrame::Report { reports: batch.reports, dropped: batch.dropped };
                        match encode_agent_frame(&frame) {
                            Ok(text) => { let _ = out.send(Out::Frame(text)); }
                            Err(e) => tracing::warn!(error = %e, "[agent] a report batch did not encode; dropped"),
                        }
                    }
                }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p fleet-agent`
Expected: all pass, including the existing conn tests (a beat with an empty ring sends nothing, so `a_ping_is_answered...` still sees the pong first). If the flush test sees a `Pong` first, drain with a loop that skips non-`Report` frames.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-agent
git commit -m "feat(agent): report error-level events to the hub on the heartbeat"
```

---

### Task 10: The desktop flusher

**Files:**
- Create: `src-tauri/src/backend/report.rs`
- Modify: `src-tauri/src/backend/mod.rs` (`pub mod report;`), `src-tauri/src/backend/remote.rs` (add `pub fn transport(&self) -> Arc<dyn HubTransport>`), `src-tauri/src/backend/startup.rs` (`FleetTasks` + `start_background_tasks`), `src-tauri/src/backend/tests_startup.rs` (Recorder + expectations), `src-tauri/src/bootstrap/tasks.rs` (`start_report_flusher`)

**Interfaces:**
- Produces: `ReportFlusher::new(cfg: RemoteConfig, transport: Arc<dyn HubTransport>, ring: &'static ReportRing) -> ReportFlusher`; `async fn flush_once(&mut self) -> Flush` where `enum Flush { Nothing, Sent(usize), Stop, Backoff(Duration) }`; `pub fn spawn_report_flusher(flusher: ReportFlusher, shutdown: CancellationToken)`; `startup::report_flusher_wanted(env: Option<&str>) -> bool`; `FleetTasks::start_report_flusher(&self)`.

- [ ] **Step 1: Write the failing tests** in `report.rs`:

```rust
//! Flushes `fleet_core::logging::report_ring()` to the hub's `POST /report`.
//! Fire-and-forget: nothing awaits it, and its answers only steer itself.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::remote::HubResponse;
    use fleet_proto::report::{Report, ReportRing};
    use std::sync::Mutex;

    struct Fake {
        answers: Mutex<Vec<Result<HubResponse, String>>>,
        seen: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl HubTransport for Fake {
        async fn post_json(&self, url: &str, bearer: &str, body: String) -> Result<HubResponse, String> {
            assert_eq!(bearer, "cl_tok");
            self.seen.lock().unwrap().push((url.to_string(), body));
            self.answers.lock().unwrap().remove(0)
        }
    }

    fn ok() -> Result<HubResponse, String> { Ok(HubResponse { status: 204, body: String::new() }) }
    fn status(s: u16) -> Result<HubResponse, String> { Ok(HubResponse { status: s, body: String::new() }) }

    fn flusher(answers: Vec<Result<HubResponse, String>>, ring: &'static ReportRing) -> (ReportFlusher, Arc<Fake>) {
        let fake = Arc::new(Fake { answers: Mutex::new(answers), seen: Mutex::new(vec![]) });
        let cfg = RemoteConfig { base_url: "https://hub.example.com".into(), token: "cl_tok".into(), client_name: "desk".into() };
        (ReportFlusher::new(cfg, fake.clone(), ring), fake)
    }

    fn ring() -> &'static ReportRing {
        Box::leak(Box::new(ReportRing::new()))
    }

    #[tokio::test]
    async fn an_empty_ring_sends_nothing() {
        let (mut f, fake) = flusher(vec![], ring());
        assert_eq!(f.flush_once().await, Flush::Nothing);
        assert!(fake.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_batch_is_posted_to_slash_report_redacted() {
        let r = ring();
        let mut rep = Report::error("fleet_core::ssh", "Bearer 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef leaked");
        rep.context = Some(serde_json::json!({ "url": "https://h/?token=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" }));
        r.push(rep);
        let (mut f, fake) = flusher(vec![ok()], r);
        assert_eq!(f.flush_once().await, Flush::Sent(1));
        let (url, body) = fake.seen.lock().unwrap()[0].clone();
        assert_eq!(url, "https://hub.example.com/report");
        assert!(!body.contains("0123456789abcdef0123456789"), "{body}");
        assert!(body.contains("\"dropped\":0"));
    }

    #[tokio::test]
    async fn a_404_stops_and_a_429_holds_a_minute_keeping_the_batch() {
        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![status(404)], r);
        assert_eq!(f.flush_once().await, Flush::Stop);

        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![status(429), ok()], r);
        assert_eq!(f.flush_once().await, Flush::Backoff(Duration::from_secs(60)));
        assert_eq!(f.flush_once().await, Flush::Sent(1), "the held batch is retried");
    }

    #[tokio::test]
    async fn a_transport_error_backs_off_doubling_to_a_minute() {
        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![Err("refused".into()); 6], r);
        let mut waits = vec![];
        for _ in 0..6 {
            match f.flush_once().await {
                Flush::Backoff(d) => waits.push(d.as_secs()),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
    }

    #[tokio::test]
    async fn a_401_stops_too() {
        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![status(401)], r);
        assert_eq!(f.flush_once().await, Flush::Stop);
    }
}
```

And in `startup.rs`'s tests (`tests_startup.rs`): add `fn start_report_flusher(&self) { self.0.lock().unwrap().push("report_flusher"); }` to `Recorder`; change `a_hub_client_starts_none_of_the_three`'s expectation to `vec!["event_bridge", "report_flusher"]`; add:

```rust
#[test]
fn the_report_flusher_is_off_with_the_env_var() {
    assert!(report_flusher_wanted(None));
    assert!(report_flusher_wanted(Some("1")));
    assert!(!report_flusher_wanted(Some("0")));
    assert!(!report_flusher_wanted(Some("false")));
    assert!(!report_flusher_wanted(Some("FALSE")));
}
```

and extend the two source-string tests' lists with `"spawn_report_flusher("` / `("spawn_report_flusher(", "the error-report flusher")`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p claude-fleet --lib backend::report backend::tests_startup` (from `src-tauri`, or `--manifest-path src-tauri/Cargo.toml`)
Expected: compile errors.

- [ ] **Step 3: Implement**

`remote.rs`: in `impl HubBackend` add

```rust
    /// The transport, for the report flusher to share one TLS client.
    pub fn transport(&self) -> Arc<dyn HubTransport> {
        Arc::clone(&self.transport)
    }
```

`report.rs` above the tests:

```rust
use crate::backend::remote::HubTransport;
use crate::backend::RemoteConfig;
use fleet_core::logging::redact;
use fleet_proto::report::{ReportBatch, ReportRing, HTTP_BATCH_MAX};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
pub const FLUSH_AT: usize = 20;
const POST_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_START: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flush {
    Nothing,
    Sent(usize),
    /// The hub predates the route or refuses this client: stop for this run.
    Stop,
    /// Keep the batch and wait this long.
    Backoff(Duration),
}

pub struct ReportFlusher {
    url: String,
    token: String,
    transport: Arc<dyn HubTransport>,
    ring: &'static ReportRing,
    /// A batch the last flush could not deliver, retried before the ring.
    held: Option<ReportBatch>,
    backoff: Duration,
}

impl ReportFlusher {
    pub fn new(cfg: RemoteConfig, transport: Arc<dyn HubTransport>, ring: &'static ReportRing) -> Self {
        ReportFlusher {
            url: format!("{}/report", cfg.base_url),
            token: cfg.token,
            transport,
            ring,
            held: None,
            backoff: BACKOFF_START,
        }
    }

    fn next_batch(&mut self) -> Option<ReportBatch> {
        if let Some(h) = self.held.take() {
            return Some(h);
        }
        let mut b = self.ring.drain(HTTP_BATCH_MAX);
        if b.reports.is_empty() && b.dropped == 0 {
            return None;
        }
        for r in &mut b.reports {
            r.message = redact(&r.message).into_owned();
            if let Some(ctx) = &r.context {
                if let Ok(text) = serde_json::to_string(ctx) {
                    let red = redact(&text);
                    if red != text {
                        r.context = serde_json::from_str(&red).ok();
                    }
                }
            }
        }
        Some(b)
    }

    fn back_off(&mut self, batch: ReportBatch) -> Flush {
        self.held = Some(batch);
        let wait = self.backoff;
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        Flush::Backoff(wait)
    }

    pub async fn flush_once(&mut self) -> Flush {
        let Some(batch) = self.next_batch() else { return Flush::Nothing };
        let n = batch.reports.len();
        let body = match serde_json::to_string(&batch) {
            Ok(b) => b,
            Err(_) => return Flush::Nothing,
        };
        let sent = tokio::time::timeout(POST_TIMEOUT, self.transport.post_json(&self.url, &self.token, body)).await;
        match sent {
            Ok(Ok(resp)) => match resp.status {
                204 => { self.backoff = BACKOFF_START; Flush::Sent(n) }
                404 => { tracing::info!("[report] the hub has no /report route; not reporting this run"); Flush::Stop }
                401 | 403 => { tracing::info!(status = resp.status, "[report] the hub refuses this client; not reporting this run"); Flush::Stop }
                429 => { self.held = Some(batch); Flush::Backoff(BACKOFF_MAX) }
                other => { tracing::warn!(status = other, "[report] unexpected answer from /report"); self.back_off(batch) }
            },
            Ok(Err(e)) => { tracing::warn!(error = %redact(&e), "[report] could not reach the hub"); self.back_off(batch) }
            Err(_) => { tracing::warn!("[report] no answer from /report within {POST_TIMEOUT:?}"); self.back_off(batch) }
        }
    }

    /// Every `FLUSH_INTERVAL`, or as soon as `FLUSH_AT` are queued.
    pub async fn run(mut self, shutdown: CancellationToken) {
        let mut wait = FLUSH_INTERVAL;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
            match self.flush_once().await {
                Flush::Stop => return,
                Flush::Backoff(d) => wait = d,
                Flush::Nothing | Flush::Sent(_) => {
                    wait = if self.ring.len() >= FLUSH_AT { Duration::ZERO } else { FLUSH_INTERVAL };
                }
            }
        }
    }
}

pub fn spawn_report_flusher(flusher: ReportFlusher, shutdown: CancellationToken) {
    fleet_core::rt::spawn(flusher.run(shutdown));
}
```

`startup.rs`: add `fn start_report_flusher(&self);` to `FleetTasks` with a doc line ("The error-report flusher: a hub client's only other background task; off with `CLAUDE_FLEET_HUB_REPORTS=0`."), and

```rust
/// `CLAUDE_FLEET_HUB_REPORTS`: unset or anything but `0`/`false` means on.
pub fn report_flusher_wanted(env: Option<&str>) -> bool {
    !env.is_some_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}
```

In `start_background_tasks`'s `Backend::Remote` arm, after `tasks.start_event_bridge();`:

```rust
            if report_flusher_wanted(std::env::var("CLAUDE_FLEET_HUB_REPORTS").ok().as_deref()) {
                tasks.start_report_flusher();
            } else {
                tracing::info!("CLAUDE_FLEET_HUB_REPORTS is off: not reporting errors to the hub");
            }
```

`bootstrap/tasks.rs`: implement

```rust
    fn start_report_flusher(&self) {
        let Some(cfg) = self.remote.clone() else {
            tracing::error!("[report] asked to flush reports with no hub configured");
            return;
        };
        let transport = HubBackend::new(cfg.clone()).transport();
        spawn_report_flusher(
            ReportFlusher::new(cfg, transport, fleet_core::logging::report_ring()),
            self.shutdown.clone(),
        );
    }
```

with `use crate::backend::report::{spawn_report_flusher, ReportFlusher};`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib backend::`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/backend src-tauri/src/bootstrap/tasks.rs
git commit -m "feat(desktop): flush error reports to the hub in hub-client mode"
```

---

### Task 11: The `report_client_error` command and its verdict

**Files:**
- Modify: `src-tauri/src/commands/hub.rs`, `src-tauri/src/lib.rs` (`generate_handler!` after `hub_stranded_token`), `src-tauri/src/backend/verdicts.rs` (a row after `hub_connection`'s), `src-tauri/src/backend/tests_routing.rs` (only if `every_command_has_a_verdict` / body checks need a `SOURCES` entry — `commands/hub.rs` is already listed)
- Regenerate: `src/lib/hub_verdicts.generated.json`, `docs/hub.md` refusal table, `docs/control-api-reference.md`, `src-tauri/src/backend/hub_contract.golden.json` if its test asks.

**Interfaces:**
- Produces: Tauri command `report_client_error(args: ReportClientErrorArgs) -> Result<(), IpcError>`; `ReportClientErrorArgs { level: String, component: String, code: Option<String>, message: String, context: Option<serde_json::Value> }` (`Deserialize`).

- [ ] **Step 1: Write the failing test** in `commands/hub.rs`'s test module (create one if absent):

```rust
    #[test]
    fn a_frontend_error_is_queued_clamped_with_its_code() {
        let before = fleet_core::logging::report_ring().len();
        report_client_error_logic(ReportClientErrorArgs {
            level: "error".into(),
            component: "frontend:unhandled".into(),
            code: Some("E_PARSE".into()),
            message: "x".repeat(5000),
            context: Some(serde_json::json!({ "url": "app://index" })),
        })
        .unwrap();
        assert_eq!(fleet_core::logging::report_ring().len(), before + 1);
        let r = fleet_core::logging::report_ring().drain(usize::MAX).reports.pop().unwrap();
        assert_eq!(r.code.as_deref(), Some("E_PARSE"));
        assert!(r.truncated);
        assert_eq!(r.component, "frontend:unhandled");
        let e = report_client_error_logic(ReportClientErrorArgs {
            level: "debug".into(), component: "frontend".into(), code: None, message: "m".into(), context: None,
        })
        .unwrap_err();
        assert_eq!(e.code, fleet_core::ipc_error::codes::E_VALIDATE);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib commands::hub`
Expected: compile error.

- [ ] **Step 3: Implement** in `commands/hub.rs`:

```rust
/// A frontend error to queue for the hub error channel.
#[derive(Debug, Deserialize)]
pub struct ReportClientErrorArgs {
    pub level: String,
    pub component: String,
    pub code: Option<String>,
    pub message: String,
    pub context: Option<serde_json::Value>,
}

pub fn report_client_error_logic(args: ReportClientErrorArgs) -> Result<(), IpcError> {
    use fleet_core::ipc_error::codes;
    if args.level != "error" && args.level != "warn" {
        return Err(IpcError::new(codes::E_VALIDATE, "level must be error or warn"));
    }
    let mut r = fleet_proto::report::Report::error(&args.component, &args.message);
    r.level = args.level;
    r.code = args.code;
    r.context = args.context;
    r.clamp();
    fleet_core::logging::report_ring().push(r);
    Ok(())
}

/// Queue a frontend error for the hub error channel. Same in both modes: the
/// push is local; in standalone nothing drains the ring, so it is a no-op.
#[tauri::command]
pub fn report_client_error(args: ReportClientErrorArgs) -> Result<(), IpcError> {
    report_client_error_logic(args)
}
```

Add `fleet-proto` to `src-tauri/Cargo.toml` dependencies if it is not already there (`fleet-proto = { path = "../crates/fleet-proto" }`). Register `commands::hub::report_client_error,` in `lib.rs`. Add the verdict row after `hub_connection`'s:

```rust
    (
        "report_client_error",
        Verdict::SameInBoth {
            why: "queues a frontend error in THIS process's report ring; in standalone \
                  nothing drains it, so the push is a no-op rather than a refusal",
        },
    ),
```

- [ ] **Step 4: Regenerate and run**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::hub backend::tests_routing
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

Expected: the regen runs report the files rewritten; the final run passes. If `tests_contract` fails on the golden, run `REGEN_HUB_CONTRACT=1 cargo test --manifest-path src-tauri/Cargo.toml --lib tests_contract` and re-run (the regen run itself reports FAILED once).

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(desktop): report_client_error queues frontend errors for the hub"
```

---

### Task 12: Frontend capture

**Files:**
- Create: `src/lib/error_report.ts`, `src/lib/error_report.test.ts`
- Modify: `src/main.ts` (call `installErrorReporting()` after `initTheme()`), `src/lib/toasts.ts` (`pushError` reports)

**Interfaces:**
- Produces: `reportError(component: string, message: string, code?: string | null, context?: Record<string, unknown>): void`; `installErrorReporting(win: Window = window): void`; `resetErrorReportingForTests(): void`; constants `MAX_PER_MINUTE = 20`, `DEDUPE_MS = 60_000`.
- Consumes: Task 11's command via `invokeCmd('report_client_error', { args })`.

- [ ] **Step 1: Write the failing tests** — `src/lib/error_report.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  reportError,
  installErrorReporting,
  resetErrorReportingForTests,
  MAX_PER_MINUTE,
} from './error_report';

const inv = () => invoke as ReturnType<typeof vi.fn>;
const calls = () => inv().mock.calls.filter((c) => c[0] === 'report_client_error');

beforeEach(() => {
  inv().mockReset();
  inv().mockResolvedValue(undefined);
  resetErrorReportingForTests();
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-09-21T10:00:00Z'));
});

describe('reportError', () => {
  it('invokes report_client_error with the record', () => {
    reportError('frontend', 'boom', 'E_PARSE', { url: 'x' });
    expect(calls()).toHaveLength(1);
    expect(calls()[0][1]).toEqual({
      args: { level: 'error', component: 'frontend', code: 'E_PARSE', message: 'boom', context: { url: 'x' } },
    });
  });

  it('sends an identical message once a minute, counting repeats on the next distinct one', () => {
    reportError('frontend', 'same');
    reportError('frontend', 'same');
    reportError('frontend', 'same');
    expect(calls()).toHaveLength(1);
    reportError('frontend', 'other');
    expect(calls()).toHaveLength(2);
    expect((calls()[1][1] as { args: { context: { repeats: number } } }).args.context.repeats).toBe(2);
    vi.advanceTimersByTime(61_000);
    reportError('frontend', 'same');
    expect(calls()).toHaveLength(3);
  });

  it('stops after MAX_PER_MINUTE and resumes the next minute', () => {
    for (let i = 0; i < MAX_PER_MINUTE + 5; i++) reportError('frontend', `m${i}`);
    expect(calls()).toHaveLength(MAX_PER_MINUTE);
    vi.advanceTimersByTime(60_001);
    reportError('frontend', 'later');
    expect(calls()).toHaveLength(MAX_PER_MINUTE + 1);
  });
});

describe('installErrorReporting', () => {
  it('reports window errors and unhandled rejections', () => {
    installErrorReporting(window);
    window.dispatchEvent(new ErrorEvent('error', { message: 'ReferenceError: x', filename: 'app.js', lineno: 3 }));
    const rej = new Event('unhandledrejection') as Event & { reason: unknown };
    rej.reason = new Error('rejected');
    window.dispatchEvent(rej);
    expect(calls().map((c) => (c[1] as { args: { component: string } }).args.component)).toEqual([
      'frontend:unhandled',
      'frontend:unhandled',
    ]);
    expect((calls()[1][1] as { args: { message: string } }).args.message).toContain('rejected');
  });
});
```

Add to `src/lib/toasts.test.ts`:

```ts
it('pushError also reports to the hub error channel', () => {
  pushError({ code: 'E_SSH', message: 'ssh refused' }, 'Opening terminal');
  const reported = inv().mock.calls.filter((c) => c[0] === 'report_client_error');
  expect(reported).toHaveLength(1);
  expect((reported[0][1] as { args: { code: string; component: string } }).args).toMatchObject({ code: 'E_SSH', component: 'frontend' });
});
```

(`toasts.test.ts` must mock `@tauri-apps/api/core` as the other tests do; add the `vi.mock` line and `inv` helper if absent, and call `resetErrorReportingForTests()` in its `beforeEach`.)

- [ ] **Step 2: Run to verify they fail**

Run: `npx vitest run src/lib/error_report.test.ts src/lib/toasts.test.ts`
Expected: FAIL, module not found.

- [ ] **Step 3: Implement** `src/lib/error_report.ts`:

```ts
import { invokeCmd } from './result';

// Frontend errors for the hub error channel (spec
// docs/superpowers/specs/2026-09-21-hub-error-channel-design.md). Each call
// becomes one `report_client_error` invoke, which the backend queues and — in
// hub-client mode — flushes to the hub. Two guards keep a render loop from
// becoming a request loop: identical component+message within DEDUPE_MS is
// counted, not resent, and at most MAX_PER_MINUTE distinct reports go out.

export const MAX_PER_MINUTE = 20;
export const DEDUPE_MS = 60_000;

interface Seen {
  at: number;
  repeats: number;
}

let seen = new Map<string, Seen>();
let windowStart = 0;
let sentThisWindow = 0;
let pendingRepeats = 0;

export function resetErrorReportingForTests(): void {
  seen = new Map();
  windowStart = 0;
  sentThisWindow = 0;
  pendingRepeats = 0;
}

export function reportError(
  component: string,
  message: string,
  code: string | null = null,
  context: Record<string, unknown> = {},
): void {
  const now = Date.now();
  const key = `${component} ${message}`;
  const prior = seen.get(key);
  if (prior && now - prior.at < DEDUPE_MS) {
    prior.repeats += 1;
    pendingRepeats += 1;
    return;
  }
  seen.set(key, { at: now, repeats: 0 });
  if (now - windowStart >= 60_000) {
    windowStart = now;
    sentThisWindow = 0;
  }
  if (sentThisWindow >= MAX_PER_MINUTE) return;
  sentThisWindow += 1;
  const ctx: Record<string, unknown> = { ...context };
  if (pendingRepeats > 0) {
    ctx.repeats = pendingRepeats;
    pendingRepeats = 0;
  }
  void invokeCmd('report_client_error', {
    args: { level: 'error', component, code, message: message.slice(0, 2048), context: ctx },
  });
}

function describe(reason: unknown): string {
  if (reason instanceof Error) return `${reason.name}: ${reason.message}`;
  if (typeof reason === 'string') return reason;
  try {
    return JSON.stringify(reason).slice(0, 500);
  } catch {
    return String(reason);
  }
}

export function installErrorReporting(win: Window = window): void {
  win.addEventListener('error', (e) => {
    const stack = e.error instanceof Error && e.error.stack ? e.error.stack.slice(0, 2000) : undefined;
    reportError('frontend:unhandled', e.message || describe(e.error), null, {
      file: e.filename,
      line: e.lineno,
      stack,
    });
  });
  win.addEventListener('unhandledrejection', (e) => {
    const reason = (e as PromiseRejectionEvent).reason;
    const stack = reason instanceof Error && reason.stack ? reason.stack.slice(0, 2000) : undefined;
    reportError('frontend:unhandled', describe(reason), null, { stack });
  });
}
```

`toasts.ts`: `import { reportError } from './error_report';` and in `pushError`, before `return push(...)`: `reportError('frontend', base, error.code ?? null);`.

`main.ts`: `import { installErrorReporting } from './lib/error_report';` and `installErrorReporting();` after `initTheme();`.

- [ ] **Step 4: Run to verify they pass**

Run: `npx vitest run src/lib/error_report.test.ts src/lib/toasts.test.ts && npx svelte-check`
Expected: pass, no type errors. Then the full frontend suite: `npx vitest run` — any test that counts `invoke` calls after a `pushError` needs its expectation to filter out `report_client_error` (search for such tests with `grep -rn "pushError" src --include=*.test.ts`).

- [ ] **Step 5: Commit**

```bash
git add src/lib/error_report.ts src/lib/error_report.test.ts src/lib/toasts.ts src/lib/toasts.test.ts src/main.ts
git commit -m "feat(ui): report frontend crashes and error toasts to the hub error channel"
```

---

### Task 13: `fleet-hub reports`

**Files:**
- Create: `crates/fleet-hub/src/reports.rs`
- Modify: `crates/fleet-hub/src/main.rs` (`mod reports;`, `Cmd::Reports`, dispatch, `cli_parses_every_subcommand`), `crates/fleet-hub/src/pair.rs` (`hub_conn`, `HubConn`, `exchange`, `display_width` → `pub(crate)`)

**Interfaces:**
- Produces: `reports::run(opts: &HubOptions, env: &HashMap<String, String>, limit: u32, since: Option<String>, origin: Option<String>, json: bool) -> Result<ExitCode, String>`; pure `parse_since(s: &str, now: i64) -> Result<i64, String>`; `table(rows: &[serde_json::Value], width: usize) -> String`; `parse_json_response(raw: &str) -> Result<serde_json::Value, String>`.

- [ ] **Step 1: Write the failing tests** in `reports.rs`:

```rust
//! `fleet-hub reports`: read the error channel back from the running hub
//! over `GET /reports` (master token, loopback), like `client list` does.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_accepts_durations_and_unix_seconds() {
        assert_eq!(parse_since("30m", 10_000).unwrap(), 10_000 - 1800);
        assert_eq!(parse_since("2h", 10_000).unwrap(), 10_000 - 7200);
        assert_eq!(parse_since("3d", 1_000_000).unwrap(), 1_000_000 - 259_200);
        assert_eq!(parse_since("1700000000", 0).unwrap(), 1_700_000_000);
        assert!(parse_since("soon", 0).is_err());
        assert!(parse_since("5w", 0).is_err());
    }

    #[test]
    fn the_table_renders_newest_first_and_marks_truncation() {
        let rows = vec![
            serde_json::json!({ "received_at": 1_790_000_000, "origin": "host:box", "level": "error",
                "component": "fleet_agent::conn", "code": null, "message": "dial refused", "truncated": false }),
            serde_json::json!({ "received_at": 1_789_999_000, "origin": "client:desk", "level": "error",
                "component": "frontend:unhandled", "code": "E_PARSE", "message": "x".repeat(300), "truncated": true }),
        ];
        let t = table(&rows, 120);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("RECEIVED"));
        assert!(lines[1].contains("host:box") && lines[1].contains("dial refused"));
        assert!(lines[2].contains("E_PARSE") && lines[2].ends_with("[trunc]"));
        assert!(lines[2].len() <= 120 + "[trunc]".len() + 1);
        assert_eq!(table(&[], 80), "no error reports");
    }

    #[test]
    fn a_json_response_is_parsed_and_a_non_200_is_an_error_with_the_body() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"id\":1}]";
        assert_eq!(parse_json_response(ok).unwrap(), serde_json::json!([{ "id": 1 }]));
        let no = "HTTP/1.1 403 Forbidden\r\n\r\nreports are the master token's to read";
        assert!(parse_json_response(no).unwrap_err().contains("master token"));
    }
}
```

`main.rs`'s `cli_parses_every_subcommand`: add

```rust
        let Cmd::Reports { limit, since, origin, json, .. } =
            Cli::try_parse_from(["fleet-hub", "reports", "--limit", "5", "--since", "2h", "--origin", "hub", "--json"]).unwrap().cmd
        else { panic!("reports did not parse") };
        assert_eq!((limit, since.as_deref(), origin.as_deref(), json), (5, Some("2h"), Some("hub"), true));
        Cli::try_parse_from(["fleet-hub", "reports"]).unwrap();
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-hub reports:: cli_parses`
Expected: compile errors.

- [ ] **Step 3: Implement**

`pair.rs`: change `fn hub_conn`, `struct HubConn`, `async fn exchange`, `fn display_width` to `pub(crate)`.

`reports.rs` above the tests:

```rust
use crate::config::HubOptions;
use crate::out;
use crate::pair::{display_width, exchange, fmt_time, hub_conn};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

/// `30m`, `2h`, `3d`, or a unix timestamp.
pub fn parse_since(s: &str, now: i64) -> Result<i64, String> {
    let s = s.trim();
    if let Ok(unix) = s.parse::<i64>() {
        return Ok(unix);
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num.parse().map_err(|_| format!("--since {s:?}: use 30m, 2h, 3d or a unix timestamp"))?;
    let secs = match unit {
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(format!("--since {s:?}: use 30m, 2h, 3d or a unix timestamp")),
    };
    Ok(now - n * secs)
}

pub fn parse_json_response(raw: &str) -> Result<serde_json::Value, String> {
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("the hub sent a malformed HTTP response")?;
    let status = head.lines().next().unwrap_or_default().split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(format!("the hub answered {status}: {}", body.trim()));
    }
    serde_json::from_str(body).map_err(|e| format!("the hub's answer is not JSON: {e}"))
}

pub fn table(rows: &[serde_json::Value], width: usize) -> String {
    if rows.is_empty() {
        return "no error reports".to_string();
    }
    let field = |r: &serde_json::Value, k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("-").to_string();
    let header = ["RECEIVED", "ORIGIN", "LEVEL", "COMPONENT", "CODE", "MESSAGE"];
    let cells: Vec<[String; 6]> = rows
        .iter()
        .map(|r| {
            [
                fmt_time(r.get("received_at").and_then(|v| v.as_i64())),
                field(r, "origin"),
                field(r, "level"),
                field(r, "component"),
                field(r, "code"),
                field(r, "message").lines().next().unwrap_or_default().to_string(),
            ]
        })
        .collect();
    let mut w = header.map(str::len);
    for row in &cells {
        for (i, c) in row.iter().enumerate().take(5) {
            w[i] = w[i].max(display_width(c));
        }
    }
    let fixed: usize = w[..5].iter().sum::<usize>() + 5 * 2;
    let msg_w = width.saturating_sub(fixed).max(20);
    let mut out = String::new();
    let line = |cols: [&str; 6], out: &mut String| {
        for (i, c) in cols.iter().enumerate().take(5) {
            out.push_str(c);
            out.extend(std::iter::repeat_n(' ', w[i].saturating_sub(display_width(c)) + 2));
        }
        out.push_str(cols[5]);
        out.push('\n');
    };
    line(header, &mut out);
    for (row, r) in cells.iter().zip(rows) {
        let mut msg: String = row[5].chars().take(msg_w).collect();
        if msg.chars().count() < row[5].chars().count() {
            msg.push('…');
        }
        if r.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false) {
            msg.push_str(" [trunc]");
        }
        line([&row[0], &row[1], &row[2], &row[3], &row[4], &msg], &mut out);
    }
    out.trim_end_matches('\n').to_string()
}

pub async fn run(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    limit: u32,
    since: Option<String>,
    origin: Option<String>,
    json: bool,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    let now = fleet_proto::report::now_unix();
    let mut query = format!("limit={}", limit.clamp(1, 1000));
    if let Some(s) = since {
        query.push_str(&format!("&since={}", parse_since(&s, now)?));
    }
    if let Some(o) = origin {
        query.push_str(&format!("&origin={}", urlencoding::encode(&o)));
    }
    let request = format!(
        "GET /reports?{query} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        conn.addr, conn.token
    );
    let raw = match tokio::time::timeout(CALL_TIMEOUT, exchange(conn.addr, conn.tls, &request)).await {
        Ok(r) => r?,
        Err(_) => return Err(format!("{} did not answer within {CALL_TIMEOUT:.0?}", conn.addr)),
    };
    let rows = parse_json_response(&raw)?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    if json {
        out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
    } else {
        let width = std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(120);
        out::line(&table(&rows, width));
    }
    Ok(ExitCode::SUCCESS)
}
```

If `urlencoding` is not a fleet-hub dependency, replace `urlencoding::encode(&o)` with a manual percent-encoding of `:` only (`o.replace(':', "%3A")`) — origins are `client:<name>`/`host:<alias>`/`hub`, and names are validated printable ASCII. Add `fleet-proto` to `fleet-hub`'s `Cargo.toml` if not present.

`main.rs`: `mod reports;`; add to `Cmd`:

```rust
    /// Show the error reports the hub has collected from its participants, newest first. Needs a running hub.
    Reports {
        /// Rows to show (1-1000). [default: 100]
        #[arg(long, default_value_t = 100)]
        limit: u32,
        /// Only rows received since: 30m, 2h, 3d or a unix timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Only rows from this origin: client:<name>, host:<alias> or hub.
        #[arg(long)]
        origin: Option<String>,
        /// Print the rows as JSON (includes each report's context).
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        opts: HubOptions,
    },
```

and dispatch: `Cmd::Reports { limit, since, origin, json, opts } => reports::run(&opts, &env, limit, since, origin, json).await,`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p fleet-hub && cargo clippy -p fleet-hub --all-targets -- -D warnings`
Expected: pass, clean.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-hub
git commit -m "feat(hub-cli): fleet-hub reports reads the error channel"
```

---

### Task 14: Documentation and the full local CI mirror

**Files:**
- Modify: `docs/hub.md` (new section after *Events* ~line 652; *Configuration* table ~line 800; *What is different from standalone* ~line 975; *Troubleshooting* ~line 1271; the `fleet-agent` config section ~line 196–305)

- [ ] **Step 1: Write the docs.** Insert after the *Events* section:

````markdown
## Error reports

The hub is also the one place its participants' errors are collected. The
desktop paired to this hub, every `fleet-agent`, and the hub itself send
their **error-level** log events here; the hub keeps a bounded, redacted
table of them, and:

```bash
fleet-hub reports                      # the newest 100
fleet-hub reports --since 2h --origin host:build-box
fleet-hub reports --limit 500 --json   # with each report's context
```

```
RECEIVED           ORIGIN            LEVEL  COMPONENT                 CODE     MESSAGE
2026-09-21 10:41Z  host:build-box    error  fleet_agent::conn         -        dial https://fleet.example.com: connection refused
2026-09-21 10:40Z  client:mac-desk   error  frontend:unhandled        -        TypeError: Cannot read properties of undefined  [trunc]
2026-09-21 10:38Z  hub               error  fleet_core::ssh           E_SSH    ssh mefistos: Host key verification failed
```

Every accepted report is also one `warn` line in the hub's own log, under
the target `fleet_core::report`, so `journalctl -u fleet-hub | grep report`
works too.

**Who sends what.** Only `error`-level `tracing` events, never warnings
(the reconcile tick warns per unreachable host per pass). Each sender keeps
a queue of 256 and drops the oldest, counted, when it is full; nothing ever
waits on the hub. The desktop sends every 5 s (or at 20 queued) to
`POST /report`, and stops for the run when the hub answers `404` (it
predates this route) or refuses the client; `CLAUDE_FLEET_HUB_REPORTS=0`
in the desktop's environment turns it off. An agent sends up to 16 per
heartbeat in a `report` frame, including errors from *before* it managed to
connect — which is the case the channel exists for; `"report_errors": false`
in its config turns it off. The hub's own errors join the table on the
reconcile tick under origin `hub`.

**Bounds.** `reports.max_rows` (default 5000) newest rows are kept, pruned
on every insert; rows older than `reports.max_age_secs` (default 604800,
seven days; `0` never) are swept on the reconcile tick. An origin may store
60 reports a minute; a batch that would cross that is refused whole with
`429`. A message is at most 2048 characters, a context 4 KB, a body 64 KB.

**Privacy.** Every string is run through the same redaction the log gets
(bearer tokens, `?token=` values, 64-hex strings) before it is stored. No
prompt, transcript or pane text has a path here: `tracing` never logs
bodies, and a frontend report carries the toast's message and, for a crash,
a stack. `GET /reports` is master-token only, since the rows hold every
client's messages.

**For a phone or any client:** `POST /report` with the client's own bearer
token (`readonly` included), body

```json
{ "reports": [ { "at": 1790000000, "level": "error", "component": "screen:sessions",
                 "code": "E_PARSE", "message": "…", "context": { "…": "…" } } ],
  "dropped": 0 }
```

at most 50 reports per call; `204` stored, `400` malformed or a level other
than `error`/`warn`, `413` over 64 KB, `429` over budget (nothing stored).
The origin is taken from the token, never from the body.
````

*Configuration* table: two rows, `| — | — | reports.max_rows | 5000 |` and `| — | — | reports.max_age_secs | 604800 |` (with a sentence under the table: "The two `reports.*` settings have no flag: set them over the API with `set_setting`.") *What is different from standalone*: one bullet, "**Its errors reach the hub.** Error-level events and frontend crashes are queued and posted to the hub's `/report` every few seconds — see *Error reports*; `CLAUDE_FLEET_HUB_REPORTS=0` turns it off." *Troubleshooting*: "**`fleet-hub reports` is empty** — the hub predates the route (the desktop logs `no /report route` once), the desktop was started with `CLAUDE_FLEET_HUB_REPORTS=0`, the agent's config has `report_errors: false`, or the sender's `RUST_LOG` silences `error`." The agent's config section: mention `"report_errors": true` (default) beside `insecure`/`ca_file`.

- [ ] **Step 2: Run the whole CI mirror**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/hub-error-channel-debug-b701a6
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo build -p fleet-hub --locked
pnpm install --frozen-lockfile
npx svelte-check
npx vitest run
pnpm run build
```

Expected: all green, unpiped (read the full output; never judge through `| tail`). `reference_is_current`, `verdict_gen` and the hub contract test must pass without `REGEN_*` set.

- [ ] **Step 3: Commit**

```bash
git add docs/hub.md
git commit -m "docs(hub): the error channel — reports, bounds, privacy, the client contract"
```

---

## Self-review

- **Spec coverage.** Record/batch/ring/clamp → Task 1. Frame variant + old-hub leniency → Task 2. Table, row cap, age sweep, list → Tasks 3, 4, 8. Redaction, rate limit, log line, `dropped` line → Task 4. Layer in desktop and hub → Task 5; in the agent → Task 9. `POST /report` (readonly may post, host tokens too, caps, statuses) and `GET /reports` (master only, filters) → Task 6. Hub stores agent frames under `host:<alias>` → Task 7. Hub's own ring → Tasks 4, 8. Desktop flusher (5 s / 20, redact, 404/401/403 stop, 429 minute, backoff, env switch, only `Backend::Remote`) → Task 10. Frontend listeners, `pushError`, dedupe, 20/min → Task 12. Command, verdict, regen → Task 11. CLI (`--limit --since --origin --json`, table, `[trunc]`) → Task 13. Docs (section, config rows, standalone bullet, troubleshooting, agent config, phone contract) → Task 14. Agent `report_errors` config → Task 9.
- **Placeholders.** None: every step carries its code; the two "read the local helper name" notes (Task 7's token constant, Task 9's `Agent::new` shape) point at exact lines.
- **Type consistency.** `ReportBatch { reports, dropped }` everywhere; `ingest(store, windows, origin, batch, batch_max, now)` in Tasks 4, 6, 7; `ReportFilter { limit, since, origin, level }` in Tasks 3, 6, 13's query string; `Flush` variants in Task 10 only; `report_client_error` args `{ level, component, code, message, context }` in Tasks 11 and 12; `report_ring()` (fleet-core) vs `report::ring()` (fleet-agent) are deliberately two functions in two crates.
