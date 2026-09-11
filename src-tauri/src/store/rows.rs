//! Row types returned by `Store`, their column lists and row mappers, and
//! the connection-level fetch helpers shared by the `&self` and `_in_tx` paths.

use super::*;

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
    /// Host whose checkout this is (migration 024): `local` for the project
    /// scan's rows, a remote alias for rows its EnterWorktree hook reported.
    pub host_alias: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
}

/// Parent-fingerprint keys per worktree row id ([`Store::fingerprint_keys`]).
/// Callers compute them BEFORE they take the store lock, since resolving a
/// local path touches the filesystem, and pass them to the delete functions.
pub type FingerprintKeys = std::collections::HashMap<i64, Vec<String>>;

/// Columns every `WorktreeRow` query selects, in [`worktree_from_row`] order.
pub(super) const WORKTREE_COLUMNS: &str = "id, project_id, host_alias, name, path, branch";

/// Map a row selected with [`WORKTREE_COLUMNS`].
pub(super) fn worktree_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorktreeRow> {
    Ok(WorktreeRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        host_alias: row.get(2)?,
        name: row.get(3)?,
        path: row.get(4)?,
        branch: row.get(5)?,
    })
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
    /// Token usage + estimated cost (migration 025), flattened onto the
    /// wire as the `usage_*` fields.
    #[serde(flatten)]
    pub usage: SessionUsage,
}

/// The `sessions` column list every `SessionRow` read shares, in the order
/// `map_session_row` consumes it. One definition so a new column is added in
/// exactly two places (here and the mapper) instead of six.
pub(super) const SESSION_COLUMNS: &str =
    "id, tmux_name, host_alias, project_id, worktree_id, created_at, \
     last_activity_at, status, notes, account_uuid, kind, reviews_session_id, \
     worktree_key, lost_at, \
     claude_session_id, claude_status, effort_level, pr_url, current_activity, \
     context_pct, stuck_kind, friendly_name, \
     safe_kill_state, safe_kill_nonce, safe_kill_detail, safe_kill_requested_at, \
     idle_since, stuck_since, last_playbook_at, last_prompt, started_at, last_turn_at, ci_status, \
     turn_seq, last_stop_at, parent_session_id, tags, \
     usage_input_tokens, usage_output_tokens, usage_cache_write_tokens, usage_cache_read_tokens, \
     usage_cost_micros, usage_model, usage_updated_at";

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
pub(super) fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
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
        usage: SessionUsage {
            usage_input_tokens: row.get(37)?,
            usage_output_tokens: row.get(38)?,
            usage_cache_write_tokens: row.get(39)?,
            usage_cache_read_tokens: row.get(40)?,
            usage_cost_micros: row.get(41)?,
            usage_model: row.get(42)?,
            usage_updated_at: row.get(43)?,
        },
    })
}

/// Token usage + estimated cost of a session (migration 025). Flattened
/// into `SessionRow` on the wire, so the fields keep their `usage_` prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SessionUsage {
    pub usage_input_tokens: i64,
    pub usage_output_tokens: i64,
    pub usage_cache_write_tokens: i64,
    pub usage_cache_read_tokens: i64,
    /// Estimated cost in millionths of a USD (see `service::usage`).
    pub usage_cost_micros: i64,
    /// Model of the most recent counted message.
    pub usage_model: Option<String>,
    /// Unix secs the totals last changed.
    pub usage_updated_at: Option<i64>,
}

impl SessionUsage {
    pub fn totals(&self) -> UsageTotals {
        UsageTotals {
            input_tokens: self.usage_input_tokens,
            output_tokens: self.usage_output_tokens,
            cache_write_tokens: self.usage_cache_write_tokens,
            cache_read_tokens: self.usage_cache_read_tokens,
            cost_micros: self.usage_cost_micros,
        }
    }
}

/// Token counts plus estimated cost (micro-USD): the unit of every usage
/// roll-up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    pub cost_micros: i64,
}

impl UsageTotals {
    pub fn add(&mut self, o: &UsageTotals) {
        self.input_tokens = self.input_tokens.saturating_add(o.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(o.output_tokens);
        self.cache_write_tokens = self.cache_write_tokens.saturating_add(o.cache_write_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(o.cache_read_tokens);
        self.cost_micros = self.cost_micros.saturating_add(o.cost_micros);
    }

    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }

    /// Field-wise `self - before`, floored at 0.
    pub(super) fn growth_since(&self, before: &UsageTotals) -> UsageTotals {
        let g = |a: i64, b: i64| a.saturating_sub(b).max(0);
        UsageTotals {
            input_tokens: g(self.input_tokens, before.input_tokens),
            output_tokens: g(self.output_tokens, before.output_tokens),
            cache_write_tokens: g(self.cache_write_tokens, before.cache_write_tokens),
            cache_read_tokens: g(self.cache_read_tokens, before.cache_read_tokens),
            cost_micros: g(self.cost_micros, before.cost_micros),
        }
    }
}

/// Where the next usage pass resumes for one session (migration 025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCursor {
    pub session_id: i64,
    pub transcript_path: Option<String>,
    pub claude_session_id: Option<String>,
    pub offset_bytes: i64,
    /// Transcript file name the offset refers to.
    pub source: Option<String>,
    pub last_msg_id: Option<String>,
    /// What was counted for `last_msg_id`: `in,out,cache_write,cache_read,cache_write_5m`.
    pub last_msg_usage: Option<String>,
}

/// One usage pass's result for a session, applied by `Store::apply_usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageDelta {
    /// The file was rewritten: `totals` REPLACE the stored totals.
    pub reset: bool,
    pub totals: UsageTotals,
    pub model: Option<String>,
    pub offset: i64,
    pub source: String,
    pub last_msg_id: Option<String>,
    pub last_msg_usage: Option<String>,
    pub now: i64,
}

/// `claude_status` values that mean "no turn in progress" — the states
/// `idle_since` is stamped on (see migration 019).
pub const IDLE_STATUSES: [&str; 3] = ["idle", "completed", "stopped"];

/// SQL fragment: the new `idle_since` given the OLD row's `idle_since` and the
/// status expression `{st}` (which must resolve to the post-write status).
/// Entering an idle status stamps `now` once; staying idle keeps the stamp;
/// leaving clears it.
pub(super) fn idle_since_sql(st: &str, now_param: &str) -> String {
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

pub(super) const TASK_COLUMNS: &str =
    "id, requester_session_id, worker_session_id, prompt, state, result, \
     error, created_at, started_at, finished_at, nonce, worker_claude_session_id";

/// [`TASK_COLUMNS`] qualified with the `t.` alias for joined queries.
pub(super) fn task_columns_t() -> String {
    TASK_COLUMNS
        .split(',')
        .map(|c| format!("t.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn map_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
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

pub(super) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- Connection-level row fetch helpers ----
//
// Free functions (not methods) so they accept a bare `&Connection`. A
// `&Transaction` derefs to `&Connection`, so the same SQL serves both the
// autocommit `&self` helpers and the transactional `_in_tx` mutation paths
// without duplicating the row-mapping closures.

pub(super) fn fetch_session(
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

pub(super) fn fetch_session_by_id(
    conn: &Connection,
    id: i64,
) -> Result<Option<SessionRow>, rusqlite::Error> {
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
pub(super) fn map_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMessage> {
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

pub(super) fn fetch_host(
    conn: &Connection,
    alias: &str,
) -> Result<Option<HostRow>, rusqlite::Error> {
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

pub(super) fn fetch_project(
    conn: &Connection,
    id: i64,
) -> Result<Option<ProjectRow>, rusqlite::Error> {
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
