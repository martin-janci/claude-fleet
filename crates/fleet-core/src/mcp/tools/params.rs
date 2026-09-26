//! Argument structs for the MCP tools whose parameters differ from the
//! `service::*Args` struct they end up calling (optional `session_id` OR
//! host+name addressing, `confirm_nonce` gates, MCP-side defaults, …). Each
//! derives `JsonSchema`, so the client sees a typed schema for every tool.
//!
//! Tools whose parameters are exactly a service `*Args` struct take that
//! struct directly (it derives `JsonSchema` with the MCP field docs and
//! keeps the original `*Params` schema title via `#[schemars(rename)]`).

use super::*;

// --- tool parameter structs ------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    /// Only this host.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only this project.
    #[serde(default)]
    pub project_id: Option<i64>,
    /// Store-level `status`: "running" or "ghost".
    #[serde(default)]
    pub status: Option<String>,
    /// One of working | blocked | completed | failed | stopped | idle (a
    /// null status never matches).
    #[serde(default)]
    pub claude_status: Option<String>,
    /// Include lost sessions (non-null `lost_at`).
    #[serde(default)]
    pub include_lost: bool,
    /// Slim rows (id, host_alias, tmux_name, project_id, worktree_id, status,
    /// claude_status, stuck_kind, lost_at, is_controller); false for full
    /// rows.
    #[serde(default = "default_true")]
    pub summary: bool,
    /// Max rows, after the filters. Omit for all.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Reconcile NOW before listing instead of serving the recent cache (a
    /// pass in flight is not duplicated).
    #[serde(default)]
    pub force: bool,
    /// Only sessions carrying this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Row projection: "phone" keeps the phone app's columns. Overrides
    /// `summary`; an unknown name is refused.
    #[serde(default)]
    pub view: Option<String>,
    /// true: only sessions that need a person (the row says why); false: only
    /// the rest.
    #[serde(default)]
    pub needs_attention: Option<bool>,
    /// Your session id: only what's new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WhoamiParams {
    /// Your tmux session name — `tmux display-message -p '#S'`.
    pub tmux_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewSessionParams {
    /// Host to create it on.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// Omit for the project root.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// tmux session name to create.
    pub name: String,
    /// Create a NEW worktree+branch of this name (not with `worktree_id`).
    #[serde(default)]
    pub new_worktree: Option<String>,
    /// Fork `new_worktree` from this branch; omitted or not found on the
    /// host: the default branch.
    #[serde(default)]
    pub base_branch: Option<String>,
    /// `"work"` (default): Claude Code; `"shell"`: a login shell, as
    /// new_shell_session.
    #[serde(default)]
    pub kind: Option<String>,
    /// Shell only: a command run once before the interactive shell.
    #[serde(default)]
    pub start_command: Option<String>,
    /// Sidebar label; omit to derive from the branch.
    #[serde(default)]
    pub friendly_name: Option<String>,
    /// Resume this Claude conversation (a resumable discover_lost_sessions
    /// candidate). `worktree_id` must be exactly the transcript's cwd, or an
    /// empty conversation starts under this id. Refused for shell sessions and
    /// for one a session on the host holds (use restore_host_sessions).
    #[serde(default)]
    pub resume_claude_session_id: Option<String>,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewShellSessionParams {
    /// Host to create it on.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// Omit for the project root.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// tmux session name to create.
    pub name: String,
    /// Create a NEW worktree+branch of this name (not with `worktree_id`).
    #[serde(default)]
    pub new_worktree: Option<String>,
    /// Fork `new_worktree` from this branch (default: the repo's default).
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Command run once before the interactive shell (e.g. `"pnpm dev"`);
    /// the pane outlives it.
    #[serde(default)]
    pub start_command: Option<String>,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct KillSessionParams {
    /// Fleet session id, or host_alias + name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux name (with `host_alias`).
    #[serde(default)]
    pub name: Option<String>,
    /// Kill even the fleet controller.
    #[serde(default)]
    pub force: bool,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ProvisionHostsParams {
    /// Mint fresh per-host tokens (invalidates each host's current one).
    #[serde(default)]
    pub rotate: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SafeKillSessionParams {
    /// Fleet session id, or host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteWorktreeParams {
    /// Worktree row id (from `list_worktrees`).
    pub worktree_id: i64,
    /// Delete even when an alive session uses it (else `E_WORKTREE_BUSY`).
    #[serde(default)]
    pub force: bool,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RenameSessionParams {
    /// Fleet session id, or host_alias + old_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `old_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Current tmux session name (with `host_alias`).
    #[serde(default)]
    pub old_name: Option<String>,
    /// New tmux session name.
    pub new_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetFriendlyNameParams {
    /// Fleet session id, or host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// 3–6 words on the current task; empty clears.
    pub friendly_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RestartSessionParams {
    /// Fleet session id, or host_alias + name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux name (with `host_alias`).
    #[serde(default)]
    pub name: Option<String>,
    /// Restart even the fleet controller.
    #[serde(default)]
    pub force: bool,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

// `recreate_session`'s arguments plus the operator's confirmation (M9.7).
// Plain comments: a doc comment would be served as the schema's description.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RecreateSessionParams {
    #[serde(flatten)]
    pub args: sessions::RecreateSessionArgs,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

// `spawn_review`'s arguments plus the operator's confirmation (M9.7).
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SpawnReviewParams {
    #[serde(flatten)]
    pub args: sessions::SpawnReviewArgs,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

// `restore_host_sessions`'s arguments plus the operator's confirmation
// (M9.7; a `dry_run` never needs one).
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RestoreHostSessionsParams {
    #[serde(flatten)]
    pub args: sessions::RestoreHostSessionsArgs,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

// `new_bg_session`'s arguments plus the operator's confirmation (M9.7).
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewBgSessionParams {
    #[serde(flatten)]
    pub args: crate::service::bg_sessions::NewBgSessionArgs,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SendPromptParams {
    /// Fleet session id, or host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// Text for the Claude REPL.
    pub prompt: String,
    /// Press Enter; false stages the text. Ignored with `keys`.
    #[serde(default = "default_true")]
    pub submit: bool,
    /// Omit the untrusted-input marker line. Master token only; agents'
    /// prompts are always marked.
    #[serde(default)]
    pub raw: bool,
    /// Deliver even to a blocked or stuck session: Enter on a permission
    /// prompt selects the highlighted answer.
    #[serde(default)]
    pub force: bool,
    /// Caller-chosen id: a repeat within ten minutes replays the first result
    /// (E_IN_FLIGHT while it runs) without delivering, even for another
    /// session.
    #[serde(default)]
    pub client_msg_id: Option<String>,
    /// Press a key instead: `Enter`, `Escape`, `C-c`, or `1`-`9` (that
    /// `pending_input` option). Unmarked, not recorded; `prompt` must be empty.
    #[serde(default)]
    pub keys: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct BroadcastPromptParams {
    /// Only this host.
    pub host: Option<String>,
    /// Only this project.
    pub project_id: Option<i64>,
    /// Only this claude_status: working | blocked | completed | failed |
    /// stopped | idle.
    pub status: Option<String>,
    /// Text for every matching session.
    pub prompt: String,
    /// Press Enter after the text (default true).
    pub submit: Option<bool>,
    /// Omit the untrusted-input marker (master token only).
    #[serde(default)]
    pub raw: bool,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CaptureSessionParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Scrollback rows to include; omit for the visible pane.
    pub scrollback_lines: Option<u32>,
    /// Keep the LAST n lines (default 200, 0 = no cap); a cut adds a one-line
    /// note.
    pub max_lines: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionActivityParams {
    /// Fleet session id.
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionHistoryParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Max events, newest first (default 50).
    pub limit: Option<i64>,
    /// Your session id: only what's new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionConversationsParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Max rows, newest first (default 20, max 500).
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UsageReportParams {
    /// Only this host (a per-host token: its own, else E_FORBIDDEN).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only usage changed in the last N seconds (per-day totals too); omit:
    /// all, last 30 days.
    #[serde(default)]
    pub since_secs: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageParams {
    /// Your session id, shown to the recipient as the sender.
    pub from_session_id: i64,
    /// Recipient's fleet session id.
    pub to_session_id: i64,
    /// Recipient's fleet address; wins over to_session_id.
    #[serde(default)]
    pub to_addr: Option<String>,
    /// Free text, seen verbatim.
    pub body: String,
    /// `message` (default), `task`, `reply`, `alert`, …
    pub kind: Option<String>,
    /// Also type it into the recipient's pane under a `[msg #id from
    /// name@host]:` header (the inbox row is written regardless).
    #[serde(default)]
    pub deliver: bool,
    /// With `deliver`: press Enter after the text.
    #[serde(default = "default_true")]
    pub submit: bool,
    /// Omit the untrusted-input marker line. Master token only.
    #[serde(default)]
    pub raw: bool,
    /// Inbox message this answers; must exist and involve the sender
    /// (E_NOTFOUND / E_INVALID).
    #[serde(default)]
    pub reply_to: Option<i64>,
    /// Nudge an idle recipient's pane; a blocked one is never typed into.
    #[serde(default)]
    pub wake: bool,
    /// Caller-chosen id; a repeat replays the first result, never delivers
    /// twice (E_IN_FLIGHT meanwhile).
    #[serde(default)]
    pub client_msg_id: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForReplyParams {
    /// Your fleet session id.
    pub session_id: i64,
    /// Only messages newer than this id.
    #[serde(default)]
    pub after_message_id: Option<i64>,
    /// Default 120, max 600.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct InboxParams {
    /// Your own session id.
    pub session_id: i64,
    /// Only unread rows.
    #[serde(default)]
    pub unread_only: bool,
    /// Max rows, newest first (default 50).
    pub limit: Option<i64>,
    /// Mark returned rows read; false peeks.
    #[serde(default = "default_true")]
    pub mark_read: bool,
    /// Slim rows (80-char body preview); false for full bodies.
    #[serde(default = "default_true")]
    pub summary: bool,
    /// Your session id: only what's new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListProjectsParams {
    /// Slim rows; false for the nested worktree tree (~10x larger: pair it
    /// with `limit`).
    #[serde(default = "default_true")]
    pub summary: bool,
    /// Max rows, newest-registered first. Omit for all.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Only projects holding a live session (applied before `limit`).
    #[serde(default)]
    pub has_sessions: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListWorktreesParams {
    /// Only this project.
    #[serde(default)]
    pub project_id: Option<i64>,
    /// Only this host.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Slim rows (occupants as a COUNT, 0 = free to delete); false adds the
    /// path and the occupant sessions.
    #[serde(default = "default_true")]
    pub summary: bool,
    /// Max rows after the filters (default 100, 0 = no cap).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListHostWorktreesParams {
    /// Host to scan.
    pub host_alias: String,
    /// Project id.
    pub project_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PeerStatusParams {
    /// Peer's fleet session id.
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RegisterSelfParams {
    /// Your fleet session id, or host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Your host (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Your tmux name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct QuickRepliesParams {
    /// The whole chip list to store, replacing what is there ([] restores the
    /// built-in defaults). Omit to read the current list instead.
    #[serde(default)]
    pub set: Option<Vec<crate::service::quick_replies::QuickReply>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetClipboardParams {
    /// Host whose clipboard to overwrite.
    pub host_alias: String,
    /// Text, at most 64 KiB.
    pub content: String,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForSessionParams {
    /// Fleet session id.
    pub session_id: i64,
    /// "idle" or "turn_gt" (see the tool).
    pub until: String,
    /// Turn number for until=turn_gt.
    #[serde(default)]
    pub turn: Option<i64>,
    /// Default 120, max 600.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionTranscriptParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Every turn after this turn_seq (e.g. send_prompt's turn_seq_before);
    /// omit for the last turn.
    #[serde(default)]
    pub since_turn: Option<i64>,
    /// Keeps the END. Default 8000, max 64000.
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// Your session id: only what's new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
}

/// `repo_diff`'s own params. Deliberately NOT `repo_read::RepoFileArgs`:
/// that struct is shared with `repo_file` and routed desktop→hub, so a new
/// field there would leak onto `repo_file` and change a wire struct.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoDiffParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
    /// Your session id: only what's new since your last read.
    #[serde(default)]
    pub fresh_for: Option<i64>,
}

impl From<&RepoDiffParams> for crate::service::repo_read::RepoFileArgs {
    fn from(p: &RepoDiffParams) -> Self {
        Self {
            session_id: p.session_id,
            path: p.path.clone(),
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionConversationParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Latest turns (default 10, max 100); the character budget scales.
    #[serde(default)]
    pub turns: Option<usize>,
    /// An earlier conversation of this session (from session_conversations;
    /// else E_INVALID).
    #[serde(default)]
    pub claude_session_id: Option<String>,
    /// Timeline events (default 50, max 200, 0 = none).
    #[serde(default)]
    pub events_limit: Option<i64>,
    /// Last turn_seq you saw: the turns since, plus the running one. `turns`
    /// wins; out of range gives the default window.
    #[serde(default)]
    pub since_turn: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RunPromptParams {
    /// Fleet session id.
    pub session_id: i64,
    /// The prompt (marked untrusted unless raw).
    pub prompt: String,
    /// Default 120, max 600.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Default 8000, max 64000.
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// Omit the untrusted-input marker (master token only).
    #[serde(default)]
    pub raw: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewWorkerSpec {
    /// Host for the worker.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// tmux name; omit to let new_session pick one.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DispatchTaskParams {
    /// Existing worker session; exactly one of this or new_worker.
    #[serde(default)]
    pub worker_session_id: Option<i64>,
    /// Spawn a worker via new_session.
    #[serde(default)]
    pub new_worker: Option<NewWorkerSpec>,
    /// The work to do (fleet appends the done-marker instruction).
    pub prompt: String,
    /// Your session id: the requester and the worker's parent; gets the
    /// result in its inbox.
    #[serde(default)]
    pub requester_session_id: Option<i64>,
    /// Omit the untrusted-input marker (master token only).
    #[serde(default)]
    pub raw: bool,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForTaskParams {
    /// Task id (from dispatch_task / list_tasks).
    pub task_id: i64,
    /// Default 120, max 600.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    /// Only tasks dispatched by this session.
    #[serde(default)]
    pub requester_session_id: Option<i64>,
    /// queued | running | done | failed | cancelled.
    #[serde(default)]
    pub state: Option<String>,
    /// Max rows (default 50, 1..=500).
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CancelTaskParams {
    /// Task id to cancel.
    pub task_id: i64,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetSessionTagsParams {
    /// Fleet session id, or host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// The full list (replaces; empty clears).
    pub tags: Vec<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoLogParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Every branch (default true), not just HEAD.
    pub all: Option<bool>,
    /// Default 50, max 2000.
    pub limit: Option<u32>,
    /// Commits to skip (pagination).
    pub skip: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct MoveSessionParams {
    /// The work session to move.
    pub session_id: i64,
    /// Target host (reachable, provisioned).
    pub target_host_alias: String,
    /// Leave the source running once the target is confirmed.
    #[serde(default)]
    pub keep_source: bool,
    /// Refuse a dirty worktree (E_MOVE_DIRTY) or unpushed branch
    /// (E_MOVE_UNPUSHED) instead of carrying them.
    #[serde(default)]
    pub strict: bool,
    /// Replace what an earlier attempt left in the target worktree.
    #[serde(default)]
    pub clean_target: bool,
    /// Preview; no changes.
    #[serde(default)]
    pub dry_run: bool,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
    /// `now`, `idle` (wait, then move) or `cancel` (end a wait).
    #[serde(default)]
    pub when: crate::service::move_session::When,
    /// Carry a live work link across the org boundary on the target (a
    /// warning) instead of E_FORBIDDEN. Default false.
    #[serde(default)]
    pub force_cross_org: bool,
}

impl MoveSessionParams {
    /// The service args this call becomes. The one place the tool's
    /// parameters are mapped, so a flag cannot reach the schema and stop
    /// short of the engine: `session_id` is the row the handler resolved (a
    /// host-scoped caller may address a session it is allowed to see), every
    /// other field travels as given.
    pub(super) fn into_args(
        self,
        session_id: i64,
    ) -> crate::service::move_session::MoveSessionArgs {
        crate::service::move_session::MoveSessionArgs {
            session_id,
            target_host_alias: self.target_host_alias,
            keep_source: self.keep_source,
            strict: self.strict,
            clean_target: self.clean_target,
            dry_run: self.dry_run,
            when: self.when,
            force_cross_org: self.force_cross_org,
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ResolveMoveParams {
    /// The TARGET session of a partial move.
    pub session_id: i64,
    /// "finish" or "undo".
    pub action: String,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepairSessionParams {
    /// Fleet session id, or host_alias + name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The session's host (with `name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux name (with `host_alias`).
    #[serde(default)]
    pub name: Option<String>,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

// --- asset catalog ---------------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ScanAssetsParams {
    /// Only this host; omit for every reachable one.
    #[serde(default)]
    pub host_alias: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PlanSyncParams {
    /// Only this host; omit for every reachable one.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only this kind: skill | agent | hook | mcp_server | plugin_ref.
    #[serde(default)]
    pub kind: Option<String>,
    /// Only this asset.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ApplySyncParams {
    /// From plan_sync; expires after 10 minutes.
    pub plan_id: String,
    /// Apply what a missing `${NAME}` secret does not block, instead of
    /// refusing the run (`E_SECRET_MISSING`).
    #[serde(default)]
    pub force_partial: bool,
    /// Nonce from an approved `E_CONFIRM_REQUIRED`.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetSecretParams {
    /// The catalog's `${NAME}`; `[A-Z0-9_]+`.
    pub name: String,
    /// The secret's value. Never returned or logged.
    pub value: String,
    /// A per-host override; omit for the global value.
    #[serde(default)]
    pub host_alias: Option<String>,
}

// --- paired clients (phones, browsers) -------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PairClientParams {
    /// Shown in `list_clients` and in the untrusted-input marker on what it
    /// sends. Rules: see the tool.
    pub name: String,
    /// `full` (default), `readonly` or `peer`; see the tool.
    #[serde(default)]
    pub mode: Option<String>,
    /// Code lifetime: default 600, max 3600; it also dies on first use.
    #[serde(default)]
    pub ttl_s: Option<u64>,
    /// A device you vouch for: its prompts reach agents unmarked.
    #[serde(default)]
    pub trusted: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetClientTrustParams {
    /// The live client's name.
    pub name: String,
    /// true: unmarked from its next call on; false: marked again.
    pub trusted: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetSettingParams {
    /// e.g. "work.journal_days".
    pub key: String,
    /// An object or array is stored as its JSON.
    pub value: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListClientsParams {
    /// Include revoked clients (kept for the audit trail).
    #[serde(default)]
    pub include_revoked: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RevokeClientParams {
    /// The live client's name.
    pub name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ResolvePreviewParams {
    /// The host.
    pub host_alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetHostLayersParams {
    /// The host.
    pub host_alias: String,
    /// The one role layer; null clears.
    #[serde(default)]
    pub role: Option<String>,
    /// Context layers, in application order.
    #[serde(default)]
    pub contexts: Vec<String>,
}
