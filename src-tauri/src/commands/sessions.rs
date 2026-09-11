//! Tauri IPC wrappers for tmux session management. Logic lives in
//! `service::sessions`; this file only adapts `tauri::State` to plain
//! references.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::service::bg_sessions::{self, NewBgSessionArgs, PeekSessionArgs, PurgeProjectArgs};
use crate::service::repair::{self, RepairReport};
use crate::service::safe_kill::{
    self, DiscardKillSessionArgs, InspectSafeKillArgs, SafeKillInspection, SafeKillSessionArgs,
};
use crate::service::sessions::{
    self, DismissGhostSessionArgs, KillSessionArgs, NewSessionArgs, RecreateSessionArgs,
    RelatedSessionsArgs, RenameSessionArgs, RestartSessionArgs, SendPromptArgs,
    SetFriendlyNameArgs, SpawnReviewArgs,
};
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

/// `force: true` (the sidebar Refresh button) always runs a fleet reconcile
/// pass; the default serves stored rows while the last pass is within the
/// configured interval.
#[tauri::command]
pub async fn list_sessions(
    force: Option<bool>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    if force.unwrap_or(false) {
        sessions::refresh_sessions(&store, &ssh).await
    } else {
        sessions::list_sessions(&store, &ssh).await
    }
}

#[tauri::command]
pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<SessionRow>, IpcError> {
    sessions::related_sessions(args, &store)
}

#[tauri::command]
pub async fn new_session(
    args: NewSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    sessions::new_session(args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn kill_session(
    args: KillSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    sessions::kill_session(args, &store, &ssh).await
}

#[tauri::command]
pub async fn safe_kill_session(
    args: SafeKillSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    safe_kill::safe_kill_session(args, &store, &ssh).await
}

/// Pre-flight check used by the UI before showing the safe-remove dialog:
/// returns dirty files + pushed-state so we can either skip the Claude prompt
/// (clean+pushed) or warn the user about what would be lost.
#[tauri::command]
pub async fn inspect_safe_kill(
    args: InspectSafeKillArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SafeKillInspection, IpcError> {
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
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    safe_kill::discard_kill_session(args, force, &store, &ssh).await
}

#[tauri::command]
pub async fn rename_session(
    args: RenameSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::rename_session(args, &store, &ssh).await
}

#[tauri::command]
pub fn set_session_friendly_name(
    args: SetFriendlyNameArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    sessions::set_session_friendly_name(args, &store)
}

#[tauri::command]
pub async fn restart_session(
    args: RestartSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::restart_session(args, &store, &ssh).await
}

#[tauri::command]
pub async fn send_prompt(
    args: SendPromptArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    sessions::send_prompt(args, &store, &ssh).await
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::spawn_review(args, &store, &ssh).await
}

#[tauri::command]
pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::recreate_session(args, &store, &ssh).await
}

#[tauri::command]
pub fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    sessions::dismiss_ghost_session(args, &store)
}

/// Make the session's directory a healthy git worktree on its branch and its
/// tmux session run there (creating tmux when it is gone). A no-op on a
/// healthy session. Logic lives in `service::repair`.
#[tauri::command]
pub async fn repair_session(
    args: RepairSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepairReport, IpcError> {
    repair::repair_session(args.session_id, args.explicit, &store, &ssh).await
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
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<bg_sessions::NewBgSessionResult, IpcError> {
    bg_sessions::new_bg_session_tracked(args, &store, &ssh).await
}

/// Fetch recent log output from a background Claude session without opening a PTY.
#[tauri::command]
pub async fn peek_session(
    args: PeekSessionArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<String, IpcError> {
    bg_sessions::peek_session(args, &ssh).await
}

/// Delete all Claude Code state for a project and remove it from the fleet database.
#[tauri::command]
pub async fn purge_project(
    args: PurgeProjectArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<bg_sessions::PurgeReport, IpcError> {
    bg_sessions::purge_project(args, &store, &ssh).await
}

// ── Operator settings (Wave 2 Track D) ──────────────────────────────────────
//
// Typed key/value settings behind the Settings dialog's automation toggles
// (playbooks, GC, reconcile cadence). The registry in `service::settings`
// owns the key list, defaults and validation; these wrappers only adapt
// `tauri::State`.

/// Every registered operator setting with its effective value.
#[tauri::command]
pub fn get_fleet_settings(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<std::collections::BTreeMap<String, String>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    Ok(crate::service::settings::read_all(&s))
}

/// Validate and persist one operator setting. `E_INVALID` for an unknown key
/// or a value of the wrong shape. Returns the full effective map so the
/// dialog can re-render from one source of truth.
#[tauri::command]
pub fn set_fleet_setting(
    key: String,
    value: String,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<std::collections::BTreeMap<String, String>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    crate::service::settings::set(&s, &key, &value)?;
    Ok(crate::service::settings::read_all(&s))
}
