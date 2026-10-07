//! Local workspace sync (`fleet_core::service::local_sync`): bind a session's
//! worktree to a folder on this machine and keep the two in step. Every
//! mutation is `LocalOnly`: the folder is on this disk and the sync runs
//! over this machine's own SSH, so a desktop paired with a hub refuses them.

use fleet_core::ipc_error::IpcError;
use fleet_core::service::local_sync::handoff::{
    self, AskAiArgs, AskPlan, CommitLocalWorkspaceArgs, DismissLocalActivityArgs, LocalChanges,
    LocalCommit, LocalWorkspacePathArgs, LocalWorkspacePathsArgs, SetLocalDriverArgs,
};
use fleet_core::service::local_sync::open::OpenApp;
use fleet_core::service::local_sync::{
    self, EnableLocalWorkspaceArgs, LocalSync, LocalWorkspaceIdArgs, ResolveLocalConflictArgs,
    SetLocalWorkspaceExcludesArgs,
};
use fleet_core::service::repo_read::FileDiff;
use fleet_core::service::sessions::{self, SendPromptArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::{LocalWorkspaceRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

use crate::backend::FleetBackend;

/// Every link on this machine, with its state and open conflicts.
#[tauri::command]
pub async fn list_local_workspaces(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<LocalWorkspaceRow>, IpcError> {
    local_sync::list(&store)
}

/// Bind a session's worktree to a local folder and start syncing it.
#[tauri::command]
pub async fn enable_local_workspace(
    args: EnableLocalWorkspaceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("enable_local_workspace")?;
    local_sync::enable(&engine, args).await
}

#[tauri::command]
pub async fn pause_local_workspace(
    args: LocalWorkspaceIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("pause_local_workspace")?;
    local_sync::pause(&engine, args.id)
}

#[tauri::command]
pub async fn resume_local_workspace(
    args: LocalWorkspaceIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("resume_local_workspace")?;
    local_sync::resume(&engine, args.id)
}

/// Run a pass now and answer the link as it ended.
#[tauri::command]
pub async fn sync_local_workspace_now(
    args: LocalWorkspaceIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("sync_local_workspace_now")?;
    engine.sync_now(args.id).await
}

/// Drop the link. Files on both sides stay as they are.
#[tauri::command]
pub async fn disconnect_local_workspace(
    args: LocalWorkspaceIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("disconnect_local_workspace")?;
    local_sync::disconnect(&engine, args.id).await
}

#[tauri::command]
pub async fn set_local_workspace_excludes(
    args: SetLocalWorkspaceExcludesArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("set_local_workspace_excludes")?;
    local_sync::set_excludes(&engine, args)
}

/// Keep one side of a conflicting file; the pass that follows carries it out.
#[tauri::command]
pub async fn resolve_local_workspace_conflict(
    args: ResolveLocalConflictArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("resolve_local_workspace_conflict")?;
    local_sync::resolve_conflict(&engine, args).await
}

// ---------------------------------------------------------------------------
// Phases 2 and 3 (docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct OpenLocalWorkspaceArgs {
    pub id: i64,
    pub app: OpenApp,
}

/// Open the link's folder in the file manager, VS Code, IntelliJ IDEA or a
/// terminal on this machine.
#[tauri::command]
pub async fn open_local_workspace(
    args: OpenLocalWorkspaceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("open_local_workspace")?;
    let path = fleet_core::ipc_error::lock(&store)?
        .local_workspace(args.id)?
        .ok_or_else(|| {
            IpcError::new(
                fleet_core::ipc_error::codes::E_NOTFOUND,
                format!("no local workspace {}", args.id),
            )
        })?
        .local_path;
    tauri::async_runtime::spawn_blocking(move || local_sync::open::open(args.app, &path))
        .await
        .map_err(|e| {
            IpcError::new(
                fleet_core::ipc_error::codes::E_INTERNAL,
                format!("open: {e}"),
            )
        })?
}

/// The worktree's uncommitted changes (after a pass), each with the side
/// that made it, and the activity log.
#[tauri::command]
pub async fn local_workspace_changes(
    args: LocalWorkspaceIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalChanges, IpcError> {
    backend.refuse_local_only("local_workspace_changes")?;
    handoff::changes(&engine, args.id).await
}

/// One file's change against HEAD in the worktree.
#[tauri::command]
pub async fn local_workspace_diff(
    args: LocalWorkspacePathArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<FileDiff, IpcError> {
    backend.refuse_local_only("local_workspace_diff")?;
    handoff::diff(&engine, args).await
}

/// Commit exactly the chosen files in the worktree on the host.
#[tauri::command]
pub async fn commit_local_workspace(
    args: CommitLocalWorkspaceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalCommit, IpcError> {
    backend.refuse_local_only("commit_local_workspace")?;
    handoff::commit(&engine, args).await
}

/// Put the chosen files back to HEAD on the host; the sync brings that to
/// the folder.
#[tauri::command]
pub async fn discard_local_workspace_changes(
    args: LocalWorkspacePathsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("discard_local_workspace_changes")?;
    handoff::discard(&engine, args).await
}

/// Forget the "N local / agent changes" list (one side or both).
#[tauri::command]
pub async fn dismiss_local_workspace_activity(
    args: DismissLocalActivityArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("dismiss_local_workspace_activity")?;
    handoff::dismiss(&engine, args)
}

/// A conflicting file as a diff from the local version to the host's.
#[tauri::command]
pub async fn compare_local_conflict(
    args: LocalWorkspacePathArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<FileDiff, IpcError> {
    backend.refuse_local_only("compare_local_conflict")?;
    handoff::compare_conflict(&engine, args).await
}

/// Keep both versions: the local one as `<path>.local-copy`, the host's in place.
#[tauri::command]
pub async fn keep_both_local_conflict(
    args: LocalWorkspacePathArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("keep_both_local_conflict")?;
    handoff::keep_both(&engine, args).await
}

async fn deliver(
    plan: &AskPlan,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    sessions::send_prompt(
        SendPromptArgs {
            host_alias: plan.host_alias.clone(),
            tmux_name: plan.tmux_name.clone(),
            prompt: plan.prompt.clone(),
            submit: true,
            keys: None,
        },
        store,
        ssh,
    )
    .await
}

/// Ask the agent on the worktree about the developer's changes (explain,
/// review, continue, tests, commit, merge, resolve a conflict, or a question
/// of the person's own).
#[tauri::command]
pub async fn ask_ai_about_local_changes(
    args: AskAiArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("ask_ai_about_local_changes")?;
    let id = args.id;
    let plan = handoff::prepare_ask(&engine, args).await?;
    deliver(&plan, &store, &ssh).await?;
    handoff::finish_ask(&engine, id, &plan.clear)
}

/// Who drives the worktree: take over (the agent is asked to keep its hands
/// off), hand back to the agent (it gets the developer's changes), or shared.
#[tauri::command]
pub async fn set_local_workspace_driver(
    args: SetLocalDriverArgs,
    backend: State<'_, Arc<FleetBackend>>,
    engine: State<'_, Arc<LocalSync>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    backend.refuse_local_only("set_local_workspace_driver")?;
    let plan = handoff::prepare_driver(&engine, &args).await?;
    if let Some(p) = &plan {
        deliver(p, &store, &ssh).await?;
    }
    handoff::finish_driver(&engine, &args, plan.as_ref())
}
