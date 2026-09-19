//! Tauri IPC wrappers for project discovery. Logic lives in `service::projects`;
//! this file only adapts `tauri::State` to plain references.
//!
//! Remote mode: `list_projects` and `refresh_projects` route to their tools.
//! `add_project` and `list_github_repos` do not — both act on a checkout on a
//! host, through this machine's SSH and `gh` credentials, and neither has a
//! hub tool.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::add_project::{self, AddProjectArgs, GithubRepo};
use fleet_core::service::projects::{self, ProjectTreeRow};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn list_projects(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectTreeRow>, IpcError> {
    routed::list_projects(&backend, &store).await
}

#[tauri::command]
pub async fn refresh_projects(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectTreeRow>, IpcError> {
    routed::refresh_projects(&backend, &store).await
}

/// Add a project fleet does not know yet: clone a GitHub repo, adopt an
/// existing checkout, or create a new one. Returns the new project row.
/// Cancellable through `args.call_id` — see `AddProjectArgs::call_id`.
#[tauri::command]
pub async fn add_project(
    args: AddProjectArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<ProjectTreeRow, IpcError> {
    backend.refuse_local_only("add_project")?;
    add_project::add_project(args, &store, &*ssh, &reg).await
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
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<GithubRepo>, IpcError> {
    backend.refuse_local_only("list_github_repos")?;
    add_project::list_github_repos(&args.host_alias, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn list_projects(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ProjectTreeRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_projects().await,
            None => projects::list_projects(store),
        }
    }

    pub async fn refresh_projects(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ProjectTreeRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.refresh_projects().await,
            None => projects::refresh_projects(store).await,
        }
    }
}
