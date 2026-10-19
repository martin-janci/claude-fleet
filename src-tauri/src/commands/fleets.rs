//! Which fleets this window shows (multi-hub design).
//!
//! Four commands, all about **this process** — which hubs it is a window
//! onto — so all four are the same in both modes (their rows in
//! `backend::verdicts::VERDICTS` say so). The fleet-scoped commands that act
//! on a fleet's rows live with their families (`commands::sessions`, …) and
//! pick their hub through `backend::fleets::Fleets::select`.

use crate::backend::connection::HubConnectionStatus;
use crate::backend::fleets::{FleetAddArgs, FleetInfo, Fleets, PrimaryView};
use crate::backend::pairing::TcpPairTransport;
use crate::backend::{normalise_base_url, Backend, REMOTE_URL_KEY};
use fleet_core::ipc_error::IpcError;
use fleet_core::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

/// One fleet, by its slot id.
#[derive(Deserialize, Debug, Clone)]
pub struct FleetIdArgs {
    pub id: String,
}

/// Every fleet this window shows: the launch-time one first, then the extra
/// fleets, each with its live connection state.
#[tauri::command(async)]
pub fn fleets_list(
    backend: State<'_, Backend>,
    hub_link: State<'_, Arc<HubConnectionStatus>>,
    store: State<'_, Arc<Mutex<Store>>>,
    fleets: State<'_, Arc<Fleets>>,
) -> Vec<FleetInfo> {
    fleets.list(&logic::primary_view(&backend, &hub_link, &store))
}

/// Pair with another hub and start showing it. Takes effect at once.
#[tauri::command]
pub async fn fleet_add(
    args: FleetAddArgs,
    backend: State<'_, Backend>,
    hub_link: State<'_, Arc<HubConnectionStatus>>,
    store: State<'_, Arc<Mutex<Store>>>,
    fleets: State<'_, Arc<Fleets>>,
) -> Result<FleetInfo, IpcError> {
    let primary = logic::primary_view(&backend, &hub_link, &store);
    fleets.add(&TcpPairTransport, &primary, args).await
}

/// Stop showing an extra fleet and forget its token on this machine. Revokes
/// nothing on its hub.
#[tauri::command(async)]
pub fn fleet_remove(args: FleetIdArgs, fleets: State<'_, Arc<Fleets>>) -> Result<(), IpcError> {
    fleets.remove(&args.id)
}

/// An extra fleet's Retry: reconnect now rather than after the backoff.
/// `async` because rebuilding an unusable fleet reads its token from the OS
/// keychain, which can wait on an unlock prompt — never on the main thread.
#[tauri::command(async)]
pub fn fleet_retry_now(args: FleetIdArgs, fleets: State<'_, Arc<Fleets>>) -> Result<(), IpcError> {
    fleets.retry(&args.id)
}

pub(crate) mod logic {
    use super::*;
    use crate::backend::connection::HubConnection;

    /// The launch-time fleet as `Fleets` needs to see it.
    pub fn primary_view(
        backend: &Backend,
        hub_link: &HubConnectionStatus,
        store: &Mutex<Store>,
    ) -> PrimaryView {
        let configured_url = store
            .lock()
            .ok()
            .and_then(|s| s.get_setting(REMOTE_URL_KEY).ok().flatten())
            .and_then(|raw| normalise_base_url(raw.trim()).ok());
        let connection = match hub_link.current() {
            HubConnection::Standalone => None,
            other => Some(other),
        };
        PrimaryView {
            fleet_id: primary_fleet_id(backend, hub_link, store),
            url: backend
                .remote()
                .map(|c| c.base_url.clone())
                .or_else(|| backend.unavailable().and_then(|u| u.url.clone())),
            client_name: backend.remote().map(|c| c.client_name.clone()),
            local: backend.owns_the_fleet(),
            connection,
            unavailable: backend.unavailable().map(|u| u.reason.clone()),
            configured_url,
        }
    }

    /// The primary's fleet id (multi-hub design D8): this machine's own when
    /// standalone, read without minting one; otherwise what the primary hub's
    /// last `ready` frame named.
    pub fn primary_fleet_id(
        backend: &Backend,
        hub_link: &HubConnectionStatus,
        store: &Mutex<Store>,
    ) -> Option<String> {
        if backend.owns_the_fleet() {
            fleet_core::service::address::stored_local_fleet_id(store)
                .ok()
                .flatten()
        } else {
            hub_link.fleet_id()
        }
    }
}
