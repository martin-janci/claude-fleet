//! Add account on the desktop (M15 step G2.9): the Accounts page's
//! "+ Add account". Standalone it runs `service::add_account` against this
//! app's own store and SSH client; paired, the hub's `add_account` tool, so
//! the profile lands on the hub's host and its account in the hub's list.
//! The API key crosses to the hub in the request body and is never logged on
//! either side.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::add_account::{self, AddAccountArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn add_account(
    args: AddAccountArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<serde_json::Value, IpcError> {
    routed::add_account(&backend, &store, &ssh, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn add_account(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: AddAccountArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("add_account", &args).await,
            None => add_account::run_with_client(&args, store, ssh).await,
        }
    }
}
