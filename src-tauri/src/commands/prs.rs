//! Pull requests on the desktop (redesign step 6.4): Work › Pull requests.
//! Standalone it reads `service::prs` here; paired, the hub's `prs` tool,
//! where the hub's own reconcile recorded them.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::prs::{self, PrList, PrsArgs};
use fleet_core::service::view_scope::ViewScope;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn list_pull_requests(
    args: PrsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<PrList, IpcError> {
    routed::list_pull_requests(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn list_pull_requests(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: PrsArgs,
    ) -> Result<PrList, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_pull_requests(&args).await,
            None => prs::list(store, &ViewScope::internal(), &args),
        }
    }
}
