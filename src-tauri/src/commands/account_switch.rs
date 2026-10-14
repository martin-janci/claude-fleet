//! Automatic account switching on the desktop: the Accounts page's "Switch
//! my sessions automatically" toggle. Standalone the person at the desktop
//! is the fleet's personal owner and the answer lives in this app's own
//! store; paired, the hub's `auto_switch_account` tool, so the answer is the
//! signed-in person's on the hub that does the switching.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{lock, IpcError};
use fleet_core::service::account_switch::{self, AutoSwitchArgs, AutoSwitchView};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn auto_switch_account(
    args: AutoSwitchArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AutoSwitchView, IpcError> {
    routed::auto_switch_account(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn auto_switch_account(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: AutoSwitchArgs,
    ) -> Result<AutoSwitchView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("auto_switch_account", &args).await,
            None => {
                let s = lock(store)?;
                let owner = s.personal_owner_id()?;
                account_switch::my_auto_switch(&s, owner, &args)
            }
        }
    }
}
