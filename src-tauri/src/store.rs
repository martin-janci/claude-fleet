// Store owns the SQLite connection. It is wrapped in `Mutex<Store>` and
// registered via `tauri::Manager::manage()` because `rusqlite::Connection`
// is not Send+Sync. Commands access it via `State<'_, Mutex<Store>>`.

// Store is a coherent data-access API; several methods (e.g. `with_transaction`,
// `get_account_by_uuid`, `delete_session`) are currently exercised only by
// `#[cfg(test)]` code, so they read as dead in a non-test build.
#![allow(dead_code)]

use crate::events::{EventBus, NoopEventBus, RowChange};
use rusqlite::{Connection, OptionalExtension, Result};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectRow {
    pub id: i64,
    pub owner: String,
    pub repo: String,
    pub base_path: String,
    pub last_session_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeRow {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
}

/// `PartialEq` covers every wire field, so `upsert_session_in_tx` can tell a
/// no-op reconcile pass from a real change before emitting `session:updated`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionRow {
    pub id: i64,
    pub tmux_name: String,
    pub host_alias: String,
    pub project_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub status: String,
    pub notes: Option<String>,
    pub account_uuid: Option<String>,
    pub kind: String,
    pub reviews_session_id: Option<i64>,
    pub worktree_key: Option<String>,
    pub lost_at: Option<i64>,
    pub claude_session_id: Option<String>,
    pub claude_status: Option<String>,
    pub effort_level: Option<String>,
    pub pr_url: Option<String>,
    pub current_activity: Option<String>,
    pub context_pct: Option<f64>,
    pub stuck_kind: Option<String>,
    /// Display label set by the in-session agent via the `set_friendly_name`
    /// MCP tool (migration 016). The sidebar shows this when the user's
    /// "friendly names" toggle is on; falls back to `tmux_name` when NULL.
    pub friendly_name: Option<String>,
    pub safe_kill_state: Option<String>,
    pub safe_kill_nonce: Option<String>,
    pub safe_kill_detail: Option<String>,
    pub safe_kill_requested_at: Option<i64>,
    // ── Lifecycle + outcome fields (migration 019) ──
    /// When `claude_status` last entered idle/completed/stopped; NULL while
    /// working/blocked/unknown. Drives the GC sweeper.
    pub idle_since: Option<i64>,
    /// When the current `stuck_kind` episode began; NULL when not stuck.
    pub stuck_since: Option<i64>,
    /// When a stuck playbook last acted on this row. One stamp shared by
    /// every playbook kind: it gates "once per stuck episode" for all of
    /// them and the 1 h spacing for `oom`.
    pub last_playbook_at: Option<i64>,
    /// First 200 chars of the last prompt sent through fleet.
    pub last_prompt: Option<String>,
    /// When fleet created the session (NULL for tmux-discovered rows).
    pub started_at: Option<i64>,
    /// Last Stop hook (turn completed).
    pub last_turn_at: Option<i64>,
    /// `passing` | `failing` | `pending` from the PR's check rollup.
    pub ci_status: Option<String>,
    // ── Orchestration fields (migration 020) ──
    /// Number of completed turns, incremented by every Stop hook. Callers
    /// snapshot it before `send_prompt` and wait for it to grow.
    pub turn_seq: i64,
    /// Unix secs of the last Stop hook (a hook-stamped status newer than a
    /// reconcile pass's pane observation wins over the pane heuristic).
    pub last_stop_at: Option<i64>,
    /// The requester session that dispatched the task this row is working
    /// on; NULL for top-level sessions.
    pub parent_session_id: Option<i64>,
    /// Free-form labels set via `set_session_tags`. Stored as a JSON array
    /// (NULL ⇒ empty) and always surfaced as a list on the wire.
    pub tags: Vec<String>,
}

/// The `sessions` column list every `SessionRow` read shares, in the order
/// `map_session_row` consumes it. One definition so a new column is added in
/// exactly two places (here and the mapper) instead of six.
const SESSION_COLUMNS: &str = "id, tmux_name, host_alias, project_id, worktree_id, created_at, \
     last_activity_at, status, notes, account_uuid, kind, reviews_session_id, \
     worktree_key, lost_at, \
     claude_session_id, claude_status, effort_level, pr_url, current_activity, \
     context_pct, stuck_kind, friendly_name, \
     safe_kill_state, safe_kill_nonce, safe_kill_detail, safe_kill_requested_at, \
     idle_since, stuck_since, last_playbook_at, last_prompt, started_at, last_turn_at, ci_status, \
     turn_seq, last_stop_at, parent_session_id, tags";

/// Decode the `sessions.tags` JSON column. NULL, empty, or malformed text
/// (never written by us, but a hand-edited DB is possible) reads as no tags
/// rather than failing every session read.
pub fn decode_tags(raw: Option<String>) -> Vec<String> {
    raw.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

/// Encode tags for the `sessions.tags` column: `None` for an empty list so
/// an untagged row stays NULL (and `tag IS NULL` style queries work).
pub fn encode_tags(tags: &[String]) -> Option<String> {
    if tags.is_empty() {
        None
    } else {
        serde_json::to_string(tags).ok()
    }
}

/// Map one `SELECT {SESSION_COLUMNS}` row to a `SessionRow`.
fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        tmux_name: row.get(1)?,
        host_alias: row.get(2)?,
        project_id: row.get(3)?,
        worktree_id: row.get(4)?,
        created_at: row.get(5)?,
        last_activity_at: row.get(6)?,
        status: row.get(7)?,
        notes: row.get(8)?,
        account_uuid: row.get(9)?,
        kind: row.get(10)?,
        reviews_session_id: row.get(11)?,
        worktree_key: row.get(12)?,
        lost_at: row.get(13)?,
        claude_session_id: row.get(14)?,
        claude_status: row.get(15)?,
        effort_level: row.get(16)?,
        pr_url: row.get(17)?,
        current_activity: row.get(18)?,
        context_pct: row.get(19)?,
        stuck_kind: row.get(20)?,
        friendly_name: row.get(21)?,
        safe_kill_state: row.get(22)?,
        safe_kill_nonce: row.get(23)?,
        safe_kill_detail: row.get(24)?,
        safe_kill_requested_at: row.get(25)?,
        idle_since: row.get(26)?,
        stuck_since: row.get(27)?,
        last_playbook_at: row.get(28)?,
        last_prompt: row.get(29)?,
        started_at: row.get(30)?,
        last_turn_at: row.get(31)?,
        ci_status: row.get(32)?,
        turn_seq: row.get(33)?,
        last_stop_at: row.get(34)?,
        parent_session_id: row.get(35)?,
        tags: decode_tags(row.get(36)?),
    })
}

/// `claude_status` values that mean "no turn in progress" — the states
/// `idle_since` is stamped on (see migration 019).
pub const IDLE_STATUSES: [&str; 3] = ["idle", "completed", "stopped"];

/// SQL fragment: the new `idle_since` given the OLD row's `idle_since` and the
/// status expression `{st}` (which must resolve to the post-write status).
/// Entering an idle status stamps `now` once; staying idle keeps the stamp;
/// leaving clears it.
fn idle_since_sql(st: &str, now_param: &str) -> String {
    format!(
        "CASE WHEN ({st}) IN ('idle','completed','stopped') \
              THEN COALESCE(idle_since, {now_param}) ELSE NULL END"
    )
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostRow {
    pub alias: String,
    pub ssh_alias: Option<String>,
    pub reachable: bool,
    pub claude_version: Option<String>,
    pub tmux_version: Option<String>,
    pub hidden: bool,
    pub last_pinged_at: Option<i64>,
    pub account_uuid: Option<String>,
    pub provisioned: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountRow {
    pub uuid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_uuid: Option<String>,
    pub seat_tier: Option<String>,
    pub last_seen_at: Option<i64>,
}

/// One row of the append-only per-session event timeline (migration 013).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: i64,
    pub at: i64,
    pub kind: String,
    pub detail: Option<String>,
}

/// One per-host control-API bearer token (migration 018). `mode` is `full`
/// or `readonly`; see `mcp::auth::TokenMode`. The token itself is never sent
/// to the frontend — `HostTokenInfo` in `commands/mcp.rs` projects this row
/// without it.
#[derive(Debug, Clone)]
pub struct HostTokenRow {
    pub host_alias: String,
    pub token: String,
    pub created_at: i64,
    pub mode: String,
}

/// One inter-session message (migration 015). The store is the source of
/// truth; pane delivery, if requested, happens separately and best-effort.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMessage {
    pub id: i64,
    pub from_session_id: i64,
    pub to_session_id: i64,
    pub body: String,
    pub kind: String,
    pub sent_at: i64,
    /// Unix-epoch second the recipient first listed this message, or `None`
    /// when still unread.
    pub read_at: Option<i64>,
    /// Id of the message this one answers (migration 020); `None` when the
    /// message is not a reply.
    pub reply_to: Option<i64>,
}

/// One dispatched unit of work (migration 020). `state` is one of
/// [`TASK_STATES`]; `result` is the paragraph the worker printed after its
/// `FLEET_TASK_DONE_<nonce>` marker, `error` the failure/cancel reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TaskRow {
    pub id: i64,
    pub requester_session_id: Option<i64>,
    pub worker_session_id: Option<i64>,
    pub prompt: Option<String>,
    pub state: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// Per-task random tag baked into the completion marker. Never sent to
    /// the frontend (the marker must not be forgeable from the UI).
    #[serde(skip_serializing)]
    pub nonce: String,
    /// Worker's `claude_session_id` at dispatch (liveness check; internal).
    #[serde(skip_serializing)]
    pub worker_claude_session_id: Option<String>,
}

/// The task state machine: `queued → running → done | failed | cancelled`.
pub const TASK_STATES: [&str; 5] = ["queued", "running", "done", "failed", "cancelled"];
/// States a task never leaves.
pub const TASK_TERMINAL_STATES: [&str; 3] = ["done", "failed", "cancelled"];

const TASK_COLUMNS: &str = "id, requester_session_id, worker_session_id, prompt, state, result, \
     error, created_at, started_at, finished_at, nonce, worker_claude_session_id";

/// [`TASK_COLUMNS`] qualified with the `t.` alias for joined queries.
fn task_columns_t() -> String {
    TASK_COLUMNS
        .split(',')
        .map(|c| format!("t.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn map_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        requester_session_id: row.get(1)?,
        worker_session_id: row.get(2)?,
        prompt: row.get(3)?,
        state: row.get(4)?,
        result: row.get(5)?,
        error: row.get(6)?,
        created_at: row.get(7)?,
        started_at: row.get(8)?,
        finished_at: row.get(9)?,
        nonce: row.get(10)?,
        worker_claude_session_id: row.get(11)?,
    })
}

/// One live session to upsert during a reconcile write-burst. `project_id`,
/// `account_uuid`, and `worktree_key` are PRE-RESOLVED by the caller (they
/// require reads — `find_project_id_for_path` / `get_session_account` /
/// `worktree_key_for_path` — that must run before the transaction opens).
pub struct ReconcileSession<'a> {
    pub tmux_name: &'a str,
    pub project_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub account_uuid: Option<String>,
    pub worktree_key: Option<String>,
    // NEW — from claude agents --json:
    pub claude_session_id: Option<String>,
    pub claude_status: Option<String>,
    pub effort_level: Option<String>,
    pub pr_url: Option<String>,
    pub current_activity: Option<String>,
    pub context_pct: Option<f64>,
    pub stuck_kind: Option<String>,
    /// Whether this pass actually captured & analyzed the session's pane. When
    /// `true`, `stuck_kind` is authoritative and a `None` CLEARS any prior stuck
    /// flag; when `false` (capture failed / pane absent) the prior `stuck_kind`
    /// is preserved. Without this, a once-set stuck flag could never clear.
    pub intel_observed: bool,
    /// `passing` | `failing` | `pending` reduced from the PR check rollup.
    pub ci_status: Option<String>,
    /// Whether this pass ran the `gh pr view` probe for the session. When
    /// `true`, `pr_url` / `ci_status` are authoritative (a `None` clears a
    /// closed PR's stale link); when `false` the prior values are preserved.
    pub pr_observed: bool,
}

/// All inputs for applying one host's probe result atomically. Consumed by
/// `Store::apply_host_reconcile`.
pub struct HostReconcile<'a> {
    pub alias: &'a str,
    /// Whether the probe succeeded. `false` ⇒ only the host row's
    /// reachability/versions are updated; sessions are left untouched.
    pub reachable: bool,
    pub claude_version: Option<&'a str>,
    pub tmux_version: Option<&'a str>,
    pub last_pinged_at: i64,
    /// Unix-epoch second at which the probe that produced this result STARTED.
    /// Rows that another writer reconciled at or after this instant (their
    /// `last_reconciled_at >= probe_started_at`) are exempt from ghosting: the
    /// probe's `keep` set predates them, so their absence from it is not
    /// evidence they are gone (BE-3). `0` disables the guard (every row is
    /// eligible), which is what the store-level tests use.
    pub probe_started_at: i64,
    /// Live sessions to upsert (empty / ignored when `!reachable`).
    pub sessions: &'a [ReconcileSession<'a>],
    /// tmux_names to keep; rows on this host not in the set are deleted
    /// (only used when `reachable`).
    pub keep: &'a [String],
}

/// Max `session_events` rows kept per session. Enforced on every
/// `insert_session_event` (oldest rows beyond the cap are pruned) so a
/// status-flapping session cannot grow the table without bound.
pub const SESSION_EVENTS_CAP: i64 = 500;

/// Max chars of a prompt kept in `sessions.last_prompt`.
pub const LAST_PROMPT_CHARS: usize = 200;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Ordered schema migrations, `(version, sql)`. Versions are contiguous from
/// 1 and every script must end by recording its own version with
/// `INSERT OR IGNORE INTO schema_version (version) VALUES (N)` — the tests
/// enforce both. `migrate()` runs entry 0 unconditionally (it is idempotent
/// and bootstraps `schema_version`) and each later entry in its own
/// transaction iff its version is above the recorded maximum. To add one:
/// drop `NNN_name.sql` into `migrations/` and append it here.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/001_init.sql")),
    (2, include_str!("../migrations/002_hosts_ssh.sql")),
    (3, include_str!("../migrations/003_accounts.sql")),
    (4, include_str!("../migrations/004_session_account.sql")),
    (5, include_str!("../migrations/005_session_reviews.sql")),
    (
        6,
        include_str!("../migrations/006_session_worktree_key.sql"),
    ),
    (7, include_str!("../migrations/007_indexes.sql")),
    (8, include_str!("../migrations/008_ghost_sessions.sql")),
    (9, include_str!("../migrations/009_session_claude_id.sql")),
    (
        10,
        include_str!("../migrations/010_claude_agent_fields.sql"),
    ),
    (11, include_str!("../migrations/011_host_provisioned.sql")),
    (
        12,
        include_str!("../migrations/012_session_context_pressure.sql"),
    ),
    (13, include_str!("../migrations/013_session_events.sql")),
    (14, include_str!("../migrations/014_last_reconciled_at.sql")),
    (15, include_str!("../migrations/015_session_messages.sql")),
    (
        16,
        include_str!("../migrations/016_session_friendly_name.sql"),
    ),
    (17, include_str!("../migrations/017_safe_kill.sql")),
    (18, include_str!("../migrations/018_host_tokens.sql")),
    (19, include_str!("../migrations/019_lifecycle_fields.sql")),
    (20, include_str!("../migrations/020_tasks_and_turns.sql")),
    (21, include_str!("../migrations/021_repair_backoff.sql")),
];

/// The schema version a fully migrated database reports.
#[cfg(test)]
const LATEST_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;

pub struct Store {
    conn: Connection,
    bus: Arc<dyn EventBus>,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::open_with_bus(path, Arc::new(NoopEventBus))
    }

    pub fn open_with_bus(path: &std::path::Path, bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn, bus };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn,
            bus: Arc::new(NoopEventBus),
        };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_with_bus_in_memory(bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn, bus };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        // The bootstrap migration is idempotent (`CREATE TABLE IF NOT EXISTS`
        // + `INSERT OR IGNORE`) and always runs: on a fresh DB it also creates
        // `schema_version`, which the gate below needs to exist.
        let (_, bootstrap) = MIGRATIONS[0];
        self.conn.execute_batch(bootstrap)?;
        // Newer migrations are applied only if not yet recorded. We can't
        // wrap them in CREATE-OR-IGNORE because they ALTER existing tables.
        let v: i64 = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        // Each ALTER-based migration runs in its OWN transaction, together
        // with the `schema_version` row it inserts. SQLite DDL is
        // transactional, so an interrupted migration rolls back entirely —
        // it can never leave a column half-added, which on the next launch
        // would re-run the migration and fail with "duplicate column",
        // bricking startup.
        for (version, sql) in MIGRATIONS.iter().copied() {
            if version <= v {
                continue;
            }
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.commit()?;
        }
        self.reap_orphan_session_events()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn has_table(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
    }

    /// Append one row to the per-session event timeline (migration 013). The
    /// timeline is append-only; callers must treat a write failure as
    /// non-fatal (log + continue) so it can never block the mutation that
    /// produced the event.
    ///
    /// Each insert also prunes the session's timeline down to
    /// `SESSION_EVENTS_CAP` newest rows. A status flap observed by the
    /// background reconcile tick can otherwise grow one session's timeline
    /// without bound (observed: ~200k `status_change` rows per session); the
    /// prune is a cheap indexed subselect and keeps the table bounded.
    pub fn insert_session_event(
        &self,
        session_id: i64,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO session_events (session_id, at, kind, detail) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![session_id, at, kind, detail],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        self.conn
            .execute(
                "DELETE FROM session_events WHERE session_id=?1 AND id NOT IN (\
                   SELECT id FROM session_events WHERE session_id=?1 \
                   ORDER BY at DESC, id DESC LIMIT ?2)",
                rusqlite::params![session_id, SESSION_EVENTS_CAP],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Return the newest-first event timeline for a session, capped at `limit`.
    /// Ordering is `at DESC, id DESC` so events inserted within the same second
    /// still come back in insertion order (newest first).
    pub fn list_session_events(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, session_id, at, kind, detail FROM session_events \
                 WHERE session_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, limit], |row| {
                Ok(SessionEvent {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    at: row.get(2)?,
                    kind: row.get(3)?,
                    detail: row.get(4)?,
                })
            })
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Insert one inter-session message (migration 015). `sent_at` is stamped
    /// here as the current unix epoch. Returns the new row id so the caller
    /// can include it in the pane-delivery header.
    pub fn insert_message(
        &self,
        from_session_id: i64,
        to_session_id: i64,
        body: &str,
        kind: &str,
        reply_to: Option<i64>,
    ) -> Result<i64, crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO session_messages \
                   (from_session_id, to_session_id, body, kind, sent_at, reply_to) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![from_session_id, to_session_id, body, kind, at, reply_to],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// One message by id (any recipient). Used to validate `reply_to`.
    pub fn get_message(
        &self,
        id: i64,
    ) -> Result<Option<SessionMessage>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to \
                 FROM session_messages WHERE id = ?1",
                rusqlite::params![id],
                map_message_row,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
    }

    /// Newest-first messages addressed to `to_session_id`, capped at `limit`.
    /// When `unread_only`, only rows whose `read_at IS NULL` are returned.
    pub fn list_inbox(
        &self,
        to_session_id: i64,
        unread_only: bool,
        limit: i64,
    ) -> Result<Vec<SessionMessage>, crate::ipc_error::IpcError> {
        let sql = if unread_only {
            "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to \
             FROM session_messages \
             WHERE to_session_id = ?1 AND read_at IS NULL \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        } else {
            "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to \
             FROM session_messages \
             WHERE to_session_id = ?1 \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![to_session_id, limit], map_message_row)
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Mark a set of inbox messages as read. Only rows whose `to_session_id`
    /// matches `recipient` are updated — never mark someone else's mail.
    /// Returns the number of rows that flipped from unread to read.
    pub fn mark_messages_read(
        &self,
        ids: &[i64],
        recipient: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE session_messages SET read_at = ?1 \
             WHERE to_session_id = ?2 AND read_at IS NULL AND id IN ({placeholders})",
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&at, &recipient];
        for id in ids {
            params.push(id);
        }
        let n = self
            .conn
            .execute(sql.as_str(), rusqlite::params_from_iter(params))
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(n)
    }

    // ---- host_tokens (migration 018) ----

    /// Every per-host token row, alias-ordered. The MCP auth layer loads
    /// this per request and compares in constant time, so a token never
    /// runs through a SQL string comparison.
    pub fn list_host_tokens(&self) -> Result<Vec<HostTokenRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host_alias, token, created_at, mode FROM host_tokens ORDER BY host_alias",
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(HostTokenRow {
                    host_alias: row.get(0)?,
                    token: row.get(1)?,
                    created_at: row.get(2)?,
                    mode: row.get(3)?,
                })
            })
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// The token row for one host, if it has been provisioned.
    pub fn get_host_token(
        &self,
        host_alias: &str,
    ) -> Result<Option<HostTokenRow>, crate::ipc_error::IpcError> {
        Ok(self
            .list_host_tokens()?
            .into_iter()
            .find(|r| r.host_alias == host_alias))
    }

    /// Insert or replace a host's token, keeping its mode when the row
    /// already exists. `created_at` is stamped now.
    pub fn upsert_host_token(
        &self,
        host_alias: &str,
        token: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO host_tokens (host_alias, token, created_at, mode) \
                 VALUES (?1, ?2, ?3, 'full') \
                 ON CONFLICT(host_alias) DO UPDATE SET \
                   token = excluded.token, created_at = excluded.created_at",
                rusqlite::params![host_alias, token, at],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Set a host's token mode (`full` | `readonly`). `E_NOTFOUND` when the
    /// host has no token yet (it must be provisioned first).
    pub fn set_host_token_mode(
        &self,
        host_alias: &str,
        mode: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let n = self
            .conn
            .execute(
                "UPDATE host_tokens SET mode = ?2 WHERE host_alias = ?1",
                rusqlite::params![host_alias, mode],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                "E_NOTFOUND",
                format!("host {host_alias} has no control-API token (provision it first)"),
            ));
        }
        Ok(())
    }

    /// Drop a host's token (e.g. when the host is removed). Idempotent.
    pub fn delete_host_token(&self, host_alias: &str) -> Result<(), crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "DELETE FROM host_tokens WHERE host_alias = ?1",
                rusqlite::params![host_alias],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Read a value from the key/value `settings` table. `None` if absent.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .optional()
    }

    /// Insert or replace a value in the `settings` table.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Record which session is the fleet controller (the calling session that
    /// must not kill/recreate/restart itself without `force`). Stored as two
    /// keys in the `settings` table.
    pub fn set_controller(&self, host: &str, tmux_name: &str) -> Result<()> {
        self.set_setting("controller.host", host)?;
        self.set_setting("controller.tmux", tmux_name)?;
        Ok(())
    }

    /// Read the registered controller as `(host, tmux_name)`. `None` unless
    /// both keys are present.
    pub fn get_controller(&self) -> Result<Option<(String, String)>> {
        let host = self.get_setting("controller.host")?;
        let tmux = self.get_setting("controller.tmux")?;
        Ok(host.zip(tmux))
    }

    // ---- Private fetch helpers used after writes to produce emit payloads ----
    //
    // The row-mapping SQL lives in free `fetch_*` functions that take a bare
    // `&Connection` so it can be reused both by these `&self` helpers AND by
    // the `_in_tx` mutation variants (a `&Transaction` derefs to `&Connection`).

    pub fn get_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        fetch_session(&self.conn, tmux_name, host_alias)
    }

    fn get_host(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
    }

    fn get_project(&self, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
        fetch_project(&self.conn, id)
    }

    fn get_worktree(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch FROM worktrees WHERE id=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    // ---- Public mutation methods ----

    pub fn upsert_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO projects (owner, repo, base_path) VALUES (?1, ?2, ?3)
             ON CONFLICT(owner, repo) DO UPDATE SET base_path=excluded.base_path",
            rusqlite::params![owner, repo, base_path],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE owner=?1 AND repo=?2",
            rusqlite::params![owner, repo],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_project(id)? {
            self.bus.project_updated(&row);
        }
        Ok(id)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, owner, repo, base_path, last_session_at FROM projects ORDER BY owner, repo",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                owner: row.get(1)?,
                repo: row.get(2)?,
                base_path: row.get(3)?,
                last_session_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Single-query variant that builds `Vec<ProjectTreeRow>` in one trip —
    /// eliminates the N+1 of calling `list_worktrees_for_project` per project.
    ///
    /// Projects are ordered: most-recently-used first, NULLs last, then by id.
    /// Within each project worktrees are ordered by id.
    pub fn list_projects_joined(
        &self,
    ) -> Result<Vec<crate::service::projects::ProjectTreeRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.owner, p.repo, p.base_path, p.last_session_at,
                    w.id, w.project_id, w.name, w.path, w.branch
             FROM projects p
             LEFT JOIN worktrees w ON w.project_id = p.id
             ORDER BY
               CASE WHEN p.last_session_at IS NULL THEN 1 ELSE 0 END,
               p.last_session_at DESC,
               p.id,
               w.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;
        let mut out: Vec<crate::service::projects::ProjectTreeRow> = Vec::new();
        let mut last_pid: Option<i64> = None;
        for r in rows {
            let (pid, owner, repo, base, last, wid, _wpid, wname, wpath, wbranch) = r?;
            if last_pid != Some(pid) {
                out.push(crate::service::projects::ProjectTreeRow {
                    project: ProjectRow {
                        id: pid,
                        owner,
                        repo,
                        base_path: base,
                        last_session_at: last,
                    },
                    worktrees: Vec::new(),
                });
                last_pid = Some(pid);
            }
            if let (Some(wid), Some(wname), Some(wpath)) = (wid, wname, wpath) {
                out.last_mut().unwrap().worktrees.push(WorktreeRow {
                    id: wid,
                    project_id: pid,
                    name: wname,
                    path: wpath,
                    branch: wbranch,
                });
            }
        }
        Ok(out)
    }

    pub fn upsert_worktree(
        &self,
        project_id: i64,
        name: &str,
        path: &str,
        branch: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO worktrees (project_id, name, path, branch) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, name) DO UPDATE SET path=excluded.path, branch=excluded.branch",
            rusqlite::params![project_id, name, path, branch],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM worktrees WHERE project_id=?1 AND name=?2",
            rusqlite::params![project_id, name],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_worktree(id)? {
            self.bus.worktree_updated(&row);
        }
        Ok(id)
    }

    pub fn list_worktrees_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch FROM worktrees WHERE project_id=?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Hard-delete one worktree row by id. Emits `worktree:removed`. Returns
    /// the row that was removed (or `None` if it didn't exist).
    ///
    /// Does NOT touch sessions referencing this worktree; the caller is
    /// expected to have checked for live occupants first (see
    /// `service::worktrees::delete_worktree`). Dead/ghost session rows that
    /// still point here have their `worktree_id` cleared so the FK stays
    /// consistent.
    pub fn delete_worktree(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let Some(row) = self.get_worktree(id)? else {
            return Ok(None);
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE sessions SET worktree_id=NULL WHERE worktree_id=?1",
            rusqlite::params![id],
        )?;
        tx.execute("DELETE FROM worktrees WHERE id=?1", rusqlite::params![id])?;
        tx.commit()?;
        self.bus.worktree_removed(id);
        Ok(Some(row))
    }

    /// Return the names + hosts of alive (non-ghost, non-dead) sessions
    /// currently attached to a worktree id. Empty when the worktree is free.
    pub fn alive_sessions_for_worktree(
        &self,
        worktree_id: i64,
    ) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, tmux_name
               FROM sessions
              WHERE worktree_id=?1 AND status='running' AND lost_at IS NULL
              ORDER BY host_alias, tmux_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![worktree_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    pub fn get_worktree_row(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        self.get_worktree(id)
    }

    /// Delete this project's worktree rows whose name is not in `keep_names`
    /// (the fresh `git worktree list`). A doomed row may still be referenced
    /// by a session: `sessions.worktree_id` has no ON DELETE and foreign keys
    /// are ON, so a plain DELETE failed and aborted this and every later
    /// refresh (main-first naming replaces the old basename-named main row
    /// while a session still points at it). So, in one transaction, each
    /// doomed row's references move to the surviving row of the same project
    /// with the same canonical path (`canon` maps a stored path to its
    /// canonical form); any left over are cleared, as `delete_worktree` does.
    /// Emits `worktree:removed` per deleted row. Returns how many went.
    pub fn delete_worktrees_not_in(
        &self,
        project_id: i64,
        keep_names: &[String],
        canon: impl Fn(&str) -> String,
    ) -> Result<usize, rusqlite::Error> {
        let (keep, doomed): (Vec<WorktreeRow>, Vec<WorktreeRow>) = self
            .list_worktrees_for_project(project_id)?
            .into_iter()
            .partition(|w| keep_names.contains(&w.name));
        if doomed.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        for d in &doomed {
            let key = canon(&d.path);
            if let Some(survivor) = keep.iter().find(|k| canon(&k.path) == key) {
                tx.execute(
                    "UPDATE sessions SET worktree_id=?1 WHERE worktree_id=?2",
                    rusqlite::params![survivor.id, d.id],
                )?;
            }
            tx.execute(
                "UPDATE sessions SET worktree_id=NULL WHERE worktree_id=?1",
                rusqlite::params![d.id],
            )?;
            tx.execute("DELETE FROM worktrees WHERE id=?1", rusqlite::params![d.id])?;
        }
        tx.commit()?;
        for d in &doomed {
            self.bus.worktree_removed(d.id);
        }
        Ok(doomed.len())
    }

    pub fn touch_project_last_session_at(
        &self,
        project_id: i64,
        ts: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE projects SET last_session_at = MAX(COALESCE(last_session_at, 0), ?1) WHERE id = ?2",
            rusqlite::params![ts, project_id],
        )?;
        if let Some(row) = self.get_project(project_id)? {
            self.bus.project_updated(&row);
        }
        Ok(())
    }

    /// Delete a project and all its associated sessions and worktrees atomically.
    /// Called after `claude project purge` removes Claude's state on the remote machine.
    pub fn delete_project(&self, project_id: i64) -> Result<(), crate::ipc_error::IpcError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(crate::ipc_error::IpcError::from)?;
        tx.execute(
            "DELETE FROM sessions WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(crate::ipc_error::IpcError::from)?;
        tx.execute(
            "DELETE FROM worktrees WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(crate::ipc_error::IpcError::from)?;
        tx.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(crate::ipc_error::IpcError::from)?;
        tx.commit().map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Delete a project row unless a session references it, either directly
    /// (`project_id`) or through one of its worktree rows (`worktree_id`).
    /// The worktree check matters: the old duplicate-scan bug left sessions
    /// whose `project_id` is another project (or NULL) pointing at this
    /// project's worktree rows, and `delete_project` removing those rows
    /// would violate the foreign key. Returns whether the row went.
    /// `refresh_projects` uses it for stale rows outside the projects root and
    /// for duplicate rows naming a checkout another project owns.
    pub fn delete_project_if_unused(
        &self,
        project_id: i64,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        let in_use: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions
               WHERE project_id = ?1
                  OR worktree_id IN (SELECT id FROM worktrees WHERE project_id = ?1))",
            rusqlite::params![project_id],
            |r| r.get(0),
        )?;
        if in_use {
            return Ok(false);
        }
        self.delete_project(project_id)?;
        Ok(true)
    }

    /// Delete one worktree row unless any session row (alive, ghost or dead)
    /// references it. Emits `worktree:removed` when the row goes. Returns
    /// whether it went.
    pub fn delete_worktree_if_unused(&self, id: i64) -> Result<bool, rusqlite::Error> {
        let n = self.conn.execute(
            "DELETE FROM worktrees WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM sessions WHERE worktree_id = ?1)",
            rusqlite::params![id],
        )?;
        if n > 0 {
            self.bus.worktree_removed(id);
        }
        Ok(n > 0)
    }

    pub fn conn_ref(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn upsert_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // Check existence first so `host_added` fires only on a genuine
        // insert. Reconcile calls this for `local` every run; without the
        // check it emitted a spurious host:added (plus a get_host fetch)
        // on every window focus.
        let existed = self
            .conn
            .query_row("SELECT 1 FROM hosts WHERE alias=?1", [alias], |_| Ok(()))
            .optional()?
            .is_some();
        self.conn.execute(
            "INSERT INTO hosts (alias, reachable) VALUES (?1, 1)
             ON CONFLICT(alias) DO UPDATE SET reachable=1",
            rusqlite::params![alias],
        )?;
        if !existed {
            if let Some(row) = self.get_host(alias)? {
                self.bus.host_added(&row);
            }
        }
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                    last_pinged_at, account_uuid, provisioned
             FROM hosts
             ORDER BY (alias='local') DESC, alias ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HostRow {
                alias: row.get(0)?,
                ssh_alias: row.get(1)?,
                reachable: row.get::<_, i64>(2)? != 0,
                claude_version: row.get(3)?,
                tmux_version: row.get(4)?,
                hidden: row.get::<_, i64>(5)? != 0,
                last_pinged_at: row.get(6)?,
                account_uuid: row.get(7)?,
                provisioned: row.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn insert_host(&self, alias: &str, ssh_alias: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, ssh_alias, reachable, hidden) VALUES (?1, ?2, 0, 0)
             ON CONFLICT(alias) DO UPDATE SET ssh_alias=excluded.ssh_alias",
            rusqlite::params![alias, ssh_alias],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_added(&row);
        }
        Ok(())
    }

    pub fn update_host_probe(
        &self,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn set_host_hidden(&self, alias: &str, hidden: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET hidden=?1 WHERE alias=?2",
            rusqlite::params![if hidden { 1 } else { 0 }, alias],
        )?;
        // Emit like every other host mutation — the HostRow carries `hidden`,
        // so subscribers see the toggle without a manual refetch.
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT uuid, email, display_name, organization_name, organization_uuid,
                    seat_tier, last_seen_at
             FROM accounts
             ORDER BY uuid ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AccountRow {
                uuid: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                organization_name: row.get(3)?,
                organization_uuid: row.get(4)?,
                seat_tier: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn upsert_account(&self, a: &AccountRow) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO accounts (uuid, email, display_name, organization_name,
                                   organization_uuid, seat_tier, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(uuid) DO UPDATE SET
               email=excluded.email,
               display_name=excluded.display_name,
               organization_name=excluded.organization_name,
               organization_uuid=excluded.organization_uuid,
               seat_tier=excluded.seat_tier,
               last_seen_at=excluded.last_seen_at",
            rusqlite::params![
                a.uuid,
                a.email,
                a.display_name,
                a.organization_name,
                a.organization_uuid,
                a.seat_tier,
                a.last_seen_at
            ],
        )?;
        self.bus.account_upserted(a);
        Ok(())
    }

    pub fn get_account_by_uuid(&self, uuid: &str) -> Result<Option<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT uuid, email, display_name, organization_name, organization_uuid,
                    seat_tier, last_seen_at
             FROM accounts WHERE uuid=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![uuid], |row| {
            Ok(AccountRow {
                uuid: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                organization_name: row.get(3)?,
                organization_uuid: row.get(4)?,
                seat_tier: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn set_host_account(
        &self,
        alias: &str,
        account_uuid: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET account_uuid=?1 WHERE alias=?2",
            rusqlite::params![account_uuid, alias],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn set_host_provisioned(
        &self,
        alias: &str,
        provisioned: bool,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET provisioned=?1 WHERE alias=?2",
            rusqlite::params![if provisioned { 1 } else { 0 }, alias],
        )?;
        Ok(())
    }

    pub fn delete_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // The `local` host is never removed.
        if alias == "local" {
            return Ok(());
        }
        // Collect orphaned session ids first so we can emit a `session_killed`
        // event per row — otherwise frontend stores subscribed to session events
        // would carry stale rows that point to a host that no longer exists.
        let orphan_ids: Vec<i64> = self
            .conn
            .prepare_cached("SELECT id FROM sessions WHERE host_alias=?1")?
            .query_map(rusqlite::params![alias], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        self.conn.execute(
            "DELETE FROM sessions WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        self.conn
            .execute("DELETE FROM hosts WHERE alias=?1", rusqlite::params![alias])?;
        // A removed host's control-API token must stop authenticating.
        self.conn.execute(
            "DELETE FROM host_tokens WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        for id in &orphan_ids {
            self.bus.session_killed(*id);
        }
        self.bus.host_removed(alias);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        status: &str,
        account_uuid: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        // Check existence before the write so we can distinguish created vs updated.
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        // INSERT ... RETURNING id — one statement instead of the old
        // INSERT then separate `SELECT id`.
        let id: i64 = self.conn.query_row(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid
             RETURNING id",
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                status,
                account_uuid
            ],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_session(tmux_name, host_alias)? {
            if existing_id.is_none() {
                self.bus.session_created(&row);
            } else {
                self.bus.session_updated(&row);
            }
        }
        Ok(id)
    }

    /// Upsert a synthetic `kind='bg'` session for a `claude --bg` agent that has
    /// NO matching tmux session. The sentinel `tmux_name` (`bg:<sessionId>`)
    /// keeps it unique under the `(host_alias, tmux_name)` constraint and signals
    /// to the UI that there is no tmux pane to attach. Refreshes the live
    /// `claude_status` on every reconcile; the row's `kind='bg'` exempts it from
    /// the tmux-keyed ghost cleanup (it is never in the tmux `keep` set) — bg
    /// rows are instead pruned against the `claude agents --json` result by
    /// `ghost_and_clean_bg_sessions`. A row that was ghosted by that pruner and
    /// whose agent reappears is resurrected here (`status='running'`,
    /// `lost_at=NULL`), mirroring the tmux upsert's ghost revival.
    pub fn upsert_bg_session(
        &self,
        host_alias: &str,
        tmux_name: &str,
        project_id: Option<i64>,
        claude_session_id: &str,
        claude_status: Option<&str>,
        last_activity_at: i64,
    ) -> Result<i64, rusqlite::Error> {
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        let sql = format!(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, kind,
                                   claude_session_id, claude_status, idle_since)
             VALUES (?1, ?2, ?3, NULL, ?4, ?4, 'running', 'bg', ?5, ?6,
                     CASE WHEN ?6 IN ('idle','completed','stopped') THEN ?7 ELSE NULL END)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=COALESCE(excluded.project_id, project_id),
               last_activity_at=excluded.last_activity_at,
               kind='bg',
               status='running',
               lost_at=NULL,
               claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
               claude_status=COALESCE(excluded.claude_status, claude_status),
               idle_since={idle}
             RETURNING id",
            idle = idle_since_sql("COALESCE(excluded.claude_status, claude_status)", "?7"),
        );
        let id: i64 = self.conn.query_row(
            &sql,
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                last_activity_at,
                claude_session_id,
                claude_status,
                now_unix()
            ],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_session(tmux_name, host_alias)? {
            if existing_id.is_none() {
                self.bus.session_created(&row);
            } else {
                self.bus.session_updated(&row);
            }
        }
        Ok(id)
    }

    /// Two-phase cleanup for synthetic `kind='bg'` rows on one host, keyed on
    /// the CURRENT `claude agents --json` result (`keep_names` = the sentinel
    /// `bg:<sessionId>` names observed this reconcile pass) instead of the tmux
    /// `keep` set. Mirrors `ghost_and_clean_sessions_in_tx`:
    ///
    /// Phase 1: live bg rows not in `keep_names` → `status='ghost'`,
    /// `lost_at=now`. Phase 2: bg rows already ghost BEFORE this pass and still
    /// absent → hard-deleted, together with their `session_events` (no FK
    /// cascade exists). The one-cycle grace matters because a failed
    /// `claude agents` probe is indistinguishable from "no agents" (both come
    /// back as an empty list): a transient miss only ghosts, and
    /// `upsert_bg_session` resurrects the row when the agent reappears.
    ///
    /// Without this pruner, dead bg rows accumulate forever (observed:
    /// 22k rows / 88MB state.db).
    pub fn ghost_and_clean_bg_sessions(
        &self,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let mut changes: Vec<RowChange> = Vec::new();

        // Phase 2 prep: already-ghost bg ids, collected BEFORE Phase 1 so rows
        // ghosted this pass survive one more cycle.
        let pre_ghost_ids: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare_cached(
                "SELECT id FROM sessions WHERE host_alias=?1 AND status='ghost' AND kind='bg'",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let phs = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id FROM sessions
                 WHERE host_alias=?1 AND status='ghost' AND kind='bg' AND tmux_name NOT IN ({phs})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        // Phase 1: ghost live bg rows whose agent vanished from the listing.
        let ghost_ids: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare_cached(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND kind='bg'
                 RETURNING id",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![now, host_alias], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let phs = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND kind='bg' AND tmux_name NOT IN ({phs})
                 RETURNING id"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        for id in &ghost_ids {
            if let Some(row) = fetch_session_by_id(&tx, *id)? {
                changes.push(RowChange::SessionUpdated(row));
            }
        }

        // Phase 2: hard-delete rows that were already ghost, plus their events.
        if !pre_ghost_ids.is_empty() {
            let phs = pre_ghost_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let params: Vec<&dyn rusqlite::ToSql> = pre_ghost_ids
                .iter()
                .map(|id| id as &dyn rusqlite::ToSql)
                .collect();
            tx.execute(
                &format!("DELETE FROM session_events WHERE session_id IN ({phs})"),
                params.as_slice(),
            )?;
            tx.execute(
                &format!("DELETE FROM sessions WHERE id IN ({phs})"),
                params.as_slice(),
            )?;
            for id in &pre_ghost_ids {
                changes.push(RowChange::SessionKilled(*id));
            }
        }

        tx.commit()?;
        // Emit only after the commit so no event fires for a rolled-back write.
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
    }

    pub fn get_session_account(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![host_alias, tmux_name], |row| {
            row.get::<_, Option<String>>(0)
        })?;
        match rows.next() {
            Some(r) => Ok(r?),
            None => Ok(None),
        }
    }

    pub fn list_sessions_for_host(
        &self,
        host_alias: &str,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            &format!(
             "SELECT {SESSION_COLUMNS} FROM sessions WHERE host_alias=?1 ORDER BY last_activity_at DESC"),
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], map_session_row)?;
        rows.collect()
    }

    /// All sessions across every host, in one query. Used by `reconcile_sessions`
    /// to collect its return value once at the end instead of N per-host reads.
    pub fn list_all_sessions(&self) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions ORDER BY last_activity_at DESC"
        ))?;
        let rows = stmt.query_map([], map_session_row)?;
        rows.collect()
    }

    pub fn list_related_sessions(
        &self,
        session_id: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        // Look up source's (project_id, worktree_key) first.
        let (proj, key): (Option<i64>, Option<String>) = self.conn.query_row(
            "SELECT project_id, worktree_key FROM sessions WHERE id=?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // Orphans (project_id=NULL) have no relateds — they share no identity.
        let Some(project_id) = proj else {
            return Ok(Vec::new());
        };
        // A project-having session always has a worktree_key after reconcile
        // ("main" at minimum). A NULL key (legacy/pre-reconcile) matches nothing.
        let Some(key) = key else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions
             WHERE project_id=?1 AND worktree_key=?2 AND id<>?3
             ORDER BY host_alias ASC, tmux_name ASC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![project_id, key, session_id], |row| {
            map_session_row(row)
        })?;
        rows.collect()
    }

    /// Mark a session as a review of `reviews_session_id` (or back to 'work' with
    /// None). Write-once at spawn_review time. Reconcile never touches these
    /// columns — they survive re-probe because upsert_session's ON CONFLICT clause
    /// omits them.
    pub fn set_session_kind(
        &self,
        id: i64,
        kind: &str,
        reviews_session_id: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET kind = ?1, reviews_session_id = ?2 WHERE id = ?3",
            rusqlite::params![kind, reviews_session_id, id],
        )?;
        if let Some(row) = self.get_session_by_id(id)? {
            self.bus.session_updated(&row);
        }
        Ok(())
    }

    /// Record the Claude Code session id minted for a session. Reconcile's
    /// `upsert_session` never writes this column, so the value survives
    /// reconciliation.
    pub fn set_claude_session_id(&self, id: i64, uuid: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET claude_session_id=?1 WHERE id=?2",
            rusqlite::params![uuid, id],
        )?;
        Ok(())
    }

    /// Set a session's portable worktree key (derived from its cwd by reconcile).
    /// Emits `session_updated` so the frontend patches in place.
    pub fn set_worktree_key(&self, id: i64, key: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET worktree_key = ?1 WHERE id = ?2",
            rusqlite::params![key, id],
        )?;
        if let Some(row) = self.get_session_by_id(id)? {
            self.bus.session_updated(&row);
        }
        Ok(())
    }

    /// One-shot startup pass: give every session row with `friendly_name IS
    /// NULL` a deterministic label derived from its branch (or the worktree
    /// name when no branch is recorded, or the tmux name as last resort).
    /// Skips `kind='bg'` rows — their synthetic `bg:<uuid>` tmux names
    /// humanise poorly. Bypasses the event bus by design: the frontend hasn't
    /// subscribed yet, and emitting one event per row at boot is pure noise.
    /// Returns the number of rows updated.
    pub fn backfill_friendly_names(&self) -> Result<usize, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.tmux_name,
                    COALESCE(p.owner, '') AS owner,
                    COALESCE(p.repo, '') AS repo,
                    COALESCE(w.branch, w.name) AS branch
               FROM sessions s
               LEFT JOIN projects p ON p.id = s.project_id
               LEFT JOIN worktrees w ON w.id = s.worktree_id
              WHERE s.friendly_name IS NULL
                AND COALESCE(s.kind, 'work') != 'bg'",
        )?;
        let rows: Vec<(i64, String, String, String, Option<String>)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut updated = 0usize;
        for (id, tmux_name, owner, repo, branch) in rows {
            let source = branch.unwrap_or(tmux_name);
            let label = crate::humanize::humanize_branch(&source, &owner, &repo);
            if label.is_empty() {
                continue;
            }
            self.conn.execute(
                "UPDATE sessions SET friendly_name = ?1 WHERE id = ?2",
                rusqlite::params![label, id],
            )?;
            updated += 1;
        }
        Ok(updated)
    }

    /// The deterministic branch-derived label `new_session` /
    /// `backfill_friendly_names` would give this row (PR #28), or `None` for
    /// bg rows and rows whose humanised name is empty. Lets callers tell a
    /// still-default label from one a human or agent chose.
    pub fn default_friendly_name(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        let row: Option<(String, String, String, Option<String>, String)> = self
            .conn
            .query_row(
                "SELECT s.tmux_name,
                        COALESCE(p.owner, '') AS owner,
                        COALESCE(p.repo, '') AS repo,
                        COALESCE(w.branch, w.name) AS branch,
                        COALESCE(s.kind, 'work')
                   FROM sessions s
                   LEFT JOIN projects p ON p.id = s.project_id
                   LEFT JOIN worktrees w ON w.id = s.worktree_id
                  WHERE s.id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((tmux_name, owner, repo, branch, kind)) = row else {
            return Ok(None);
        };
        if kind == "bg" {
            return Ok(None);
        }
        let source = branch.unwrap_or(tmux_name);
        let label = crate::humanize::humanize_branch(&source, &owner, &repo);
        Ok(if label.is_empty() { None } else { Some(label) })
    }

    /// Set the session's display label (migration 016). `None` clears it.
    /// Emits `session_updated` so the sidebar patches in place.
    pub fn set_friendly_name(
        &self,
        host_alias: &str,
        tmux_name: &str,
        friendly_name: Option<&str>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let changed = self.conn.execute(
            "UPDATE sessions SET friendly_name = ?1 \
             WHERE host_alias = ?2 AND tmux_name = ?3",
            rusqlite::params![friendly_name, host_alias, tmux_name],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let row = fetch_session(&self.conn, tmux_name, host_alias)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Mark a safe-kill request: stamp state="requested", store the nonce we
    /// embedded in the prompt, and clear any prior failure detail. Emits
    /// session_updated.
    pub fn set_safe_kill_requested(
        &self,
        id: i64,
        nonce: &str,
        requested_at: i64,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions
                SET safe_kill_state='requested',
                    safe_kill_nonce=?1,
                    safe_kill_detail=NULL,
                    safe_kill_requested_at=?2
              WHERE id=?3",
            rusqlite::params![nonce, requested_at, id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Transition a safe-kill request to its terminal state ("ready" or
    /// "failed") and store the optional failure detail. Emits session_updated.
    pub fn set_safe_kill_outcome(
        &self,
        id: i64,
        state: &str,
        detail: Option<&str>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions
                SET safe_kill_state=?1,
                    safe_kill_detail=?2
              WHERE id=?3",
            rusqlite::params![state, detail, id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Clear the safe-kill state (used when the user cancels or retries a
    /// failed attempt). Emits session_updated.
    pub fn clear_safe_kill(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions
                SET safe_kill_state=NULL,
                    safe_kill_nonce=NULL,
                    safe_kill_detail=NULL,
                    safe_kill_requested_at=NULL
              WHERE id=?1",
            rusqlite::params![id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Transition a session back to running (clears `lost_at`). Called by the
    /// `recreate_session` flow after `new_session` rebuilds the tmux session on
    /// the host — for both ghost and live (RAM/wedged) recreates.
    pub fn restore_session(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET status='running', lost_at=NULL WHERE id=?1",
            rusqlite::params![id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    pub fn get_session_by_id(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        fetch_session_by_id(&self.conn, id)
    }

    /// Remember the most recent prompt sent to a session (first 200 chars,
    /// migration 019). Emits `session_updated`.
    pub fn set_last_prompt(
        &self,
        id: i64,
        prompt: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let truncated: String = prompt.chars().take(LAST_PROMPT_CHARS).collect();
        self.conn.execute(
            "UPDATE sessions SET last_prompt=?1 WHERE id=?2",
            rusqlite::params![truncated, id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Stamp when fleet created this session (migration 019). Only sets the
    /// value once — a re-create keeps the original start.
    pub fn set_started_at(&self, id: i64, at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET started_at=COALESCE(started_at, ?1) WHERE id=?2",
            rusqlite::params![at, id],
        )?;
        Ok(())
    }

    /// Record that a stuck playbook acted on this row: stamps
    /// `last_playbook_at`, appends a `playbook_applied` timeline event carrying
    /// the kind, and emits `session_updated`.
    pub fn mark_playbook_applied(
        &self,
        id: i64,
        at: i64,
        detail: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET last_playbook_at=?1 WHERE id=?2",
            rusqlite::params![at, id],
        )?;
        if let Err(e) = self.insert_session_event(id, "playbook_applied", Some(detail)) {
            eprintln!("[playbook] session_event insert failed for {id}: {e}");
        }
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    pub fn get_host_row(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
    }

    pub fn worktree_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT path FROM worktrees WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Worktree's logical name (the leaf used in remote
    /// `~/projects/.../.claude/worktrees/<name>` paths). The on-host `path`
    /// column is local-machine-only and unusable for remote rebuilds.
    pub fn worktree_name(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT name FROM worktrees WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn project_base_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT base_path FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Run `f` under the implicit lock and return its result.
    ///
    /// The helper exists for documentation: at call sites,
    /// `let data = { let s = store.lock().unwrap(); s.with_snapshot(|s| s.list_hosts()) };`
    /// makes it visible that the lock is held only for the duration of the closure
    /// — and downstream readers can see the I/O happens after the lock drops.
    pub fn with_snapshot<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Store) -> R,
    {
        f(self)
    }

    /// Run `f` inside a single `conn.transaction()`. Used by reconcile paths
    /// that batch many upserts/deletes after a fan-out of off-lock probes —
    /// one fsync per batch instead of one per row.
    pub fn with_transaction<F, R>(&mut self, f: F) -> rusqlite::Result<R>
    where
        F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<R>,
    {
        let tx = self.conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }

    // ---- Reconcile write-burst: single transaction + emit-after-commit ----
    //
    // The `*_in_tx` helpers below run ONLY their SQL against an ambient
    // `&Transaction` and return a `RowChange` describing the event to emit —
    // they do NOT touch `self.bus`. `apply_host_reconcile` drives them inside
    // one transaction, commits, and only THEN flushes the collected changes to
    // the bus. A mid-batch error rolls the whole transaction back, so no event
    // fires for a write that didn't persist.
    //
    // The public `update_host_probe` / `upsert_session` /
    // `touch_project_last_session_at` / `delete_sessions_not_in` methods are
    // intentionally left untouched — direct (non-reconcile) callers keep
    // emitting immediately. Note: `ghost_and_clean_sessions_in_tx` has no
    // public twin by design — reconcile is the only caller.
    //
    // MAINTENANCE: each `*_in_tx` helper deliberately mirrors the SQL of its
    // public twin (same column lists, same upsert ON CONFLICT clause, same
    // SELECT-ids-before-DELETE). They differ ONLY in: (a) `tx` vs `self.conn`,
    // and (b) collecting a `RowChange` vs emitting via `self.bus`. If you change
    // a schema/SQL detail in a public method, change its `_in_tx` twin too.
    // Both paths are test-covered (direct: the `*_emits_*` event tests; tx: the
    // `apply_host_reconcile` rollback + happy-path tests), so a divergence will
    // surface as a test failure rather than silent corruption.
    //
    // `worktree_key` is written by `upsert_session_in_tx` ONLY — the public
    // `upsert_session` intentionally omits it (reconcile is the only path that
    // knows the session's cwd and can compute the key).

    fn update_host_probe_in_tx(
        tx: &rusqlite::Transaction,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        tx.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        if let Some(row) = fetch_host(tx, alias)? {
            out.push(RowChange::HostProbed(row));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_session_in_tx(
        tx: &rusqlite::Transaction,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        account_uuid: Option<&str>,
        worktree_key: Option<&str>,
        claude_session_id: Option<&str>,
        claude_status: Option<&str>,
        effort_level: Option<&str>,
        pr_url: Option<&str>,
        current_activity: Option<&str>,
        context_pct: Option<f64>,
        stuck_kind: Option<&str>,
        intel_observed: bool,
        ci_status: Option<&str>,
        pr_observed: bool,
        probe_started_at: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // Read the prior row (not just its id) before the write: it tells us
        // created-vs-updated AND, after the write, whether anything the
        // frontend can see actually changed. Reconcile upserts every live
        // session every pass; without this diff each pass emitted one
        // `session:updated` per session — sixty store flushes per tick for a
        // fleet that had not changed at all (BE-11 / FE-10). Note that
        // `update_host_probe_in_tx` still emits one `host:probed` per host
        // per pass: `last_pinged_at` moves every time, so it cannot be diffed
        // away — one event per host, not per session.
        let prior: Option<SessionRow> = fetch_session(tx, tmux_name, host_alias)?;

        // The post-write stuck_kind, spelled out once and reused: SQLite's
        // upsert SET clauses see the OLD row (unqualified) and the candidate
        // (`excluded`), never each other's results.
        const NEW_STUCK: &str = "CASE WHEN ?16 THEN excluded.stuck_kind \
                                 ELSE COALESCE(excluded.stuck_kind, stuck_kind) END";
        // The post-write claude_status. A Stop hook that landed at or after
        // this pass's probe STARTED (`last_stop_at >= ?20`) is fresher than
        // the pane the pass captured, so its `idle` must win over the pane
        // heuristic (MCP-1: reconcile used to clobber the hook every tick).
        // `?20 <= 0` disables the guard (store-level tests pass 0).
        // `last_hook_at` is stamped by BOTH hooks (Stop → idle,
        // UserPromptSubmit → working). The guard only covers passes that were
        // already in flight when the hook landed; a pass that starts later
        // observes the pane afresh and wins, as it should.
        const NEW_STATUS: &str = "CASE WHEN ?20 > 0 AND last_hook_at IS NOT NULL \
                                            AND last_hook_at >= ?20 \
                                       THEN claude_status \
                                       ELSE COALESCE(excluded.claude_status, claude_status) END";
        let sql = format!(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid,
                                   worktree_key, lost_at,
                                   claude_session_id, claude_status, effort_level, pr_url, current_activity,
                                   context_pct, stuck_kind, ci_status, idle_since, stuck_since)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?17,
                     CASE WHEN ?10 IN ('idle','completed','stopped') THEN ?19 ELSE NULL END,
                     CASE WHEN ?15 IS NULL THEN NULL ELSE ?19 END)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               last_activity_at=excluded.last_activity_at,
               account_uuid=COALESCE(excluded.account_uuid, account_uuid),
               worktree_key=COALESCE(excluded.worktree_key, worktree_key),
               status=CASE WHEN status='ghost' THEN 'running' ELSE status END,
               lost_at=NULL,
               claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
               claude_status={new_status},
               effort_level=COALESCE(excluded.effort_level, effort_level),
               -- pr_url / ci_status are authoritative when the gh probe ran
               -- this pass (?18) so a closed PR's link clears; otherwise the
               -- prior values are preserved.
               pr_url=CASE WHEN ?18 THEN excluded.pr_url ELSE COALESCE(excluded.pr_url, pr_url) END,
               ci_status=CASE WHEN ?18 THEN excluded.ci_status
                              ELSE COALESCE(excluded.ci_status, ci_status) END,
               current_activity=COALESCE(excluded.current_activity, current_activity),
               context_pct=COALESCE(excluded.context_pct, context_pct),
               -- stuck_kind is authoritative when the pane was observed this
               -- pass (?16): a NULL then CLEARS a stale flag. When the pane was
               -- NOT observed (capture failed) we preserve the prior value.
               stuck_kind={new_stuck},
               -- stuck_since: keep the episode start while the kind is
               -- unchanged, restart it when the kind changes, clear when the
               -- flag clears.
               stuck_since=CASE WHEN ({new_stuck}) IS NULL THEN NULL
                                WHEN ({new_stuck}) IS stuck_kind THEN COALESCE(stuck_since, ?19)
                                ELSE ?19 END,
               idle_since={idle}",
            new_stuck = NEW_STUCK,
            new_status = NEW_STATUS,
            idle = idle_since_sql(NEW_STATUS, "?19"),
        );
        tx.execute(
            &sql,
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                account_uuid,
                worktree_key,
                claude_session_id,
                claude_status,
                effort_level,
                pr_url,
                current_activity,
                context_pct,
                stuck_kind,
                intel_observed,
                ci_status,
                pr_observed,
                now_unix(),
                probe_started_at
            ],
        )?;
        if let Some(row) = fetch_session(tx, tmux_name, host_alias)? {
            match prior {
                None => out.push(RowChange::SessionCreated(row)),
                // Every wire field identical ⇒ a no-op pass; emit nothing.
                Some(ref before) if *before == row => {}
                Some(_) => out.push(RowChange::SessionUpdated(row)),
            }
        }
        Ok(())
    }

    fn touch_project_last_session_at_in_tx(
        tx: &rusqlite::Transaction,
        project_id: i64,
        ts: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // Same no-op filter as `upsert_session_in_tx`: the MAX() update
        // matches the row every pass even when `last_session_at` is already
        // at least `ts`, so diff before/after instead of trusting the
        // affected-row count.
        let before = fetch_project(tx, project_id)?;
        tx.execute(
            "UPDATE projects SET last_session_at = MAX(COALESCE(last_session_at, 0), ?1) WHERE id = ?2",
            rusqlite::params![ts, project_id],
        )?;
        if let Some(row) = fetch_project(tx, project_id)? {
            if before.as_ref() != Some(&row) {
                out.push(RowChange::ProjectUpdated(row));
            }
        }
        Ok(())
    }

    /// Phase 1: sessions not in `keep_names` that are currently live (`status !=
    /// 'ghost'`) are soft-deleted by setting `status='ghost'` and `lost_at=now`.
    /// Phase 2: sessions that are already ghost (from a previous cycle) and still
    /// not in `keep_names` are hard-deleted.
    ///
    /// `kind='bg'` rows are EXCLUDED from both phases: background (`claude --bg`)
    /// sessions are never tmux sessions, so they can never appear in
    /// `keep_names`. Ghosting them on every reconcile would be wrong — they're
    /// surfaced from `claude agents --json`, not from tmux. They get their own
    /// agents-keyed pruner instead: `ghost_and_clean_bg_sessions`.
    ///
    /// `probe_started_at` (unix secs) guards Phase 1 against a stale probe:
    /// a row stamped `last_reconciled_at >= probe_started_at` was observed
    /// live by a writer whose probe began after this one's, so its absence
    /// from `keep_names` only means this probe is older than the row (e.g. a
    /// tick that listed tmux just before `new_session` created it). Such rows
    /// are left alone; the next pass, whose probe starts later, judges them.
    /// `0` disables the guard.
    fn ghost_and_clean_sessions_in_tx(
        tx: &rusqlite::Transaction,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
        probe_started_at: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // ── Phase 2 prep: collect already-ghost IDs BEFORE Phase 1 modifies rows
        // so that sessions newly ghosted in Phase 1 are not immediately deleted.
        let pre_ghost_ids: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare_cached(
                "SELECT id FROM sessions WHERE host_alias=?1 AND status='ghost' AND kind!='bg'",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let phs = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id FROM sessions
                 WHERE host_alias=?1 AND status='ghost' AND kind!='bg' AND tmux_name NOT IN ({phs})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        // ── Phase 1: ghost live sessions not in keep ──────────────────────────
        // Rows reconciled by a NEWER probe than ours are skipped (see doc).
        let ghost_ids: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare_cached(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND kind!='bg'
                   AND COALESCE(last_reconciled_at, 0) < ?3
                 RETURNING id",
            )?;
            let ids = stmt
                .query_map(
                    rusqlite::params![now, host_alias, ghost_cutoff(probe_started_at)],
                    |r| r.get(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let phs = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND kind!='bg'
                   AND COALESCE(last_reconciled_at, 0) < ?3 AND tmux_name NOT IN ({phs})
                 RETURNING id"
            );
            let cutoff = ghost_cutoff(probe_started_at);
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &host_alias, &cutoff];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        for id in &ghost_ids {
            if let Some(row) = fetch_session_by_id(tx, *id)? {
                out.push(RowChange::SessionUpdated(row));
            }
        }

        // ── Phase 2: hard-delete sessions that were already ghost before this cycle
        if !pre_ghost_ids.is_empty() {
            let phs = pre_ghost_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let params: Vec<&dyn rusqlite::ToSql> = pre_ghost_ids
                .iter()
                .map(|id| id as &dyn rusqlite::ToSql)
                .collect();
            // No FK cascade on session_events — delete them with the row or
            // they linger as orphans forever.
            tx.execute(
                &format!("DELETE FROM session_events WHERE session_id IN ({phs})"),
                params.as_slice(),
            )?;
            tx.execute(
                &format!("DELETE FROM sessions WHERE id IN ({phs})"),
                params.as_slice(),
            )?;
            for id in &pre_ghost_ids {
                out.push(RowChange::SessionKilled(*id));
            }
        }

        Ok(())
    }

    /// One probed/live session to apply during a reconcile write-burst, with
    /// its `(project_id, account_uuid)` ALREADY resolved by the caller (those
    /// are reads — `find_project_id_for_path` / `get_session_account` — and
    /// must happen before the transaction opens).
    pub fn apply_host_reconcile(&mut self, spec: HostReconcile<'_>) -> Result<(), rusqlite::Error> {
        // Phase 1: run all SQL inside one transaction, collecting RowChanges.
        let changes = self.with_transaction(|tx| {
            let mut out: Vec<RowChange> = Vec::new();
            Self::update_host_probe_in_tx(
                tx,
                spec.alias,
                spec.reachable,
                spec.claude_version,
                spec.tmux_version,
                spec.last_pinged_at,
                &mut out,
            )?;
            // Only a reachable probe rewrites the session set. An unreachable
            // host keeps its last-known rows (no upserts, no delete-not-in).
            if spec.reachable {
                // Accumulate the latest activity per project, then touch each
                // project ONCE — N sessions in one project would otherwise
                // fire N redundant UPDATEs + N `project:updated` events.
                let mut project_touch: std::collections::HashMap<i64, i64> =
                    std::collections::HashMap::new();
                for sess in spec.sessions {
                    Self::upsert_session_in_tx(
                        tx,
                        sess.tmux_name,
                        spec.alias,
                        sess.project_id,
                        None,
                        sess.created_at,
                        sess.last_activity_at,
                        sess.account_uuid.as_deref(),
                        sess.worktree_key.as_deref(),
                        sess.claude_session_id.as_deref(),
                        sess.claude_status.as_deref(),
                        sess.effort_level.as_deref(),
                        sess.pr_url.as_deref(),
                        sess.current_activity.as_deref(),
                        sess.context_pct,
                        sess.stuck_kind.as_deref(),
                        sess.intel_observed,
                        sess.ci_status.as_deref(),
                        sess.pr_observed,
                        spec.probe_started_at,
                        &mut out,
                    )?;
                    if let Some(pid) = sess.project_id {
                        let latest = project_touch.entry(pid).or_insert(0);
                        *latest = (*latest).max(sess.last_activity_at);
                    }
                }
                for (pid, ts) in project_touch {
                    Self::touch_project_last_session_at_in_tx(tx, pid, ts, &mut out)?;
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                Self::ghost_and_clean_sessions_in_tx(
                    tx,
                    spec.alias,
                    spec.keep,
                    now,
                    spec.probe_started_at,
                    &mut out,
                )?;
            }
            Ok(out)
        })?;

        // Phase 2: transaction committed — now it is safe to emit.
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
    }

    /// Stamp `last_reconciled_at = at` on the sessions a reconcile pass just
    /// observed live on `host_alias` (the `keep` set). This is the proactive
    /// freshness marker the Wave-2 background tick (Task H) relies on so the
    /// frontend can gray out rows whose host has gone quiet. Best-effort and
    /// emit-free: it does not change any user-visible row field, so it neither
    /// fires row events nor aborts reconcile on failure.
    pub fn mark_sessions_reconciled(
        &self,
        host_alias: &str,
        keep_names: &[String],
        at: i64,
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return Ok(0);
        }
        let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE sessions SET last_reconciled_at=?1 \
             WHERE host_alias=?2 AND tmux_name IN ({placeholders})"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&at, &host_alias];
        for n in keep_names {
            params.push(n);
        }
        self.conn.execute(&sql, params.as_slice())
    }

    /// Hard-delete one session row (ghost dismissal) together with what dies
    /// with it, in one transaction: its `session_events` timeline and the
    /// messages addressed TO it (an inbox nobody can read). Neither table has
    /// an FK cascade, and `sessions.id` has no AUTOINCREMENT, so leftovers
    /// would surface on the next session that reuses the id. Kept: messages it
    /// SENT (they live in the recipients' inboxes) and tasks it requested or
    /// worked — the task sweep fails a task whose worker row is gone.
    pub fn delete_session(&self, id: i64) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM session_events WHERE session_id=?1",
            rusqlite::params![id],
        )?;
        tx.execute(
            "DELETE FROM session_messages WHERE to_session_id=?1",
            rusqlite::params![id],
        )?;
        tx.execute("DELETE FROM sessions WHERE id=?1", rusqlite::params![id])?;
        tx.commit()?;
        self.bus.session_killed(id);
        Ok(())
    }

    /// Drop `session_events` rows whose session no longer exists. Deletes that
    /// predate `delete_session` reaping the timeline left such orphans behind;
    /// a reused session id would inherit them. Idempotent, runs on open.
    fn reap_orphan_session_events(&self) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM session_events \
             WHERE NOT EXISTS (SELECT 1 FROM sessions WHERE sessions.id = session_events.session_id)",
            [],
        )
    }

    pub fn delete_sessions_not_in(
        &self,
        host_alias: &str,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        // `DELETE ... RETURNING id` — delete and collect deleted ids in one
        // statement (no separate SELECT-then-DELETE).
        let ids_to_delete: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = self
                .conn
                .prepare_cached("DELETE FROM sessions WHERE host_alias=?1 RETURNING id")?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders}) RETURNING id"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        for id in &ids_to_delete {
            self.bus.session_killed(*id);
        }
        Ok(ids_to_delete.len())
    }

    /// Update `claude_status` for the session whose `claude_session_id` matches.
    /// No-ops silently when no row matches (hook arrived before reconcile enriched it).
    pub fn set_claude_status_by_session_id(
        &self,
        claude_session_id: &str,
        status: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        // Only the Stop hook calls this (a turn just completed), so the
        // write doubles as the `last_turn_at` stamp and maintains `idle_since`
        // for the GC sweeper — the hook handler lives in another track's
        // file, so the lifecycle bookkeeping is kept here in the store.
        let now = now_unix();
        let sql = format!(
            "UPDATE sessions SET claude_status = ?1, last_turn_at = ?3, idle_since = {idle} \
             WHERE claude_session_id = ?2",
            idle = idle_since_sql("?1", "?3"),
        );
        let changed = self
            .conn
            .execute(&sql, rusqlite::params![status, claude_session_id, now])
            .map_err(crate::ipc_error::IpcError::from)?;
        if changed > 0 {
            // Emit session_updated so the frontend patches the row in real-time.
            if let Ok(row) = self.fetch_session_by_claude_id(claude_session_id) {
                self.bus.session_updated(&row);
            }
        }
        Ok(())
    }

    // ── Orchestration (migration 020) ────────────────────────────────────

    /// The Stop hook's write: the turn is over. Sets `claude_status = idle`,
    /// bumps `turn_seq`, stamps `last_stop_at` / `last_turn_at` and maintains
    /// `idle_since`. Matches by `claude_session_id`; returns the updated row
    /// (`None` when no row carries this id yet). Emits `session_updated`.
    pub fn record_stop_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET claude_status = 'idle', turn_seq = turn_seq + 1, \
                 last_stop_at = ?2, last_turn_at = ?2, last_hook_at = ?2, \
                 idle_since = COALESCE(idle_since, ?2) \
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

    /// The UserPromptSubmit hook's write: a turn is starting. Sets
    /// `claude_status = working` and clears `idle_since` so "idle because
    /// never started" and "idle after a turn" are distinguishable from
    /// "busy". Returns the updated row (`None` when unmatched). Emits
    /// `session_updated`.
    pub fn record_prompt_submit_hook(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET claude_status = 'working', idle_since = NULL, \
                 last_hook_at = ?2 WHERE claude_session_id = ?1",
                rusqlite::params![claude_session_id, now_unix()],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        if changed == 0 {
            return Ok(None);
        }
        let row = self.fetch_session_by_claude_id(claude_session_id)?;
        self.bus.session_updated(&row);
        Ok(Some(row))
    }

    /// Replace a session's tags (migration 020). Emits `session_updated`.
    pub fn set_session_tags(
        &self,
        id: i64,
        tags: &[String],
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET tags=?1 WHERE id=?2",
            rusqlite::params![encode_tags(tags), id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Record which requester dispatched work to this session. Emits
    /// `session_updated`.
    pub fn set_parent_session_id(
        &self,
        id: i64,
        parent: Option<i64>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET parent_session_id=?1 WHERE id=?2",
            rusqlite::params![parent, id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Create a task in state `queued`. Returns the row. Emits `task_updated`.
    pub fn insert_task(
        &self,
        requester_session_id: Option<i64>,
        worker_session_id: Option<i64>,
        prompt: &str,
        nonce: &str,
    ) -> Result<TaskRow, crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "INSERT INTO tasks (requester_session_id, worker_session_id, prompt, state, \
                                    created_at, nonce) \
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5)",
                rusqlite::params![
                    requester_session_id,
                    worker_session_id,
                    prompt,
                    now_unix(),
                    nonce
                ],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let id = self.conn.last_insert_rowid();
        let row = self
            .fetch_task(id)?
            .ok_or_else(|| crate::ipc_error::IpcError::new("E_DB", "task vanished after insert"))?;
        self.bus.task_updated(&row);
        Ok(row)
    }

    pub fn get_task(&self, id: i64) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.fetch_task(id)
    }

    fn fetch_task(&self, id: i64) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                rusqlite::params![id],
                map_task_row,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
    }

    /// Tasks newest-first, optionally narrowed by requester and/or state,
    /// capped at `limit`.
    ///
    /// `host` scopes the result for a per-host caller IN SQL: only tasks
    /// whose requester or worker session lives on that host.
    pub fn list_tasks(
        &self,
        requester_session_id: Option<i64>,
        state: Option<&str>,
        host: Option<&str>,
        limit: i64,
    ) -> Result<Vec<TaskRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {cols} FROM tasks t \
                 LEFT JOIN sessions r ON r.id = t.requester_session_id \
                 LEFT JOIN sessions w ON w.id = t.worker_session_id \
                 WHERE (?1 IS NULL OR t.requester_session_id = ?1) \
                   AND (?2 IS NULL OR t.state = ?2) \
                   AND (?3 IS NULL OR r.host_alias = ?3 OR w.host_alias = ?3) \
                 ORDER BY t.created_at DESC, t.id DESC LIMIT ?4",
                cols = task_columns_t()
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(
                rusqlite::params![requester_session_id, state, host, limit],
                map_task_row,
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Every `queued` / `running` task (oldest first), for the liveness sweep.
    pub fn open_tasks(&self) -> Result<Vec<TaskRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE state IN ('queued','running') \
                 ORDER BY created_at ASC, id ASC"
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map([], map_task_row)
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Remember the worker's Claude conversation id for a task.
    pub fn set_task_worker_claude_id(
        &self,
        id: i64,
        claude_session_id: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE tasks SET worker_claude_session_id = ?1 WHERE id = ?2",
                rusqlite::params![claude_session_id, id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Store the transcript path a hook reported for this Claude session.
    /// The caller validates it (`service::hooks::valid_transcript_path`).
    pub fn set_transcript_path_by_claude_id(
        &self,
        claude_session_id: &str,
        path: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE sessions SET transcript_path = ?1 WHERE claude_session_id = ?2",
                rusqlite::params![path, claude_session_id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// The hook-reported transcript path of a session, if any.
    pub fn session_transcript_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT transcript_path FROM sessions WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// The `queued` / `running` tasks a worker session is executing (oldest
    /// first — the marker scan resolves them in dispatch order).
    pub fn open_tasks_for_worker(
        &self,
        worker_session_id: i64,
    ) -> Result<Vec<TaskRow>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks \
                 WHERE worker_session_id = ?1 AND state IN ('queued','running') \
                 ORDER BY created_at ASC, id ASC"
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![worker_session_id], map_task_row)
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Attach (or replace) the worker of a queued task.
    pub fn set_task_worker(
        &self,
        id: i64,
        worker_session_id: i64,
    ) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE tasks SET worker_session_id = ?1 WHERE id = ?2",
                rusqlite::params![worker_session_id, id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        self.emit_task(id)
    }

    /// `queued → running`; stamps `started_at`. A no-op (returns the current
    /// row) for any other state.
    pub fn mark_task_running(
        &self,
        id: i64,
    ) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        self.conn
            .execute(
                "UPDATE tasks SET state = 'running', started_at = ?1 \
                 WHERE id = ?2 AND state = 'queued'",
                rusqlite::params![now_unix(), id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        self.emit_task(id)
    }

    /// Move a task to a terminal state (`done` / `failed` / `cancelled`),
    /// storing `result` / `error` and stamping `finished_at`. Only an open
    /// (`queued` / `running`) task transitions — a terminal task is never
    /// rewritten, so a late marker scan cannot resurrect a cancelled task.
    /// Returns the row and whether THIS call performed the transition.
    pub fn finish_task(
        &self,
        id: i64,
        state: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<(Option<TaskRow>, bool), crate::ipc_error::IpcError> {
        if !TASK_TERMINAL_STATES.contains(&state) {
            return Err(crate::ipc_error::IpcError::new(
                "E_INVALID",
                format!("{state} is not a terminal task state"),
            ));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE tasks SET state = ?1, result = ?2, error = ?3, finished_at = ?4 \
                 WHERE id = ?5 AND state IN ('queued','running')",
                rusqlite::params![state, result, error, now_unix(), id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let row = if changed > 0 {
            self.emit_task(id)?
        } else {
            self.fetch_task(id)?
        };
        Ok((row, changed > 0))
    }

    fn emit_task(&self, id: i64) -> Result<Option<TaskRow>, crate::ipc_error::IpcError> {
        let row = self.fetch_task(id)?;
        if let Some(ref r) = row {
            self.bus.task_updated(r);
        }
        Ok(row)
    }

    /// Lookup helper: returns `None` rather than erroring when no row matches.
    /// Used by the safe-kill flow (Stop hook may arrive before reconcile
    /// enriched the row).
    pub fn get_session_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        match self.fetch_session_by_claude_id(claude_session_id) {
            Ok(row) => Ok(Some(row)),
            Err(e) => {
                // `query_row` returns this exact rusqlite error when zero rows
                // matched. Treat it as a clean miss.
                if e.message.contains("Query returned no rows") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn fetch_session_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<SessionRow, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SESSION_COLUMNS} FROM sessions WHERE claude_session_id = ?1"
            ))
            .map_err(crate::ipc_error::IpcError::from)?;
        stmt.query_row(rusqlite::params![claude_session_id], |row| {
            map_session_row(row)
        })
        .map_err(crate::ipc_error::IpcError::from)
    }

    /// Run `f` against this store inside one SQLite transaction: commit when
    /// `f` returns `Ok`, roll back (drop the transaction) when it returns
    /// `Err`. Unlike [`Store::with_transaction`] the closure receives the
    /// `&Store` itself, so it can compose the ordinary `&self` write helpers
    /// (`insert_message`, `insert_session_event`, …) atomically without
    /// `_in_tx` twins. Must not be nested, and `f` must not call a helper
    /// that opens its own transaction (`BEGIN` inside `BEGIN` errors). Bus
    /// emission is unaffected — none of the helpers this is meant for emit.
    pub fn atomically<F, R>(&self, f: F) -> Result<R, crate::ipc_error::IpcError>
    where
        F: FnOnce(&Store) -> Result<R, crate::ipc_error::IpcError>,
    {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(crate::ipc_error::IpcError::from)?;
        let r = f(self)?;
        tx.commit().map_err(crate::ipc_error::IpcError::from)?;
        Ok(r)
    }
}

// ---- Connection-level row fetch helpers ----
//
// Free functions (not methods) so they accept a bare `&Connection`. A
// `&Transaction` derefs to `&Connection`, so the same SQL serves both the
// autocommit `&self` helpers and the transactional `_in_tx` mutation paths
// without duplicating the row-mapping closures.

fn fetch_session(
    conn: &Connection,
    tmux_name: &str,
    host_alias: &str,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE tmux_name=?1 AND host_alias=?2"
    ))?;
    let mut rows = stmt.query_map(rusqlite::params![tmux_name, host_alias], |row| {
        map_session_row(row)
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Translate `HostReconcile::probe_started_at` into the `last_reconciled_at`
/// cutoff used by `ghost_and_clean_sessions_in_tx`: rows stamped at or after
/// the probe start are protected, and `0` ("no guard") protects nothing.
fn ghost_cutoff(probe_started_at: i64) -> i64 {
    if probe_started_at <= 0 {
        i64::MAX
    } else {
        probe_started_at
    }
}

fn fetch_session_by_id(conn: &Connection, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"
    ))?;
    let mut rows = stmt.query_map(rusqlite::params![id], map_session_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Map one `SELECT id, from_session_id, to_session_id, body, kind, sent_at,
/// read_at, reply_to` row of `session_messages`.
fn map_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMessage> {
    Ok(SessionMessage {
        id: row.get(0)?,
        from_session_id: row.get(1)?,
        to_session_id: row.get(2)?,
        body: row.get(3)?,
        kind: row.get(4)?,
        sent_at: row.get(5)?,
        read_at: row.get(6)?,
        reply_to: row.get(7)?,
    })
}

fn fetch_host(conn: &Connection, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                last_pinged_at, account_uuid, provisioned
         FROM hosts WHERE alias=?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![alias], |row| {
        Ok(HostRow {
            alias: row.get(0)?,
            ssh_alias: row.get(1)?,
            reachable: row.get::<_, i64>(2)? != 0,
            claude_version: row.get(3)?,
            tmux_version: row.get(4)?,
            hidden: row.get::<_, i64>(5)? != 0,
            last_pinged_at: row.get(6)?,
            account_uuid: row.get(7)?,
            provisioned: row.get::<_, i64>(8)? != 0,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn fetch_project(conn: &Connection, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, owner, repo, base_path, last_session_at FROM projects WHERE id=?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], |row| {
        Ok(ProjectRow {
            id: row.get(0)?,
            owner: row.get(1)?,
            repo: row.get(2)?,
            base_path: row.get(3)?,
            last_session_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_TABLES: &[&str] = &[
        "hosts",
        "projects",
        "worktrees",
        "sessions",
        "handoffs",
        "settings",
        "schema_version",
        "session_events",
        "session_messages",
        "host_tokens",
        "tasks",
    ];

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = Store::open_in_memory().expect("open");
        for t in EXPECTED_TABLES {
            assert!(store.has_table(t).expect("has_table"), "missing table: {t}");
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(
            store.schema_version().expect("version"),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrations_are_contiguous_from_one() {
        assert!(!MIGRATIONS.is_empty());
        for (i, (version, _)) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                *version,
                i as i64 + 1,
                "MIGRATIONS[{i}] has version {version}"
            );
        }
        assert_eq!(LATEST_SCHEMA_VERSION, MIGRATIONS.len() as i64);
    }

    #[test]
    fn every_migration_records_its_own_version() {
        for (version, sql) in MIGRATIONS {
            // Normalise whitespace so formatting differences between scripts
            // (line breaks, double spaces) do not matter.
            let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            let stamp =
                format!("INSERT OR IGNORE INTO schema_version (version) VALUES ({version});");
            assert!(
                flat.contains(&stamp),
                "migration {version} must contain `{stamp}`"
            );
            // …and must not stamp any *other* version by mistake.
            let stamps = flat
                .matches("INTO schema_version (version) VALUES (")
                .count();
            assert_eq!(stamps, 1, "migration {version} stamps {stamps} versions");
        }
    }

    #[test]
    fn second_migrate_on_fresh_db_is_a_noop() {
        let store = Store::open_in_memory().expect("open");
        let before = store.schema_version().expect("version");
        let count = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
                .unwrap()
        };
        let rows_before = count(&store);
        let tables_before = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        store.migrate().expect("second migrate");
        assert_eq!(store.schema_version().expect("version"), before);
        assert_eq!(count(&store), rows_before);
        let tables_after = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(tables_after, tables_before);
    }

    #[test]
    fn session_events_insert_then_list_newest_first_with_limit() {
        let s = Store::open_in_memory().expect("open");
        // Insert several events for session 7 (and a decoy for another session).
        s.insert_session_event(7, "status_change", Some("working"))
            .unwrap();
        s.insert_session_event(7, "prompt_sent", Some("hello"))
            .unwrap();
        s.insert_session_event(7, "stuck", Some("auth_menu"))
            .unwrap();
        s.insert_session_event(99, "killed", None).unwrap();

        // Newest-first (at DESC, id DESC): same-second inserts come back in
        // reverse insertion order.
        let all = s.list_session_events(7, 50).unwrap();
        assert_eq!(all.len(), 3, "decoy session 99 must be excluded");
        assert_eq!(all[0].kind, "stuck");
        assert_eq!(all[0].detail.as_deref(), Some("auth_menu"));
        assert_eq!(all[1].kind, "prompt_sent");
        assert_eq!(all[2].kind, "status_change");
        assert_eq!(all[2].session_id, 7);

        // Limit caps the result to the newest N.
        let limited = s.list_session_events(7, 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].kind, "stuck");
        assert_eq!(limited[1].kind, "prompt_sent");

        // NULL detail round-trips.
        let other = s.list_session_events(99, 50).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].detail, None);
    }

    #[test]
    fn insert_session_event_caps_timeline_per_session() {
        let s = Store::open_in_memory().expect("open");
        // Insert well past the cap for session 7, plus a decoy for session 8.
        for i in 0..(SESSION_EVENTS_CAP + 25) {
            s.insert_session_event(7, "status_change", Some(&format!("v{i}")))
                .unwrap();
        }
        s.insert_session_event(8, "killed", None).unwrap();

        let count: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, SESSION_EVENTS_CAP, "timeline capped per session");
        // Newest rows survive, oldest are pruned.
        let newest = s.list_session_events(7, 1).unwrap();
        assert_eq!(
            newest[0].detail.as_deref(),
            Some(&*format!("v{}", SESSION_EVENTS_CAP + 24))
        );
        // The other session's timeline is untouched.
        assert_eq!(s.list_session_events(8, 50).unwrap().len(), 1);
    }

    #[test]
    fn ghost_and_clean_bg_sessions_two_phase_with_event_cleanup() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store.upsert_host("beta").unwrap();
        bus.take();
        let id = store
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 100)
            .unwrap();
        store
            .insert_session_event(id, "status_change", None)
            .unwrap();
        // A bg row on ANOTHER host must never be touched.
        let other = store
            .upsert_bg_session("beta", "bg:u9", None, "u9", Some("working"), 100)
            .unwrap();
        bus.take();

        // Pass 1: agent vanished → row is ghosted (soft), not deleted.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 200)
            .unwrap();
        let row = store.get_session_by_id(id).unwrap().expect("still present");
        assert_eq!(row.status, "ghost");
        assert_eq!(row.lost_at, Some(200));
        assert!(bus.take().contains(&format!("session:updated:{id}")));

        // Pass 2: still vanished → hard-deleted, events reaped, kill emitted.
        store
            .ghost_and_clean_bg_sessions("alpha", &[], 300)
            .unwrap();
        assert!(store.get_session_by_id(id).unwrap().is_none());
        let orphans: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "events must not outlive the row");
        assert!(bus.take().contains(&format!("session:killed:{id}")));

        // The other host's bg row is untouched throughout.
        let other_row = store.get_session_by_id(other).unwrap().expect("beta row");
        assert_eq!(other_row.status, "running");
    }

    #[test]
    fn ghost_and_clean_bg_sessions_keeps_listed_agents_and_tmux_rows() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let kept = store
            .upsert_bg_session("alpha", "bg:live", None, "live", Some("working"), 100)
            .unwrap();
        // A normal tmux-backed row — ghosted or not, the bg pruner must skip it.
        store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 1,
                probe_started_at: 0,
                sessions: &[],
                keep: &[],
            })
            .unwrap(); // ghosts work-a

        let keep = vec!["bg:live".to_string()];
        store
            .ghost_and_clean_bg_sessions("alpha", &keep, 200)
            .unwrap();
        store
            .ghost_and_clean_bg_sessions("alpha", &keep, 300)
            .unwrap();

        let rows = store.list_sessions_for_host("alpha").unwrap();
        let live = rows.iter().find(|r| r.tmux_name == "bg:live").unwrap();
        assert_eq!(live.status, "running", "listed agent's row stays live");
        let work = rows.iter().find(|r| r.tmux_name == "work-a").unwrap();
        assert_eq!(
            work.status, "ghost",
            "tmux row is left for the tmux-keyed cleanup, not deleted here"
        );
        assert!(store.get_session_by_id(kept).unwrap().is_some());
    }

    #[test]
    fn upsert_bg_session_resurrects_ghosted_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        let id = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 100)
            .unwrap();
        s.ghost_and_clean_bg_sessions("alpha", &[], 200).unwrap();
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().status, "ghost");

        // Agent reappears (e.g. the previous probe transiently failed).
        let id2 = s
            .upsert_bg_session("alpha", "bg:u1", None, "u1", Some("working"), 300)
            .unwrap();
        assert_eq!(id2, id, "same row, not a new one");
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);
    }

    #[test]
    fn reconcile_hard_delete_reaps_session_events() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(id, "status_change", None)
            .unwrap();
        // Two empty reconciles: ghost, then hard-delete.
        for ts in [10, 20] {
            store
                .apply_host_reconcile(HostReconcile {
                    alias: "alpha",
                    reachable: true,
                    claude_version: None,
                    tmux_version: None,
                    last_pinged_at: ts,
                    probe_started_at: 0,
                    sessions: &[],
                    keep: &[],
                })
                .unwrap();
        }
        assert!(store.get_session_by_id(id).unwrap().is_none());
        let orphans: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "events must not outlive the hard-deleted row");
    }

    #[test]
    fn session_messages_inbox_roundtrip_and_mark_read() {
        let s = Store::open_in_memory().expect("open");
        // Two messages to session 5, one decoy to session 9.
        let m1 = s.insert_message(1, 5, "hello", "message", None).unwrap();
        let m2 = s.insert_message(2, 5, "second", "task", Some(m1)).unwrap();
        s.insert_message(1, 9, "noise", "message", None).unwrap();

        // list_inbox returns newest-first and excludes the decoy.
        let all = s.list_inbox(5, false, 50).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, m2);
        assert_eq!(all[0].body, "second");
        assert_eq!(all[0].kind, "task");
        assert_eq!(all[0].reply_to, Some(m1));
        assert_eq!(all[1].reply_to, None);
        assert_eq!(all[1].id, m1);
        assert!(all.iter().all(|m| m.read_at.is_none()));

        // unread_only filter and limit.
        let unread = s.list_inbox(5, true, 1).unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id, m2);

        // Mark only one as read; the other stays unread. A mismatched
        // recipient cannot mark someone else's mail.
        let updated = s.mark_messages_read(&[m1, m2], 5).unwrap();
        assert_eq!(updated, 2);
        let again = s.list_inbox(5, true, 50).unwrap();
        assert!(again.is_empty(), "all unread were marked");
        let foreign = s.mark_messages_read(&[m1], 9).unwrap();
        assert_eq!(foreign, 0, "wrong recipient cannot mark");
    }

    #[test]
    fn controller_set_get_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.get_controller().unwrap(), None, "unset is None");
        s.set_controller("mac", "dev-fleet").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
        // overwrite
        s.set_controller("mefistos", "ctrl").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mefistos".to_string(), "ctrl".to_string()))
        );
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().expect("open");
        let on: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma");
        assert_eq!(on, 1, "foreign_keys pragma should be ON");
    }

    #[test]
    fn upsert_and_list_projects_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .upsert_project("martin-janci", "claude-fleet", "/tmp/cf")
            .unwrap();
        assert!(id > 0);
        let id2 = s
            .upsert_project("martin-janci", "claude-fleet", "/other/path")
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_projects().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner, "martin-janci");
        assert_eq!(rows[0].repo, "claude-fleet");
        assert_eq!(rows[0].base_path, "/other/path");
    }

    #[test]
    fn worktrees_upsert_list_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        s.upsert_worktree(pid, "main", "/tmp/r", Some("main"))
            .unwrap();
        s.upsert_worktree(
            pid,
            "feature-x",
            "/tmp/r/.worktrees/feature-x",
            Some("feature-x"),
        )
        .unwrap();
        s.upsert_worktree(pid, "bugfix", "/tmp/r/.worktrees/bugfix", Some("bugfix"))
            .unwrap();
        assert_eq!(s.list_worktrees_for_project(pid).unwrap().len(), 3);
        let removed = s
            .delete_worktrees_not_in(
                pid,
                &["main".to_string(), "feature-x".to_string()],
                |p: &str| p.to_string(),
            )
            .unwrap();
        assert_eq!(removed, 1);
        let names: Vec<String> = s
            .list_worktrees_for_project(pid)
            .unwrap()
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, vec!["feature-x", "main"]);
    }

    #[test]
    fn delete_worktrees_not_in_repoints_sessions_to_the_same_checkout() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        // The old basename-named main row (logical spelling) and a worktree
        // that no longer exists, each referenced by a session.
        let old = s.upsert_worktree(pid, "r", "/tmp/link/r", None).unwrap();
        let gone = s
            .upsert_worktree(pid, "gone", "/tmp/r/.worktrees/gone", None)
            .unwrap();
        let on_old = s
            .upsert_session("dev", "local", Some(pid), Some(old), 1, 1, "running", None)
            .unwrap();
        let on_gone = s
            .upsert_session(
                "dev2",
                "local",
                Some(pid),
                Some(gone),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let main = s
            .upsert_worktree(pid, "main", "/tmp/r", Some("main"))
            .unwrap();
        // `/tmp/link/r` is another spelling of `/tmp/r`.
        let canon = |p: &str| p.replace("/tmp/link/", "/tmp/");
        let removed = s
            .delete_worktrees_not_in(pid, &["main".to_string()], canon)
            .expect("referenced rows must not fail the foreign key");
        assert_eq!(removed, 2);
        assert_eq!(
            s.get_session_by_id(on_old).unwrap().unwrap().worktree_id,
            Some(main),
            "moved to the surviving row of the same checkout"
        );
        assert_eq!(
            s.get_session_by_id(on_gone).unwrap().unwrap().worktree_id,
            None,
            "no survivor: the reference is cleared"
        );
    }

    #[test]
    fn delete_project_if_unused_counts_worktree_references_from_other_projects() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        // The duplicate-scan bug: a session of ANOTHER project points at a
        // worktree row of `dup`.
        let dup = s.upsert_project("o", "dup", "/tmp/dup").unwrap();
        let w = s.upsert_worktree(dup, "main", "/tmp/dup", None).unwrap();
        let real = s.upsert_project("o", "real", "/tmp/real").unwrap();
        s.upsert_session("dev", "local", Some(real), Some(w), 1, 1, "running", None)
            .unwrap();
        assert!(
            !s.delete_project_if_unused(dup).unwrap(),
            "still referenced"
        );
        assert!(s.get_worktree_row(w).unwrap().is_some());
        let free = s.upsert_project("o", "free", "/tmp/free").unwrap();
        s.upsert_worktree(free, "main", "/tmp/free", None).unwrap();
        assert!(s.delete_project_if_unused(free).unwrap());
    }

    #[test]
    fn upsert_and_list_sessions_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("dev-foo", "local", None, None, 1000, 2000, "running", None)
            .unwrap();
        assert!(id > 0);
        let id2 = s
            .upsert_session("dev-foo", "local", None, None, 1000, 3000, "running", None)
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_sessions_for_host("local").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_activity_at, 3000);
    }

    #[test]
    fn touch_project_last_session_at_takes_max() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        // First write
        s.touch_project_last_session_at(pid, 1000).unwrap();
        let rows = s.list_projects().unwrap();
        assert_eq!(rows[0].last_session_at, Some(1000));
        // Earlier timestamp shouldn't go backward
        s.touch_project_last_session_at(pid, 500).unwrap();
        let rows = s.list_projects().unwrap();
        assert_eq!(rows[0].last_session_at, Some(1000));
        // Later timestamp wins
        s.touch_project_last_session_at(pid, 2000).unwrap();
        let rows = s.list_projects().unwrap();
        assert_eq!(rows[0].last_session_at, Some(2000));
    }

    #[test]
    fn sessions_prune_removes_stale_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev-a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-b", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-c", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let removed = s
            .delete_sessions_not_in("local", &["dev-a".to_string()])
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(s.list_sessions_for_host("local").unwrap().len(), 1);
    }

    #[test]
    fn migration_002_adds_ssh_alias_column_to_hosts() {
        let s = Store::open_in_memory().expect("open");
        // sqlite_master pragma_table_info path
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "ssh_alias"),
            "expected `ssh_alias` column; got: {cols:?}"
        );
    }

    #[test]
    fn schema_version_is_latest_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn host_tokens_upsert_keeps_mode_and_rotates_token() {
        let s = Store::open_in_memory().expect("open");
        assert!(s.list_host_tokens().unwrap().is_empty());
        assert!(s.get_host_token("mefistos").unwrap().is_none());
        // A mode change on an unprovisioned host is a typed error.
        assert_eq!(
            s.set_host_token_mode("mefistos", "readonly")
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );

        s.upsert_host_token("mefistos", "tok-1").unwrap();
        let row = s.get_host_token("mefistos").unwrap().unwrap();
        assert_eq!(row.token, "tok-1");
        assert_eq!(row.mode, "full", "new rows default to full");

        s.set_host_token_mode("mefistos", "readonly").unwrap();
        // Rotating the token must not reset the operator's mode choice.
        s.upsert_host_token("mefistos", "tok-2").unwrap();
        let row = s.get_host_token("mefistos").unwrap().unwrap();
        assert_eq!(row.token, "tok-2");
        assert_eq!(row.mode, "readonly");

        s.upsert_host_token("local", "tok-3").unwrap();
        let all = s.list_host_tokens().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].host_alias, "local", "alias-ordered");

        s.delete_host_token("mefistos").unwrap();
        s.delete_host_token("mefistos").unwrap(); // idempotent
        assert_eq!(s.list_host_tokens().unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_reviewed_source_nulls_the_review_link_not_errors() {
        // Self-FK uses ON DELETE SET NULL: deleting a source session that a
        // review still points at must succeed (link nulls), not fail the FK.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let src = store
            .upsert_session("src", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rev = store
            .upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_session_kind(rev, "review", Some(src)).unwrap();
        // Delete the source while the review still references it.
        store
            .delete_session(src)
            .expect("delete source must not trip the self-FK");
        let row = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .find(|r| r.tmux_name == "src--review-1")
            .unwrap();
        assert_eq!(
            row.reviews_session_id, None,
            "link should be nulled by ON DELETE SET NULL"
        );
        assert_eq!(row.kind, "review", "the review row itself survives");
    }

    #[test]
    fn migration_004_adds_account_uuid_column_to_sessions() {
        let s = Store::open_in_memory().expect("open");
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('sessions')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on sessions; got: {cols:?}"
        );
    }

    #[test]
    fn migration_003_adds_accounts_table_and_host_account_uuid_column() {
        let s = Store::open_in_memory().expect("open");
        assert!(
            s.has_table("accounts").expect("has_table"),
            "expected accounts table"
        );
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on hosts; got: {cols:?}"
        );
    }

    #[test]
    fn list_hosts_orders_local_first_then_alpha() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.insert_host("zebra", Some("zebra")).unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let names: Vec<String> = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(names, vec!["local", "mefistos", "zebra"]);
    }

    #[test]
    fn insert_host_records_ssh_alias() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "mefistos")
            .unwrap();
        assert_eq!(row.ssh_alias.as_deref(), Some("mefistos"));
        assert!(!row.reachable);
        assert!(!row.hidden);
    }

    #[test]
    fn update_host_probe_persists_versions_and_reachability() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.update_host_probe("h", true, Some("2.1.144"), Some("3.6a"), 1000)
            .unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|x| x.alias == "h")
            .unwrap();
        assert!(row.reachable);
        assert_eq!(row.claude_version.as_deref(), Some("2.1.144"));
        assert_eq!(row.tmux_version.as_deref(), Some("3.6a"));
        assert_eq!(row.last_pinged_at, Some(1000));
    }

    #[test]
    fn delete_host_removes_host_and_its_sessions() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 1);
        s.delete_host("h").unwrap();
        assert_eq!(
            s.list_hosts()
                .unwrap()
                .iter()
                .filter(|x| x.alias == "h")
                .count(),
            0
        );
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 0);
    }

    #[test]
    fn delete_host_refuses_to_remove_local() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.delete_host("local").unwrap();
        assert!(s.list_hosts().unwrap().iter().any(|h| h.alias == "local"));
    }

    #[test]
    fn set_host_hidden_toggles() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_hidden("h", true).unwrap();
        assert!(
            s.list_hosts()
                .unwrap()
                .iter()
                .find(|x| x.alias == "h")
                .unwrap()
                .hidden
        );
        s.set_host_hidden("h", false).unwrap();
        assert!(
            !s.list_hosts()
                .unwrap()
                .iter()
                .find(|x| x.alias == "h")
                .unwrap()
                .hidden
        );
    }

    #[test]
    fn host_provisioned_defaults_false_and_round_trips() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        assert!(!s.get_host_row("local").unwrap().unwrap().provisioned);
        s.set_host_provisioned("local", true).unwrap();
        assert!(s.get_host_row("local").unwrap().unwrap().provisioned);
    }

    #[test]
    fn upsert_account_inserts_then_updates_keeping_uuid_pk() {
        let s = Store::open_in_memory().unwrap();
        let a = AccountRow {
            uuid: "uuid-1".into(),
            email: Some("a@b.com".into()),
            display_name: Some("A".into()),
            organization_name: None,
            organization_uuid: None,
            seat_tier: Some("max".into()),
            last_seen_at: Some(1000),
        };
        s.upsert_account(&a).unwrap();
        let mut a2 = a.clone();
        a2.email = Some("a@c.com".into());
        a2.last_seen_at = Some(2000);
        s.upsert_account(&a2).unwrap();
        let listed = s.list_accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].uuid, "uuid-1");
        assert_eq!(listed[0].email.as_deref(), Some("a@c.com"));
        assert_eq!(listed[0].last_seen_at, Some(2000));
    }

    #[test]
    fn list_accounts_orders_by_uuid_ascending() {
        let s = Store::open_in_memory().unwrap();
        for uuid in ["zzz", "aaa", "mmm"] {
            s.upsert_account(&AccountRow {
                uuid: uuid.into(),
                email: None,
                display_name: None,
                organization_name: None,
                organization_uuid: None,
                seat_tier: None,
                last_seen_at: None,
            })
            .unwrap();
        }
        let listed = s.list_accounts().unwrap();
        assert_eq!(
            listed.iter().map(|a| a.uuid.as_str()).collect::<Vec<_>>(),
            vec!["aaa", "mmm", "zzz"]
        );
    }

    #[test]
    fn get_account_by_uuid_returns_none_when_missing() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.get_account_by_uuid("nope").unwrap().is_none());
    }

    #[test]
    fn get_account_by_uuid_returns_some_when_present() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: Some("x@y.com".into()),
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        let got = s.get_account_by_uuid("u1").unwrap().unwrap();
        assert_eq!(got.email.as_deref(), Some("x@y.com"));
    }

    #[test]
    fn set_host_account_assigns_and_clears() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        s.set_host_account("h", Some("u1")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert_eq!(row.account_uuid.as_deref(), Some("u1"));
        s.set_host_account("h", None).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn list_hosts_includes_account_uuid_in_output() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn get_session_account_returns_none_for_missing_then_some_after_upsert() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // No session yet → None
        assert!(s.get_session_account("h", "dev-foo").unwrap().is_none());
        // Upsert with an account uuid
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        s.upsert_session("dev-foo", "h", None, None, 1, 1, "running", Some("u1"))
            .unwrap();
        assert_eq!(
            s.get_session_account("h", "dev-foo").unwrap().as_deref(),
            Some("u1")
        );
    }

    #[test]
    fn related_matches_same_project_and_worktree_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("main")).unwrap();
        let r = store.list_related_sessions(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tmux_name, "b");
    }

    #[test]
    fn related_excludes_different_worktree_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("feat-x")).unwrap();
        assert!(store.list_related_sessions(a).unwrap().is_empty());
    }

    #[test]
    fn related_matches_across_hosts_same_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        store.upsert_host("mefistos").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "mefistos", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("main")).unwrap();
        let r = store.list_related_sessions(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].host_alias, "mefistos");
    }

    #[test]
    fn related_returns_empty_for_null_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let _b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        assert!(store.list_related_sessions(a).unwrap().is_empty());
    }

    #[test]
    fn with_snapshot_returns_owned_data_for_off_lock_use() {
        let store = Store::open_in_memory().expect("in-memory store");
        store
            .insert_host("alpha", Some("alpha-ssh"))
            .expect("insert");
        let hosts = store.with_snapshot(|s| s.list_hosts().expect("list"));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "alpha");
    }

    #[test]
    fn with_transaction_commits_on_ok() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["foo", "foo-ssh"],
            )?;
            Ok(())
        });
        assert!(r.is_ok());
        let hosts = store.list_hosts().expect("list");
        assert!(hosts.iter().any(|h| h.alias == "foo"));
    }

    #[test]
    fn with_transaction_rolls_back_on_err() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["bar", "bar-ssh"],
            )?;
            // Trigger an error to force rollback.
            Err(rusqlite::Error::QueryReturnedNoRows)
        });
        assert!(r.is_err());
        let hosts = store.list_hosts().expect("list");
        assert!(
            !hosts.iter().any(|h| h.alias == "bar"),
            "rollback should have removed the bar row"
        );
    }

    #[test]
    fn list_projects_joined_groups_worktrees_by_project() {
        let s = Store::open_in_memory().expect("store");
        s.upsert_project("o1", "r1", "/p1").unwrap();
        s.upsert_project("o2", "r2", "/p2").unwrap();
        s.upsert_worktree(1, "main", "/p1", None).unwrap();
        s.upsert_worktree(1, "feature", "/p1/.worktrees/feature", Some("feature"))
            .unwrap();
        s.upsert_worktree(2, "main", "/p2", None).unwrap();
        let trees = s.list_projects_joined().expect("joined");
        assert_eq!(trees.len(), 2);
        let p1 = trees.iter().find(|t| t.project.repo == "r1").expect("p1");
        let p2 = trees.iter().find(|t| t.project.repo == "r2").expect("p2");
        assert_eq!(p1.worktrees.len(), 2);
        assert_eq!(p2.worktrees.len(), 1);
    }

    #[test]
    fn list_related_sessions_excludes_orphans() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s
            .upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let _b = s
            .upsert_session("dev-b", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert!(
            related.is_empty(),
            "orphans should not match each other; got: {:?}",
            related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn store_holds_event_bus_field_and_default_is_noop() {
        use crate::events::NoopEventBus;
        let store = Store::open_in_memory().expect("store");
        // Just constructing the store with the default Noop bus exercises the
        // new field. The bus is a private implementation detail; we don't expose
        // it as a public getter, so this test is intentionally minimal.
        let _ = std::sync::Arc::new(NoopEventBus); // also exercises Send+Sync
        let _ = store; // touch it to keep it alive past the new
    }

    fn store_with_recorder() -> (Store, Arc<crate::events::RecordingEventBus>) {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let store = Store::open_with_bus_in_memory(bus.clone()).expect("store");
        (store, bus)
    }

    /// One live `ReconcileSession` for the no-op / changed-row event tests.
    fn live_session(tmux_name: &'static str, pid: i64, activity: i64) -> ReconcileSession<'static> {
        ReconcileSession {
            tmux_name,
            project_id: Some(pid),
            created_at: 1,
            last_activity_at: activity,
            account_uuid: None,
            worktree_key: Some("main".to_string()),
            claude_session_id: None,
            claude_status: Some("idle".to_string()),
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct: Some(12.5),
            stuck_kind: None,
            intel_observed: true,
            ci_status: None,
            pr_observed: false,
        }
    }

    #[test]
    fn upsert_session_in_tx_identical_row_pushes_no_change() {
        // Direct, transaction-level check of the BE-11 diff: the same upsert
        // twice yields one `SessionCreated` and then nothing at all; a single
        // changed field yields exactly one `SessionUpdated`.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let upsert = |store: &mut Store, activity: i64| -> Vec<RowChange> {
            store
                .with_transaction(|tx| {
                    let mut out = Vec::new();
                    Store::upsert_session_in_tx(
                        tx,
                        "s1",
                        "alpha",
                        None,
                        None,
                        1,
                        activity,
                        None,
                        Some("main"),
                        None,
                        Some("idle"),
                        None,
                        None,
                        None,
                        Some(12.5),
                        None,
                        true,
                        None,
                        false,
                        0,
                        &mut out,
                    )?;
                    Ok(out)
                })
                .unwrap()
        };
        let first = upsert(&mut store, 10);
        assert_eq!(first.len(), 1);
        assert!(matches!(first[0], RowChange::SessionCreated(_)));
        let second = upsert(&mut store, 10);
        assert!(
            second.is_empty(),
            "identical row must push no change, got {} entries",
            second.len()
        );
        let third = upsert(&mut store, 11);
        assert_eq!(third.len(), 1);
        assert!(matches!(third[0], RowChange::SessionUpdated(_)));
    }

    #[test]
    fn apply_host_reconcile_identical_row_emits_no_session_or_project_event() {
        // BE-11 / FE-10: the tick upserts every live session every pass. A
        // pass that observes exactly what the store already holds must not
        // fan `session:updated` / `project:updated` out to the frontend.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let keep = vec!["s1".to_string()];

        // Pass 1: the row is new → created + project touched.
        let sessions = vec![live_session("s1", pid, 10)];
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 1,
                probe_started_at: 0,
                sessions: &sessions,
                keep: &keep,
            })
            .unwrap();
        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:created:")),
            "first pass creates; got {evts:?}"
        );
        assert!(
            evts.contains(&format!("project:updated:{pid}")),
            "first pass touches the project; got {evts:?}"
        );

        // Pass 2: identical observation → only the host probe stamp moves.
        let sessions = vec![live_session("s1", pid, 10)];
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 2,
                probe_started_at: 0,
                sessions: &sessions,
                keep: &keep,
            })
            .unwrap();
        assert_eq!(
            bus.take(),
            vec!["host:probed:alpha".to_string()],
            "an unchanged row must emit neither session nor project events"
        );

        // Pass 3: one field changed → exactly one session:updated and, since
        // last_session_at moves too, exactly one project:updated.
        let sessions = vec![live_session("s1", pid, 20)];
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 3,
                probe_started_at: 0,
                sessions: &sessions,
                keep: &keep,
            })
            .unwrap();
        let evts = bus.take();
        assert_eq!(
            evts.iter()
                .filter(|e| e.starts_with("session:updated:"))
                .count(),
            1,
            "changed row emits once; got {evts:?}"
        );
        assert_eq!(
            evts.iter()
                .filter(|e| e.starts_with("project:updated:"))
                .count(),
            1,
            "project touched once; got {evts:?}"
        );
    }

    #[test]
    fn stale_probe_does_not_ghost_row_reconciled_after_its_start() {
        // BE-3: a tick that listed tmux BEFORE `new_session` created a
        // session, but whose write lands AFTER the create's own reconcile
        // stamped the new row, carries a `keep` set without the new name.
        // The row must survive that write; a probe started after the stamp
        // ghosts it normally.
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let probe_started = 1_000;
        store
            .upsert_session("fresh", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        // Same second as the probe start: the guard must be inclusive.
        store
            .mark_sessions_reconciled("alpha", &["fresh".to_string()], probe_started)
            .unwrap();
        store
            .upsert_session("old", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .mark_sessions_reconciled("alpha", &["old".to_string()], probe_started - 1)
            .unwrap();
        // A row that was never stamped at all is also fair game.
        store
            .upsert_session("never", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take();

        let stale_write = |store: &mut Store, keep: &[String]| {
            store
                .apply_host_reconcile(HostReconcile {
                    alias: "alpha",
                    reachable: true,
                    claude_version: None,
                    tmux_version: None,
                    last_pinged_at: 5,
                    probe_started_at: probe_started,
                    sessions: &[],
                    keep,
                })
                .unwrap();
        };
        // Both keep shapes take different SQL paths; exercise each.
        stale_write(&mut store, &[]);
        let status =
            |store: &Store, name: &str| store.get_session(name, "alpha").unwrap().unwrap().status;
        assert_eq!(status(&store, "fresh"), "running", "newer row untouched");
        assert!(store
            .get_session("fresh", "alpha")
            .unwrap()
            .unwrap()
            .lost_at
            .is_none());
        assert_eq!(status(&store, "old"), "ghost", "older row still ghosted");
        assert_eq!(
            status(&store, "never"),
            "ghost",
            "unstamped row still ghosted"
        );
        let evts = bus.take();
        assert_eq!(
            evts.iter()
                .filter(|e| e.starts_with("session:updated:"))
                .count(),
            2,
            "exactly the two ghosted rows emit; got {evts:?}"
        );

        // Non-empty keep path: `fresh` is still absent from keep and still safe.
        stale_write(&mut store, &["unrelated".to_string()]);
        assert_eq!(status(&store, "fresh"), "running");

        // A probe that started after the stamp is authoritative again.
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 6,
                probe_started_at: probe_started + 1,
                sessions: &[],
                keep: &[],
            })
            .unwrap();
        assert_eq!(status(&store, "fresh"), "ghost", "later probe ghosts it");
    }

    #[test]
    fn upsert_session_emits_created_then_updated() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 100, 100, "running", None)
            .unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 100, 200, "running", None)
            .unwrap();
        let evts = bus.take();
        assert_eq!(
            evts.len(),
            2,
            "expected one created + one updated, got {evts:?}"
        );
        assert!(evts[0].starts_with("session:created:"), "got: {}", evts[0]);
        assert!(evts[1].starts_with("session:updated:"), "got: {}", evts[1]);
    }

    #[test]
    fn delete_session_emits_killed() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 100, 100, "running", None)
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().expect("row").id;
        bus.take(); // drain created event
        store.delete_session(id).unwrap();
        let evts = bus.take();
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0], format!("session:killed:{id}"));
    }

    #[test]
    fn delete_session_reaps_timeline_and_inbox_but_keeps_sent_messages_and_tasks() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let dead = store
            .upsert_session("dead", "alpha", None, None, 1, 1, "ghost", None)
            .unwrap();
        let peer = store
            .upsert_session("peer", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(dead, "status_change", Some("idle"))
            .unwrap();
        store
            .insert_session_event(peer, "status_change", Some("idle"))
            .unwrap();
        store
            .insert_message(peer, dead, "to the dead", "message", None)
            .unwrap();
        store
            .insert_message(dead, peer, "from the dead", "message", None)
            .unwrap();
        let task = store.insert_task(Some(peer), Some(dead), "p", "n").unwrap();

        store.delete_session(dead).unwrap();

        assert!(store.list_session_events(dead, 10).unwrap().is_empty());
        assert!(store.list_inbox(dead, false, 10).unwrap().is_empty());
        assert_eq!(store.list_session_events(peer, 10).unwrap().len(), 1);
        assert_eq!(
            store.list_inbox(peer, false, 10).unwrap().len(),
            1,
            "a message the dead session SENT stays in the recipient's inbox"
        );
        assert!(
            store.get_task(task.id).unwrap().is_some(),
            "the task sweep needs the task to fail it"
        );
    }

    #[test]
    fn open_reaps_orphaned_session_events_idempotently() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let live = store
            .upsert_session("live", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(live, "status_change", Some("idle"))
            .unwrap();
        // Left behind by a delete that predates the reap in delete_session.
        store
            .insert_session_event(live + 1000, "killed", None)
            .unwrap();

        store.migrate().unwrap();
        store.migrate().unwrap();

        assert_eq!(store.list_session_events(live, 10).unwrap().len(), 1);
        assert!(store
            .list_session_events(live + 1000, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn delete_sessions_not_in_emits_killed_per_row() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s2", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s3", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take(); // drain creates
        store
            .delete_sessions_not_in("alpha", &["s2".to_string()])
            .unwrap();
        let evts = bus.take();
        assert_eq!(evts.len(), 2, "expected 2 killed (s1, s3), got {evts:?}");
        assert!(evts.iter().all(|e| e.starts_with("session:killed:")));
    }

    #[test]
    fn delete_host_emits_session_killed_per_orphaned_session() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s2", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take(); // drain host:added + 2x session:created
        store.delete_host("alpha").unwrap();
        let evts = bus.take();
        // Expected order: 2x session:killed (one per orphan), then host:removed.
        assert_eq!(
            evts.len(),
            3,
            "expected 2 session:killed + 1 host:removed, got {evts:?}"
        );
        assert!(evts[0].starts_with("session:killed:"), "got: {}", evts[0]);
        assert!(evts[1].starts_with("session:killed:"), "got: {}", evts[1]);
        assert_eq!(evts[2], "host:removed:alpha");
    }

    #[test]
    fn restore_session_clears_ghost_status_and_lost_at() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        // Manually ghost it
        store
            .conn
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
                rusqlite::params![id],
            )
            .unwrap();
        bus.take(); // drain

        let row = store.restore_session(id).unwrap().expect("row must exist");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);

        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "restore must emit session:updated; got: {evts:?}"
        );
    }

    #[test]
    fn migration_005_adds_kind_and_reviews_columns_with_defaults() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rows = store.list_sessions_for_host("alpha").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "work");
        assert_eq!(rows[0].reviews_session_id, None);
    }

    #[test]
    fn migration_008_adds_lost_at_column() {
        let store = Store::open_in_memory().expect("store");
        let v: i64 = store
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 21, "schema_version should be 21 after migration");
        // Column exists and defaults to NULL
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let lost: Option<i64> = store
            .conn
            .query_row(
                "SELECT lost_at FROM sessions WHERE tmux_name='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lost, None, "lost_at should be NULL for a fresh session");
    }

    #[test]
    fn mark_sessions_reconciled_stamps_only_kept_rows() {
        // Task H freshness marker: the background tick stamps last_reconciled_at
        // on every session it observed live (the keep set) and leaves the rest
        // (and a fresh row's default) NULL.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("kept", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("gone", "alpha", None, None, 1, 1, "running", None)
            .unwrap();

        let updated = store
            .mark_sessions_reconciled("alpha", &["kept".to_string()], 1234)
            .expect("mark");
        assert_eq!(updated, 1, "only the kept session is stamped");

        let kept_at: Option<i64> = store
            .conn
            .query_row(
                "SELECT last_reconciled_at FROM sessions WHERE tmux_name='kept'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept_at, Some(1234));

        let gone_at: Option<i64> = store
            .conn
            .query_row(
                "SELECT last_reconciled_at FROM sessions WHERE tmux_name='gone'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone_at, None, "non-kept session keeps NULL");

        // Empty keep set is a no-op (no rows touched, no error).
        let none = store
            .mark_sessions_reconciled("alpha", &[], 9999)
            .expect("empty keep");
        assert_eq!(none, 0);
    }

    #[test]
    fn set_session_kind_marks_review_and_survives_reupsert() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let src = store
            .upsert_session("src", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rev = store
            .upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_session_kind(rev, "review", Some(src)).unwrap();
        store
            .upsert_session("src--review-1", "alpha", None, None, 1, 2, "running", None)
            .unwrap();
        let row = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .find(|r| r.tmux_name == "src--review-1")
            .unwrap();
        assert_eq!(row.kind, "review", "kind must survive re-upsert");
        assert_eq!(row.reviews_session_id, Some(src));
    }

    #[test]
    fn reconcile_clears_stuck_kind_only_when_pane_observed() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();

        let pass = |store: &mut Store, stuck: Option<&str>, observed: bool| {
            let sessions = vec![ReconcileSession {
                tmux_name: "s1",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: stuck.map(|s| s.to_string()),
                intel_observed: observed,
                ci_status: None,
                pr_observed: false,
            }];
            store
                .apply_host_reconcile(HostReconcile {
                    alias: "alpha",
                    reachable: true,
                    claude_version: None,
                    tmux_version: None,
                    last_pinged_at: 1,
                    probe_started_at: 0,
                    sessions: &sessions,
                    keep: &["s1".to_string()],
                })
                .unwrap();
        };
        let stuck_of = |store: &Store| {
            store
                .get_session("s1", "alpha")
                .unwrap()
                .unwrap()
                .stuck_kind
        };

        // Observed pane, stuck detected → flag stored.
        pass(&mut store, Some("reconnect"), true);
        assert_eq!(stuck_of(&store).as_deref(), Some("reconnect"));

        // Capture FAILED (pane not observed), no stuck → prior flag preserved.
        pass(&mut store, None, false);
        assert_eq!(
            stuck_of(&store).as_deref(),
            Some("reconnect"),
            "must preserve stuck_kind when the pane was not observed"
        );

        // Observed pane, stuck no longer present → flag CLEARED.
        pass(&mut store, None, true);
        assert_eq!(
            stuck_of(&store),
            None,
            "must clear stuck_kind when the pane was observed and shows no stuck state"
        );
    }

    #[test]
    fn apply_host_reconcile_happy_path_persists_all_and_emits_after_commit() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        // Pre-seed a stale row that should be pruned (kill), and one that
        // already exists so it produces an `updated` (not `created`).
        store
            .upsert_session("stale", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("keep-existing", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let stale_id = store.get_session("stale", "alpha").unwrap().unwrap().id;
        bus.take(); // drain all setup events

        let sessions = vec![
            // existing → update
            ReconcileSession {
                tmux_name: "keep-existing",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 50,
                account_uuid: None,
                worktree_key: Some("main".to_string()),
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
                ci_status: None,
                pr_observed: false,
            },
            // brand new → create
            ReconcileSession {
                tmux_name: "fresh",
                project_id: Some(pid),
                created_at: 10,
                last_activity_at: 60,
                account_uuid: None,
                worktree_key: Some("main".to_string()),
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
                ci_status: None,
                pr_observed: false,
            },
        ];
        let keep = vec!["keep-existing".to_string(), "fresh".to_string()];
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: Some("2.1"),
                tmux_version: Some("3.6"),
                last_pinged_at: 999,
                probe_started_at: 0,
                sessions: &sessions,
                keep: &keep,
            })
            .expect("reconcile ok");

        // (a) rows persisted: stale ghosted, two live, host probe updated.
        let live: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .filter(|r| r.status != "ghost")
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(live, vec!["fresh", "keep-existing"], "two live sessions");
        let ghosts: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .filter(|r| r.status == "ghost")
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(ghosts, vec!["stale"], "stale is now ghost");
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "alpha")
            .unwrap();
        assert_eq!(host.claude_version.as_deref(), Some("2.1"));
        assert_eq!(host.last_pinged_at, Some(999));
        assert_eq!(store.list_projects().unwrap()[0].last_session_at, Some(60));

        // (b) the events fired — and only after commit (we drained pre-batch, so
        // everything here was emitted by the flush phase).
        let evts = bus.take();
        assert!(
            evts.contains(&"host:probed:alpha".to_string()),
            "got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "expected an update for keep-existing; got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("session:created:")),
            "expected a create for fresh; got: {evts:?}"
        );
        assert!(
            evts.contains(&format!("session:updated:{stale_id}")),
            "stale becomes ghost (session:updated); got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("project:updated:")),
            "expected project:updated; got: {evts:?}"
        );
    }

    #[test]
    fn bg_session_survives_reconcile_with_empty_tmux() {
        // A `kind='bg'` row is never a tmux session, so it never appears in the
        // `keep` set. Ghost cleanup must NOT reap it — even when the host's tmux
        // list is empty and a normal (work) row gets ghosted.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        // A bg row + a normal work row.
        store
            .upsert_bg_session(
                "alpha",
                "bg:sess-uuid-1",
                None,
                "sess-uuid-1",
                Some("working"),
                100,
            )
            .unwrap();
        store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();

        // Reconcile with NO live tmux sessions (empty keep).
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 1,
                probe_started_at: 0,
                sessions: &[],
                keep: &[],
            })
            .expect("reconcile ok");

        let rows = store.list_sessions_for_host("alpha").unwrap();
        let bg = rows
            .iter()
            .find(|r| r.tmux_name == "bg:sess-uuid-1")
            .expect("bg row must survive");
        assert_eq!(bg.kind, "bg");
        assert_eq!(bg.status, "running", "bg row must NOT be ghosted");
        assert_eq!(bg.claude_session_id.as_deref(), Some("sess-uuid-1"));
        // The plain work row, in contrast, gets ghosted.
        let work = rows.iter().find(|r| r.tmux_name == "work-a").unwrap();
        assert_eq!(work.status, "ghost", "work row IS ghosted when not in tmux");

        // A SECOND reconcile (the bg row is now an old row) still doesn't reap
        // it via the Phase-2 hard-delete.
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 2,
                probe_started_at: 0,
                sessions: &[],
                keep: &[],
            })
            .expect("reconcile ok");
        assert!(
            store
                .list_sessions_for_host("alpha")
                .unwrap()
                .iter()
                .any(|r| r.tmux_name == "bg:sess-uuid-1"),
            "bg row must survive repeated reconciles"
        );
    }

    #[test]
    fn reconcile_batch_rolls_back_and_emits_nothing_on_error() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added

        let row_count = |s: &Store| -> i64 {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sessions WHERE host_alias='alpha'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(row_count(&store), 0);

        // First a good upsert, then one whose project_id points at a
        // non-existent project — with foreign_keys=ON this trips the
        // sessions.project_id FK mid-batch and aborts the transaction.
        let sessions = vec![
            ReconcileSession {
                tmux_name: "good",
                project_id: None,
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
                ci_status: None,
                pr_observed: false,
            },
            ReconcileSession {
                tmux_name: "bad",
                project_id: Some(999_999), // no such project → FK violation
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
                ci_status: None,
                pr_observed: false,
            },
        ];
        let keep = vec!["good".to_string(), "bad".to_string()];
        let res = store.apply_host_reconcile(HostReconcile {
            alias: "alpha",
            reachable: true,
            claude_version: Some("9.9"),
            tmux_version: None,
            last_pinged_at: 12345,
            probe_started_at: 0,
            sessions: &sessions,
            keep: &keep,
        });

        assert!(res.is_err(), "FK violation should abort the batch");
        // (a) NO rows persisted — not even the 'good' one before the failure.
        assert_eq!(
            row_count(&store),
            0,
            "transaction must have rolled back all writes"
        );
        // host probe row must also be untouched (it was part of the same tx).
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "alpha")
            .unwrap();
        assert_ne!(
            host.claude_version.as_deref(),
            Some("9.9"),
            "host probe rolled back"
        );
        assert_eq!(host.last_pinged_at, None, "host probe rolled back");
        // (b) NO events emitted — the flush phase never runs on rollback.
        assert!(
            bus.take().is_empty(),
            "no event may fire for a rolled-back batch"
        );
    }

    #[test]
    fn reconcile_ghosts_sessions_on_first_empty_probe_then_deletes_on_second() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 10, "running", None)
            .unwrap();
        let s1_id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        bus.take(); // drain setup events

        // First reachable probe with no sessions — s1 should become ghost
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 100,
                probe_started_at: 0,
                sessions: &[],
                keep: &[],
            })
            .unwrap();

        let s1 = store.get_session_by_id(s1_id).unwrap().unwrap();
        assert_eq!(s1.status, "ghost", "first empty probe should ghost s1");
        assert!(s1.lost_at.is_some(), "lost_at must be set");

        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "ghost transition should emit session:updated; got: {evts:?}"
        );
        assert!(
            !evts.iter().any(|e| e.starts_with("session:killed:")),
            "no kill event on first cycle; got: {evts:?}"
        );

        // Second reachable probe with no sessions — ghost s1 should be deleted
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 200,
                probe_started_at: 0,
                sessions: &[],
                keep: &[],
            })
            .unwrap();

        assert!(
            store.get_session_by_id(s1_id).unwrap().is_none(),
            "second empty probe should hard-delete the ghost"
        );
        let evts2 = bus.take();
        assert!(
            evts2.contains(&format!("session:killed:{s1_id}")),
            "second cycle must emit session:killed; got: {evts2:?}"
        );
    }

    #[test]
    fn claude_session_id_round_trips_and_defaults_none() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().claude_session_id,
            None
        );
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn upsert_session_preserves_claude_session_id() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        s.upsert_session("dev", "local", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn upsert_session_preserves_friendly_name_on_conflict() {
        // Regression guard for the "deterministic backup, agent refines"
        // design: once a friendly_name is set, a subsequent reconcile-driven
        // upsert must NOT NULL it back out.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_friendly_name("local", "dev", Some("Friendly name"))
            .unwrap();
        s.upsert_session("dev", "local", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session("dev", "local")
                .unwrap()
                .unwrap()
                .friendly_name
                .as_deref(),
            Some("Friendly name")
        );
    }

    #[test]
    fn backfill_friendly_names_humanises_null_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        // Project + worktree so the JOIN finds owner/repo and a branch.
        let project_id = s
            .upsert_project("martin-janci", "claude-fleet", "/p")
            .unwrap();
        let wt_id = s
            .upsert_worktree(
                project_id,
                "friendly-name",
                "/p/.claude/worktrees/friendly-name",
                Some("friendly-name"),
            )
            .unwrap();
        s.upsert_session(
            "dev-martin-janci-claude-fleet--friendly-name",
            "local",
            Some(project_id),
            Some(wt_id),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        // A row that already has a label must NOT be touched.
        s.upsert_session(
            "dev-other",
            "local",
            Some(project_id),
            None,
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        s.set_friendly_name("local", "dev-other", Some("Already set"))
            .unwrap();

        let updated = s.backfill_friendly_names().unwrap();
        assert_eq!(updated, 1);
        assert_eq!(
            s.get_session("dev-martin-janci-claude-fleet--friendly-name", "local")
                .unwrap()
                .unwrap()
                .friendly_name
                .as_deref(),
            Some("Friendly name")
        );
        assert_eq!(
            s.get_session("dev-other", "local")
                .unwrap()
                .unwrap()
                .friendly_name
                .as_deref(),
            Some("Already set")
        );
    }

    // ── migration 020: orchestration fields, tasks, reply_to ──

    #[test]
    fn migration_020_adds_orchestration_columns_with_defaults() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.turn_seq, 0);
        assert_eq!(row.last_stop_at, None);
        assert_eq!(row.parent_session_id, None);
        assert!(row.tags.is_empty());
        assert!(s.has_table("tasks").unwrap());
    }

    #[test]
    fn tags_round_trip_as_a_json_array_and_null_when_empty() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s
            .set_session_tags(id, &["review".to_string(), "wip".to_string()])
            .unwrap()
            .unwrap();
        assert_eq!(row.tags, vec!["review".to_string(), "wip".to_string()]);
        let raw: Option<String> = s
            .conn
            .query_row("SELECT tags FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(raw.as_deref(), Some("[\"review\",\"wip\"]"));
        let row = s.set_session_tags(id, &[]).unwrap().unwrap();
        assert!(row.tags.is_empty());
        let raw: Option<String> = s
            .conn
            .query_row("SELECT tags FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, None, "an empty list is stored as NULL");
        // A hand-edited / malformed column reads as no tags rather than failing.
        assert!(decode_tags(Some("not json".into())).is_empty());
        assert!(decode_tags(Some("".into())).is_empty());
        assert_eq!(decode_tags(Some("[\"a\"]".into())), vec!["a".to_string()]);
        assert_eq!(encode_tags(&[]), None);
        // A reconcile pass does not touch tags / parent / turn_seq.
        s.set_session_tags(id, &["keep".to_string()]).unwrap();
        s.set_parent_session_id(id, Some(7)).unwrap();
        s.set_claude_session_id(id, "uuid-1").unwrap();
        s.record_stop_hook("uuid-1").unwrap();
        let mut s = s;
        let row = reconcile_one(&mut s, "sess", Some("idle"), None, None);
        assert_eq!(row.tags, vec!["keep".to_string()]);
        assert_eq!(row.parent_session_id, Some(7));
        assert_eq!(row.turn_seq, 1);
    }

    #[test]
    fn stop_hook_write_bumps_turn_seq_and_prompt_submit_marks_working() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        // Unmatched id: None, nothing changes.
        assert!(s.record_stop_hook("nope").unwrap().is_none());
        assert!(s.record_prompt_submit_hook("nope").unwrap().is_none());
        s.set_claude_session_id(id, "uuid-1").unwrap();
        let row = s.record_stop_hook("uuid-1").unwrap().unwrap();
        assert_eq!(row.turn_seq, 1);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        let stop = row.last_stop_at.expect("stamped");
        assert_eq!(row.last_turn_at, Some(stop));
        assert!(row.idle_since.is_some());
        let row = s.record_prompt_submit_hook("uuid-1").unwrap().unwrap();
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        assert_eq!(row.turn_seq, 1, "a submit is not a turn");
        assert_eq!(row.last_stop_at, Some(stop), "a submit keeps the last stop");
        let row = s.record_stop_hook("uuid-1").unwrap().unwrap();
        assert_eq!(row.turn_seq, 2);
    }

    #[test]
    fn reconcile_does_not_clobber_a_hook_stamped_idle_newer_than_the_pass() {
        // MCP-1: the reconcile pass captured the pane at T0; the Stop hook
        // stamped idle at T1 >= T0; the pass's write lands at T2 with the
        // stale "working" it derived from the pane. The hook must win.
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-a").unwrap();
        let stop_at = s
            .record_stop_hook("uuid-a")
            .unwrap()
            .unwrap()
            .last_stop_at
            .unwrap();
        let write = |s: &mut Store, status: &str, probe_started_at: i64| -> SessionRow {
            s.apply_host_reconcile(HostReconcile {
                alias: "local",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 1,
                probe_started_at,
                sessions: &[ReconcileSession {
                    tmux_name: "a",
                    project_id: None,
                    created_at: 1,
                    last_activity_at: 1,
                    account_uuid: None,
                    worktree_key: None,
                    claude_session_id: None,
                    claude_status: Some(status.to_string()),
                    effort_level: None,
                    pr_url: None,
                    current_activity: None,
                    context_pct: None,
                    stuck_kind: None,
                    intel_observed: true,
                    ci_status: None,
                    pr_observed: false,
                }],
                keep: &["a".to_string()],
            })
            .unwrap();
            s.get_session("a", "local").unwrap().unwrap()
        };
        // Pass started before (or at) the hook stamp: hook wins, idle_since kept.
        let row = write(&mut s, "working", stop_at - 5);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        assert!(row.idle_since.is_some());
        let row = write(&mut s, "working", stop_at);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
        // Pass started after the hook stamp: the pane observation is fresher.
        let row = write(&mut s, "working", stop_at + 5);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        // Guard disabled (0): legacy COALESCE behaviour.
        s.record_stop_hook("uuid-a").unwrap();
        let row = write(&mut s, "working", 0);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
    }

    #[test]
    fn tasks_crud_and_terminal_transitions() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .insert_task(Some(1), Some(2), "do it", "abcd1234")
            .unwrap();
        assert_eq!((t.state.as_str(), t.nonce.as_str()), ("queued", "abcd1234"));
        assert_eq!(s.get_task(t.id).unwrap().unwrap(), t);
        assert!(s.get_task(999).unwrap().is_none());
        let t2 = s.insert_task(None, None, "later", "ffff0000").unwrap();
        let t2 = s.set_task_worker(t2.id, 2).unwrap().unwrap();
        assert_eq!(t2.worker_session_id, Some(2));
        // Listing: newest first, filters.
        let all = s.list_tasks(None, None, None, 50).unwrap();
        assert_eq!(
            all.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![t2.id, t.id]
        );
        assert_eq!(s.list_tasks(Some(1), None, None, 50).unwrap().len(), 1);
        assert_eq!(
            s.list_tasks(None, Some("queued"), None, 1).unwrap().len(),
            1
        );
        assert_eq!(s.open_tasks_for_worker(2).unwrap().len(), 2);
        // running stamps started_at once.
        let r = s.mark_task_running(t.id).unwrap().unwrap();
        assert!(r.started_at.is_some());
        assert_eq!(s.mark_task_running(t.id).unwrap().unwrap(), r);
        // A non-terminal state is refused; a terminal one flips once.
        assert_eq!(
            s.finish_task(t.id, "running", None, None).unwrap_err().code,
            "E_INVALID"
        );
        let (row, changed) = s.finish_task(t.id, "done", Some("ok"), None).unwrap();
        assert!(changed);
        let row = row.unwrap();
        assert_eq!(
            (row.state.as_str(), row.result.as_deref()),
            ("done", Some("ok"))
        );
        assert!(row.finished_at.is_some());
        let (row, changed) = s
            .finish_task(t.id, "cancelled", None, Some("late"))
            .unwrap();
        assert!(!changed, "terminal tasks are never rewritten");
        assert_eq!(row.unwrap().state, "done");
        assert_eq!(s.open_tasks_for_worker(2).unwrap().len(), 1);
        // The nonce never serialises to the wire.
        let json = serde_json::to_value(&t).unwrap();
        assert!(json.get("nonce").is_none());
        assert_eq!(json["state"], "queued");
    }

    #[test]
    fn reconcile_preserves_a_submit_stamped_working_status() {
        // S3: UserPromptSubmit stamped `working` at T1; a pass that started at
        // T0 <= T1 derived `idle` from a pane captured before the submit.
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-a").unwrap();
        s.record_prompt_submit_hook("uuid-a").unwrap();
        let hook_at: i64 = s
            .conn
            .query_row("SELECT last_hook_at FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let pass = |s: &mut Store, started: i64| {
            s.apply_host_reconcile(HostReconcile {
                alias: "local",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 1,
                probe_started_at: started,
                sessions: &[ReconcileSession {
                    tmux_name: "a",
                    project_id: None,
                    created_at: 1,
                    last_activity_at: 1,
                    account_uuid: None,
                    worktree_key: None,
                    claude_session_id: None,
                    claude_status: Some("idle".into()),
                    effort_level: None,
                    pr_url: None,
                    current_activity: None,
                    context_pct: None,
                    stuck_kind: None,
                    intel_observed: true,
                    ci_status: None,
                    pr_observed: false,
                }],
                keep: &["a".to_string()],
            })
            .unwrap();
            s.get_session("a", "local").unwrap().unwrap()
        };
        let row = pass(&mut s, hook_at - 3);
        assert_eq!(row.claude_status.as_deref(), Some("working"));
        assert_eq!(row.idle_since, None);
        // A pass that started after the submit is authoritative.
        let row = pass(&mut s, hook_at + 3);
        assert_eq!(row.claude_status.as_deref(), Some("idle"));
    }

    #[test]
    fn list_tasks_scopes_by_host_in_sql() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("a").unwrap();
        s.upsert_host("b").unwrap();
        let ra = s
            .upsert_session("r", "a", None, None, 1, 1, "running", None)
            .unwrap();
        let wb = s
            .upsert_session("w", "b", None, None, 1, 1, "running", None)
            .unwrap();
        let rb = s
            .upsert_session("r2", "b", None, None, 1, 1, "running", None)
            .unwrap();
        let t1 = s.insert_task(Some(ra), Some(wb), "x", "n1").unwrap();
        let t2 = s.insert_task(Some(rb), Some(wb), "y", "n2").unwrap();
        let _orphan = s.insert_task(None, None, "z", "n3").unwrap();
        let ids = |h: Option<&str>| -> Vec<i64> {
            let mut v: Vec<i64> = s
                .list_tasks(None, None, h, 50)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect();
            v.sort();
            v
        };
        assert_eq!(ids(Some("a")), vec![t1.id]);
        assert_eq!(ids(Some("b")), vec![t1.id, t2.id]);
        assert_eq!(ids(None).len(), 3);
        assert!(ids(Some("c")).is_empty());
    }

    #[test]
    fn transcript_path_and_task_worker_claude_id_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.session_transcript_path(id).unwrap(), None);
        s.set_claude_session_id(id, "uuid-a").unwrap();
        s.set_transcript_path_by_claude_id("uuid-a", "/h/.claude/projects/x/uuid-a.jsonl")
            .unwrap();
        assert_eq!(
            s.session_transcript_path(id).unwrap().as_deref(),
            Some("/h/.claude/projects/x/uuid-a.jsonl")
        );
        let t = s.insert_task(None, Some(id), "p", "n").unwrap();
        assert_eq!(t.worker_claude_session_id, None);
        s.set_task_worker_claude_id(t.id, "uuid-a").unwrap();
        let t = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(t.worker_claude_session_id.as_deref(), Some("uuid-a"));
        assert_eq!(s.open_tasks().unwrap().len(), 1);
        assert!(serde_json::to_value(&t)
            .unwrap()
            .get("worker_claude_session_id")
            .is_none());
    }

    #[test]
    fn task_writes_emit_task_updated_events() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        let t = s.insert_task(None, Some(1), "x", "n").unwrap();
        s.mark_task_running(t.id).unwrap();
        s.finish_task(t.id, "failed", None, Some("boom")).unwrap();
        s.finish_task(t.id, "done", None, None).unwrap();
        assert_eq!(
            bus.take(),
            vec![
                format!("task:updated:{}:queued", t.id),
                format!("task:updated:{}:running", t.id),
                format!("task:updated:{}:failed", t.id),
            ],
            "no event for the refused rewrite of a terminal task"
        );
    }

    #[test]
    fn messages_carry_reply_to() {
        let s = Store::open_in_memory().unwrap();
        let m1 = s.insert_message(1, 5, "q", "message", None).unwrap();
        let m2 = s.insert_message(5, 1, "a", "reply", Some(m1)).unwrap();
        assert_eq!(s.get_message(m2).unwrap().unwrap().reply_to, Some(m1));
        assert_eq!(s.get_message(m1).unwrap().unwrap().reply_to, None);
        assert!(s.get_message(999).unwrap().is_none());
        assert_eq!(s.list_inbox(1, false, 10).unwrap()[0].reply_to, Some(m1));
    }

    // ── migration 019: lifecycle + outcome fields ──

    #[test]
    fn migration_019_adds_lifecycle_columns_defaulting_to_null() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.idle_since, None);
        assert_eq!(row.stuck_since, None);
        assert_eq!(row.last_playbook_at, None);
        assert_eq!(row.last_prompt, None);
        assert_eq!(row.started_at, None);
        assert_eq!(row.last_turn_at, None);
        assert_eq!(row.ci_status, None);
    }

    fn reconcile_one(
        s: &mut Store,
        name: &'static str,
        status: Option<&str>,
        stuck: Option<&str>,
        pr: Option<(Option<&str>, Option<&str>)>,
    ) -> SessionRow {
        let (pr_url, ci_status, pr_observed) = match pr {
            Some((u, c)) => (u.map(String::from), c.map(String::from), true),
            None => (None, None, false),
        };
        s.apply_host_reconcile(HostReconcile {
            alias: "local",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 1,
            probe_started_at: 0,
            sessions: &[ReconcileSession {
                tmux_name: name,
                project_id: None,
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: status.map(String::from),
                effort_level: None,
                pr_url,
                current_activity: None,
                context_pct: None,
                stuck_kind: stuck.map(String::from),
                intel_observed: true,
                ci_status,
                pr_observed,
            }],
            keep: &[name.to_string()],
        })
        .unwrap();
        s.get_session(name, "local").unwrap().unwrap()
    }

    #[test]
    fn reconcile_stamps_idle_since_on_entering_idle_and_clears_on_leaving() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        assert_eq!(r.idle_since, None);
        let r = reconcile_one(&mut s, "a", Some("idle"), None, None);
        let stamp = r.idle_since.expect("stamped on entering idle");
        assert!(stamp > 0);
        // Staying idle keeps the ORIGINAL stamp (the GC TTL counts from it).
        let r = reconcile_one(&mut s, "a", Some("completed"), None, None);
        assert_eq!(r.idle_since, Some(stamp));
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        assert_eq!(r.idle_since, None);
        // A fresh row inserted already idle is stamped on insert.
        let r = reconcile_one(&mut s, "b", Some("stopped"), None, None);
        assert!(r.idle_since.is_some());
    }

    #[test]
    fn reconcile_tracks_stuck_since_per_episode() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("blocked"), None, None);
        assert_eq!(r.stuck_since, None);
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        let start = r.stuck_since.expect("episode start stamped");
        // Same kind on the next pass: the episode start is preserved.
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        assert_eq!(r.stuck_since, Some(start));
        // The flag clearing (pane observed, no stuck) clears the stamp …
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        assert_eq!(r.stuck_kind, None);
        assert_eq!(r.stuck_since, None);
        // … and a different kind later starts a new episode.
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("oom"), None);
        assert!(r.stuck_since.is_some());
    }

    #[test]
    fn reconcile_pr_fields_are_authoritative_only_when_probed() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(
            &mut s,
            "a",
            None,
            None,
            Some((Some("https://github.com/o/r/pull/1"), Some("pending"))),
        );
        assert_eq!(r.pr_url.as_deref(), Some("https://github.com/o/r/pull/1"));
        assert_eq!(r.ci_status.as_deref(), Some("pending"));
        // Unprobed pass (cache fresh): both survive.
        let r = reconcile_one(&mut s, "a", None, None, None);
        assert_eq!(r.pr_url.as_deref(), Some("https://github.com/o/r/pull/1"));
        assert_eq!(r.ci_status.as_deref(), Some("pending"));
        // Probed again, PR now closed: both clear.
        let r = reconcile_one(&mut s, "a", None, None, Some((None, None)));
        assert_eq!(r.pr_url, None);
        assert_eq!(r.ci_status, None);
    }

    #[test]
    fn stop_hook_status_write_stamps_last_turn_and_idle_since() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-1").unwrap();
        s.set_claude_status_by_session_id("uuid-1", "idle").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(row.last_turn_at.is_some());
        let idle = row.idle_since.expect("idle stamped by the hook");
        s.set_claude_status_by_session_id("uuid-1", "working")
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.idle_since, None);
        s.set_claude_status_by_session_id("uuid-1", "idle").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(row.idle_since.unwrap() >= idle);
    }

    #[test]
    fn bg_upsert_maintains_idle_since_from_agent_status() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 1)
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            None
        );
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("completed"), 2)
            .unwrap();
        let stamp = s
            .get_session("bg:u1", "local")
            .unwrap()
            .unwrap()
            .idle_since
            .expect("stamped");
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("completed"), 3)
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            Some(stamp)
        );
        s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 4)
            .unwrap();
        assert_eq!(
            s.get_session("bg:u1", "local").unwrap().unwrap().idle_since,
            None
        );
    }

    #[test]
    fn last_prompt_is_truncated_started_at_set_once_and_playbook_stamp_emits() {
        let (s, bus) = store_with_recorder();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let long: String = "x".repeat(LAST_PROMPT_CHARS + 50);
        let row = s.set_last_prompt(id, &long).unwrap().unwrap();
        assert_eq!(
            row.last_prompt.as_deref().map(|p| p.chars().count()),
            Some(LAST_PROMPT_CHARS)
        );

        s.set_started_at(id, 100).unwrap();
        s.set_started_at(id, 200).unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().started_at,
            Some(100)
        );

        let _ = bus.take();
        let row = s
            .mark_playbook_applied(id, 555, "oom:recreate")
            .unwrap()
            .unwrap();
        assert_eq!(row.last_playbook_at, Some(555));
        assert_eq!(bus.take(), vec![format!("session:updated:{id}")]);
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events
            .iter()
            .any(|e| e.kind == "playbook_applied" && e.detail.as_deref() == Some("oom:recreate")));
    }
}
