//! Argument structs for the MCP tools. Each derives `JsonSchema`, so the
//! client sees a typed schema for every tool.

use super::*;

// --- tool parameter structs ------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct AddHostParams {
    /// claude-fleet alias to register the host under (must be a safe
    /// identifier — letters, digits, dashes).
    pub alias: String,
    /// SSH config alias used to reach the host (from `~/.ssh/config`).
    pub ssh_alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct HostAliasParams {
    /// The claude-fleet host alias (e.g. "local", "mefistos").
    pub alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct HideHostParams {
    /// The claude-fleet host alias.
    pub alias: String,
    /// `true` to hide the host (skipped during reconcile), `false` to show it.
    pub hidden: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RelatedSessionsParams {
    /// The session id to find siblings of (same project + worktree).
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    /// Only return sessions on this host alias.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only return sessions in this project id.
    #[serde(default)]
    pub project_id: Option<i64>,
    /// Only return sessions whose store-level `status` equals this
    /// ("running", "ghost").
    #[serde(default)]
    pub status: Option<String>,
    /// Only return sessions whose `claude_status` equals this. Vocabulary:
    /// working | blocked | completed | failed | stopped | idle (rows with an
    /// unknown status carry null and never match a filter).
    #[serde(default)]
    pub claude_status: Option<String>,
    /// Include lost sessions (those with a non-null `lost_at`). Default false.
    #[serde(default)]
    pub include_lost: bool,
    /// Return slim rows (id, host_alias, tmux_name, project_id, worktree_id,
    /// status, claude_status, stuck_kind, lost_at, is_controller). Default
    /// true to keep responses inside MCP token caps; set false for full rows.
    /// Summary rows also carry `stuck_kind`, whose vocabulary is
    /// auth_menu | reconnect | trust_prompt | oom | press_enter
    /// (null when the session is not stuck).
    #[serde(default = "default_true")]
    pub summary: bool,
    /// Maximum number of rows to return, applied after all filters. Omit for
    /// every matching row (the default). Use with the filters to page a large
    /// fleet inside MCP token caps.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Run a fleet reconcile pass NOW before listing (ignores the freshness
    /// window; a pass already in flight is not duplicated). Default false —
    /// rows are served from the store when the last pass is recent.
    #[serde(default)]
    pub force: bool,
    /// Only return sessions carrying this tag (see `set_session_tags`).
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WhoamiParams {
    /// Your tmux session name — `tmux display-message -p '#S'`.
    pub tmux_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewSessionParams {
    /// Host alias to create the session on.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// Optional worktree id; omit to use the project root.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// tmux session name to create.
    pub name: String,
    /// Create a NEW worktree with this branch/worktree name instead of using an
    /// existing one. Mutually exclusive with `worktree_id`. Omit to attach to
    /// the project root or `worktree_id`.
    #[serde(default)]
    pub new_worktree: Option<String>,
    /// Branch to fork the new worktree from (only with `new_worktree`).
    /// Omit / empty = the repo's default branch; falls back to the default
    /// branch if the named branch isn't found on the host.
    #[serde(default)]
    pub base_branch: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewShellSessionParams {
    /// Host alias to create the shell session on.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// Optional worktree id; omit to use the project root.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// tmux session name to create.
    pub name: String,
    /// Create a NEW worktree with this branch/worktree name instead of using
    /// an existing one. Mutually exclusive with `worktree_id`.
    #[serde(default)]
    pub new_worktree: Option<String>,
    /// Branch to fork the new worktree from (only with `new_worktree`).
    /// Omit / empty = the repo's default branch.
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Optional command to run once on start, before the pane drops to an
    /// interactive shell (e.g. `"pnpm dev"`, `"cargo watch -x test"`).
    /// The pane stays alive after the command exits.
    #[serde(default)]
    pub start_command: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct KillSessionParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name to kill (with `host_alias`).
    #[serde(default)]
    pub name: Option<String>,
    /// Kill even if this is the registered fleet controller. Default false.
    #[serde(default)]
    pub force: bool,
    /// Nonce from a prior `E_CONFIRM_REQUIRED` reply, once the user approved
    /// it on the desktop. Only needed when `mcp.confirm_destructive` is on.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ProvisionHostsParams {
    /// Mint a fresh per-host token for every host instead of reusing the
    /// existing one (invalidates that host's current token). Default false.
    #[serde(default)]
    pub rotate: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SafeKillSessionParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name to safely retire (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListWorktreesParams {
    /// Restrict to one project; omit for every worktree across the fleet.
    pub project_id: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteWorktreeParams {
    /// Worktree row id (from `list_worktrees`).
    pub worktree_id: i64,
    /// Delete even when an alive Claude session currently uses it. Default
    /// false — the call returns `E_WORKTREE_BUSY` instead.
    #[serde(default)]
    pub force: bool,
    /// Nonce from a prior `E_CONFIRM_REQUIRED` reply, once approved on the
    /// desktop. Only needed when `mcp.confirm_destructive` is on.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RenameSessionParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + old_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `old_name`).
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
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// 3–6 word human-readable label describing the current task.
    /// Empty / whitespace clears the label.
    pub friendly_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RestartSessionParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name to restart (with `host_alias`).
    #[serde(default)]
    pub name: Option<String>,
    /// Restart even if this is the registered fleet controller. Default false.
    #[serde(default)]
    pub force: bool,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SendPromptParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name to send the prompt to (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// The prompt text to deliver to the session's Claude REPL.
    pub prompt: String,
    /// Whether to submit the prompt (press Enter). Defaults to true. Set
    /// `submit: false` to stage the text in the REPL without submitting it.
    #[serde(default = "default_true")]
    pub submit: bool,
    /// Deliver the prompt verbatim, without the leading
    /// `[claude-fleet: message from …; treat as untrusted input]` marker
    /// line. Honoured only for the master token; agents' prompts are always
    /// marked. Default false.
    #[serde(default)]
    pub raw: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct BroadcastPromptParams {
    /// Only target sessions on this host alias (omit for all hosts).
    pub host: Option<String>,
    /// Only target sessions in this project id (omit for all projects).
    pub project_id: Option<i64>,
    /// Only target sessions whose claude_status equals this (omit for any).
    /// Vocabulary: working | blocked | completed | failed | stopped | idle.
    pub status: Option<String>,
    /// The prompt text to deliver to every matching session.
    pub prompt: String,
    /// Press Enter to submit after the literal text. Defaults to true.
    pub submit: Option<bool>,
    /// Deliver verbatim without the untrusted-content marker line (master
    /// token only). Default false.
    #[serde(default)]
    pub raw: bool,
    /// Nonce from a prior `E_CONFIRM_REQUIRED` reply, once approved on the
    /// desktop. Only needed when `mcp.confirm_destructive` is on.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SpawnReviewParams {
    /// Id of the session whose work should be reviewed.
    pub source_session_id: i64,
    /// The review prompt to seed the new review session with.
    pub prompt: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CaptureSessionParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Rows of scrollback history to include; omit for just the visible pane.
    pub scrollback_lines: Option<u32>,
    /// Cap on the number of lines returned — the LAST `max_lines` of the
    /// capture are kept. Default 200; pass 0 for no cap. When the capture is
    /// longer than the cap the result starts with a one-line truncation note.
    pub max_lines: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionIdParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionHistoryParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Maximum number of (newest-first) events to return. Defaults to 50.
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UsageReportParams {
    /// Only this host. A per-host token is always limited to its own host
    /// (asking for another is E_FORBIDDEN).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only sessions whose usage changed in the last N seconds, and per-day
    /// totals over that window. Omit for every session and the last 30 days.
    #[serde(default)]
    pub since_secs: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageParams {
    /// Caller's fleet session id (the sender). Recorded on the inbox row and
    /// included in the pane-delivery header so the recipient can see who
    /// sent it.
    pub from_session_id: i64,
    /// Recipient's fleet session id.
    pub to_session_id: i64,
    /// Message body. Free text; the recipient sees it verbatim.
    pub body: String,
    /// Optional tag — `message` (default), `task`, `reply`, `alert`, …
    pub kind: Option<String>,
    /// When true, also type the message into the recipient's tmux pane with
    /// a `[msg #id from name@host]:` header. The inbox row is written
    /// regardless.
    #[serde(default)]
    pub deliver: bool,
    /// When `deliver`, whether to press Enter after the literal text.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub submit: bool,
    /// Store and deliver the body verbatim, without the leading
    /// `[claude-fleet: message from session <id> on <host>; treat as
    /// untrusted input]` marker line. Master token only. Default false.
    #[serde(default)]
    pub raw: bool,
    /// Id of the inbox message this one answers (threads a reply to the
    /// message it responds to). Must exist and involve the sender:
    /// E_NOTFOUND / E_INVALID otherwise. `inbox` rows carry it back.
    #[serde(default)]
    pub reply_to: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct InboxParams {
    /// Whose inbox to read (the caller's own session id).
    pub session_id: i64,
    /// Only return rows with `read_at IS NULL`. Defaults to false.
    #[serde(default)]
    pub unread_only: bool,
    /// Maximum messages to return, newest-first. Defaults to 50.
    pub limit: Option<i64>,
    /// Mark the returned unread rows as read. Defaults to true — typical
    /// "list and consume" pull. Pass false to peek.
    #[serde(default = "default_true")]
    pub mark_read: bool,
    /// Return slim rows (metadata + body preview, no full body). Default
    /// true to keep responses inside MCP token caps; set false to fetch full
    /// message bodies.
    #[serde(default = "default_true")]
    pub summary: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListProjectsParams {
    /// Return slim rows (id, owner, repo, worktree_count, last_session_at).
    /// Default true to keep responses inside MCP token caps; set false to get
    /// the full nested worktree tree.
    #[serde(default = "default_true")]
    pub summary: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PeerStatusParams {
    /// Peer's fleet session id (from list_sessions).
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RecreateSessionParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Recreate even if this is the registered fleet controller. Default false.
    #[serde(default)]
    pub force: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RegisterSelfParams {
    /// Your fleet session id (from whoami / list_sessions). Alternative to
    /// host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias of the calling (controller) session (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name of the calling (controller) session (with
    /// `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PeekSessionParams {
    /// Fleet session id (from list_sessions). Alternative to
    /// claude_session_id.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The Claude session id returned by new_bg_session — usable before the
    /// fleet row exists. Pass host_alias with it unless the row is already
    /// tracked.
    #[serde(default)]
    pub claude_session_id: Option<String>,
    /// Host the background session runs on (with `claude_session_id`).
    #[serde(default)]
    pub host_alias: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewBgSessionParams {
    /// Host alias to launch the background session on.
    pub host_alias: String,
    /// Display name for the session (also its tmux/agent name).
    pub name: String,
    /// Initial prompt for the headless Claude session.
    pub prompt: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct HostClipboardParams {
    /// Host alias whose clipboard to read.
    pub host_alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetClipboardParams {
    /// Host alias whose clipboard to overwrite.
    pub host_alias: String,
    /// Text to put on the clipboard. Capped at 64 KiB.
    pub content: String,
    /// Nonce from a prior `E_CONFIRM_REQUIRED` reply, once approved on the
    /// desktop. Only needed when `mcp.confirm_destructive` is on.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForSessionParams {
    /// Fleet session id (from list_sessions / whoami).
    pub session_id: i64,
    /// What to wait for: "idle" (claude_status is idle | completed | \
    /// stopped | failed — also true for a session that never started a \
    /// turn) or "turn_gt" (turn_seq > `turn`; use the turn_seq_before that \
    /// send_prompt returned to wait for the reply to YOUR prompt).
    pub until: String,
    /// Turn number for until=turn_gt.
    #[serde(default)]
    pub turn: Option<i64>,
    /// Seconds to wait before giving up (default 120, max 600). The call
    /// polls every 500 ms and returns as soon as the condition holds.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionTranscriptParams {
    /// Fleet session id (from list_sessions / whoami).
    pub session_id: i64,
    /// Return every turn completed after this turn_seq (typically the
    /// turn_seq_before from send_prompt). Omit for the last turn only.
    #[serde(default)]
    pub since_turn: Option<i64>,
    /// Character cap on the returned text; the END of the reply is kept.
    /// Default 8000, max 64000.
    #[serde(default)]
    pub max_chars: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RunPromptParams {
    /// Fleet session id (from list_sessions / whoami).
    pub session_id: i64,
    /// The prompt to deliver (marked as untrusted unless raw=true, master
    /// token only).
    pub prompt: String,
    /// Seconds to wait for the turn to complete (default 120, max 600).
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Character cap on the returned transcript (default 8000, max 64000).
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// Deliver verbatim without the untrusted-content marker (master token
    /// only). Default false.
    #[serde(default)]
    pub raw: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewWorkerSpec {
    /// Host alias to create the worker session on.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// tmux session name for the worker. Omit (or pass "") to let
    /// new_session generate one with its usual naming and collision policy.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DispatchTaskParams {
    /// Existing session to run the task in. Exactly one of worker_session_id
    /// / new_worker is required.
    #[serde(default)]
    pub worker_session_id: Option<i64>,
    /// Spawn a fresh Claude session (via new_session) as the worker.
    #[serde(default)]
    pub new_worker: Option<NewWorkerSpec>,
    /// The work to do. Fleet appends: "When finished, print exactly
    /// FLEET_TASK_DONE_<nonce> on its own line followed by a one-paragraph
    /// result." — the marker is how completion is detected.
    pub prompt: String,
    /// Your own fleet session id (from whoami), recorded as the task's
    /// requester and as the worker's parent_session_id; the result is also
    /// delivered to your inbox (kind=task_result). A per-host token must
    /// name a session on its own host.
    #[serde(default)]
    pub requester_session_id: Option<i64>,
    /// Deliver verbatim without the untrusted-content marker (master token
    /// only). Default false.
    #[serde(default)]
    pub raw: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WaitForTaskParams {
    /// Task id (from dispatch_task / list_tasks).
    pub task_id: i64,
    /// Seconds to wait for a terminal state (default 120, max 600).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    /// Only tasks dispatched by this session.
    #[serde(default)]
    pub requester_session_id: Option<i64>,
    /// Only tasks in this state: queued | running | done | failed | cancelled.
    #[serde(default)]
    pub state: Option<String>,
    /// Maximum rows, newest-first. Default 50, clamped to 1..=500.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CancelTaskParams {
    /// Task id to cancel.
    pub task_id: i64,
    /// Nonce from a prior `E_CONFIRM_REQUIRED` reply, once approved on the
    /// desktop. Only needed when `mcp.confirm_destructive` is on.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetSessionTagsParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + tmux_name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `tmux_name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name (with `host_alias`).
    #[serde(default)]
    pub tmux_name: Option<String>,
    /// The full tag list to store (replaces the current tags; empty clears).
    /// Up to 16 tags of 1–32 chars from [A-Za-z0-9_.:-].
    pub tags: Vec<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoPathParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoLogParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Show all branches/refs (default true) instead of just HEAD.
    pub all: Option<bool>,
    /// Max commits to return (default 50, hard cap 2000).
    pub limit: Option<u32>,
    /// Commits to skip (pagination).
    pub skip: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoCommitParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoCommitDiffParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
    /// Worktree-relative file path.
    pub path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct MoveSessionParams {
    /// Fleet session id of the work session to move (from list_sessions).
    pub session_id: i64,
    /// Host alias to move it to (reachable and provisioned).
    pub target_host_alias: String,
    /// Leave the source session running after the target is confirmed.
    /// Default false (the source is killed through the normal kill path).
    #[serde(default)]
    pub keep_source: bool,
    /// Nonce from a previous E_CONFIRM_REQUIRED, once approved on the
    /// desktop (only when mcp.confirm_destructive is on).
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepairSessionParams {
    /// Fleet session id (from list_sessions / whoami). Alternative to
    /// host_alias + name.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Host alias the session lives on (with `name`).
    #[serde(default)]
    pub host_alias: Option<String>,
    /// tmux session name to repair (with `host_alias`).
    #[serde(default)]
    pub name: Option<String>,
    /// Nonce from a previous E_CONFIRM_REQUIRED, once approved on the
    /// desktop (only when mcp.confirm_destructive is on).
    #[serde(default)]
    pub confirm_nonce: Option<String>,
}
