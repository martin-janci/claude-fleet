//! Search everything (search phase 3, `service::search`): one query over the
//! full-text index. Paired, the hub's `search` tool, which fences every hit
//! for this device's person; standalone, this desktop's own index, which
//! holds only its own fleet.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::search::{SearchArgs, SearchPage};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn search(
    args: SearchArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SearchPage, IpcError> {
    routed::search(&backend, args, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn search(
        backend: &FleetBackend,
        args: SearchArgs,
        store: &Mutex<Store>,
    ) -> Result<SearchPage, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("search", &args).await,
            None => fleet_core::service::search::search(
                store,
                &fleet_core::service::view_scope::ViewScope::internal(),
                &args,
            ),
        }
    }
}
