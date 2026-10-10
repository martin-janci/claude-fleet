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
    /// `claude --model` (alias or id).
    #[serde(default)]
    pub model: Option<String>,
    /// low|medium|high|xhigh|max.
    #[serde(default)]
    pub effort: Option<String>,
    /// Login profile (~/.claude-profiles/<name>).
    #[serde(default)]
    pub profile: Option<String>,
    /// claude (default), codex or shell.
    #[serde(default)]
    pub agent: Option<String>,
    /// Opaque id (1–64 of A-Za-z0-9_-) its `start:progress` events carry.
    #[serde(default)]
    pub start_token: Option<String>,
    /// The person chose a login past `accounts.pause_at`.
    #[serde(default)]
    pub over_limit_ok: Option<bool>,
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
pub struct ShellTerminalsParams {
    /// The session whose terminals to list, open or close.
    pub session_id: i64,
    /// list (default), open or close.
    #[serde(default)]
    pub action: crate::service::sessions::ShellTerminalAction,
    /// The terminal's number, 1 to 9: close needs it; open without it
    /// takes the lowest free one.
    #[serde(default)]
    pub n: Option<u32>,
    /// open only: worktree (default) or home.
    #[serde(default)]
    pub at: crate::service::sessions::ShellTerminalStart,
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
pub struct ForgetProjectParams {
    /// The project's id.
    pub project_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ProvisionHostsParams {
    /// Mint fresh per-host tokens (invalidates each host's current one).
    #[serde(default)]
    pub rotate: bool,
    /// One host alias; every active host when omitted.
    #[serde(default)]
    pub host: Option<String>,
    /// Skills, CLAUDE.md block and hooks only: no token, no ~/.claude.json
    /// rewrite, no tunnel.
    #[serde(default)]
    pub content_only: bool,
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
    /// Switch login profile ("" = host login).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RewindConversationParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Keep the transcript strictly before this turn's prompt_uuid (from
    /// session_conversation). Required to rewind; omit to fork keeping all
    /// of it.
    #[serde(default)]
    pub anchor_uuid: Option<String>,
    /// "rewind" restarts this session on the truncated copy; "fork" leaves
    /// it alone and starts a new session on the copy.
    pub mode: String,
    /// Fork into a new worktree+branch of this name, off HEAD.
    #[serde(default)]
    pub new_worktree: Option<String>,
    /// Approved confirmation; required for "rewind".
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
pub struct QueuePromptParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Text typed as a new turn.
    pub prompt: String,
    /// Omit the untrusted-input marker line. Master token only.
    #[serde(default)]
    pub raw: bool,
    /// Operator only: the nonce a person approved.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
    /// Send later: not typed before this unix second.
    #[serde(default)]
    pub not_before: Option<i64>,
    /// Hold it while the session's account is at or past accounts.pause_at.
    #[serde(default)]
    pub until_limit_reset: bool,
    /// Drop it instead if the session is archived before it goes out.
    #[serde(default)]
    pub skip_if_archived: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct QueuedPromptsParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Take back this waiting prompt instead of listing.
    #[serde(default)]
    pub cancel: Option<i64>,
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
    /// Press a key instead; `1`-`9` picks that `pending_input` option
    /// (toggles it when `multi`). Not recorded; `prompt` must be empty.
    #[serde(default)]
    #[schemars(extend("enum" = crate::tmux::NamedKey::all_names()))]
    pub keys: Option<String>,
    /// Operator only: the nonce a person approved.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
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

// `add_project`'s arguments plus the operator's confirmation (M9.7; only a
// `create_remote` carrying the service's token ever needs one).
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct AddProjectParams {
    #[serde(flatten)]
    pub args: crate::service::add_project::AddProjectArgs,
    /// Approved confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListGithubReposParams {
    /// Host whose `gh` login lists the repositories.
    pub host_alias: String,
    /// A GitHub user or organisation to list instead of the login's own.
    #[serde(default)]
    pub owner: Option<String>,
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
    /// With `set`: the list last read; E_CONFLICT if it changed since.
    #[serde(default)]
    pub expected: Option<Vec<crate::service::quick_replies::QuickReply>>,
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
pub struct SessionSummarySinceParams {
    /// Fleet session id.
    pub session_id: i64,
    /// The start of the window, unix seconds: the turns that ended at or
    /// after it are summarised.
    pub since: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionToolDetailParams {
    /// Fleet session id.
    pub session_id: i64,
    /// The tool call's id (a tool item's `id` in session_conversation).
    pub tool_use_id: String,
    /// An earlier conversation of this session (from session_conversations;
    /// else E_INVALID).
    #[serde(default)]
    pub claude_session_id: Option<String>,
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
    /// Operator only: the nonce a person approved.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
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
pub struct DecideRelatedSessionParams {
    /// The session whose `related_session` proposal this answers.
    pub session_id: i64,
    /// The proposal's `run_id`.
    pub run_id: i64,
    /// true: Link (same work, stays listed); false: Not related.
    pub linked: bool,
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
pub struct ListAssetsParams {
    /// Every catalog, not only personal (master/unbound full only).
    #[serde(default)]
    pub all_catalogs: bool,
}

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
    /// Plan remote hosts that have no layers (they would get the whole
    /// catalog). Off by default.
    #[serde(default)]
    pub allow_unlayered: Option<bool>,
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
    /// `full` (default), `readonly`, `peer` or `updater`; see the tool.
    #[serde(default)]
    pub mode: Option<String>,
    /// Code lifetime: default 600, max 3600; it also dies on first use.
    #[serde(default)]
    pub ttl_s: Option<u64>,
    /// A device you vouch for: its prompts reach agents unmarked.
    #[serde(default)]
    pub trusted: bool,
    /// Bind to this org: it reads only that org's and unassigned work.
    #[serde(default)]
    pub org_id: Option<i64>,
    /// Whose device it is. Default: this hub's owner.
    #[serde(default)]
    pub person: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetClientTrustParams {
    /// The live client's name.
    pub name: String,
    /// true: unmarked from its next call on; false: marked again.
    pub trusted: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GetSettingsParams {
    /// true: every key's metadata (label, help, bounds, danger) and value.
    pub describe: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetSettingParams {
    /// e.g. "work.recent_days".
    pub key: String,
    /// An object or array is stored as its JSON.
    pub value: serde_json::Value,
    /// Only propose it: a person applies or rejects it in Settings.
    #[serde(default)]
    pub propose: bool,
    /// With propose: why, shown to the person (≤500 chars).
    #[serde(default)]
    pub why: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SettingHistoryParams {
    /// A registered setting, e.g. "work.recent_days".
    pub key: String,
    /// Newest first, 1-100; 20 when unset.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ControlHandoffsParams {
    /// How many, newest first (default 50, at most 500).
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ControlRouteParams {
    /// propose | follow
    pub action: String,
    /// propose: the message just sent in Control.
    #[serde(default)]
    pub text: Option<String>,
    /// follow: the run the receipt came from.
    #[serde(default)]
    pub run_id: Option<i64>,
    /// follow: the option kept or picked (m<id>, s<id>, control).
    #[serde(default)]
    pub chosen: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct AnswerMcpConfirmParams {
    /// The confirm_nonce the waiting call was handed.
    pub nonce: String,
    /// true runs the call on its retry; false refuses it.
    pub approved: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DecideSettingProposalsParams {
    /// Proposal ids to apply.
    #[serde(default)]
    pub accept: Vec<i64>,
    /// Proposal ids to reject.
    #[serde(default)]
    pub reject: Vec<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuideAction {
    Catalog,
    Validate,
    Propose,
    List,
    Decide,
    Remove,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GuideParams {
    /// catalog, validate, propose, list, decide or remove.
    pub action: GuideAction,
    /// validate / propose: fleet.page/1, layout guide.
    #[serde(default)]
    pub spec: Option<serde_json::Value>,
    /// propose: why (≤500 chars).
    #[serde(default)]
    pub why: Option<String>,
    /// decide: proposal id.
    #[serde(default)]
    pub id: Option<i64>,
    /// decide: approve or reject.
    #[serde(default)]
    pub approve: Option<bool>,
    /// remove: guide id.
    #[serde(default)]
    pub page_id: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema, Default)]
pub struct AskListFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// pending, answered, declined, cancelled or expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct AskParams {
    /// A fleet.form/1 form for your own session's chat (docs/forms.md).
    #[serde(default)]
    pub form: Option<serde_json::Value>,
    /// form / draft: why you ask (≤500 chars).
    #[serde(default)]
    pub why: Option<String>,
    /// The form's JSON so far (≤16 KiB), drawn in until `form`; "" drops it.
    #[serde(default)]
    pub draft: Option<String>,
    /// Wait again on this pending form_id.
    #[serde(default)]
    pub wait: Option<String>,
    /// Withdraw this form_id.
    #[serde(default)]
    pub cancel: Option<String>,
    /// List forms: {session_id?, state?}.
    #[serde(default)]
    pub list: Option<AskListFilter>,
    /// Read this form_id.
    #[serde(default)]
    pub get: Option<String>,
    /// Answer this form_id with `values`.
    #[serde(default)]
    pub answer: Option<String>,
    /// answer: field name → value.
    #[serde(default)]
    pub values: Option<serde_json::Map<String, serde_json::Value>>,
    /// Decline this form_id, with an optional `note`.
    #[serde(default)]
    pub decline: Option<String>,
    /// decline: a short reason.
    #[serde(default)]
    pub note: Option<String>,
    /// form / wait: default and max 600.
    #[serde(default)]
    pub timeout_s: Option<u64>,
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

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SetHostHarnessesParams {
    /// The host.
    pub host_alias: String,
    /// null = auto; else harness ids, "claude" required.
    #[serde(default)]
    pub harnesses: Option<Vec<String>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CatalogAdminParams {
    /// config|configure|load|get_asset|template|create_asset|update_asset|
    /// delete_asset|add_resource_bytes|remove_resource|lint_asset|lint_all|
    /// commit_pending|push|repo_status|inventory|import_host|plan_sync|
    /// apply_sync|last_sync|list_secrets|set_secret|delete_secret|
    /// list_layers|resolve_preview|propose_layers|set_host_layers|set_host_harnesses|
    /// layer_template|write_layer|delete_layer|list_catalogs|add_catalog|
    /// remove_catalog|admit_catalog|unadmit_catalog|asset_history|drift_diff
    pub action: String,
    /// The desktop command's own argument object.
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    /// apply_sync: nonce of an approved E_CONFIRM_REQUIRED.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
    /// Catalog name (default personal) for config|load|list_layers|
    /// set_host_layers and the authoring actions; configure is personal
    /// only. remove/admit/unadmit_catalog: only the name in args.
    /// add/remove_catalog: master only. Fleet-wide actions refuse it.
    #[serde(default)]
    pub catalog: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ChangesetsParams {
    /// list|propose|propose_layer|apply|undo|dismiss|reject_item
    pub action: String,
    /// The card: list shows it in full; apply|undo|dismiss|reject_item need it.
    #[serde(default)]
    pub id: Option<i64>,
    /// apply: items (default all pending but "needs a look"; drift: one).
    /// reject_item: required.
    #[serde(default)]
    pub positions: Option<Vec<i64>>,
    /// apply of a rollout or restore: nonce of an approved E_CONFIRM_REQUIRED.
    #[serde(default)]
    pub confirm_nonce: Option<String>,
    /// propose_layer: {op: create|rename|move, catalog?, layer, to?, member?, axis?, description?, members?}.
    #[serde(default)]
    pub change: Option<crate::service::catalog::changesets::LayerChange>,
}

#[derive(serde::Deserialize, schemars::JsonSchema, Default)]
pub struct UpdateStatusParams {
    /// client:<id>, agent:<alias> or hub:self: its whole decision.
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema, Clone)]
pub struct UpdatePolicyParams {
    /// list | set | clear.
    pub action: String,
    /// Defaults to this device's org.
    #[serde(default)]
    pub org_id: Option<i64>,
    /// set, clear: hub | agent | desktop | android | ios.
    #[serde(default)]
    pub component: Option<String>,
    /// set: manual | notify | automatic.
    #[serde(default)]
    pub mode: Option<String>,
    /// set: the org's floor version.
    #[serde(default)]
    pub minimum: Option<String>,
    /// set: HH:MM-HH:MM UTC, "" = any time.
    #[serde(default)]
    pub window: Option<String>,
    /// set: pin the org to this release.
    #[serde(default)]
    pub version: Option<String>,
    /// set: the pin is required.
    #[serde(default)]
    pub mandatory: Option<bool>,
    /// set: why, for the dashboard.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAdminParams {
    /// pin | unpin | update_now | refresh | rollout_start | rollout_pause | rollout_resume |
    /// rollout_abort | set_policy | clear_policy.
    pub action: String,
    /// hub | agent | desktop | android | ios.
    #[serde(default)]
    pub component: Option<String>,
    /// One target (hub:self, agent:<alias>, client:<id>); absent = every target.
    #[serde(default)]
    pub target: Option<String>,
    /// pin: the release to serve.
    #[serde(default)]
    pub version: Option<String>,
    /// pin: required, not merely offered.
    #[serde(default)]
    pub mandatory: Option<bool>,
    /// pin, rollout_pause: why, for the dashboard.
    #[serde(default)]
    pub reason: Option<String>,
    /// rollout_start: percents ending at 100 (default [10,50,100]).
    #[serde(default)]
    pub waves: Option<Vec<u8>>,
    /// rollout_start: failure ratio that pauses it (default 0.2).
    #[serde(default)]
    pub halt_failure_ratio: Option<f64>,
    /// set_policy, clear_policy.
    #[serde(default)]
    pub org_id: Option<i64>,
    /// set_policy: manual | notify | automatic.
    #[serde(default)]
    pub mode: Option<String>,
    /// set_policy: the org's floor version.
    #[serde(default)]
    pub minimum: Option<String>,
    /// set_policy: HH:MM-HH:MM UTC, "" = any time.
    #[serde(default)]
    pub window: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RoutinesParams {
    /// list | get | runs | failing | save | delete | set_enabled | skip_next | run_now.
    pub action: String,
    /// Every action but list, and save of a change.
    #[serde(default)]
    pub routine_id: Option<i64>,
    /// save: the whole routine.
    #[serde(default)]
    pub routine: Option<crate::service::routines::RoutineInput>,
    /// set_enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// skip_next: false takes the skip back (default true).
    #[serde(default)]
    pub skip: Option<bool>,
    /// runs: how many, newest first (default 20, at most 200).
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct LibraryParams {
    /// list | add | remove.
    pub action: String,
    /// list: only this session's files; add: the session they were put beside.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// list: only this host's files.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// list: at most this many, newest first (default and cap 200).
    #[serde(default)]
    pub limit: Option<usize>,
    /// add: upload | attachment.
    #[serde(default)]
    pub kind: Option<String>,
    /// add: the files, already on the session's host.
    #[serde(default)]
    pub files: Option<Vec<crate::service::library::LibraryFile>>,
    /// remove: the row's id.
    #[serde(default)]
    pub id: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RemoveDownloadParams {
    /// The download's id (`list_downloads`).
    pub id: i64,
}

// --- sharing a session (multi-user M1, T12) --------------------------------
//
// **Every one of these addresses the session by ROW ID and by nothing else.**
// No `host_alias` + `tmux_name` fallback, which every other session-addressed
// tool offers: that pair is REUSABLE — a killed session's tmux name is taken
// by the next one started on that host — so a share or a claim resolved by
// name could be written against a different session than the one the caller
// read. An id names one row for the life of the row.

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionShareParams {
    /// The session to share, by fleet row id.
    pub session_id: i64,
    /// A person's name, or none with `org` (an org's name).
    #[serde(default)]
    pub person: String,
    /// Or an org.
    #[serde(default)]
    pub org: Option<String>,
    /// watch (read it), answer (also answer its dialogs) or drive (also
    /// prompt it). Nothing else — "own" is not a grantable level.
    pub level: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionGrantParams {
    /// The session, by fleet row id.
    pub session_id: i64,
    /// A person's name, or none with `org`.
    #[serde(default)]
    pub person: String,
    /// Or an org.
    #[serde(default)]
    pub org: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionAccessParams {
    /// The session, by fleet row id.
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionClaimParams {
    /// The session to claim, by fleet row id (`fleet-hub session unclaimed`
    /// counts them per host).
    pub session_id: i64,
    /// Whose it becomes, by person name. The person must already exist.
    pub person: String,
}

/// `debug_devices`: one tool, by `action`, so the served definition stays
/// one entry.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DebugDevicesParams {
    /// list | scan | claim | release | run | install | logs | screenshot |
    /// boot | shutdown | configure | forget (configure, forget: a person).
    pub action: String,
    /// An id, label, name or serial (`host/<that>`).
    #[serde(default)]
    pub device: Option<String>,
    /// scan: one host. boot: `name`'s host. install: where `path` is.
    #[serde(default)]
    pub host: Option<String>,
    /// boot: a stopped device from scan's `bootable`.
    #[serde(default)]
    pub name: Option<String>,
    /// list: scan first.
    #[serde(default)]
    pub refresh: Option<bool>,
    /// run: e.g. ["shell","pm","list","packages"].
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// run: ≤ 600 s.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// install: .apk, .app or .ipa.
    #[serde(default)]
    pub path: Option<String>,
    /// install: allow an Android downgrade.
    #[serde(default)]
    pub downgrade: Option<bool>,
    /// logs: ≤ 2000.
    #[serde(default)]
    pub lines: Option<u32>,
    /// logs (simulator).
    #[serde(default)]
    pub since_s: Option<u32>,
    /// logs: logcat filterspec or simulator predicate.
    #[serde(default)]
    pub filter: Option<String>,
    /// logs: only lines holding this.
    #[serde(default)]
    pub contains: Option<String>,
    /// claim: seconds (1800).
    #[serde(default)]
    pub claim_s: Option<i64>,
    /// claim: what for.
    #[serde(default)]
    pub note: Option<String>,
    /// configure: "" clears.
    #[serde(default)]
    pub label: Option<String>,
    /// configure: other hosts of its org may use it.
    #[serde(default)]
    pub shared: Option<bool>,
}

/// `runs`: one tool, by `action` (only `list` today), so the Automation
/// screen's later actions stay one entry.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RunsParams {
    /// list.
    pub action: String,
    /// Started at or after (unix s).
    #[serde(default)]
    pub since: Option<i64>,
    /// Started before (unix s).
    #[serde(default)]
    pub until: Option<i64>,
    /// operator | task | mission | jev | routine, or a fleet `claude -p`
    /// run: planner | summary | commit_message | release_note |
    /// morning_brief | brief | watch_summary | triage.
    #[serde(default)]
    pub kind: Option<String>,
    /// ok | failed | needs_person | nothing_to_do | running.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Only this org's.
    #[serde(default)]
    pub org_id: Option<i64>,
    /// Only this mission's.
    #[serde(default)]
    pub mission_id: Option<i64>,
    /// Runs that ran in or acted on it.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// One routine's fires.
    #[serde(default)]
    pub routine_id: Option<i64>,
    /// ≤ 200 (50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Rows to skip.
    #[serde(default)]
    pub offset: Option<i64>,
}
