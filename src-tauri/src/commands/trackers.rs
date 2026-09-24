//! Tauri commands for trackers (work graph M3.1): add, rename, set the
//! credential, test, remove. Thin wrappers over
//! `fleet_core::service::trackers::admin`, the same code the hub's
//! master-only `work_admin` tool runs.
//!
//! Every one is `LocalOnly`: trackers and their credentials are fleet
//! administration, and a paired desktop is a client, never the master
//! (review C17) — it says "configure on the hub" and names the CLI. Reading
//! trackers and their items is a `work` action and routes like any read.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::trackers::admin::{self, TestReport, WorkAdminArgs};
use fleet_core::store::{Store, TrackerRow};
use serde::Deserialize;
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
        fleet_core::service::trackers::direct_transport(),
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
