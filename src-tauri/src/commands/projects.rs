//! Tauri IPC wrappers for project discovery. Logic lives in `service::projects`;
//! this file only adapts `tauri::State` to plain references.
//!
//! Remote mode: all four commands route to the hub tools of the same names.
//! `add_project` and `list_github_repos` clone / run `gh` ON THE HOST over
//! the hub's transport to it, so the credentials are the host's, not this
//! machine's. `call_id` stays local: it keys this process's cancellation
//! registry, and on a hub the run simply completes (or hits its deadline)
//! after the desktop stops waiting.

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
    routed::add_project(&backend, args, &store, &ssh, &reg).await
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
    routed::list_github_repos(&backend, args, &store, &ssh).await
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

    /// `commands::projects::add_project`. `call_id` is this process's own
    /// cancellation-registry key and has no hub counterpart, so the hub
    /// branch spells the arguments out rather than serialising the struct.
    pub async fn add_project(
        backend: &FleetBackend,
        args: AddProjectArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<ProjectTreeRow, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "add_project",
                    &serde_json::json!({
                        "host_alias": args.host_alias,
                        "source": args.source,
                    }),
                )
                .await
            }
            None => add_project::add_project(args, store, &**ssh, reg).await,
        }
    }

    pub async fn list_github_repos(
        backend: &FleetBackend,
        args: ListGithubReposArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<GithubRepo>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "list_github_repos",
                    &serde_json::json!({ "host_alias": args.host_alias }),
                )
                .await
            }
            None => add_project::list_github_repos(&args.host_alias, store, ssh).await,
        }
    }
}
