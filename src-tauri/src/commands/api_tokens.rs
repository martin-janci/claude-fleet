//! Named Control API tokens on the desktop (M15 step G2.8): Settings →
//! Control API's "+ Token". Standalone it runs `service::control_tokens`
//! against this app's own control API, as the master; paired, the hub's
//! `api_tokens` tool, which serves the owner's trusted full device read and
//! act tokens (never admin) for the hub.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::control_tokens::{self, ApiTokensArgs, Minter};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn api_tokens(
    args: ApiTokensArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::api_tokens(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn api_tokens(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: ApiTokensArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("api_tokens", &args).await,
            // This app's own control API: the desktop is its master.
            None => control_tokens::run(
                store,
                Ok(Minter::Admin),
                &args,
                fleet_core::store::now_unix(),
            ),
        }
    }
}
