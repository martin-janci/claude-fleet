//! Tauri commands for trackers (work graph M3.1): add, rename, set the
//! credential, test, remove. Thin wrappers over
//! `fleet_core::service::trackers::admin`, the same code the hub's
//! master-only `work_admin` tool runs.
//!
//! The five admin commands are `LocalOnly`: trackers and their credentials
//! are fleet administration, and a paired desktop is a client, never the
//! master (review C17) — it says "configure on the hub" and names the CLI.
//! Reading trackers and tickets (`list_trackers`, `work_tickets`,
//! `work_lookup`) routes to the hub's `work` tool, and `start_work` to
//! `work_link { action: start }`, so a paired desktop starts work on the hub
//! exactly as a standalone one does here (M3.4).

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::trackers::admin::{self, TestReport, WorkAdminArgs};
use fleet_core::service::trackers::tickets::Ticket;
use fleet_core::ssh::SshClient;
use fleet_core::store::{SessionRow, Store, TrackerRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Deserialize)]
pub struct AddTrackerArgs {
    /// The site, or any ticket URL on it ("connect by paste").
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateTrackerArgs {
    pub tracker_id: i64,
    pub name: String,
}

/// The credential. No `Debug`, no `Serialize`: the secret goes to the store
/// and nowhere else.
#[derive(Deserialize)]
pub struct SetTrackerCredentialArgs {
    pub tracker_id: i64,
    pub username: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub credential_ref: Option<String>,
}

#[derive(Deserialize)]
pub struct TrackerIdArgs {
    pub tracker_id: i64,
}

fn row(v: serde_json::Value) -> Result<TrackerRow, IpcError> {
    serde_json::from_value(v)
        .map_err(|e| IpcError::new(fleet_core::ipc_error::codes::E_SERIALIZE, e.to_string()))
}

#[tauri::command]
pub fn add_tracker(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddTrackerArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TrackerRow, IpcError> {
    backend.refuse_local_only("add_tracker")?;
    row(admin::admin_sync(
        &WorkAdminArgs {
            action: "add".into(),
            site_url: Some(args.url),
            name: args.name,
            ..Default::default()
        },
        &store,
    )?)
}

#[tauri::command]
pub fn update_tracker(
    backend: State<'_, Arc<FleetBackend>>,
    args: UpdateTrackerArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TrackerRow, IpcError> {
    backend.refuse_local_only("update_tracker")?;
    row(admin::admin_sync(
        &WorkAdminArgs {
            action: "update".into(),
            tracker_id: Some(args.tracker_id),
            name: Some(args.name),
            ..Default::default()
        },
        &store,
    )?)
}

#[tauri::command]
pub fn set_tracker_credential(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetTrackerCredentialArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TrackerRow, IpcError> {
    backend.refuse_local_only("set_tracker_credential")?;
    row(admin::admin_sync(
        &WorkAdminArgs {
            action: "set_credential".into(),
            tracker_id: Some(args.tracker_id),
            auth_kind: Some("basic".into()),
            username: Some(args.username),
            secret: args.secret,
            credential_ref: args.credential_ref,
            ..Default::default()
        },
        &store,
    )?)
}

#[tauri::command]
pub async fn test_tracker(
    backend: State<'_, Arc<FleetBackend>>,
    args: TrackerIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TestReport, IpcError> {
    backend.refuse_local_only("test_tracker")?;
    admin::test_tracker(
        args.tracker_id,
        &store,
        &fleet_core::service::trackers::default_net(),
    )
    .await
}

#[tauri::command]
pub fn remove_tracker(
    backend: State<'_, Arc<FleetBackend>>,
    args: TrackerIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("remove_tracker")?;
    admin::admin_sync(
        &WorkAdminArgs {
            action: "remove".into(),
            tracker_id: Some(args.tracker_id),
            ..Default::default()
        },
        &store,
    )?;
    Ok(())
}

// --- reads and start (routed) -------------------------------------------------

/// Cached tickets, filtered by view (`mine`, `sprint`, `recent`,
/// `filter:<id>`) and text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkTicketsArgs {
    #[serde(default)]
    pub tracker_id: Option<i64>,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One ticket by key or URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkLookupArgs {
    pub reference: String,
}

/// Start work on a ticket: a new session, linked `started`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartWorkArgs {
    /// A key or a ticket URL …
    #[serde(default)]
    pub reference: Option<String>,
    /// … or a work item.
    #[serde(default)]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Brief Claude with the ticket.
    #[serde(default)]
    pub with_brief: bool,
    /// The brief as a person edited it in the preview.
    #[serde(default)]
    pub brief: Option<String>,
    /// The session name as edited in the dialog.
    #[serde(default)]
    pub name: Option<String>,
    /// The worktree name as edited in the dialog.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[tauri::command]
pub async fn list_trackers(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<TrackerRow>, IpcError> {
    routed::list_trackers(&backend, &store).await
}

#[tauri::command]
pub async fn work_tickets(
    args: WorkTicketsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<Ticket>, IpcError> {
    routed::work_tickets(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_lookup(
    args: WorkLookupArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Ticket, IpcError> {
    routed::work_lookup(&backend, args, &store).await
}

#[tauri::command]
pub async fn start_work(
    args: StartWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    routed::start_work(&backend, args, &store, &ssh, &reg).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::service::trackers::{default_net, tickets};
    use fleet_core::service::work::{WorkArgs, WorkLinkArgs};

    pub async fn list_trackers(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<TrackerRow>, IpcError> {
        let args = WorkArgs {
            action: Some("trackers".into()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("list_trackers", &args).await,
            None => tickets::trackers(store, tickets::Scope::All),
        }
    }

    pub async fn work_tickets(
        backend: &FleetBackend,
        args: WorkTicketsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<Ticket>, IpcError> {
        let wire = WorkArgs {
            action: Some("tickets".into()),
            tracker_id: args.tracker_id,
            view: args.view.clone(),
            query: args.query.clone(),
            limit: args.limit,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_tickets", &wire).await,
            None => tickets::tickets(
                store,
                args.tracker_id,
                args.view.as_deref(),
                args.query.as_deref(),
                args.limit,
                tickets::Scope::All,
            ),
        }
    }

    pub async fn work_lookup(
        backend: &FleetBackend,
        args: WorkLookupArgs,
        store: &Mutex<Store>,
    ) -> Result<Ticket, IpcError> {
        let is_url = args.reference.contains("://");
        let wire = WorkArgs {
            action: Some("lookup".into()),
            url: is_url.then(|| args.reference.clone()),
            key: (!is_url).then(|| args.reference.clone()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_lookup", &wire).await,
            None => {
                tickets::lookup(store, &args.reference, tickets::Scope::All, &default_net()).await
            }
        }
    }

    pub async fn start_work(
        backend: &FleetBackend,
        args: StartWorkArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SessionRow, IpcError> {
        let is_url = args.reference.as_deref().is_some_and(|r| r.contains("://"));
        let wire = WorkLinkArgs {
            action: "start".into(),
            key: args.reference.clone().filter(|_| !is_url),
            url: args.reference.clone().filter(|_| is_url),
            item_id: args.item_id,
            project_id: args.project_id,
            host_alias: args.host_alias.clone(),
            with_brief: args.with_brief.then_some(true),
            brief: args.brief.clone(),
            name: args.name.clone(),
            worktree: args.worktree.clone(),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("start_work", &wire).await,
            None => {
                tickets::start_work(
                    store,
                    ssh,
                    reg,
                    &fleet_core::service::work::start_args(&wire),
                    tickets::Scope::All,
                    &default_net(),
                )
                .await
            }
        }
    }
}
