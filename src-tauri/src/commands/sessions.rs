//! Tauri IPC wrappers for tmux session management. Logic lives in
//! `service::sessions`; this file only adapts `tauri::State` to plain
//! references, and chooses the backend.
//!
//! # What routes and what refuses
//!
//! A command routes when the desktop's arguments map **one to one** onto the
//! hub tool's parameters. Where they do not, it refuses with `E_LOCAL_ONLY`
//! and says why, rather than calling a tool that would quietly drop a field.
//! A silent argument mismatch on a session mutation is the worst thing this
//! layer can do: it would succeed, and do something other than what the user
//! asked.
//!
//! Two commands are refused for that reason rather than for want of a tool:
//!
//! - `repair_session` with `explicit: false` — the tool always runs the
//!   **explicit** repair (which may unregister a stale worktree entry, adopt
//!   a moved checkout and recreate a branch). The desktop's automatic
//!   pre-attach check has no counterpart, and turning it into an explicit
//!   repair would be destructive by surprise, so only `explicit: true` (the
//!   Repair workspace button) routes.
//! - `session_activity` — the hub's pane and transcript reads answer a
//!   different shape than `ActivityProbe`.
//!
//! `new_session` used to be refused here too: `NewSessionArgs` carried
//! `kind`, `start_command` and `friendly_name`, and `NewSessionParams`
//! carried none of them. #146 added the three fields to the tool's
//! params (optional — absent means today's MCP behaviour), so the desktop's
//! arguments now map one-to-one and it routes unconditionally.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::bg_sessions::{
    self, DismissAgentArgs, NewBgSessionArgs, PurgeProjectArgs,
};
use fleet_core::service::decide::lost_target::LostTarget;
use fleet_core::service::repair::{self, RepairReport};
use fleet_core::service::rewind::{self, RewindArgs};
use fleet_core::service::safe_kill::{
    self, DiscardKillSessionArgs, InspectSafeKillArgs, SafeKillInspection, SafeKillSessionArgs,
};
use fleet_core::service::sessions::{
    self, AdoptSessionArgs, DiscoverLostSessionsArgs, DismissGhostSessionArgs, KillSessionArgs,
    LostCandidate, LostTargetArgs, NewSessionArgs, PlaceTranscriptArgs, PlacedTranscript,
    RecreateSessionArgs, RenameSessionArgs, RestartSessionArgs, RestoreHostSessionsArgs,
    RestoreReport, SendPromptArgs, SetFriendlyNameArgs, SpawnReviewArgs, TouchSessionViewedArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::{DeferredPromptRow, SessionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

/// `force: true` (the sidebar Refresh button) always runs a fleet reconcile
/// pass; the default serves stored rows while the last pass is within the
/// configured interval.
#[tauri::command]
pub async fn list_sessions(
    force: Option<bool>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    routed::list_sessions(&backend, force, &store, &ssh).await
}

#[tauri::command]
pub async fn new_session(
    args: NewSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    routed::new_session(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn kill_session(
    args: KillSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    routed::kill_session(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn safe_kill_session(
    args: SafeKillSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::safe_kill_session(&backend, args, &store, &ssh).await
}

/// Pre-flight check used by the UI before showing the safe-remove dialog:
/// returns dirty files + pushed-state so we can either skip the Claude prompt
/// (clean+pushed) or warn the user about what would be lost.
#[tauri::command]
pub async fn inspect_safe_kill(
    args: InspectSafeKillArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SafeKillInspection, IpcError> {
    backend.refuse_local_only("inspect_safe_kill")?;
    safe_kill::inspect_safe_kill(args, &store, &ssh).await
}

/// Direct remove: skip the Claude prompt, drop the worktree, kill the
/// session. `force` is the discard-dirty toggle — pass `false` for the
/// clean+pushed fast path (any unexpected dirty state errors out), and `true`
/// when the user explicitly chose "discard & kill" from the dialog.
#[tauri::command]
pub async fn discard_kill_session(
    args: DiscardKillSessionArgs,
    force: bool,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    backend.refuse_local_only("discard_kill_session")?;
    safe_kill::discard_kill_session(args, force, &store, &ssh).await
}

#[tauri::command]
pub async fn rename_session(
    args: RenameSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::rename_session(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn set_session_friendly_name(
    args: SetFriendlyNameArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::set_session_friendly_name(&backend, args, &store).await
}

/// The session on screen (redesign 2.3): its finished turns read as seen.
#[tauri::command]
pub async fn touch_session_viewed(
    args: TouchSessionViewedArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::touch_session_viewed(&backend, args, &store).await
}

#[tauri::command]
pub async fn restart_session(
    args: RestartSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::restart_session(&backend, args, &store, &ssh).await
}

/// `RewindArgs` is also `service::rewind`'s own parameter type (it derives
/// `Serialize`/`Deserialize` for exactly this reason), so the frontend's
/// `mode: 'rewind' | 'fork'` deserialises straight into `RewindMode` with no
/// second params struct in between.
///
/// Needs `reg`, unlike `restart_session` beside it: the Fork arm spawns a new
/// session through `sessions::new_session`, which hard-requires a
/// `CancellationRegistry` for its remote git/clone step.
#[tauri::command]
pub async fn rewind_conversation(
    args: RewindArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    routed::rewind_conversation(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn send_prompt(
    args: SendPromptArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    // A key into a standalone session (a dialog answer): re-read just that
    // pane afterwards, as the hub's own `send_prompt` does, so the answered
    // dialog leaves the row now rather than on the next tick.
    let key_target = (args.keys.is_some() && backend.hub().is_none())
        .then(|| (args.host_alias.clone(), args.tmux_name.clone()));
    routed::send_prompt(&backend, args, &store, &ssh).await?;
    if let Some((host, name)) = key_target {
        let id = store.lock().ok().and_then(|s| {
            s.find_sessions_by_tmux_name(&name, Some(&host))
                .ok()
                .and_then(|rows| rows.first().map(|r| r.id))
        });
        if let Some(id) = id {
            fleet_core::service::sessions::spawn_dialog_followup(
                Arc::clone(&*store),
                Arc::clone(&*ssh),
                id,
                fleet_core::service::sessions::DialogFollowup::Answered,
            );
        }
    }
    Ok(())
}

/// Send prompt's "busy sessions get it when they are idle" (step 5.10).
#[tauri::command]
pub async fn queue_prompt(
    args: sessions::QueuePromptArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<sessions::QueuePromptResult, IpcError> {
    routed::queue_prompt(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn queued_prompts(
    args: sessions::QueuedPromptsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<DeferredPromptRow>, IpcError> {
    routed::queued_prompts(&backend, args, &store).await
}

/// Take back a waiting prompt; answers what is still waiting.
#[tauri::command]
pub async fn cancel_queued_prompt(
    args: sessions::CancelQueuedPromptArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<DeferredPromptRow>, IpcError> {
    routed::cancel_queued_prompt(&backend, args, &store).await
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::spawn_review(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn recreate_session(
    args: RecreateSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::recreate_session(&backend, args, &store, &ssh).await
}

/// Batch-restore a host's sessions lost to a reboot or a tmux server
/// restart, over `recreate_session`. Logic lives in `service::sessions::restore`.
#[tauri::command]
pub async fn restore_host_sessions(
    args: RestoreHostSessionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RestoreReport, IpcError> {
    routed::restore_host_sessions(&backend, args, &store, &ssh).await
}

/// Scan a host's Claude transcripts for lost conversations (rows that already
/// hold one are flagged via `existing_session_id`) and rank/enrich them. Read-only. Logic lives in `service::sessions::discover`.
#[tauri::command]
pub async fn discover_lost_sessions(
    args: DiscoverLostSessionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<LostCandidate>, IpcError> {
    routed::discover_lost_sessions(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    routed::dismiss_ghost_session(&backend, args, &store).await
}

/// Adopt a live tmux session fleet did not start (Lost and found, step 4.8).
#[tauri::command]
pub async fn adopt_session(
    args: AdoptSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::adopt_session(&backend, args, &store).await
}

/// The project a Lost and found entry would go into, to prefill Adopt or
/// Restore (step 4.12). Read-only.
#[tauri::command]
pub async fn lost_target(
    args: LostTargetArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<LostTarget, IpcError> {
    routed::lost_target(&backend, args, &store, &ssh).await
}

/// Restore into a project: copy a found conversation where `claude
/// --resume` in the project's root finds it (step 4.12).
#[tauri::command]
pub async fn place_transcript(
    args: PlaceTranscriptArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<PlacedTranscript, IpcError> {
    routed::place_transcript(&backend, args, &store, &ssh).await
}

/// Remove an inactive background agent (`kind='bg'`, not working) from the
/// list. Frontend-only; logic lives in `service::bg_sessions`.
#[tauri::command(async)]
pub fn dismiss_agent_session(
    args: DismissAgentArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("dismiss_agent_session")?;
    bg_sessions::dismiss_agent_session(args, &store)
}

/// Make the session's directory a healthy git worktree on its branch and its
/// tmux session run there (creating tmux when it is gone). A no-op on a
/// healthy session. Logic lives in `service::repair`.
#[tauri::command]
pub async fn repair_session(
    args: RepairSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepairReport, IpcError> {
    routed::repair_session(&backend, args, &store, &ssh).await
}

#[derive(serde::Deserialize)]
pub struct RepairSessionArgs {
    pub session_id: i64,
    /// `true`: the Repair workspace button — an explicit repair that may
    /// unregister this worktree's stale entry, adopt a moved checkout,
    /// recreate the branch from its base, re-link, and respawn a live pane.
    /// `false` (default): the automatic pre-attach check, which only creates
    /// what is confirmed missing and reports the rest.
    #[serde(default)]
    pub explicit: bool,
}

/// Launch a Claude background session on the given host.
#[tauri::command]
pub async fn new_bg_session(
    args: NewBgSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<bg_sessions::NewBgSessionResult, IpcError> {
    routed::new_bg_session(&backend, args, &store, &ssh).await
}

/// Delete all Claude Code state for a project and remove it from the fleet database.
#[tauri::command]
pub async fn purge_project(
    args: PurgeProjectArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<bg_sessions::PurgeReport>, IpcError> {
    backend.refuse_local_only("purge_project")?;
    bg_sessions::purge_project(args, &store, &ssh).await
}

// ── Session timeline (Q9) ───────────────────────────────────────────────────

/// Default and ceiling for `session_history`'s `limit`. The store caps the
/// timeline at 500 rows per session, so asking for more returns nothing extra.
const HISTORY_DEFAULT_LIMIT: i64 = 200;
const HISTORY_MAX_LIMIT: i64 = 500;

#[derive(serde::Deserialize)]
pub struct SessionHistoryArgs {
    pub session_id: i64,
    /// Newest-first cap; `None` or non-positive means the default.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Pure: clamp the requested timeline length into `1..=HISTORY_MAX_LIMIT`.
fn history_limit(requested: Option<i64>) -> i64 {
    match requested {
        Some(n) if n > 0 => n.min(HISTORY_MAX_LIMIT),
        _ => HISTORY_DEFAULT_LIMIT,
    }
}

/// The recorded event timeline for one session, newest first. Same data as
/// the MCP `session_history` tool; rendered by the details pane's Timeline.
#[tauri::command]
pub async fn session_history(
    args: SessionHistoryArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<fleet_core::store::SessionEvent>, IpcError> {
    routed::session_history(&backend, args, &store).await
}

#[derive(serde::Deserialize)]
pub struct SessionConversationsArgs {
    pub session_id: i64,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Conversations a session has run, newest first (migration 037). Default
/// 50, max 500. Same data as the MCP `session_conversations` tool.
#[tauri::command]
pub async fn session_conversations(
    args: SessionConversationsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<fleet_core::store::ConversationRow>, IpcError> {
    routed::session_conversations(&backend, args, &store).await
}

// ── Conversation (structured transcript) ────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SessionConversationArgs {
    pub session_id: i64,
    /// Most-recent turns to return; omitted = the default window. Clamped
    /// (see `transcript::conv_limits`).
    #[serde(default)]
    pub turns: Option<usize>,
    /// Read this earlier conversation of the session instead of the current
    /// one. `E_INVALID` when it is not one of the session's conversations.
    #[serde(default)]
    pub claude_session_id: Option<String>,
}

/// The session's recent conversation — prompts, assistant text and one line
/// per tool call — read from its Claude Code transcript. Rendered by the
/// details pane's Conversation tab; `claude_session_id` reads an earlier
/// conversation. Errors: `E_NOTFOUND`, `E_INVALID` (not one of the session's
/// conversations), `E_INVALID_STATE`
/// (no `claude_session_id` yet), `E_NO_TRANSCRIPT`, transport codes.
#[tauri::command]
pub async fn session_conversation(
    args: SessionConversationArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<fleet_core::service::transcript::Conversation, IpcError> {
    routed::session_conversation(&backend, args, &store, &ssh).await
}

#[derive(serde::Deserialize)]
pub struct SessionToolDetailArgs {
    pub session_id: i64,
    pub tool_use_id: String,
    /// Look in this earlier conversation of the session instead of the
    /// current one.
    #[serde(default)]
    pub claude_session_id: Option<String>,
}

/// The input (edit before/after, Bash command, or pretty JSON) and result
/// of one tool call, read on demand from the session's transcript — the
/// Conversation tab's poll never carries them. Each text is capped at 8 000
/// chars. Errors: `E_NOTFOUND` (session, or tool call not in the
/// transcript), `E_INVALID` (bad tool id / not one of the session's
/// conversations), `E_INVALID_STATE`, `E_NO_TRANSCRIPT`, transport codes.
#[tauri::command]
pub async fn session_tool_detail(
    args: SessionToolDetailArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<fleet_core::service::transcript::ToolDetail, IpcError> {
    routed::session_tool_detail(&backend, args, &store, &ssh).await
}

// ── Activity probe (live indicator) ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SessionActivityArgs {
    pub session_id: i64,
}

/// What the session's pane shows right now (status, spinner, stuck / dialog
/// state), read on demand for the Conversation tab's live indicator. Errors:
/// `E_NOTFOUND`, `E_INVALID_STATE` (runs outside tmux), transport codes.
#[tauri::command]
pub async fn session_activity(
    args: SessionActivityArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<sessions::ActivityProbe, IpcError> {
    routed::session_activity(&backend, args, &store, &ssh).await
}

// ── sharing a session, and the watcher's pane (multi-user M1, T13) ──────────
//
// Six commands, each routed to the hub tool T12 added for it, each argument
// struct a field-for-field twin of that tool's params — which is what lets
// them route at all (*parity or refusal*, in `backend::routing`'s header).
//
// **What is NOT here, and why.** `session_claim` has no desktop command:
// parity fails on the CALLER, not the arguments — the tool is
// `Access::HostToken`, and a desktop's client token can never satisfy it —
// and spec §4.3 states the prohibition in words ("a UI 'claim' button for an
// arbitrary org member is exactly what must not exist"). The operator claims
// with `fleet-hub session claim <id> --person <name>`, or the agent in the
// session claims through its own per-host token. There is likewise no
// desktop `session_transcript`: the Conversation tab already reads four
// routed, richer sources, and a second transcript path would be a second
// thing to gate.
//
// **Sharing never confers a terminal** (spec §4.3 invariant 5), which is why
// `capture_session` is here: a watcher gets a read-only pane SNAPSHOT, a
// routed read the hub re-gates on every poll and can revoke between two of
// them, where `pty_open` would be a direct SSH into the owner's pane that no
// revoke could reach.

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CaptureSessionArgs {
    pub session_id: i64,
    /// Scrollback rows above the visible pane; `None` = the visible pane.
    /// Clamped by the service (`clamp_scrollback`), on either arm.
    #[serde(default)]
    pub scrollback_lines: Option<u32>,
    /// Keep the last n lines; `None` = `CAPTURE_DEFAULT_MAX_LINES`, `0` = no
    /// cap. A cut adds one note line.
    #[serde(default)]
    pub max_lines: Option<u32>,
}

/// A read-only snapshot of a session's tmux pane, as plain text.
///
/// Added for the WATCHER (multi-user M1, R5-e): this is what a `watch` grant
/// gives back after the live terminal is taken away. The hub gates it per
/// request at `Reach::Read`, so a revoked grant stops the next poll; the
/// owner does not use it, they attach.
///
/// Errors: `E_NOTFOUND`, `E_INVALID_STATE` (no tmux pane), transport codes.
#[tauri::command]
pub async fn capture_session(
    args: CaptureSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<String, IpcError> {
    routed::capture_session(&backend, args, &store, &ssh).await
}

/// `session_summary_since`'s arguments — `SessionSummarySinceParams`
/// field for field.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SessionSummarySinceArgs {
    pub session_id: i64,
    /// Unix seconds: the turns that ended at or after it are summarised.
    pub since: i64,
}

/// A short summary of what a session did since a time (Orbit Fleet 11.11),
/// for its watcher or its owner: drafted on the session's host under its
/// account, only with its org's consent, and hidden when the Jev check
/// (`decide.jev.summary_check`) cannot confirm it.
///
/// Errors: `E_FORBIDDEN` (no consent), `E_NOTFOUND`, `E_CLAUDE_CLI`,
/// `E_TIMEOUT`, transport codes.
#[tauri::command]
pub async fn session_summary_since(
    args: SessionSummarySinceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<fleet_core::service::watch_summary::WatchSummary, IpcError> {
    routed::session_summary_since(&backend, args, &store, &ssh).await
}

/// `session_share`'s arguments — `SessionShareParams` field for field.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SessionShareArgs {
    pub session_id: i64,
    /// The recipient, by person name …
    #[serde(default)]
    pub person: String,
    /// … or an org, by name (org administration phase D): its members and
    /// admins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// `watch` or `drive`. Passed through as the string the user chose: the
    /// store is the one validator, so a level this build has never heard of
    /// is refused there rather than silently coerced here.
    pub level: String,
}

/// `session_unshare`'s and `session_narrow`'s arguments —
/// `SessionGrantParams` field for field.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SessionGrantArgs {
    pub session_id: i64,
    #[serde(default)]
    pub person: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
}

/// `session_access`'s arguments — `SessionAccessParams` field for field.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SessionAccessArgs {
    pub session_id: i64,
}

/// Share a session you own with one person, at `watch` or `drive`.
///
/// Owner only, and the refusal is the store's own `WHERE owner_person_id =
/// ?granter` on either arm — so the rule holds standalone, where no reach
/// gate runs. Returns the session row for the optimistic patch.
/// Errors: `E_NOTFOUND`, `E_FORBIDDEN`, `E_VALIDATE`, `E_EXISTS`.
#[tauri::command]
pub async fn session_share(
    args: SessionShareArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::session_share(&backend, args, &store).await
}

/// Revoke one person's grant on a session you own. Owner only; the grant row
/// is kept, revoked, for the audit trail.
#[tauri::command]
pub async fn session_unshare(
    args: SessionGrantArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::session_unshare(&backend, args, &store).await
}

/// Lower one person's grant from `drive` to `watch`. Owner only.
///
/// There is deliberately no twin that raises one, here or on the hub or in
/// the store: a grant moves downward only (spec §4.3 invariant 3), and
/// widening is the owner revoking and sharing again.
#[tauri::command]
pub async fn session_narrow(
    args: SessionGrantArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::session_narrow(&backend, args, &store).await
}

/// Who holds a live grant on a session you own — the Share sheet's list.
///
/// Owner only on the hub (`Reach::Own`, not `Read`: the answer names OTHER
/// people, which is not part of what a `watch` grant promised).
#[tauri::command]
pub async fn session_access(
    args: SessionAccessArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<sessions::SessionGrantView>, IpcError> {
    routed::session_access(&backend, args, &store).await
}

/// Who this client is on this fleet, and every live grant TO it.
///
/// The one place a client learns its own person id: with each row's
/// `owner_person_id` and `visibility` (which arrive on every row already) and
/// T9's `grant:changed` to keep the set current, this is what
/// `src/lib/access.ts` derives a row's access from. No arguments on purpose —
/// "whose grants" is the connection's own identity, never an argument a
/// caller could point at someone else.
#[tauri::command]
pub async fn my_grants(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<sessions::MyGrants, IpcError> {
    routed::my_grants(&backend, &store).await
}

/// A session's shell terminals (step 5.3): list, open or close one. The
/// terminal pane then attaches to the `tmux_name` it answers with, through
/// `pty_open`, like the agent's.
#[tauri::command]
pub async fn shell_terminals(
    args: sessions::ShellTerminalsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<sessions::ShellTerminalsResult, IpcError> {
    routed::shell_terminals(&backend, args, &store, &ssh).await
}

/// The routing, away from `tauri::State` so the tests can drive it.
pub(crate) mod routed {
    use super::*;

    pub async fn list_sessions(
        backend: &FleetBackend,
        force: Option<bool>,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<SessionRow>, IpcError> {
        let force = force.unwrap_or(false);
        match backend.hub() {
            // `force` becomes the tool's `force`, so the sidebar's Refresh
            // button makes the HUB reconcile rather than this app.
            Some(hub) => hub.list_sessions(force).await,
            None if force => sessions::refresh_sessions(store, ssh).await,
            None => sessions::list_sessions(store, ssh).await,
        }
    }

    pub async fn new_session(
        backend: &FleetBackend,
        mut args: NewSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            // The hub derives the owner from the connection's own person, and
            // `HubBackend::new_session` deliberately does not send one — see
            // `NewSessionArgs::owner_person_id`.
            Some(hub) => hub.new_session(&args).await,
            // Standalone: the person behind the window is this fleet's own
            // personal owner (multi-user M1, T5). The field arrives empty
            // whatever the frontend sent — it is `skip_deserializing` — so
            // this is the only place it can be filled, and the session the
            // user just started is theirs rather than `unclaimed`.
            None => {
                args.owner_person_id = fleet_core::service::sessions::hub_personal_owner(store);
                sessions::new_session(args, store, ssh, reg).await
            }
        }
    }

    pub async fn kill_session(
        backend: &FleetBackend,
        args: KillSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<i64, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("kill_session", &args).await,
            None => sessions::kill_session(args, store, ssh).await,
        }
    }

    pub async fn safe_kill_session(
        backend: &FleetBackend,
        args: SafeKillSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("safe_kill_session", &args).await,
            None => safe_kill::safe_kill_session(args, store, ssh).await,
        }
    }

    pub async fn rename_session(
        backend: &FleetBackend,
        args: RenameSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("rename_session", &args).await,
            None => sessions::rename_session(args, store, ssh).await,
        }
    }

    /// The command is `set_session_friendly_name` and the tool is
    /// `set_friendly_name`: one of the two places the two vocabularies
    /// differ, and the row is what resolves it.
    pub async fn set_session_friendly_name(
        backend: &FleetBackend,
        args: SetFriendlyNameArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_session_friendly_name", &args).await,
            None => sessions::set_session_friendly_name(args, store),
        }
    }

    pub async fn touch_session_viewed(
        backend: &FleetBackend,
        args: TouchSessionViewedArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("touch_session_viewed", &args).await,
            None => sessions::touch_session_viewed(args, store),
        }
    }

    pub async fn restart_session(
        backend: &FleetBackend,
        args: RestartSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("restart_session", &args).await,
            None => sessions::restart_session(args, store, ssh).await,
        }
    }

    pub async fn rewind_conversation(
        backend: &FleetBackend,
        args: RewindArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("rewind_conversation", &args).await,
            None => rewind::rewind_conversation(args, store, ssh, reg).await,
        }
    }

    /// **Pointed at a hub, the prompt arrives marked.** `apply_marker` wraps
    /// every prompt from a non-master caller in the untrusted-input marker,
    /// and a paired client is never the master
    /// (`mcp::tools::support::apply_marker`, whose own doc comment says "text
    /// typed on a phone always reaches an agent marked"). That is the hub's
    /// client model working as designed, not a defect here — but it is a
    /// visible difference from standalone and belongs in the docs.
    pub async fn send_prompt(
        backend: &FleetBackend,
        args: SendPromptArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<(), IpcError> {
        match backend.hub() {
            Some(hub) => {
                // The tool answers `{"delivered": …}` where the command
                // answers `()`; the body is read and discarded so a tool
                // error still surfaces.
                let _: serde_json::Value = hub.route("send_prompt", &args).await?;
                Ok(())
            }
            None => sessions::send_prompt(args, store, ssh).await,
        }
    }

    pub async fn queue_prompt(
        backend: &FleetBackend,
        args: sessions::QueuePromptArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<sessions::QueuePromptResult, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("queue_prompt", &args).await,
            None => sessions::queue_prompt(args, store, ssh).await,
        }
    }

    pub async fn queued_prompts(
        backend: &FleetBackend,
        args: sessions::QueuedPromptsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<DeferredPromptRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("queued_prompts", &args).await,
            None => sessions::queued_prompts(args, store),
        }
    }

    /// The hub's `queued_prompts { cancel }` takes the prompt back and lists
    /// what is left; standalone does the same two steps.
    pub async fn cancel_queued_prompt(
        backend: &FleetBackend,
        args: sessions::CancelQueuedPromptArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<DeferredPromptRow>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                // The command's row names the `queued_prompts` tool.
                hub.route(
                    "cancel_queued_prompt",
                    &serde_json::json!({ "session_id": args.session_id, "cancel": args.id }),
                )
                .await
            }
            None => {
                let session_id = args.session_id;
                sessions::cancel_queued_prompt(args, store)?;
                sessions::queued_prompts(sessions::QueuedPromptsArgs { session_id }, store)
            }
        }
    }

    pub async fn spawn_review(
        backend: &FleetBackend,
        args: SpawnReviewArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("spawn_review", &args).await,
            None => sessions::spawn_review(args, store, ssh).await,
        }
    }

    pub async fn recreate_session(
        backend: &FleetBackend,
        args: RecreateSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("recreate_session", &args).await,
            None => sessions::recreate_session(args, store, ssh).await,
        }
    }

    pub async fn restore_host_sessions(
        backend: &FleetBackend,
        args: RestoreHostSessionsArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<RestoreReport, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("restore_host_sessions", &args).await,
            None => sessions::restore_host_sessions(args, store, ssh).await,
        }
    }

    pub async fn discover_lost_sessions(
        backend: &FleetBackend,
        args: DiscoverLostSessionsArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<LostCandidate>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("discover_lost_sessions", &args).await,
            None => sessions::discover_lost_sessions(args, store, ssh).await,
        }
    }

    pub async fn lost_target(
        backend: &FleetBackend,
        args: LostTargetArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<LostTarget, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("lost_target", &args).await,
            None => {
                let ctx = fleet_core::service::decide::DecideCtx::jev(Arc::clone(store));
                sessions::lost_target_over_ssh(args, store, ssh, &ctx).await
            }
        }
    }

    pub async fn place_transcript(
        backend: &FleetBackend,
        args: PlaceTranscriptArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<PlacedTranscript, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("place_transcript", &args).await,
            // The desktop's commands are a person's.
            None => sessions::place_transcript_over_ssh(args, store, ssh, true).await,
        }
    }

    pub async fn dismiss_ghost_session(
        backend: &FleetBackend,
        args: DismissGhostSessionArgs,
        store: &Mutex<Store>,
    ) -> Result<(), IpcError> {
        match backend.hub() {
            Some(hub) => {
                // The tool answers `{"dismissed": id}` where the command
                // answers `()`; the body is read and discarded so a tool
                // error still surfaces.
                let _: serde_json::Value = hub.route("dismiss_ghost_session", &args).await?;
                Ok(())
            }
            None => sessions::dismiss_ghost_session(args, store),
        }
    }

    pub async fn adopt_session(
        backend: &FleetBackend,
        mut args: AdoptSessionArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            // The hub makes the connection's own person the owner; the field
            // is `skip_deserializing`, so it is never on the wire.
            Some(hub) => hub.route("adopt_session", &args).await,
            // Standalone: the person behind the window, as in `new_session`.
            None => {
                args.owner_person_id = fleet_core::service::sessions::hub_personal_owner(store);
                sessions::adopt_session(args, store)
            }
        }
    }

    /// `explicit: true` (the Repair workspace button) maps one-to-one onto
    /// the tool's own — always explicit — repair, and routes. `explicit:
    /// false` (the automatic pre-attach check) has no counterpart; turning it
    /// into an explicit repair would be destructive by surprise, so it stays
    /// local-only in remote mode. See the module header.
    pub async fn repair_session(
        backend: &FleetBackend,
        args: RepairSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<RepairReport, IpcError> {
        if let Some(hub) = backend.hub() {
            if args.explicit {
                return hub.repair_session(args.session_id).await;
            }
            backend.refuse_local_only("repair_session")?;
        }
        repair::repair_session(args.session_id, args.explicit, store, ssh).await
    }

    pub async fn new_bg_session(
        backend: &FleetBackend,
        args: NewBgSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<bg_sessions::NewBgSessionResult, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("new_bg_session", &args).await,
            // Multi-user M1 (T5): on a standalone desktop the person behind
            // the window is this fleet's own personal owner (migration 098
            // gives a standalone hub one), so the agent's row is theirs. Routed
            // to a hub instead, the hub resolves the owner from the connection
            // and this arm is not reached.
            None => {
                let owner = fleet_core::service::sessions::hub_personal_owner(store);
                bg_sessions::new_bg_session_tracked(args, store, ssh, owner).await
            }
        }
    }

    /// The `limit` clamp applies to both backends, so the hub is asked for
    /// the same window the local store would have returned.
    pub async fn session_history(
        backend: &FleetBackend,
        args: SessionHistoryArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<fleet_core::store::SessionEvent>, IpcError> {
        let limit = history_limit(args.limit);
        match backend.hub() {
            Some(hub) => hub.session_history(args.session_id, Some(limit)).await,
            None => {
                let s = lock(store)?;
                s.list_session_events(args.session_id, limit)
            }
        }
    }

    pub async fn session_conversation(
        backend: &FleetBackend,
        args: SessionConversationArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<fleet_core::service::transcript::Conversation, IpcError> {
        use fleet_core::service::transcript;
        if let Some(hub) = backend.hub() {
            // The hub owns the transcript file and the clamp; `turns` goes
            // over unclamped so `conv_limits` runs once, there.
            return hub
                .session_conversation(
                    args.session_id,
                    args.turns,
                    args.claude_session_id.as_deref(),
                    transcript::CONV_EVENTS_LIMIT_UI,
                )
                .await;
        }
        let row = {
            let s = lock(store)?;
            s.get_session_by_id(args.session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.session_id),
                )
            })?
        };
        // The helper takes (and releases) the lock itself; nothing holds it
        // across the fetch.
        let (turns, max_chars) = transcript::conv_limits(args.turns);
        transcript::fetch_conversation_for_row(
            store,
            ssh,
            &row,
            args.claude_session_id.as_deref(),
            turns,
            max_chars,
            transcript::CONV_EVENTS_LIMIT_UI,
        )
        .await
    }

    /// The session id is the hub's and the transcript lives on the hub's
    /// hosts, so a hub client asks the hub's tool; `claude_session_id` goes
    /// over only when set.
    pub async fn session_tool_detail(
        backend: &FleetBackend,
        args: SessionToolDetailArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<fleet_core::service::transcript::ToolDetail, IpcError> {
        if let Some(hub) = backend.hub() {
            return hub
                .session_tool_detail(
                    args.session_id,
                    &args.tool_use_id,
                    args.claude_session_id.as_deref(),
                )
                .await;
        }
        let row = {
            let s = lock(store)?;
            s.get_session_by_id(args.session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.session_id),
                )
            })?
        };
        fleet_core::service::transcript::fetch_tool_detail(
            store,
            ssh,
            &row,
            args.claude_session_id.as_deref(),
            &args.tool_use_id,
        )
        .await
    }

    /// The `limit` clamp applies to both backends, like `session_history`.
    /// The probe is the Conversation tab's only live signal between the
    /// row's own status changes: without it a hub client saw nothing move
    /// for the whole of a turn. The hub reads the pane over ITS ssh, which
    /// is the only machine that can reach the host anyway.
    pub async fn session_activity(
        backend: &FleetBackend,
        args: SessionActivityArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<sessions::ActivityProbe, IpcError> {
        match backend.hub() {
            Some(hub) => hub.session_activity(args.session_id).await,
            None => sessions::session_activity(store, ssh, args.session_id).await,
        }
    }

    // ── sharing, and the watcher's pane (multi-user M1, T13) ────────────────
    //
    // **The `None` arm of all six is the same service call the hub makes, not
    // `E_UNSUPPORTED`.** Migration 086 gives a standalone fleet a personal
    // owner, so ownership and grants are meaningful locally: the person
    // behind a standalone window IS that owner (the rule `new_session`'s own
    // `None` arm already applies), and the ordinary answer is "this is mine,
    // shared with nobody".
    //
    // **The owner-only rule still holds on the standalone arm**, and not
    // because this layer checks it: the three mutations go through the
    // store's own statements, whose `WHERE` carries
    // `sessions.owner_person_id = ?granter`, so a row that is not this
    // person's is refused there. `hub_personal_owner` answering `None` (a
    // fleet with no personal owner at all) is refused too —
    // `sharing::require_granter` turns it into `E_FORBIDDEN` rather than
    // letting a caller who proves no person write a grant.

    pub async fn session_summary_since(
        backend: &FleetBackend,
        args: SessionSummarySinceArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<fleet_core::service::watch_summary::WatchSummary, IpcError> {
        if let Some(hub) = backend.hub() {
            return hub.route("session_summary_since", &args).await;
        }
        let row = {
            let s = lock(store)?;
            s.get_session_by_id(args.session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.session_id),
                )
            })?
        };
        let decide = fleet_core::service::decide::DecideCtx::jev(Arc::clone(store));
        fleet_core::service::watch_summary::summarize_since(store, ssh, &decide, &row, args.since)
            .await
    }

    pub async fn capture_session(
        backend: &FleetBackend,
        args: CaptureSessionArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<String, IpcError> {
        match backend.hub() {
            // `route_text`, not `route`: the tool answers a pane as prose,
            // and a JSON-encoded string would turn every newline into `\n`.
            Some(hub) => hub.route_text("capture_session", &args).await,
            None => {
                let text = sessions::capture_session_output(
                    args.session_id,
                    store,
                    ssh,
                    args.scrollback_lines,
                )
                .await?;
                // The SAME shaper the tool uses, so a watcher's pane does not
                // read differently on a standalone desktop: the blank-pane
                // line and the truncation note are one implementation in
                // `service::sessions`.
                Ok(sessions::shape_capture(&text, args.max_lines))
            }
        }
    }

    pub async fn session_share(
        backend: &FleetBackend,
        args: SessionShareArgs,
        store: &Arc<Mutex<Store>>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_share", &args).await,
            None => {
                // Read the owner BEFORE taking the lock: `hub_personal_owner`
                // takes it itself.
                let granter = sessions::hub_personal_owner(store);
                let s = lock(store)?;
                let to = sessions::ShareTo::from_fields(&args.person, args.org.as_deref())?;
                sessions::share_session_to(&s, args.session_id, to, &args.level, granter)
            }
        }
    }

    pub async fn session_unshare(
        backend: &FleetBackend,
        args: SessionGrantArgs,
        store: &Arc<Mutex<Store>>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_unshare", &args).await,
            None => {
                let granter = sessions::hub_personal_owner(store);
                let s = lock(store)?;
                let to = sessions::ShareTo::from_fields(&args.person, args.org.as_deref())?;
                sessions::unshare_session_to(&s, args.session_id, to, granter)
            }
        }
    }

    pub async fn session_narrow(
        backend: &FleetBackend,
        args: SessionGrantArgs,
        store: &Arc<Mutex<Store>>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_narrow", &args).await,
            None => {
                let granter = sessions::hub_personal_owner(store);
                let s = lock(store)?;
                let to = sessions::ShareTo::from_fields(&args.person, args.org.as_deref())?;
                sessions::narrow_session_share_to(&s, args.session_id, to, granter)
            }
        }
    }

    pub async fn session_access(
        backend: &FleetBackend,
        args: SessionAccessArgs,
        store: &Arc<Mutex<Store>>,
    ) -> Result<Vec<sessions::SessionGrantView>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_access", &args).await,
            None => {
                // No reach gate on this arm, which is the standing rule for
                // every standalone read in this file: a standalone desktop is
                // `ViewScope::internal()`, the fleet's own reader, holding
                // the sqlite file itself. The hub arm is where `Reach::Own`
                // keeps one person's grant list away from another's device.
                let s = lock(store)?;
                sessions::session_access(&s, args.session_id)
            }
        }
    }

    pub async fn my_grants(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
    ) -> Result<sessions::MyGrants, IpcError> {
        match backend.hub() {
            // No arguments: the hub answers for the CONNECTION's own person.
            Some(hub) => hub.route("my_grants", &serde_json::json!({})).await,
            None => {
                let who = sessions::hub_personal_owner(store);
                let s = lock(store)?;
                // `who == None` answers an EMPTY grant set, never every
                // grant — `sharing::my_grants`' own first line.
                sessions::my_grants(&s, who)
            }
        }
    }

    pub async fn shell_terminals(
        backend: &FleetBackend,
        args: sessions::ShellTerminalsArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<sessions::ShellTerminalsResult, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("shell_terminals", &args).await,
            None => sessions::shell_terminals(args, store, ssh).await,
        }
    }

    pub async fn session_conversations(
        backend: &FleetBackend,
        args: SessionConversationsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<fleet_core::store::ConversationRow>, IpcError> {
        let limit = args.limit.unwrap_or(50).clamp(1, 500);
        match backend.hub() {
            Some(hub) => hub.session_conversations(args.session_id, limit).await,
            None => {
                let s = lock(store)?;
                s.list_conversations(args.session_id, limit)
            }
        }
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn history_limit_defaults_and_clamps() {
        assert_eq!(history_limit(None), HISTORY_DEFAULT_LIMIT);
        assert_eq!(history_limit(Some(0)), HISTORY_DEFAULT_LIMIT);
        assert_eq!(history_limit(Some(-3)), HISTORY_DEFAULT_LIMIT);
        assert_eq!(history_limit(Some(25)), 25);
        assert_eq!(history_limit(Some(10_000)), HISTORY_MAX_LIMIT);
    }

    #[test]
    fn session_history_args_accept_a_missing_limit() {
        let a: SessionHistoryArgs = serde_json::from_str(r#"{"session_id":7}"#).unwrap();
        assert_eq!(a.session_id, 7);
        assert!(a.limit.is_none());
    }
}
