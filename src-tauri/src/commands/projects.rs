//! Tauri IPC wrappers for project discovery. Logic lives in `service::projects`;
//! this file only adapts `tauri::State` to plain references.
//!
//! Remote mode: all six commands route to the hub tools of the same names.
//! `add_project` and `list_github_repos` clone / run `gh` ON THE HOST over
//! the hub's transport to it, so the credentials are the host's, not this
//! machine's. `call_id` stays local: it keys this process's cancellation
//! registry, where the hub branch of `add_project` binds it too, so Stop
//! waiting abandons the HTTP call (decision D3); the hub's run simply
//! completes (or hits its deadline) after the desktop stops waiting.

use crate::backend::FleetBackend;
use fleet_core::cancel::{CancelGuard, CancellationRegistry};
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::add_project::{self, AddProjectArgs, AddProjectSource, GithubRepo};
use fleet_core::service::project_picks::{self, SetProjectPickArgs};
use fleet_core::service::projects::{self, ProjectTreeRow};
use fleet_core::ssh::SshClient;
use fleet_core::store::{ProjectPickRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;
use tokio_util::sync::CancellationToken;

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

/// The New session picker's choices per project (pinned, hide/keep, group).
#[tauri::command]
pub async fn project_picks(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectPickRow>, IpcError> {
    routed::project_picks(&backend, &store).await
}

/// Replace one project's picker choices (full replace). Returns the row.
#[tauri::command]
pub async fn set_project_pick(
    args: SetProjectPickArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ProjectPickRow, IpcError> {
    routed::set_project_pick(&backend, &store, args).await
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
    ///
    /// It is still bound here, in the hub branch: the dialog's Stop waiting
    /// fires `cancel_command(call_id)`, and without a token under that id the
    /// click found nothing and the dialog stayed busy for the whole of the
    /// hub's deadline. Cancelling drops the HTTP call only (D3, no hub-side
    /// cancel), so the answer says the hub may still finish.
    pub async fn add_project(
        backend: &FleetBackend,
        args: AddProjectArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<ProjectTreeRow, IpcError> {
        let Some(hub) = backend.hub() else {
            return add_project::add_project(args, store, &**ssh, reg).await;
        };
        let (cancel_id, token) = match args.call_id {
            Some(id) => {
                let token = CancellationToken::new();
                reg.bind(id, token.clone());
                (id, token)
            }
            None => reg.register_anonymous(),
        };
        let _guard = CancelGuard::new(Arc::clone(reg), cancel_id);
        let github = matches!(
            args.source,
            AddProjectSource::New {
                create_remote: true,
                ..
            }
        );
        let body = serde_json::json!({
            "host_alias": args.host_alias,
            "source": args.source,
        });
        tokio::select! {
            r = hub.route("add_project", &body) => r,
            () = token.cancelled() => Err(stopped_waiting(&args.host_alias, github)),
        }
    }

    /// `E_CANCELLED` for a Stop waiting on a hub client. The dialog shows
    /// this message verbatim for a `create_remote` run, so it carries the
    /// same GitHub hedge the service's own cancel does.
    fn stopped_waiting(host: &str, github: bool) -> IpcError {
        let mut msg = format!(
            "Stopped waiting \u{2014} the hub may still finish adding the project on {host}."
        );
        if github {
            msg.push_str(" The GitHub repository may already exist; check GitHub before retrying.");
        }
        IpcError::new(codes::E_CANCELLED, msg)
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

    pub async fn project_picks(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ProjectPickRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("project_picks", &serde_json::json!({})).await,
            None => project_picks::list(store),
        }
    }

    pub async fn set_project_pick(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: SetProjectPickArgs,
    ) -> Result<ProjectPickRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_project_pick", &args).await,
            None => project_picks::set(store, &args),
        }
    }
}
