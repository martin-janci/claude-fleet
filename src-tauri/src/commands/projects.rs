//! Tauri IPC wrappers for project discovery. Logic lives in `service::projects`;
//! this file only adapts `tauri::State` to plain references.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::service::add_project::{self, AddProjectArgs, GithubRepo};
use crate::service::projects::{self, ProjectTreeRow};
use crate::ssh::SshClient;
use crate::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn list_projects(store: State<'_, Arc<Mutex<Store>>>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    projects::list_projects(&store)
}

#[tauri::command]
pub async fn refresh_projects(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectTreeRow>, IpcError> {
    projects::refresh_projects(&store).await
}

/// Add a project fleet does not know yet: clone a GitHub repo, adopt an
/// existing checkout, or create a new one. Returns the new project row.
/// Cancellable through `args.call_id` — see `AddProjectArgs::call_id`.
#[tauri::command]
pub async fn add_project(
    args: AddProjectArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<ProjectTreeRow, IpcError> {
    add_project::add_project(args, &store, &ssh, &reg).await
}

#[derive(Deserialize)]
pub struct ListGithubReposArgs {
    pub host_alias: String,
}

/// The repositories `gh` can see on `host_alias`, for the Add-project
/// dialog's browse mode. Read-only.
#[tauri::command]
pub async fn list_github_repos(
    args: ListGithubReposArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<GithubRepo>, IpcError> {
    add_project::list_github_repos(&args.host_alias, &store, &ssh).await
}
