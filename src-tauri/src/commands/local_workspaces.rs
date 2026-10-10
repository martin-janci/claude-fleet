//! Local workspace sync (`fleet_core::service::local_sync`): bind a session's
//! worktree to a folder on this machine and keep the two in step. The same in
//! both modes: the folder is on this disk and the sync runs over this
//! machine's own SSH, so a desktop paired with a hub runs it too. What it
//! needs from the fleet (the session, its project, delivering a prompt)
//! comes from the hub there.

use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::local_sync::handoff::{
    self, AskAiArgs, AskPlan, CommitLocalWorkspaceArgs, DismissLocalActivityArgs, HubSessions,
    LocalChanges, LocalCommit, LocalWorkspacePathArgs, LocalWorkspacePathsArgs, SetLocalDriverArgs,
};
use fleet_core::service::local_sync::open::OpenApp;
use fleet_core::service::local_sync::{
    self, EnableLocalWorkspaceArgs, HubSessionWorktree, LocalSync, LocalWorkspaceIdArgs,
    ResolveLocalConflictArgs, SetLocalWorkspaceExcludesArgs,
};
use fleet_core::service::repo_read::FileDiff;
use fleet_core::service::sessions::SendPromptArgs;
use fleet_core::ssh::SshClient;
use fleet_core::store::{LocalWorkspaceRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

use crate::backend::FleetBackend;

/// On a desktop paired with a hub, the hub's sessions and its id of the
/// link's project; `None` standalone, where this machine's database has both.
async fn hub_sessions(
    backend: &FleetBackend,
    store: &Mutex<Store>,
    id: i64,
) -> Result<Option<HubSessions>, IpcError> {
    let Some(hub) = backend.hub() else {
        return Ok(None);
    };
    let link = fleet_core::ipc_error::lock(store)?
        .local_workspace(id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no local workspace {id}")))?;
    let project_id = hub
        .list_projects()
        .await?
        .into_iter()
        .map(|t| t.project)
        .filter(|p| p.owner == link.owner && p.repo == link.repo)
        .min_by_key(|p| (p.system, p.id))
        .map(|p| p.id);
    Ok(Some(HubSessions {
        sessions: hub.list_sessions(false).await?,
        project_id,
    }))
}

/// Send the plan's prompt to its session: over this machine's SSH
/// standalone, through the hub's `send_prompt` when paired.
async fn deliver(
    backend: &FleetBackend,
    plan: &AskPlan,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    crate::commands::sessions::routed::send_prompt(
        backend,
        SendPromptArgs {
            host_alias: plan.host_alias.clone(),
            tmux_name: plan.tmux_name.clone(),
            prompt: plan.prompt.clone(),
            submit: true,
            keys: None,
            expect: None,
        },
        store,
        ssh,
    )
    .await
}

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
    let Some(hub) = backend.hub() else {
        return local_sync::enable(&engine, args).await;
    };
    let session = hub
        .list_sessions(false)
        .await?
        .into_iter()
        .find(|s| s.id == args.session_id)
        .ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("no session {}", args.session_id))
        })?;
    let pid = session.project_id.ok_or_else(|| {
        IpcError::new(
            codes::E_NOREPO,
            "this session is not in a project, so it has no worktree to sync",
        )
    })?;
    let project = hub
        .list_projects()
        .await?
        .into_iter()
        .map(|t| t.project)
        .find(|p| p.id == pid)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no project {pid}")))?;
    let worktree = HubSessionWorktree {
        host_alias: session.host_alias,
        owner: project.owner,
        repo: project.repo,
        worktree_key: session
            .worktree_key
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "main".into()),
        tmux_name: session.tmux_name,
    };
    local_sync::enable_on_hub_session(&engine, worktree, &args.local_path, args.excludes).await
}

#[tauri::command]
pub async fn pause_local_workspace(
    args: LocalWorkspaceIdArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    local_sync::pause(&engine, args.id)
}

#[tauri::command]
pub async fn resume_local_workspace(
    args: LocalWorkspaceIdArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    local_sync::resume(&engine, args.id)
}

/// Run a pass now and answer the link as it ended.
#[tauri::command]
pub async fn sync_local_workspace_now(
    args: LocalWorkspaceIdArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    engine.sync_now(args.id).await
}

/// Drop the link. Files on both sides stay as they are.
#[tauri::command]
pub async fn disconnect_local_workspace(
    args: LocalWorkspaceIdArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<(), IpcError> {
    local_sync::disconnect(&engine, args.id).await
}

#[tauri::command]
pub async fn set_local_workspace_excludes(
    args: SetLocalWorkspaceExcludesArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    local_sync::set_excludes(&engine, args)
}

/// Keep one side of a conflicting file; the pass that follows carries it out.
#[tauri::command]
pub async fn resolve_local_workspace_conflict(
    args: ResolveLocalConflictArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
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
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
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
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalChanges, IpcError> {
    handoff::changes(&engine, args.id).await
}

/// One file's change against HEAD in the worktree.
#[tauri::command]
pub async fn local_workspace_diff(
    args: LocalWorkspacePathArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<FileDiff, IpcError> {
    handoff::diff(&engine, args).await
}

/// Commit exactly the chosen files in the worktree on the host.
#[tauri::command]
pub async fn commit_local_workspace(
    args: CommitLocalWorkspaceArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalCommit, IpcError> {
    handoff::commit(&engine, args).await
}

/// Put the chosen files back to HEAD on the host; the sync brings that to
/// the folder.
#[tauri::command]
pub async fn discard_local_workspace_changes(
    args: LocalWorkspacePathsArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    handoff::discard(&engine, args).await
}

/// Forget the "N local / agent changes" list (one side or both).
#[tauri::command]
pub async fn dismiss_local_workspace_activity(
    args: DismissLocalActivityArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    handoff::dismiss(&engine, args)
}

/// A conflicting file as a diff from the local version to the host's.
#[tauri::command]
pub async fn compare_local_conflict(
    args: LocalWorkspacePathArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<FileDiff, IpcError> {
    handoff::compare_conflict(&engine, args).await
}

/// Keep both versions: the local one as `<path>.local-copy`, the host's in place.
#[tauri::command]
pub async fn keep_both_local_conflict(
    args: LocalWorkspacePathArgs,
    engine: State<'_, Arc<LocalSync>>,
) -> Result<LocalWorkspaceRow, IpcError> {
    handoff::keep_both(&engine, args).await
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
    let id = args.id;
    let hub = hub_sessions(&backend, &store, id).await?;
    let plan = handoff::prepare_ask(&engine, args, hub.as_ref()).await?;
    deliver(&backend, &plan, &store, &ssh).await?;
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
    let hub = hub_sessions(&backend, &store, args.id).await?;
    let plan = handoff::prepare_driver(&engine, &args, hub.as_ref()).await?;
    if let Some(p) = &plan {
        deliver(&backend, p, &store, &ssh).await?;
    }
    handoff::finish_driver(&engine, &args, plan.as_ref())
}
