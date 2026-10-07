//! Local workspace sync (`fleet_core::service::local_sync`): bind a session's
//! worktree to a folder on this machine and keep the two in step. Every
//! mutation is `LocalOnly`: the folder is on this disk and the sync runs
//! over this machine's own SSH, so a desktop paired with a hub refuses them.

use fleet_core::ipc_error::IpcError;
use fleet_core::service::local_sync::{
    self, EnableLocalWorkspaceArgs, LocalSync, LocalWorkspaceIdArgs, ResolveLocalConflictArgs,
    SetLocalWorkspaceExcludesArgs,
};
use fleet_core::store::{LocalWorkspaceRow, Store};
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
