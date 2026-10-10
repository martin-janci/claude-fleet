//! Row types returned by `Store`, their column lists and row mappers, and
//! the connection-level fetch helpers shared by the `&self` and `_in_tx` paths.

use super::*;

/// Defined once in `service::pane_intel` (they are built from a parsed pane
/// tail); re-exported here so `SessionRow` can name them.
pub use crate::service::pane_intel::{PendingInput, PendingOption};

/// The chat form a session's agent is waiting on (chat forms, migration
/// 119): read by a subselect on `form_requests`, null when none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingForm {
    pub form_id: String,
    pub title: String,
}

/// The form a session's agent is still writing (`ask { draft }`, migration
/// 153): the JSON so far, which the chat draws in as skeleton fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FormDraft {
    pub draft: String,
    #[serde(default)]
    pub why: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectRow {
    pub id: i64,
    pub owner: String,
    pub repo: String,
    pub base_path: String,
    #[serde(default)]
    pub last_session_at: Option<i64>,
    /// Set by `service::add_project`'s `folder` source (migration 027): this
    /// row was registered from a checkout already on disk, possibly outside
    /// the local projects root. `refresh_projects`'s stale-rows sweep must
    /// never delete such a row for being outside the root — that is its
    /// normal shape, not evidence of staleness (see `service::projects`).
    pub adopted: bool,
    /// Set by `service::operator` (migration 038): this row is the UX agent's
    /// own working directory, not one of the user's repositories. The project
    /// picker hides it and `refresh_projects`'s stale-rows sweep leaves it
    /// alone — the same bargain `adopted` makes, for a different reason.
    ///
    /// `serde(default)` is load-bearing, not tidiness: this row is read off
    /// the wire by a desktop paired to a hub, and a hub built before 038
    /// sends no `system` key at all. Without the default, `list_projects`
    /// fails the whole response with `E_PARSE` and the paired desktop shows
    /// no projects — which is exactly what happened in 0.2.27. `false` is
    /// also the right answer for such a hub: it has no system projects.
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorktreeRow {
    pub id: i64,
    pub project_id: i64,
    /// Host whose checkout this is (migration 024): `local` for the project
    /// scan's rows, a remote alias for rows its EnterWorktree hook reported.
    pub host_alias: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
}

/// Parent-fingerprint keys per worktree row id ([`Store::fingerprint_keys`]).
/// Callers compute them BEFORE they take the store lock, since resolving a
/// local path touches the filesystem, and pass them to the delete functions.
pub type FingerprintKeys = std::collections::HashMap<i64, Vec<String>>;

/// Columns every `ProjectRow` query selects, in [`map_project_row`] order.
pub(super) const PROJECT_COLUMNS: &str =
    "id, owner, repo, base_path, last_session_at, adopted, system";

/// Map a row selected with [`PROJECT_COLUMNS`] (at column offset 0).
pub(super) fn map_project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: row.get(0)?,
        owner: row.get(1)?,
        repo: row.get(2)?,
        base_path: row.get(3)?,
        last_session_at: row.get(4)?,
        adopted: row.get::<_, i64>(5)? != 0,
        system: row.get::<_, i64>(6)? != 0,
    })
}

/// `cols` (a comma-separated column list) with every column qualified by
/// `alias.`, for joined queries.
pub(super) fn qualified(cols: &str, alias: &str) -> String {
    cols.split(',')
        .map(|c| format!("{alias}.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

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

/// Whether a session `kind` has no tmux pane: a `claude --bg` agent (`bg`) or
/// an interactive Claude session running outside fleet (`external`). Both
/// carry a `bg:<sessionId>` sentinel `tmux_name`; nothing that needs a pane,
/// a worktree or process ownership may act on them.
pub fn has_no_pane(kind: &str) -> bool {
    matches!(kind, "bg" | "external")
}

/// `Store::ghost_and_clean` kind filter: tmux-backed rows, pruned by the
/// reconcile write-burst against the host's tmux list.
pub(super) const KIND_TMUX: &str = "kind NOT IN ('bg','external')";

/// `Store::ghost_and_clean` kind filter: pane-less rows ([`has_no_pane`]),
/// pruned by `Store::ghost_and_clean_bg_sessions` against `claude agents
/// --json`. They are never tmux sessions, so the tmux-keyed pass must skip
/// them.
pub(super) const KIND_PANE_LESS: &str = "kind IN ('bg','external')";

/// `PartialEq` covers every wire field including `row_version`. For the
/// no-op-reconcile-pass check `upsert_session_in_tx` wants (a real change vs.
/// a pass that observed exactly what is already stored), use
/// [`SessionRow::eq_ignoring_row_version`] instead: `row_version` also moves
/// for a change the wire row does not show (a non-wire column, or an
/// explicit bump), so plain `==` can call two equal-content reads different.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionRow {
    pub id: i64,
    pub tmux_name: String,
    pub host_alias: String,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub worktree_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub status: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub account_uuid: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub reviews_session_id: Option<i64>,
    #[serde(default)]
    pub worktree_key: Option<String>,
    #[serde(default)]
    pub lost_at: Option<i64>,
    /// Why the row was marked lost (migration 036): `host_reboot` |
    /// `tmux_server_gone` | `missing` | `killed`. `None` while the row is
    /// live (or never lost).
    #[serde(default)]
    pub lost_reason: Option<String>,
    #[serde(default)]
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub claude_status: Option<String>,
    #[serde(default)]
    pub effort_level: Option<String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub current_activity: Option<String>,
    #[serde(default)]
    pub context_pct: Option<f64>,
    #[serde(default)]
    pub stuck_kind: Option<String>,
    /// Display label set by the in-session agent via the `set_friendly_name`
    /// MCP tool (migration 016). The sidebar shows this when the user's
    /// "friendly names" toggle is on; falls back to `tmux_name` when NULL.
    #[serde(default)]
    pub friendly_name: Option<String>,
    #[serde(default)]
    pub safe_kill_state: Option<String>,
    #[serde(default)]
    pub safe_kill_nonce: Option<String>,
    #[serde(default)]
    pub safe_kill_detail: Option<String>,
    #[serde(default)]
    pub safe_kill_requested_at: Option<i64>,
    // ── Lifecycle + outcome fields (migration 019) ──
    /// When `claude_status` last entered idle/completed/stopped; NULL while
    /// working/blocked/unknown. Drives the GC sweeper.
    #[serde(default)]
    pub idle_since: Option<i64>,
    /// When the current `stuck_kind` episode began; NULL when not stuck.
    #[serde(default)]
    pub stuck_since: Option<i64>,
    /// When a stuck playbook last acted on this row. One stamp shared by
    /// every playbook kind: it gates "once per stuck episode" for all of
    /// them and the 1 h spacing for `oom`.
    #[serde(default)]
    pub last_playbook_at: Option<i64>,
    /// First 200 chars of the last prompt sent through fleet.
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// When fleet created the session (NULL for tmux-discovered rows).
    #[serde(default)]
    pub started_at: Option<i64>,
    /// Last Stop hook (turn completed).
    #[serde(default)]
    pub last_turn_at: Option<i64>,
    /// `passing` | `failing` | `pending` from the PR's check rollup.
    #[serde(default)]
    pub ci_status: Option<String>,
    // ── Orchestration fields (migration 020) ──
    /// Number of completed turns, incremented by every Stop hook. Callers
    /// snapshot it before `send_prompt` and wait for it to grow.
    pub turn_seq: i64,
    /// Unix secs of the last Stop hook (a hook-stamped status newer than a
    /// reconcile pass's pane observation wins over the pane heuristic).
    #[serde(default)]
    pub last_stop_at: Option<i64>,
    /// When the tick demoted this row from a stale `working` to `idle`
    /// (migration 065, lifecycle F2); `None` otherwise. Cleared by the next
    /// hook or a pane that shows a live turn. `#[serde(default)]`: a hub
    /// older than the column sends none.
    #[serde(default)]
    pub stale_working_at: Option<i64>,
    /// The stale-working demotion's own memory (`sessions.stale_demoted_at`,
    /// migration 080): set with `stale_working_at` by the tick, but lifted
    /// only by a hook, a pane that shows a live turn, or the row being
    /// `working` / `blocked` again — never by an attach or the TTL. While
    /// set, the row's stored `idle` is a guess (`store::trusted_status`).
    /// Server-side only: `#[serde(skip)]` keeps it off the wire, the row
    /// events and the hub JSON (a deserialized row reads `None`), and
    /// [`SessionRow::eq_ignoring_row_version`] ignores it, so a change to it
    /// alone is not a client-visible change.
    #[serde(skip)]
    pub stale_demoted_at: Option<i64>,
    /// The requester session that dispatched the task this row is working
    /// on; NULL for top-level sessions.
    #[serde(default)]
    pub parent_session_id: Option<i64>,
    /// Free-form labels set via `set_session_tags`. Stored as a JSON array
    /// (NULL ⇒ empty) and always surfaced as a list on the wire.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Bumped by a trigger on every UPDATE that changes a column other than
    /// the reconcile's `last_reconciled_at` stamp (migrations 042 and 063),
    /// and explicitly for a `work` / org change. The frontend's merge guard
    /// orders a command's return value against a row event by it.
    /// `#[serde(default)]`: a hub older than the column sends none.
    #[serde(default)]
    pub row_version: i64,
    /// How many UserPromptSubmit hooks this row has recorded (migration
    /// 042). A composer reads it as a delivery receipt: the count moving
    /// past the one its send started from is Claude taking the prompt.
    /// `#[serde(default)]`: a hub older than this field sends none, and a
    /// client must then treat 0 as "no receipts", not "never submitted".
    #[serde(default)]
    pub prompt_submit_seq: i64,
    /// Token usage + estimated cost (migration 025), flattened onto the
    /// wire as the `usage_*` fields.
    #[serde(flatten)]
    pub usage: SessionUsage,
    /// Current-conversation context (migration 037), flattened on the wire.
    #[serde(flatten)]
    pub context: SessionContext,
    /// The permission/question dialog a blocked pane is showing (migration
    /// 040), derived by the reconcile pass alongside `current_activity`.
    /// `None` whenever the pane shows no such dialog. `serde(default)` so a
    /// hub/desktop/phone built before this column never fails to parse a row
    /// that omits it.
    #[serde(default)]
    pub pending_input: Option<PendingInput>,
    /// The session's primary work link (migration 046), read-only here: it
    /// is set through `work_link`. `None` when the session has none.
    /// `serde(default)` so a peer older than the work graph parses the row.
    #[serde(default)]
    pub work: Option<WorkSummary>,
    /// Keys this session's user said it does NOT work on (sticky rejections,
    /// migration 046). A client that recognises keys itself (the sidebar's
    /// branch/tag fallback) must not show these. Empty for most rows.
    #[serde(default)]
    pub work_rejected: Vec<String>,
    /// The session's top link SUGGESTION (work graph M4: a guess no one has
    /// decided), with the number of live suggestions in `suggestions`.
    /// Once the session has confirmed work, a weak suggestion (a mention in
    /// a prompt, a PR text, a trailer) no longer counts here: only a strong
    /// one asks for a decision. The link itself stays in `work_links`.
    /// Kept apart from `work` on purpose: a peer older than M4 reads only
    /// `work`, so it can never mistake a guess for a link — and a guess
    /// never moves a session into a work group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_suggested: Option<WorkSummary>,
    /// The session's org (work graph M5): the most specific `org_rules`
    /// match (path > owner/repo > owner > host rule), else its host's org;
    /// `None` = unassigned. Computed in SQL (`session_org_sql!`), so listed
    /// and emitted rows agree. Absent from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// A checksum of the session's live links — their ids and versions
    /// (work graph M14). `work` / `work_suggested` show only the primary and
    /// the top guess; this moves on every link change, a secondary's too, so
    /// a client knows when to re-read the session's tasks. Opaque; `0` (and
    /// absent) when the session has no live link. Never sent to a scoped
    /// caller (`OrgScope::redact_row_org_only`): another org's link would move it.
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub work_rev: i64,
    /// What the PR probe last read as evidence about the session's PR
    /// (migration 082, result evidence): the commit the checks describe,
    /// the worktree's own HEAD, review and merge state. `None` without a
    /// PR, before the first probe, or from a host whose `gh` answers only
    /// the basic fields. Absent from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_evidence: Option<crate::service::outcome::PrEvidence>,
    /// When the probe last observed the PR (migration 082): exact when the
    /// evidence changed, else at most `outcome::PR_CHECKED_REFRESH_SECS`
    /// old while probes succeed. A reading older than
    /// `outcome::PR_EVIDENCE_STALE_SECS` describes the past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_checked_at: Option<i64>,
    /// Whose session this is (migration 099, multi-user M1): the `people`
    /// row that owns it. `None` for a row nobody can speak for — one
    /// reconcile discovered on a host, or a pre-M1 row fleet did not create
    /// (`visibility = 'unclaimed'`).
    ///
    /// A caller-INDEPENDENT fact: it says who the owner is, not what the
    /// reader may do, so it is safe on the broadcast bus. There is
    /// deliberately no per-caller access field on this row; the answer to
    /// "may THIS caller see it" is computed at the gate from this field,
    /// [`SessionRow::visibility`] and the caller's grants, and never
    /// stored on the row a broadcast carries.
    ///
    /// It is **not** what the stream fence keys on. `BroadcastEventBus::emit`
    /// runs `strip_nulls` before the frame enters the replay ring
    /// (`events.rs`), so an unowned row's key is *absent* rather than null —
    /// indistinguishable from a hub built before the column, and "absent"
    /// reads as "no restriction" (spec §3.7). `visibility`, which is NOT
    /// NULL, is the key that survives that and the one a fence may read.
    /// Hence no `skip_serializing_if` on either field.
    #[serde(default)]
    pub owner_person_id: Option<i64>,
    /// [`VISIBILITY_PRIVATE`] or [`VISIBILITY_UNCLAIMED`] (migration 099,
    /// whose `CHECK` admits nothing else — in particular no `'org'`, which
    /// the owner removed from M1 because the schema's only referent for
    /// "the org can see it" is a column an admin binds their own device to;
    /// spec §4.3). `private` is the default for anything a person starts;
    /// `unclaimed` is the safe holding state.
    ///
    /// `#[serde(default = "visibility_unclaimed")]`, never a bare
    /// `#[serde(default)]`: see that function for why the difference is the
    /// privacy-critical one.
    #[serde(default = "visibility_unclaimed")]
    pub visibility: String,
    /// The credential profile the session runs under (migration 113,
    /// docs/accounts.md): `CLAUDE_CONFIG_DIR` is
    /// `~/.claude-profiles/<name>` on its host. `None` = the host's own login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_profile: Option<String>,
    /// Which coding agent runs in the session's pane (migration 121): one of
    /// [`AGENTS`]. A different axis from `kind` (what the session is for);
    /// a `shell` session is the one place they meet, and
    /// `Store::set_session_kind` keeps the two in step there.
    ///
    /// `#[serde(default = "agent_claude")]`: a hub older than the column
    /// sends no `agent`, and every session such a hub runs is Claude Code
    /// (a shell row there still says `kind: "shell"`). Never skipped when
    /// serialised, so a client can tell a Claude row from an old hub's.
    #[serde(default = "agent_claude")]
    pub agent: String,
    /// Who or what started the session (migration 124): one of
    /// [`ORIGINS`], or `None` for a row fleet did not start (found on a
    /// host by reconcile, or older than the column). See [`SessionOrigin`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// What [`SessionRow::origin`] points at, as text: a person id, a
    /// session id, a mission id (migration 124 lists which). `None` when
    /// the origin has nothing to point at, or is itself `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_ref: Option<String>,
    /// When a person last looked at the session (migration 125), unix
    /// seconds: `touch_session_viewed`. A turn that ended after it
    /// (`last_stop_at`) is unread. `None` for a row nobody has opened since
    /// fleet found it; a session fleet started counts from `started_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_viewed_at: Option<i64>,
    /// What a finished turn came to when hooks said nothing (migration
    /// 129, J2 in step 5.11): one of [`TURN_OUTCOMES`]. A hook event always
    /// wins over it. `None` when nothing answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_outcome: Option<String>,
    /// What a rule, Jev or an LLM proposes about this session, one per
    /// feature (step 2.8): read from `decision_runs`, never stored on the
    /// row. A person confirms or changes each; none ever acts. Empty, and
    /// absent on the wire, when nothing proposes anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<DecisionProposal>,
    /// The form this session's agent asked and is waiting on. `serde(default)`
    /// so an older hub's row (without it) still parses.
    #[serde(default)]
    pub pending_form: Option<PendingForm>,
    /// The form this session's agent is still writing (`ask { draft }`),
    /// absent when none. `serde(default)` for an older hub's row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form_draft: Option<FormDraft>,
}

/// `sessions.agent` (migration 121): Claude Code, the default.
pub const AGENT_CLAUDE: &str = "claude";

/// `sessions.agent` (migration 121): OpenAI's Codex CLI (redesign 12.2).
pub const AGENT_CODEX: &str = "codex";

/// `sessions.agent` (migration 121): Google's Antigravity CLI (`agy`),
/// launched through `agent_adapter::Agy` (redesign step 12.3).
pub const AGENT_AGY: &str = "agy";

/// `sessions.agent` (migration 121): no agent, a plain login shell
/// (`kind = 'shell'`).
pub const AGENT_SHELL: &str = "shell";

/// Every value migration 121's `CHECK` admits. `agy` has its adapter but
/// `new_session` refuses it until its start path is enabled.
pub const AGENTS: [&str; 4] = [AGENT_CLAUDE, AGENT_CODEX, AGENT_AGY, AGENT_SHELL];

/// [`SessionRow::agent`] when a frame carries no `agent` key: a hub built
/// before migration 121, whose sessions all run Claude Code.
fn agent_claude() -> String {
    AGENT_CLAUDE.to_string()
}

/// `sessions.turn_outcome` (migration 130): every value its `CHECK` admits.
pub const TURN_OUTCOMES: [&str; 5] = ["finished", "asked", "stuck", "working", "unsure"];

/// `decision_runs.subject_kind` of a run about one session: its
/// `subject_id` is the session id as text. What [`SessionRow::proposals`]
/// reads.
pub const PROPOSAL_SUBJECT_SESSION: &str = "session";

/// `decision_runs.subject_kind` of a run about one work item: its
/// `subject_id` is the `work_items.id` as text.
pub const PROPOSAL_SUBJECT_WORK_ITEM: &str = "work_item";

/// Below this confidence an answer is recorded, never proposed: the UI
/// pre-selects nothing (redesign ai.md, "below the confidence floor").
pub const PROPOSAL_MIN_CONFIDENCE: f64 = 0.5;

/// One proposal on the wire (step 2.8): a value a rule, Jev or an LLM
/// suggests for one question about a row, with where it came from. The
/// shape every row carries, so the UI draws "Proposed by Jev · why ·
/// Change" one way. Only a live `assist` answer is proposed: never a
/// shadow run, a fallback, `unsure`, one under [`PROPOSAL_MIN_CONFIDENCE`],
/// or one a person already confirmed, corrected or rejected (save a linked
/// `related_session`, see [`DecisionProposal::linked`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecisionProposal {
    /// The question: a `decide` feature (`start_project`, `turn_outcome`,
    /// `work_link`…).
    pub feature: String,
    /// The proposed answer, an id or a vocabulary word (`p12`,
    /// `finished`).
    pub value: String,
    /// `rule`, `jev` or `llm`.
    pub source: String,
    /// Fleet's own words for why, when the use case composes them; never
    /// model text. A run read back from `decision_runs` (which holds no
    /// text) carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The model's confidence in whole percent, when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// The recorded run, which a confirm or a change marks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
    /// When it was decided, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    /// `Some(true)` on a `related_session` a person confirmed with Link
    /// (M15 G4.3): no longer a proposal but the row's linked partner, kept
    /// so Details lists it. Absent on every other proposal and from an
    /// older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// `DecisionProposal::source` for a `decision_runs.provider`.
pub fn proposal_source(provider: &str) -> &'static str {
    match provider {
        "rules" => "rule",
        "llm" => "llm",
        _ => "jev",
    }
}

/// Decode a [`proposals_sql!`] subselect: a JSON array, sorted by feature
/// so two reads of the same runs compare equal. Malformed text reads as
/// no proposals rather than failing the row.
pub(super) fn decode_proposals(raw: Option<String>) -> Vec<DecisionProposal> {
    let mut v: Vec<DecisionProposal> = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    v.sort_by(|a, b| a.feature.cmp(&b.feature));
    v
}

/// `sessions.origin` (migration 124): every value its `CHECK` admits.
pub const ORIGINS: [&str; 6] = [
    "person",
    "operator",
    "mission",
    "background",
    "token",
    "routine",
];

/// Who or what started a session, as `finalize_new_session` and the other
/// start paths record it (`sessions.origin` / `origin_ref`, migration 124).
///
/// Never read from a request: like `owner_person_id` it follows from the
/// connection or from the code path that starts the session, so a client
/// cannot label its own session as a mission's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOrigin {
    /// One of [`ORIGINS`].
    pub origin: &'static str,
    pub origin_ref: Option<String>,
}

impl SessionOrigin {
    /// A person started it, from the desktop, the phone or the master token.
    /// `None` when the hub cannot say who that person is.
    pub fn person(person_id: Option<i64>) -> Self {
        Self::with("person", person_id)
    }

    /// The operator (the UX agent) started it; its own session's id.
    pub fn operator(operator_session: Option<i64>) -> Self {
        Self::with("operator", operator_session)
    }

    /// A mission's run started it.
    pub fn mission(mission_id: i64) -> Self {
        Self::with("mission", Some(mission_id))
    }

    /// A `claude --bg` agent, launched for `requester` when one is named.
    pub fn background(requester: Option<i64>) -> Self {
        Self::with("background", requester)
    }

    /// A per-host token started it: an agent in the session whose pane the
    /// request proved, or a script on the host (`None`).
    pub fn token(proven_session: Option<i64>) -> Self {
        Self::with("token", proven_session)
    }

    /// A routine's run started it (redesign 8.5).
    pub fn routine(routine_id: i64) -> Self {
        Self::with("routine", Some(routine_id))
    }

    /// The origin a row records, to carry it onto another row (a move).
    /// `None` for a row with no origin, or one this build does not know.
    pub fn of_row(row: &SessionRow) -> Option<Self> {
        let origin = ORIGINS
            .iter()
            .find(|o| Some(**o) == row.origin.as_deref())?;
        Some(Self {
            origin,
            origin_ref: row.origin_ref.clone(),
        })
    }

    fn with(origin: &'static str, id: Option<i64>) -> Self {
        Self {
            origin,
            origin_ref: id.map(|i| i.to_string()),
        }
    }
}

/// `sessions.visibility` (migration 100): private to its owner, and to the
/// people the owner has granted `watch` or `drive` to. The default for
/// anything a person starts through fleet.
pub const VISIBILITY_PRIVATE: &str = "private";

/// `sessions.visibility` (migration 100): nobody can speak for this row —
/// reconcile found it on a host, or it predates M1 and fleet did not create
/// it. An out-of-scope caller learns a per-host COUNT of these and not one
/// byte more (spec §4.3); claiming one needs proof of host access.
pub const VISIBILITY_UNCLAIMED: &str = "unclaimed";

/// [`SessionRow::visibility`] when a frame carries no `visibility` key at
/// all: a hub built before migration 099, or a row read from one.
///
/// **It must be a named function.** A bare `#[serde(default)]` on a `String`
/// yields `String::default()` — the empty string — which is neither
/// `private` nor `unclaimed` and so matches no arm anybody writes: the
/// privacy fence would fail *open* into a case that does not exist (spec
/// §3.7). The repo's convention for exactly this shape is a named default
/// function (`store/orgs.rs::bound_sees_unassigned_default`,
/// `store/trackers.rs::default_true`, `store/work.rs::default_role`), and
/// `rows::tests::a_session_row_with_no_visibility_key_reads_unclaimed`
/// asserts the VALUE, not merely that parsing succeeded.
fn visibility_unclaimed() -> String {
    VISIBILITY_UNCLAIMED.to_string()
}

fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

impl SessionRow {
    /// Field-by-field equality excluding `row_version` and the server-only
    /// `stale_demoted_at`: whether two reads of this row carry the same
    /// user-visible content.
    ///
    /// See the note above the `PartialEq` derive: plain `==` cannot answer
    /// that question, because `row_version` also moves for a change no wire
    /// field shows (a non-wire column, an explicit bump).
    ///
    /// The equal-version case — every no-op reconcile pass since migration
    /// 063, which is what calls this — compares in place. Only a genuine version difference
    /// pays for a clone, and then for one row rather than two: this runs
    /// once per session per pass, and a `SessionRow` is some forty fields
    /// with a dozen heap allocations among them.
    pub fn eq_ignoring_row_version(&self, other: &Self) -> bool {
        if self.row_version == other.row_version && self.stale_demoted_at == other.stale_demoted_at
        {
            return self == other;
        }
        Self {
            row_version: other.row_version,
            stale_demoted_at: other.stale_demoted_at,
            ..self.clone()
        } == *other
    }
}

/// What losing a session clears, in every path that loses one (a host's
/// sessions lost, a kill, a reconcile ghosting): the pane-derived state,
/// the stale-working stamp and its veto, none of which a dead row can act on.
pub(super) const LOSS_CLEARS: &str = "claude_status=NULL, stuck_kind=NULL, stuck_since=NULL, \
     current_activity=NULL, pending_input=NULL, stale_working_at=NULL, stale_demoted_at=NULL";

/// The `sessions` column list every `SessionRow` read shares, in the order
/// `map_session_row` consumes it. One definition so a new column is added in
/// exactly two places (here and the mapper) instead of six.
pub(super) const SESSION_COLUMNS: &str = concat!(
    "id, tmux_name, host_alias, project_id, worktree_id, created_at, \
     last_activity_at, status, notes, account_uuid, kind, reviews_session_id, \
     worktree_key, lost_at, \
     claude_session_id, claude_status, effort_level, pr_url, current_activity, \
     context_pct, stuck_kind, friendly_name, \
     safe_kill_state, safe_kill_nonce, safe_kill_detail, safe_kill_requested_at, \
     idle_since, stuck_since, last_playbook_at, last_prompt, started_at, last_turn_at, ci_status, \
     turn_seq, last_stop_at, parent_session_id, tags, \
     usage_input_tokens, usage_output_tokens, usage_cache_write_tokens, usage_cache_read_tokens, \
     usage_cost_micros, usage_model, usage_updated_at, \
     model, context_tokens, context_window, context_source, context_at, context_stale, tmux_pane_id, \
     pending_input, row_version, lost_reason, \
     (SELECT json_object('link_id', l.id, 'item_id', l.item_id, \
                         'key', COALESCE(i.key, l.ref_key), 'title', COALESCE(i.title, ''), \
                         'source', l.source, \
                         'kind', \
                           CASE WHEN i.id IS NULL THEN 'ref' \
                                WHEN i.tracker_id IS NOT NULL THEN 'tracker' \
                                ELSE 'local' END, \
                         'status_category', \
                           CASE WHEN i.tracker_id IS NOT NULL THEN i.status_category END, \
                         'effective_status', ", crate::effective_status_sql!(), ", \
                         'status_name', i.status_name, 'url', i.url, \
                         'unavailable', json(CASE WHEN i.unavailable_at IS NOT NULL \
                                                  THEN 'true' ELSE 'false' END), \
                         'state', l.state, 'strength', l.strength, 'rule', l.rule, \
                         'archived_at', l.archived_at, \
                         'org_id', COALESCE((SELECT t.org_id FROM trackers t WHERE t.id = i.tracker_id), CASE WHEN i.tracker_id IS NULL THEN i.org_id END)) \
        FROM participants p \
        JOIN work_links l ON l.participant_id = p.id AND l.ended_at IS NULL \
                         AND l.is_primary = 1 AND l.state = 'confirmed' \
        LEFT JOIN work_items i ON i.id = l.item_id \
       WHERE p.session_id = sessions.id AND p.retired_at IS NULL LIMIT 1) AS work, \
     (SELECT json_group_array(COALESCE(i.key, l.ref_key)) \
        FROM participants p \
        JOIN work_links l ON l.participant_id = p.id AND l.ended_at IS NULL \
                         AND l.state = 'rejected' \
        LEFT JOIN work_items i ON i.id = l.item_id \
       WHERE p.session_id = sessions.id AND p.retired_at IS NULL \
         AND COALESCE(i.key, l.ref_key) IS NOT NULL) AS work_rejected, \
     (SELECT json_object('link_id', l.id, 'item_id', l.item_id, \
                         'key', COALESCE(i.key, l.ref_key), 'title', COALESCE(i.title, ''), \
                         'source', l.source, 'state', l.state, 'strength', l.strength, \
                         'rule', l.rule, \
                         'preselected', json(CASE WHEN l.preselected = 1 \
                                                  THEN 'true' ELSE 'false' END), \
                         'kind', \
                           CASE WHEN i.id IS NULL THEN 'ref' \
                                WHEN i.tracker_id IS NOT NULL THEN 'tracker' \
                                ELSE 'local' END, \
                         'status_category', \
                           CASE WHEN i.tracker_id IS NOT NULL THEN i.status_category END, \
                         'effective_status', ", crate::effective_status_sql!(), ", \
                         'status_name', i.status_name, 'url', i.url, \
                         'suggestions', (SELECT COUNT(*) FROM work_links s2 \
                                          WHERE s2.participant_id = p.id \
                                            AND s2.ended_at IS NULL \
                                            AND s2.state = 'suggested' \
                                            AND (s2.strength IS NOT 'weak' OR NOT EXISTS \
                                                 (SELECT 1 FROM work_links c \
                                                   WHERE c.participant_id = p.id \
                                                     AND c.ended_at IS NULL \
                                                     AND c.is_primary = 1 \
                                                     AND c.state = 'confirmed'))), \
                         'org_id', COALESCE((SELECT t.org_id FROM trackers t WHERE t.id = i.tracker_id), CASE WHEN i.tracker_id IS NULL THEN i.org_id END)) \
        FROM participants p \
        JOIN work_links l ON l.participant_id = p.id AND l.ended_at IS NULL \
                         AND l.state = 'suggested' \
        LEFT JOIN work_items i ON i.id = l.item_id \
       WHERE p.session_id = sessions.id AND p.retired_at IS NULL \
         AND (l.strength IS NOT 'weak' OR NOT EXISTS \
              (SELECT 1 FROM work_links c \
                WHERE c.participant_id = p.id AND c.ended_at IS NULL \
                  AND c.is_primary = 1 AND c.state = 'confirmed')) \
       ORDER BY l.preselected DESC, \
                CASE l.strength WHEN 'strong' THEN 0 WHEN 'inferred' THEN 1 ELSE 2 END, \
                COALESCE(l.decided_at, l.created_at) DESC, l.id DESC \
       LIMIT 1) AS work_suggested, ",
    crate::session_org_sql!("sessions"),
    " AS org_id, prompt_submit_seq, stale_working_at, stale_demoted_at, \
     (SELECT COALESCE(SUM(l.version * 1000003 + l.id), 0) FROM work_links l \
        JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
       WHERE p.session_id = sessions.id AND l.ended_at IS NULL) AS work_rev, \
     pr_evidence, pr_checked_at, owner_person_id, visibility, claude_profile, \
     (SELECT json_object('form_id', f.form_id, 'title', json_extract(f.spec, '$.title')) \
        FROM form_requests f WHERE f.session_id = sessions.id AND f.state = 'pending') \
       AS pending_form, agent, origin, origin_ref, last_viewed_at, turn_outcome, ",
    crate::proposals_sql!("session", "CAST(sessions.id AS TEXT)"),
    " AS proposals, \
     (SELECT json_object('draft', d.draft, 'why', d.why, 'updated_at', d.updated_at) \
        FROM form_drafts d WHERE d.session_id = sessions.id) AS form_draft"
);

/// Decode `sessions.pr_evidence`. Malformed text (never written by us)
/// reads as no evidence rather than failing every session read.
pub(super) fn decode_pr_evidence(
    raw: Option<String>,
) -> Option<crate::service::outcome::PrEvidence> {
    raw.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str(s).ok())
}

/// Decode the `sessions.tags` JSON column. NULL, empty, or malformed text
/// (never written by us, but a hand-edited DB is possible) reads as no tags
/// rather than failing every session read.
pub(super) fn decode_tags(raw: Option<String>) -> Vec<String> {
    raw.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

/// Encode tags for the `sessions.tags` column: `None` for an empty list so
/// an untagged row stays NULL (and `tag IS NULL` style queries work).
pub(super) fn encode_tags(tags: &[String]) -> Option<String> {
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
        row_version: row.get(52)?,
        usage: SessionUsage {
            usage_input_tokens: row.get(37)?,
            usage_output_tokens: row.get(38)?,
            usage_cache_write_tokens: row.get(39)?,
            usage_cache_read_tokens: row.get(40)?,
            usage_cost_micros: row.get(41)?,
            usage_model: row.get(42)?,
            usage_updated_at: row.get(43)?,
        },
        context: SessionContext {
            model: row.get(44)?,
            context_tokens: row.get(45)?,
            context_window: row.get(46)?,
            context_source: row.get(47)?,
            context_at: row.get(48)?,
            context_stale: row.get::<_, i64>(49)? != 0,
            tmux_pane_id: row.get(50)?,
        },
        pending_input: decode_pending_input(row.get(51)?),
        lost_reason: row.get(53)?,
        work: decode_work(row.get(54)?).map(|mut w| {
            w.suggestions = 0;
            w
        }),
        work_rejected: decode_tags(row.get(55)?),
        work_suggested: decode_work(row.get(56)?),
        org_id: row.get(57)?,
        prompt_submit_seq: row.get(58)?,
        stale_working_at: row.get(59)?,
        stale_demoted_at: row.get(60)?,
        work_rev: row.get(61)?,
        pr_evidence: decode_pr_evidence(row.get(62)?),
        pr_checked_at: row.get(63)?,
        owner_person_id: row.get(64)?,
        visibility: row.get(65)?,
        claude_profile: row.get(66)?,
        pending_form: row
            .get::<_, Option<String>>(67)?
            .and_then(|j| serde_json::from_str(&j).ok()),
        agent: row.get(68)?,
        origin: row.get(69)?,
        origin_ref: row.get(70)?,
        last_viewed_at: row.get(71)?,
        turn_outcome: row.get(72)?,
        proposals: decode_proposals(row.get(73)?),
        form_draft: row
            .get::<_, Option<String>>(74)?
            .and_then(|j| serde_json::from_str(&j).ok()),
    })
    .map(|mut r| {
        // A link's org is its tracker item's, else the session's (M5).
        for w in [r.work.as_mut(), r.work_suggested.as_mut()]
            .into_iter()
            .flatten()
        {
            if w.org_id.is_none() {
                w.org_id = r.org_id;
            }
        }
        // The primary's `suggestions` counts what is still to decide, which
        // only the suggestion subselect reads.
        if let (Some(w), Some(sg)) = (r.work.as_mut(), r.work_suggested.as_ref()) {
            w.suggestions = sg.suggestions;
        }
        r
    })
}

/// Decode the primary-work subselect of `SESSION_COLUMNS` (a JSON object, or
/// NULL when the session has no primary link). Malformed text reads as none.
pub(super) fn decode_work(raw: Option<String>) -> Option<WorkSummary> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// Decode the `sessions.pending_input` JSON column. NULL or malformed text
/// (never written by us, but a hand-edited DB is possible) reads as `None`
/// rather than failing the row.
pub(super) fn decode_pending_input(raw: Option<String>) -> Option<PendingInput> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// Encode `pending_input` for the `sessions.pending_input` column: `None`
/// for no dialog, or when the value fails to serialize (never happens for
/// the derived type, but a NULL is safer than a write error).
pub(super) fn encode_pending_input(pending_input: Option<&PendingInput>) -> Option<String> {
    pending_input.and_then(|p| serde_json::to_string(p).ok())
}

/// Token usage + estimated cost of a session (migration 025). Flattened
/// into `SessionRow` on the wire, so the fields keep their `usage_` prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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

/// Current-conversation state (migration 037). Flattened into `SessionRow`
/// on the wire, so the field names are the wire names.
/// `Deserialize` with `default` so a desktop reads a hub that predates these
/// columns (remote mode): every field then comes back empty.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SessionContext {
    /// Model of the current conversation (SessionStart / transcript).
    pub model: Option<String>,
    /// Prompt size of the latest request: input + cache read + cache write.
    pub context_tokens: Option<i64>,
    /// Context window of `model` (200 000 or 1 000 000).
    pub context_window: Option<i64>,
    /// `transcript` | `hook` | `pane`: who wrote the context value last.
    pub context_source: Option<String>,
    /// Unix secs of the last context write.
    pub context_at: Option<i64>,
    /// True after a compaction or resume until the next usage line.
    pub context_stale: bool,
    /// tmux pane id (`%17`) reconcile last saw for this row.
    pub tmux_pane_id: Option<String>,
}

/// Token counts plus estimated cost (micro-USD): the unit of every usage
/// roll-up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
    /// The file's size when this cursor last started it from byte 0
    /// (migration 072): a read starting below it is still that file's
    /// history. 0 = none pending.
    pub backfill_until: i64,
}

/// The `sessions` columns every `UsageCursor` read selects, in
/// [`map_usage_cursor`] order.
pub(super) const USAGE_CURSOR_COLUMNS: &str =
    "id, transcript_path, claude_session_id, usage_offset_bytes, usage_source, \
     usage_last_msg_id, usage_last_msg_usage, usage_backfill_until";

/// Map a row selected with [`USAGE_CURSOR_COLUMNS`].
pub(super) fn map_usage_cursor(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageCursor> {
    Ok(UsageCursor {
        session_id: row.get(0)?,
        transcript_path: row.get(1)?,
        claude_session_id: row.get(2)?,
        offset_bytes: row.get(3)?,
        source: row.get(4)?,
        last_msg_id: row.get(5)?,
        last_msg_usage: row.get(6)?,
        backfill_until: row.get(7)?,
    })
}

/// One UTC day's slice of a [`UsageDelta`], priced. `backfill` marks history
/// a fresh cursor read in one go (perf-logs §6a): kept apart in
/// `usage_daily` so a takeover day never reads as a $850 day.
#[derive(Debug, Clone, PartialEq)]
pub struct DayDelta {
    pub day: i64,
    pub totals: UsageTotals,
    pub backfill: bool,
    /// The model that spent this slice (`usage_daily_account`, step 4.2);
    /// `None` when the transcript line carried none.
    pub model: Option<String>,
}

/// One account's LIVE spend on one model over a window, summed from
/// `usage_daily_account` (migration 130, redesign step 4.2). `''` names an
/// unknown account (a session whose login fleet has not read yet) or a
/// transcript line with no model.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AccountModelCost {
    pub account_uuid: String,
    pub model: String,
    pub totals: UsageTotals,
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
    /// Per-day slices of `totals`; empty means "book everything to the day
    /// of `now`" (a reader without `D` lines).
    pub by_day: Vec<DayDelta>,
    /// A read from byte 0 sets the cursor's backfill mark (the file size it
    /// saw); `None` keeps the stored one.
    pub backfill_until: Option<i64>,
}

/// `claude_status` values that mean "no turn in progress" — the states
/// `idle_since` is stamped on (see migration 019).
pub const IDLE_STATUSES: [&str; 3] = ["idle", "completed", "stopped"];

/// A `claude_status` whose turn is over: an idle status, or `failed` (a turn
/// that ended in an error) — [`ClaudeStatus::is_quiet`], the one definition
/// (the frontend's `isQuietStatus` is held to it by the shared fixture
/// `service/testdata/quiet_statuses.json`). An unknown value is not quiet.
/// What `wait_for_session { until: idle }`, `run_prompt` and a move's source
/// check all wait for — one set, so a new terminal status cannot reach one
/// and not the others. Those callers read it through [`trusted_status`].
///
/// [`ClaudeStatus::is_quiet`]: crate::service::pane_intel::ClaudeStatus::is_quiet
pub fn turn_over(status: Option<&str>) -> bool {
    status
        .and_then(|s| s.parse::<crate::service::pane_intel::ClaudeStatus>().ok())
        .is_some_and(crate::service::pane_intel::ClaudeStatus::is_quiet)
}

/// The status a turn-over check may believe for `row`. A row the tick
/// demoted for staleness reads `idle` only because nothing moved for
/// `reconcile.stale_working_secs` — and one tool call running longer than
/// that fires no hook and grows no transcript. Its `idle` is a guess, so for
/// such a row this is `live`, the pane's own reading taken just now
/// (`session_activity`), and `None` — unknown, never over — when nobody
/// asked the pane or it could not tell. Any other row's stored status
/// stands.
///
/// The demotion's own memory is `row.stale_demoted_at` (migration 080): an
/// attach or `reconcile.stale_working_ttl_secs` clears the
/// `stale_working_at` attention stamp, but the stored `idle` is still a
/// guess until a hook or the pane lifts the demotion. The stamp counts too,
/// so a row read without the column (a hub's JSON) errs towards asking
/// while it is set.
pub fn trusted_status<'a>(row: &'a SessionRow, live: Option<&'a str>) -> Option<&'a str> {
    let stored = row.claude_status.as_deref();
    if needs_pane_confirmation(row) {
        live
    } else {
        stored
    }
}

/// [`turn_over`] of the row's [`trusted_status`] with no pane reading: a
/// stale-demoted row is never over on its stored `idle` alone.
pub fn turn_over_row(row: &SessionRow) -> bool {
    turn_over(trusted_status(row, None))
}

/// Whether `row` is stale-demoted (`stale_demoted_at`, or its attention
/// stamp still set) with a stored status that only a live pane reading can
/// confirm (see [`trusted_status`]): the one case a turn-over check should
/// spend a `session_activity` probe on.
pub fn needs_pane_confirmation(row: &SessionRow) -> bool {
    (row.stale_demoted_at.is_some() || row.stale_working_at.is_some())
        && turn_over(row.claude_status.as_deref())
}

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

/// `PartialEq` covers every wire field, for the same reason [`SessionRow`]
/// derives it: `update_host_probe_in_tx` compares the row it wrote against the
/// row that was there, so a probe that found the host unchanged can say so in
/// forty bytes instead of resending all of it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HostRow {
    pub alias: String,
    #[serde(default)]
    pub ssh_alias: Option<String>,
    pub reachable: bool,
    #[serde(default)]
    pub claude_version: Option<String>,
    #[serde(default)]
    pub tmux_version: Option<String>,
    pub hidden: bool,
    #[serde(default)]
    pub last_pinged_at: Option<i64>,
    #[serde(default)]
    pub account_uuid: Option<String>,
    pub provisioned: bool,
    /// `"ssh"` | `"agent"` (migration 034). See `Store::set_host_transport`.
    pub transport: String,
    /// The host's org (work graph M5, migration 050): the boundary of its
    /// per-host token. `None` = no org (the token sees only unassigned work).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// When `claude_version` / `tmux_version` were last read from the host
    /// (migration 076). `None`: never — the values are whatever `add_host`
    /// or an older store left. Per-field default: an older hub omits it.
    #[serde(default)]
    pub claude_version_at: Option<i64>,
    /// Health sample from the last reachable probe (migration 077). All
    /// per-field default: an older hub omits them.
    #[serde(default)]
    pub disk_home_free_kb: Option<i64>,
    #[serde(default)]
    pub disk_home_total_kb: Option<i64>,
    #[serde(default)]
    pub disk_tmp_free_kb: Option<i64>,
    #[serde(default)]
    pub load_1m: Option<f64>,
    #[serde(default)]
    pub mem_avail_kb: Option<i64>,
    #[serde(default)]
    pub uptime_secs: Option<i64>,
    /// When the sample above was taken. `None`: never.
    #[serde(default)]
    pub health_at: Option<i64>,
    /// Last hook accepted from this host's own token.
    #[serde(default)]
    pub last_hook_at: Option<i64>,
    /// The fleet-agent version its last hello reported (agent hosts).
    #[serde(default)]
    pub agent_version: Option<String>,
    /// When `provision_hosts` last completed on this host (migration 078).
    #[serde(default)]
    pub provisioned_at: Option<i64>,
    /// `provisioned` but with content older than this build ships (or
    /// unknown). Computed from the stored fingerprint, never stored.
    #[serde(default)]
    pub provision_stale: bool,
    /// How many `unclaimed` sessions this host carries (multi-user M1,
    /// spec §4.3) — the ONE thing a caller ever learns about a row nobody
    /// can speak for. Computed per request by
    /// `service::hosts::list_hosts`, never stored.
    ///
    /// **`None` and `Some(0)` are different answers, and the difference is
    /// the privacy rule.** `Some(0)` says "this host has no unclaimed
    /// sessions"; `None` says "you are not being told", and it is what a
    /// hub with more than one person serves everybody (R5-d). Serving `0`
    /// there would be a claim about the host that the caller is not
    /// entitled to. On such a hub the count reaches a human through
    /// `fleet-hub session unclaimed` — shell access on the hub machine —
    /// and through no API at all.
    ///
    /// Plain `#[serde(default)]` and no `skip_serializing_if`: the key is
    /// always on the wire, as `null` when withheld, so an older hub (which
    /// omits it entirely) and a hub that withheld it read the same —
    /// `None`, which is the closed answer either way.
    #[serde(default)]
    pub unclaimed_sessions: Option<i64>,
    /// Which harnesses the asset catalog syncs on this host (multi-harness
    /// F3a, migration 089). `None` = auto: Claude, plus Codex where a scan
    /// finds it or fleet already manages Codex assets there. `Some` = exactly
    /// these (always including `claude`). Per-field default: an older hub
    /// omits it.
    #[serde(default)]
    pub harnesses: Option<Vec<String>>,
    /// What the last provisioning warned about, when it delivered the content
    /// but degraded part way — the `ag` launcher did not install, say
    /// (migration 092). Cleared by the next clean run; `None` is "nothing
    /// wrong with the last run". Returned to the caller AND kept here,
    /// because the call that produced it is long gone by the time an operator
    /// looks. Per-field default: an older hub omits it.
    #[serde(default)]
    pub provision_warning: Option<String>,
    /// Credential variables set on the host that outrank its `/login`, by
    /// name only (migration 111, [`crate::tmux::AUTH_OVERRIDE_VARS`]).
    /// `None`: never sampled, or the host could not tell. Per-field default:
    /// an older hub omits it.
    #[serde(default)]
    pub auth_overrides: Option<Vec<String>>,
    /// The host's Claude login profiles (migration 114, docs/accounts.md),
    /// by name, each with the account it is logged into when known. `None`:
    /// never read. Per-field default: an older hub omits it.
    #[serde(default)]
    pub claude_profiles: Option<Vec<HostProfileRow>>,
    /// Probe facts for the Hosts page (Orbit Fleet 4.6, migration 123):
    /// online CPUs, physical memory, the boot epoch the host states, the
    /// round trip of an empty command (`None` for `local`), and the disk
    /// fleet's worktrees hold with when that was last asked. All
    /// per-field default: an older hub omits them.
    #[serde(default)]
    pub cpu_count: Option<i64>,
    #[serde(default)]
    pub mem_total_kb: Option<i64>,
    #[serde(default)]
    pub boot_at: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    #[serde(default)]
    pub worktree_kb: Option<i64>,
    #[serde(default)]
    pub worktree_at: Option<i64>,
    /// Which agent CLIs the host has on its `PATH` (Orbit Fleet 12.4,
    /// migration 134). `None`: never sampled, or the host could not tell.
    /// Per-field default: an older hub omits it.
    #[serde(default)]
    pub agents_on_path: Option<Vec<String>>,
    /// When the host last answered a probe (migration 150). Unlike
    /// `last_pinged_at`, which a failed probe stamps too, this stays put
    /// while the host is unreachable. `None`: never answered. Per-field
    /// default: an older hub omits it.
    #[serde(default)]
    pub last_reachable_at: Option<i64>,
    /// Why the last probe failed: its `IpcError` code and a short message
    /// (migration 150). Cleared by the next probe the host answers.
    /// Per-field default: an older hub omits them.
    #[serde(default)]
    pub last_probe_error_code: Option<String>,
    #[serde(default)]
    pub last_probe_error: Option<String>,
}

/// One login profile on a host, as `hosts.claude_profiles` stores it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostProfileRow {
    pub name: String,
    /// The `accounts` row its `/login` is; `None` = not logged in yet.
    #[serde(default)]
    pub account_uuid: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// The volatile half of a host row, as `host:pinged` carries it (host
/// identity & health, task 2): a value that moves every pass must not turn
/// every ping into a full-row `host:probed`. Mirrors the migration-077
/// columns.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HostHealth {
    pub disk_home_free_kb: Option<i64>,
    pub disk_home_total_kb: Option<i64>,
    pub disk_tmp_free_kb: Option<i64>,
    pub load_1m: Option<f64>,
    pub mem_avail_kb: Option<i64>,
    pub uptime_secs: Option<i64>,
    pub health_at: Option<i64>,
    /// Sampled with the rest (migration 111). Per-field default: an older
    /// hub's ping omits it.
    #[serde(default)]
    pub auth_overrides: Option<Vec<String>>,
    /// Migration 123 (Orbit Fleet 4.6). Per-field default: an older hub's
    /// ping omits them.
    #[serde(default)]
    pub cpu_count: Option<i64>,
    #[serde(default)]
    pub mem_total_kb: Option<i64>,
    #[serde(default)]
    pub boot_at: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    #[serde(default)]
    pub worktree_kb: Option<i64>,
    /// Migration 134 (Orbit Fleet 12.4). Per-field default: an older hub's
    /// ping omits it.
    #[serde(default)]
    pub agents_on_path: Option<Vec<String>>,
}

impl HostHealth {
    pub fn of(row: &HostRow) -> Self {
        HostHealth {
            disk_home_free_kb: row.disk_home_free_kb,
            disk_home_total_kb: row.disk_home_total_kb,
            disk_tmp_free_kb: row.disk_tmp_free_kb,
            load_1m: row.load_1m,
            mem_avail_kb: row.mem_avail_kb,
            uptime_secs: row.uptime_secs,
            health_at: row.health_at,
            auth_overrides: row.auth_overrides.clone(),
            cpu_count: row.cpu_count,
            mem_total_kb: row.mem_total_kb,
            boot_at: row.boot_at,
            latency_ms: row.latency_ms,
            worktree_kb: row.worktree_kb,
            agents_on_path: row.agents_on_path.clone(),
        }
    }
}

/// What [`crate::store::Store::merge_host_alias`] did (host identity &
/// health, task 5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeReport {
    pub from: String,
    pub into: String,
    pub worktrees_moved: usize,
    pub sessions_moved: usize,
    pub sessions_dropped: usize,
    pub usage_days_merged: usize,
}

/// The only values `hosts.transport` may hold (migration 034). The single
/// definition `Store::set_host_transport` and `service::hosts::add_host`
/// both validate against, so the allowed set can't drift between them.
pub const HOST_TRANSPORTS: [&str; 2] = ["ssh", "agent"];

/// Columns every `HostRow` query selects, in [`map_host_row`] order.
pub(super) const HOST_COLUMNS: &str =
    "alias, ssh_alias, reachable, claude_version, tmux_version, hidden, \
     last_pinged_at, account_uuid, provisioned, transport, org_id, claude_version_at, \
     disk_home_free_kb, disk_home_total_kb, disk_tmp_free_kb, load_1m, mem_avail_kb, \
     uptime_secs, health_at, last_hook_at, agent_version, provisioned_at, provision_fingerprint, \
     harnesses, provision_warning, auth_overrides, claude_profiles, cpu_count, mem_total_kb, \
     boot_at, latency_ms, worktree_kb, worktree_at, agents_on_path, \
     last_reachable_at, last_probe_error_code, last_probe_error";

/// Map a row selected with [`HOST_COLUMNS`].
pub(super) fn map_host_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostRow> {
    let provisioned = row.get::<_, i64>(8)? != 0;
    // Task 6: a provisioned host whose stored content fingerprint is not
    // this build's (or unknown, an older provisioning) reads as stale.
    let fingerprint: Option<String> = row.get(22)?;
    Ok(HostRow {
        alias: row.get(0)?,
        ssh_alias: row.get(1)?,
        reachable: row.get::<_, i64>(2)? != 0,
        claude_version: row.get(3)?,
        tmux_version: row.get(4)?,
        hidden: row.get::<_, i64>(5)? != 0,
        last_pinged_at: row.get(6)?,
        account_uuid: row.get(7)?,
        provisioned,
        transport: row.get(9)?,
        org_id: row.get(10)?,
        claude_version_at: row.get(11)?,
        disk_home_free_kb: row.get(12)?,
        disk_home_total_kb: row.get(13)?,
        disk_tmp_free_kb: row.get(14)?,
        load_1m: row.get(15)?,
        mem_avail_kb: row.get(16)?,
        uptime_secs: row.get(17)?,
        health_at: row.get(18)?,
        last_hook_at: row.get(19)?,
        agent_version: row.get(20)?,
        provisioned_at: row.get(21)?,
        provision_stale: provisioned
            && fingerprint.as_deref() != Some(crate::service::provision::fingerprint()),
        // Never stored, and never derivable from a `hosts` row:
        // `service::hosts::list_hosts` fills it in per request, for the
        // callers R5-d entitles to it.
        unclaimed_sessions: None,
        // Migration 089. A value that is not a JSON string array reads as
        // auto rather than failing the whole row.
        harnesses: row
            .get::<_, Option<String>>(23)?
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()),
        // Migration 091: what the last provisioning warned about, if it
        // degraded. Cleared by the next clean run.
        provision_warning: row.get(24)?,
        // Migration 111. Same lenient read as `harnesses`.
        auth_overrides: row
            .get::<_, Option<String>>(25)?
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()),
        // Migration 114. Same lenient read.
        claude_profiles: row
            .get::<_, Option<String>>(26)?
            .and_then(|t| serde_json::from_str::<Vec<HostProfileRow>>(&t).ok()),
        // Migration 123 (Orbit Fleet 4.6).
        cpu_count: row.get(27)?,
        mem_total_kb: row.get(28)?,
        boot_at: row.get(29)?,
        latency_ms: row.get(30)?,
        worktree_kb: row.get(31)?,
        worktree_at: row.get(32)?,
        // Migration 134 (Orbit Fleet 12.4). Same lenient read.
        agents_on_path: row
            .get::<_, Option<String>>(33)?
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()),
        // Migration 150 (review r13).
        last_reachable_at: row.get(34)?,
        last_probe_error_code: row.get(35)?,
        last_probe_error: row.get(36)?,
    })
}

/// A host's recorded boot identity (migration 036): the kernel boot id and
/// tmux server pid from the last probe that could read them. Not part of
/// [`HostRow`] — reached only through [`Store::get_host_identity`] and
/// [`Store::set_host_identity`], since [`HostRow`] is serialised to the
/// frontend and this identity is backend-only bookkeeping for the reboot
/// safety net.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredIdentity {
    pub boot_id: Option<String>,
    pub tmux_server_pid: Option<i64>,
}

/// What one [`Store::mark_host_sessions_lost`] call touched, split by how:
/// `marked` rows were live and are now ghost (they changed on the wire and
/// were announced with `SessionUpdated`); `reclassified` rows were ALREADY a
/// `missing` ghost (a failed first post-loss pass pruned them routinely) and
/// only had their `lost_reason` upgraded to the verdict's reason. `lost_reason`
/// is now on the wire ([`SessionRow::lost_reason`]), so this IS a change the
/// frontend sees — a `SessionUpdated` is announced for them too.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkedLost {
    pub marked: Vec<SessionRow>,
    pub reclassified: Vec<SessionRow>,
}

impl MarkedLost {
    /// `true` when the call recorded no loss at all (neither list has a row).
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty() && self.reclassified.is_empty()
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AccountRow {
    pub uuid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_uuid: Option<String>,
    pub seat_tier: Option<String>,
    pub last_seen_at: Option<i64>,
    /// User-set short label (migration 028). Never overwritten by
    /// `Store::upsert_account` (the probe path) — only `set_account_nickname`
    /// changes it.
    pub nickname: Option<String>,
    /// `oauthAccount.hasExtraUsageEnabled` from the last probe (migration
    /// 028): hitting a usage limit spends pay-as-you-go money instead of
    /// blocking the account.
    pub has_extra_usage: bool,
}

/// Columns every `AccountRow` query selects, in [`map_account_row`] order.
pub(super) const ACCOUNT_COLUMNS: &str =
    "uuid, email, display_name, organization_name, organization_uuid, \
     seat_tier, last_seen_at, nickname, has_extra_usage";

/// Map a row selected with [`ACCOUNT_COLUMNS`].
pub(super) fn map_account_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        uuid: row.get(0)?,
        email: row.get(1)?,
        display_name: row.get(2)?,
        organization_name: row.get(3)?,
        organization_uuid: row.get(4)?,
        seat_tier: row.get(5)?,
        last_seen_at: row.get(6)?,
        nickname: row.get(7)?,
        has_extra_usage: row.get::<_, i64>(8)? != 0,
    })
}

/// One row of the append-only per-session event timeline (migration 013).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: i64,
    pub at: i64,
    pub kind: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// The conversation this event belongs to (migration 037); `None` for
    /// events not tied to one (ops, reconcile transitions).
    #[serde(default)]
    pub claude_session_id: Option<String>,
}

/// One per-host control-API bearer token (migration 018). `mode` is `full`
/// or `readonly`; see `mcp::auth::TokenMode`. The token itself is never sent
/// to the frontend — `HostTokenInfo` in `commands/mcp.rs` projects this row
/// without it.
#[derive(Debug, Clone)]
pub struct HostTokenRow {
    pub host_alias: String,
    pub token: String,
    /// When the host's first token was minted.
    pub created_at: i64,
    pub mode: String,
    /// The last request this token authenticated (migration 126), stamped
    /// at most once a minute; `None` never since.
    pub last_used_at: Option<i64>,
    /// When a fresh token last replaced the host's (migration 126).
    pub rotated_at: Option<i64>,
}

/// One paired client token (migration 032): a phone, a laptop browser. Unlike
/// `HostTokenRow`, only the SHA-256 of the token is stored — the plaintext is
/// shown once at pairing and never needs to be displayed again. `mode` is
/// `full` or `readonly`, mirroring `HostTokenRow::mode`. `revoked_at` is
/// `None` for a live token; a revoked row keeps its name for the audit trail.
/// `trusted_at` (migration 039) is `Some` for a client the operator vouches
/// for: its prompts are delivered unmarked (see `mcp::tools::apply_marker`).
#[derive(Debug, Clone)]
pub struct ClientTokenRow {
    pub id: i64,
    pub name: String,
    pub token_sha256: String,
    pub mode: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub trusted_at: Option<i64>,
    /// The org the client is bound to (migration 066), `None` unbound.
    pub org_id: Option<i64>,
    /// Set when the operator lets this client manage the asset catalog
    /// (migration 074, `fleet-hub client grant <name> assets`): the hub's
    /// `catalog_admin` tool answers it as it answers the master.
    pub assets_admin_at: Option<i64>,
    /// Whose device this is (multi-user M1, migration 100): the `people` row
    /// this token belongs to. `None` is the `person: None` privilege level —
    /// a caller nobody owns — which every gate must refuse and no scope can
    /// resolve; migration 098 leaves no live row in that state, and
    /// `Store::set_client_person` is the only thing that puts one back.
    /// Deliberately no foreign key: deleting a person leaves the token bound
    /// to an id nothing has (fail closed), never widened.
    pub person_id: Option<i64>,
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
    /// The sender's true end when `from_session_id` is `0` (migration 054):
    /// another fleet's session, addressed like `fleet-a/session/h/a1`. Never
    /// present for a local message — `skip_serializing_if` keeps a local
    /// row's wire shape byte-identical to before this field existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_addr: Option<String>,
    /// The same, for the recipient end (migration 054).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_addr: Option<String>,
}

/// Columns every `SessionMessage` query selects, in [`map_message_row`] order.
/// The last two are correlated subqueries onto `participants` resolving
/// `from_participant_id`/`to_participant_id` — a remote end's row (migration
/// 054) carries its address there, `NULL` for every local end. Every query
/// using this constant MUST select `FROM session_messages` unaliased so
/// these subqueries' unqualified `session_messages.*` references resolve.
pub(super) const MESSAGE_COLUMNS: &str =
    "id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to, \
     (SELECT address FROM participants p WHERE p.id = session_messages.from_participant_id), \
     (SELECT address FROM participants p WHERE p.id = session_messages.to_participant_id)";

/// One dispatched unit of work (migration 020). `state` is one of
/// [`TASK_STATES`]; `result` is the paragraph the worker printed after its
/// `FLEET_TASK_DONE_<nonce>` marker, `error` the failure/cancel reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskRow {
    pub id: i64,
    #[serde(default)]
    pub requester_session_id: Option<i64>,
    #[serde(default)]
    pub worker_session_id: Option<i64>,
    #[serde(default)]
    pub prompt: Option<String>,
    pub state: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub finished_at: Option<i64>,
    /// Per-task random tag baked into the completion marker. Never sent to
    /// the frontend (the marker must not be forgeable from the UI), so a row
    /// read back from a hub carries an empty one — which is right: a client
    /// must not be able to forge the marker either.
    #[serde(skip_serializing, default)]
    pub nonce: String,
    /// Worker's `claude_session_id` at dispatch (liveness check; internal).
    /// Not serialised either, hence `None` on a row read back from a hub.
    #[serde(skip_serializing, default)]
    pub worker_claude_session_id: Option<String>,
    /// When a session this task names was DELETED, so the task's ends no
    /// longer identify anybody (migration 101, multi-user M1 T9d).
    ///
    /// `sessions.id` is reused, and a task outlives its sessions, so an id
    /// kept past the row's death would make the task read as belonging to
    /// whoever holds that id next. The trigger NULLs the id and stamps this;
    /// [`crate::service::tasks::task_visible_in_scope_pure`] then answers
    /// `false` for everyone but the hub's own reader, because the sessions
    /// are over and there is nothing left for a person to drive.
    ///
    /// `#[serde(skip)]`, like `SessionRow::stale_demoted_at`: it is a fence
    /// input, decided where the rows live, and never a field a client reads
    /// or a hub client has to be told. So it is `None` on a row read back
    /// from a hub — harmless, because the hub already applied the fence.
    #[serde(skip)]
    pub detached_at: Option<i64>,
    /// The work item this task is an attempt at (`work_link { run }`,
    /// migration 110); `None` for a plain dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<i64>,
    /// 1, 2, … within (`work_item_id`, `role`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<i64>,
    /// implement | review | test | research | integrate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The worker's structured report (`tasks.result_json`, orchestration
    /// O3): what it SAYS it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<super::TaskReport>,
    /// What git said about its checkout when it finished
    /// (`tasks.evidence_json`): fleet's own reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<super::TaskEvidence>,
}

/// Where the catalog repo lives and its last-loaded HEAD (migration 030).
/// Singleton row (`id = 1`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CatalogConfigRow {
    pub repo_path: String,
    pub remote_url: Option<String>,
    pub head_commit: Option<String>,
    pub last_loaded_at: Option<i64>,
}

/// A catalog: a source with an owner (migration 090). `org_id: None` is the
/// personal catalog — exactly one such row exists (schema `CHECK`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogRow {
    pub id: i64,
    pub name: String,
    pub repo_path: String,
    pub remote_url: Option<String>,
    pub org_id: Option<i64>,
    pub head_commit: Option<String>,
    pub last_loaded_at: Option<i64>,
}

/// What `Store::remove_catalog` dropped along with the row (migration 093,
/// Rulings R13): the catalog's layer assignments, admissions and grants all
/// go by `ON DELETE CASCADE`. The checkout on disk is never touched.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogRemoval {
    pub id: i64,
    pub name: String,
    pub layer_rows: usize,
    pub admissions: usize,
    pub grants: usize,
    /// Open changeset cards that named the catalog, withdrawn with it
    /// (Assets M4, Rulings R26).
    #[serde(default)]
    pub cards: usize,
}

/// Drift state of one catalog asset on one host for one harness
/// (migration 030). `state` is one of in_sync | drifted | missing |
/// unmanaged | unsupported.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AssetInventoryRow {
    pub host_alias: String,
    pub harness: String,
    pub kind: String,
    pub name: String,
    pub state: String,
    pub catalog_hash: Option<String>,
    pub host_hash: Option<String>,
    pub scanned_at: i64,
    /// Whether the host's fleet manifest names this asset (migration 031).
    pub managed: bool,
    /// Its config looks like it carries a credential (migration 087).
    #[serde(default)]
    pub secret_like: bool,
    /// Fleet provisioned it: its own hooks, MCP entry or skills (087).
    #[serde(default)]
    pub fleet_owned: bool,
    /// Which catalog this asset came from (migration 091). `None` for an
    /// `unmanaged`/`orphan` row, which names nothing the catalog defines.
    #[serde(default)]
    pub catalog_id: Option<i64>,
    /// Assets M5 (migration 096, Rulings R4): on a `drifted` managed row,
    /// `host` (edited there) or `catalog` (the host copy is as fleet wrote
    /// it; the catalog moved on). `None` otherwise, or when the manifest
    /// entry cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_side: Option<String>,
}

/// A secret name known to the sync engine (migration 031). Never carries the
/// value: `list_secrets` is for display, `secret_values_for_host` resolves
/// actual values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecretRow {
    pub name: String,
    pub host_alias: Option<String>,
    pub updated_at: i64,
}

/// One completed sync apply (migration 031): kept so the UI can show the
/// last run's summary across restarts.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncRunRow {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: i64,
    pub summary_json: String,
}

/// The task state machine: `queued → running → done | failed | cancelled`.
pub const TASK_STATES: [&str; 5] = ["queued", "running", "done", "failed", "cancelled"];
/// States a task never leaves.
pub const TASK_TERMINAL_STATES: [&str; 3] = ["done", "failed", "cancelled"];

pub(super) const TASK_COLUMNS: &str =
    "id, requester_session_id, worker_session_id, prompt, state, result, \
     error, created_at, started_at, finished_at, nonce, worker_claude_session_id, \
     detached_at, work_item_id, attempt, role, result_json, evidence_json";

/// [`TASK_COLUMNS`] qualified with the `t.` alias for joined queries.
pub(super) fn task_columns_t() -> String {
    qualified(TASK_COLUMNS, "t")
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
        detached_at: row.get(12)?,
        work_item_id: row.get(13)?,
        attempt: row.get(14)?,
        role: row.get(15)?,
        report: super::task_report::decode(row.get(16)?),
        evidence: super::task_report::decode(row.get(17)?),
    })
}

/// One live session to upsert during a reconcile write-burst. `project_id`,
/// `account_uuid`, and `worktree_key` are PRE-RESOLVED by the caller (they
/// require reads — `find_project_id_for_path` / `get_session_account` /
/// `worktree_key_for_path` — that must run before the transaction opens).
#[derive(Default)]
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
    /// The session's active tmux pane (`%N`). `None` keeps the stored one.
    pub tmux_pane_id: Option<String>,
    /// The dialog `pane_intel::analyze` found this pass, if any. Governed by
    /// `intel_observed` exactly like `stuck_kind`: authoritative (and a
    /// `None` CLEARS a stale dialog) when the pane was captured this pass,
    /// preserved when it was not.
    pub pending_input: Option<PendingInput>,
    /// The pane captured this pass shows a live turn (`derived_status ==
    /// Working`, the spinner's "esc to interrupt"). Stamps
    /// `sessions.pane_working_at` (migration 081), which the stale-working
    /// sweep respects: a row whose pane is visibly working is never demoted,
    /// whatever the agents cadence let the status say this pass.
    pub pane_working: bool,
    /// The PR's evidence this pass read (result evidence), governed by
    /// `pr_observed` like `ci_status`: authoritative when the probe ran (a
    /// `None` clears it), preserved when it did not.
    pub pr_evidence: Option<crate::service::outcome::PrEvidence>,
}

/// All inputs for applying one host's probe result atomically. Consumed by
/// `Store::apply_host_reconcile`.
#[derive(Default)]
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
    /// Unix-epoch cutoff (see [`Store::ghost_and_clean`]'s `lost_ttl_cutoff`
    /// doc) at or above which a resumable mass-loss row (`claude_session_id
    /// IS NOT NULL`, `lost_reason IN ('host_reboot','tmux_server_gone')`,
    /// `lost_at >= cutoff`) is spared Phase 2's hard-delete. `None` (the
    /// default) disables the exemption entirely — today's behaviour.
    pub lost_ttl_cutoff: Option<i64>,
    /// Skip the tmux-keyed ghost/reap pass entirely this write (Task 6): set
    /// when this pass already mass-marked the host's sessions lost via
    /// `Store::mark_host_sessions_lost` (a reboot or a vanished tmux
    /// server), so the routine `keep`-set ghosting must not immediately
    /// re-ghost (and start the reap clock on) rows the mass-loss path just
    /// stamped with their specific `lost_reason`. `false` (the default) is
    /// today's behaviour — every reachable pass prunes.
    pub skip_prune: bool,
    /// Stamp `last_reconciled_at` with this value on every session the
    /// upsert writes (the live set this pass observed): the Task H freshness
    /// marker and the BE-3 ghost guard's evidence. Folded into the upsert
    /// rather than written by a second UPDATE, so a pass is one physical
    /// UPDATE per row; the stamp alone does not bump `row_version`
    /// (migration 065). `None` (the default, store-level tests) leaves the
    /// stored stamp alone.
    pub reconciled_at: Option<i64>,
}

/// Max `session_events` rows kept per session. Enforced on every
/// `insert_session_event` (oldest rows beyond the cap are pruned) so a
/// status-flapping session cannot grow the table without bound.
pub const SESSION_EVENTS_CAP: i64 = 500;

/// Max chars of a prompt kept in `sessions.last_prompt`.
pub const LAST_PROMPT_CHARS: usize = 200;

/// `?,?,…`: `n` positional placeholders for an `IN (…)` list.
pub(super) fn in_clause(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// The bind list for a statement whose fixed parameters `head` (write them
/// with `rusqlite::params![..]`) are followed by an `IN ({in_clause(tail.len())})`
/// list of `tail`.
pub(super) fn params_then<'a, T: rusqlite::ToSql>(
    head: &[&'a dyn rusqlite::ToSql],
    tail: &'a [T],
) -> Vec<&'a dyn rusqlite::ToSql> {
    head.iter()
        .copied()
        .chain(tail.iter().map(|t| t as &dyn rusqlite::ToSql))
        .collect()
}

/// Unix seconds, now. `pub` (not `pub(super)`) so a caller outside the store
/// — `fleet-hub peer remove`, in particular — has one definition to import
/// rather than a local duplicate; `service/gc.rs`, `service/health.rs` and a
/// few others still carry their own private copies predating this export.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Unix milliseconds, now: the unit of `changesets.applied_at` (Assets M4,
/// Rulings PF13), so two cards applied in the same second still order by
/// when they were applied. Everything else in the store keeps seconds
/// ([`now_unix`]).
pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
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
    conn.prepare_cached(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE tmux_name=?1 AND host_alias=?2"
    ))?
    .query_row(rusqlite::params![tmux_name, host_alias], map_session_row)
    .optional()
}

pub(super) fn fetch_session_by_id(
    conn: &Connection,
    id: i64,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    conn.prepare_cached(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"
    ))?
    .query_row(rusqlite::params![id], map_session_row)
    .optional()
}

/// Map a `session_messages` row selected with [`MESSAGE_COLUMNS`].
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
        from_addr: row.get(8)?,
        to_addr: row.get(9)?,
    })
}

pub(super) fn fetch_host(
    conn: &Connection,
    alias: &str,
) -> Result<Option<HostRow>, rusqlite::Error> {
    conn.prepare_cached(&format!("SELECT {HOST_COLUMNS} FROM hosts WHERE alias=?1"))?
        .query_row(rusqlite::params![alias], map_host_row)
        .optional()
}

pub(super) fn fetch_project(
    conn: &Connection,
    id: i64,
) -> Result<Option<ProjectRow>, rusqlite::Error> {
    conn.prepare_cached(&format!(
        "SELECT {PROJECT_COLUMNS} FROM projects WHERE id=?1"
    ))?
    .query_row(rusqlite::params![id], map_project_row)
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A desktop paired to a hub parses that hub's rows, and the hub may be
    /// older than the desktop. Migration 038 added `system` to `ProjectRow`;
    /// a hub built before it sends the key not at all. Without
    /// `serde(default)` the whole `list_projects` response fails with
    /// `E_PARSE` and the paired desktop shows no projects — shipped in
    /// 0.2.27 and seen against a live hub.
    ///
    /// The payload below is exactly what a pre-038 hub sends: `adopted` is
    /// there (027 long predates hub-client mode), `system` is not.
    #[test]
    fn a_project_row_from_a_hub_older_than_migration_038_still_parses() {
        let older_hub = r#"{
            "id": 3,
            "owner": "martin-janci",
            "repo": "claude-fleet",
            "base_path": "/srv/projects/claude-fleet",
            "last_session_at": 1758400000,
            "adopted": false
        }"#;
        let row: ProjectRow = serde_json::from_str(older_hub)
            .expect("a pre-038 hub's project row must still deserialize");
        assert_eq!(row.id, 3);
        assert!(
            !row.system,
            "a hub that does not know about system projects has none"
        );
    }

    /// The same row from a current hub still round-trips, so the default has
    /// not made the field write-only.
    #[test]
    fn a_project_row_from_a_current_hub_keeps_its_system_flag() {
        let current_hub = r#"{
            "id": 4,
            "owner": "fleet",
            "repo": "operator",
            "base_path": "/home/fleet/.claude-fleet/operator",
            "last_session_at": null,
            "adopted": false,
            "system": true
        }"#;
        let row: ProjectRow =
            serde_json::from_str(current_hub).expect("a current hub's project row parses");
        assert!(row.system, "the flag survives the wire when it is sent");
    }

    /// `stale_demoted_at` (migration 080) is read into `SessionRow` for the
    /// server's own turn-over checks, but never leaves it: not on the wire,
    /// not in a row event, not in a hub's JSON — and a row parsed from one
    /// reads `None`.
    #[test]
    fn stale_demoted_at_is_read_but_never_serialized() {
        let s = crate::store::Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("w", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = 'idle', stale_demoted_at = 7 WHERE id = ?1",
                [id],
            )
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.stale_demoted_at, Some(7));
        let json = serde_json::to_value(&row).unwrap();
        assert!(
            json.get("stale_demoted_at").is_none(),
            "stale_demoted_at must stay off the wire: {json}"
        );
        assert!(
            json.get("stale_working_at").is_some(),
            "its neighbour is on it"
        );
        let back: SessionRow = serde_json::from_value(json).unwrap();
        assert_eq!(back.stale_demoted_at, None);
        assert!(
            back.eq_ignoring_row_version(&row),
            "a demoted-only difference is not a visible one"
        );
    }

    /// One private, owned session, for the two wire tests below.
    fn owned_private_session(s: &Store) -> (i64, i64) {
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("w", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let person = s.create_person("ada", None).unwrap().id;
        s.conn_ref()
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' \
                 WHERE id = ?2",
                rusqlite::params![person, id],
            )
            .unwrap();
        (id, person)
    }

    /// The inverse of the test above, and the privacy-critical one
    /// (migration 099, spec §3.7): a `SessionRow` parsed from a frame with
    /// **no `visibility` key at all** — an older hub, or a replayed frame
    /// from before the column — reads [`VISIBILITY_UNCLAIMED`].
    ///
    /// This is what the named `#[serde(default = "visibility_unclaimed")]`
    /// buys. A bare `#[serde(default)]` on a `String` yields the empty
    /// string, which is neither value 097's `CHECK` admits and so matches no
    /// arm any fence writes — the one default that fails OPEN. The assertion
    /// is therefore on the VALUE, not on parsing having succeeded: parsing
    /// succeeds either way, which is exactly why this test has to exist.
    #[test]
    fn a_session_row_with_no_visibility_key_reads_unclaimed() {
        // Built from a real row and then stripped, rather than hand-written:
        // a hand-written payload drifts, and this must stay a test about the
        // two missing keys and nothing else. The row is private and owned
        // BEFORE the strip, so the assertion tells the serde default apart
        // from the value passing through — a row that was already
        // `unclaimed` would pass either way.
        let s = Store::open_in_memory().unwrap();
        let (id, _) = owned_private_session(&s);
        let row = s.get_session_by_id(id).unwrap().unwrap();
        let mut json = serde_json::to_value(&row).unwrap();
        let obj = json.as_object_mut().expect("a row is an object");
        let was = obj.remove("visibility");
        assert_eq!(
            was.as_ref().and_then(|v| v.as_str()),
            Some(VISIBILITY_PRIVATE),
            "the key must have been there to be removed"
        );
        obj.remove("owner_person_id");
        let back: SessionRow = serde_json::from_value(json)
            .expect("a pre-095 hub's session row must still deserialize");
        assert_eq!(
            back.visibility, VISIBILITY_UNCLAIMED,
            "a missing visibility reads `unclaimed`, never the empty string"
        );
        assert_ne!(
            back.visibility, "",
            "String::default() is the default that fails open"
        );
        assert_eq!(back.owner_person_id, None);
    }

    /// And the round trip: a current hub's values survive the wire, so the
    /// default has not quietly made the field write-only — the failure
    /// `a_project_row_from_a_current_hub_keeps_its_system_flag` pins for
    /// `ProjectRow`.
    ///
    /// Neither field carries `skip_serializing_if`, and `visibility` must
    /// not: `BroadcastEventBus::emit` runs `strip_nulls` before a frame
    /// enters the replay ring, so an unowned row's `owner_person_id` is
    /// ABSENT on the stream and indistinguishable from a pre-095 hub's.
    /// `visibility` is NOT NULL and is therefore the only key a fence can
    /// safely read (spec §3.7).
    #[test]
    fn visibility_and_owner_survive_the_wire() {
        let s = Store::open_in_memory().unwrap();
        let (id, person) = owned_private_session(&s);
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.owner_person_id, Some(person));
        assert_eq!(row.visibility, VISIBILITY_PRIVATE);
        let json = serde_json::to_value(&row).unwrap();
        let vis = json.get("visibility").and_then(|v| v.as_str());
        let own = json.get("owner_person_id").and_then(|v| v.as_i64());
        assert_eq!(vis, Some(VISIBILITY_PRIVATE), "the fence's key");
        assert_eq!(own, Some(person), "and the owner beside it");
        let back: SessionRow = serde_json::from_value(json).unwrap();
        assert!(back.eq_ignoring_row_version(&row));
    }

    /// `WorkSummary.effective_status` (native item status task 4, fix round
    /// 2): `crate::effective_status_sql!`'s SQL must agree with
    /// `service::work::status::effective_status`'s Rust precedence, checked
    /// end to end through `Store::get_session` — the real production path,
    /// not a standalone query. SQLite cannot call into Rust, so the logic
    /// is duplicated; these tests are what keeps the two from drifting.
    /// `status_category` stays tracker-only throughout (wire compat, see
    /// its doc on `WorkSummary`).
    mod effective_status_on_the_session_row {
        use super::*;

        fn seed(s: &Store, name: &str) -> i64 {
            s.upsert_host("h").unwrap();
            s.upsert_session(name, "h", None, None, 1, 1, "running", None)
                .unwrap()
        }

        fn mark_working(s: &Store, sid: i64, claude_session_id: &str) {
            s.set_claude_session_id(sid, claude_session_id).unwrap();
            s.set_claude_status_by_session_id(claude_session_id, "working")
                .unwrap();
        }

        fn tracker_item(s: &Store, key: &str, status_category: &str) -> i64 {
            let t = s
                .add_tracker("jira", "Jira", "https://x.atlassian.net")
                .unwrap();
            s.upsert_tracker_item(
                t.id,
                &TrackerItemWrite {
                    external_id: key.into(),
                    key: Some(key.into()),
                    title: "Ticket".into(),
                    status_name: "status".into(),
                    status_category: status_category.into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        }

        #[test]
        fn a_local_items_status_shows_through_effective_status_not_status_category() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            s.create_local_work_item(Some("LOC-1"), "Local work")
                .unwrap();
            s.link_session_work(sid, WorkTarget::Key("LOC-1"), "manual")
                .unwrap();
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(
                w.status_category, None,
                "wire compat: status_category stays tracker-only"
            );
            assert_eq!(w.effective_status.as_deref(), Some("todo"));
            assert_eq!(w.kind, "local");
        }

        #[test]
        fn a_tracker_items_status_shows_in_both_fields() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            let item = tracker_item(&s, "TK-1", "in_progress");
            s.link_session_work(sid, WorkTarget::Item(item), "manual")
                .unwrap();
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(w.status_category.as_deref(), Some("in_progress"));
            assert_eq!(w.effective_status.as_deref(), Some("in_progress"));
            assert_eq!(w.kind, "tracker");
        }

        #[test]
        fn a_working_session_lifts_effective_status_to_in_progress() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            s.create_local_work_item(Some("LOC-2"), "Local work")
                .unwrap();
            s.link_session_work(sid, WorkTarget::Key("LOC-2"), "manual")
                .unwrap();
            mark_working(&s, sid, "c-loc-2");
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(w.effective_status.as_deref(), Some("in_progress"));
        }

        /// The lift belongs to the ITEM, not "this row's own session": a
        /// DIFFERENT session's confirmed link to the same item, working,
        /// lifts it too — matching the Work view's `Graph`-based check,
        /// which also looks at every confirmed link, not only the primary.
        #[test]
        fn a_working_session_elsewhere_on_the_same_item_also_lifts_it() {
            let s = Store::open_in_memory().unwrap();
            let a = seed(&s, "a");
            let b = seed(&s, "b");
            let item = s.create_local_work_item(Some("LOC-3"), "Shared").unwrap();
            s.link_session_work(a, WorkTarget::Item(item.id), "manual")
                .unwrap();
            s.link_session_work(b, WorkTarget::Item(item.id), "manual")
                .unwrap();
            mark_working(&s, b, "c-loc-3-b");
            let w = s.get_session("a", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(
                w.effective_status.as_deref(),
                Some("in_progress"),
                "a's own session is idle, but b's confirmed link to the same item is working"
            );
        }

        #[test]
        fn a_persons_status_is_final_even_while_a_session_works_it() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            let item = s
                .create_local_work_item(Some("LOC-4"), "Local work")
                .unwrap();
            s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                .unwrap();
            s.set_item_status(item.id, "todo").unwrap();
            mark_working(&s, sid, "c-loc-4");
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(w.effective_status.as_deref(), Some("todo"));
        }

        /// The exact arm the SQL's `IN ('person', 'derived')` protects and
        /// nothing else in this crate exercises through the store's real
        /// write path (`set_item_status` only ever writes `'person'`):
        /// narrowing the SQL to `= 'person'` would read a PR-stamped `done`
        /// back as `in_progress` here, with every other test in this file
        /// still green.
        #[test]
        fn a_derived_done_stamp_is_final_even_while_a_session_works_it() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            let item = s
                .create_local_work_item(Some("LOC-5"), "Local work")
                .unwrap();
            s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                .unwrap();
            assert!(s.stamp_derived_done(item.id).unwrap(), "stamps once");
            mark_working(&s, sid, "c-loc-5");
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(
                w.effective_status.as_deref(),
                Some("done"),
                "a PR-stamped done must not read back as in_progress"
            );
        }

        #[test]
        fn a_working_session_never_lifts_a_tracker_item() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            let item = tracker_item(&s, "TK-2", "todo");
            s.link_session_work(sid, WorkTarget::Item(item), "manual")
                .unwrap();
            mark_working(&s, sid, "c-tk-2");
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(
                w.effective_status.as_deref(),
                Some("todo"),
                "a tracker item's column is its tracker's"
            );
        }

        #[test]
        fn a_bare_key_has_no_item_and_no_effective_status() {
            let s = Store::open_in_memory().unwrap();
            let sid = seed(&s, "dev");
            s.link_session_work(sid, WorkTarget::Key("BARE-1"), "manual")
                .unwrap();
            let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
            assert_eq!(w.item_id, None);
            assert_eq!(w.status_category, None);
            assert_eq!(w.effective_status, None);
            assert_eq!(w.kind, "ref");
        }

        /// Cross-check (fix round 3): the SQL macro and the Rust function
        /// must agree — not two independently hand-written literal
        /// expectations that could each be wrong the same way. Each arm
        /// reads the item's REAL fields back from the store
        /// (`Store::get_work_item`) and calls
        /// `service::work::status::effective_status` with them directly,
        /// then compares against what the SQL computed for the very same
        /// item through the real `Store::get_session` path — so a drift
        /// between the two implementations shows up as a mismatch, not as
        /// two tests that each independently typed the "right" answer.
        #[test]
        fn the_sql_macro_and_the_rust_function_agree_arm_by_arm() {
            use crate::service::work::status::effective_status;

            fn agree(
                s: &Store,
                session_name: &str,
                item_id: i64,
                has_working_session: bool,
                label: &str,
            ) {
                let raw = s.get_work_item(item_id).unwrap().unwrap();
                let want = effective_status(
                    &raw.status_category,
                    raw.status_set_by.as_deref(),
                    &raw.source,
                    has_working_session,
                );
                let got = s
                    .get_session(session_name, "h")
                    .unwrap()
                    .unwrap()
                    .work
                    .unwrap()
                    .effective_status;
                assert_eq!(got.as_deref(), want, "{label}");
            }

            // person
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "person");
                let item = s.create_local_work_item(Some("X-1"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                    .unwrap();
                s.set_item_status(item.id, "done").unwrap();
                agree(&s, "person", item.id, false, "person's setting");
            }
            // derived, with a working session (the arm `IN ('person',
            // 'derived')` protects and nothing but this cross-check and
            // `a_derived_done_stamp_is_final_even_while_a_session_works_it`
            // exercises).
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "derived");
                let item = s.create_local_work_item(Some("X-2"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                    .unwrap();
                s.stamp_derived_done(item.id).unwrap();
                mark_working(&s, sid, "c-x-2");
                agree(
                    &s,
                    "derived",
                    item.id,
                    true,
                    "derived stamp, working session",
                );
            }
            // a job's status ('task'), with a working session: final.
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "task");
                let item = s.create_local_work_item(Some("X-6"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                    .unwrap();
                s.conn_ref()
                    .execute(
                        "UPDATE work_items SET status_category = 'todo', status_set_by = 'task' WHERE id = ?1",
                        rusqlite::params![item.id],
                    )
                    .unwrap();
                mark_working(&s, sid, "c-x-6");
                agree(&s, "task", item.id, true, "job status, working session");
            }
            // live-lift on a local item
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "lift-local");
                let item = s.create_local_work_item(Some("X-3"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                    .unwrap();
                mark_working(&s, sid, "c-x-3");
                agree(&s, "lift-local", item.id, true, "live lift on a local item");
            }
            // live-lift attempted on a tracker item (must not lift)
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "lift-tracker");
                let item = tracker_item(&s, "TK-9", "todo");
                s.link_session_work(sid, WorkTarget::Item(item), "manual")
                    .unwrap();
                mark_working(&s, sid, "c-x-4");
                agree(
                    &s,
                    "lift-tracker",
                    item,
                    true,
                    "live lift attempted on a tracker item",
                );
            }
            // empty stored status: the schema (`NOT NULL DEFAULT 'todo'`)
            // and every writer this crate has prevent it in practice — a
            // narrow, deliberate raw `UPDATE` here (not the banned pattern
            // of faking a person's/tracker's status through one) is the
            // only way to drive this arm, simulating a hand-edited or
            // pre-migration row.
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "empty");
                let item = s.create_local_work_item(Some("X-5"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                    .unwrap();
                s.conn_ref()
                    .execute(
                        "UPDATE work_items SET status_category = '' WHERE id = ?1",
                        rusqlite::params![item.id],
                    )
                    .unwrap();
                agree(&s, "empty", item.id, false, "empty stored status");
            }
        }
    }
}
