//! Tauri IPC wrapper for the health-check command. The logic lives in
//! `service::health`; this file only adapts `tauri::State` to plain references.
//!
//! # Now routed
//!
//! This used to return a bare `Health` rather than a `Result`, so it had
//! nowhere to put `E_HUB_UNREACHABLE` and could not be routed: in remote mode
//! it answered from the local database, whose fleet roll-ups
//! (`sessions_total`, `ghosts`, `stuck`, `context_red`) are all zero because
//! nothing fills it. A zeroed health panel is the most reassuring thing this
//! app can say, and it was saying it about a fleet it was not looking at.
//!
//! Giving it a `Result` changes what `App.svelte` can receive, so the
//! frontend needed to change too. The frontend half is
//! `src/lib/ipc.ts`'s `healthCheck(): Promise<Result<Health>>`
//! and the footer, which now shows the hub's own error rather than a fleet of
//! zeroes.
//!
//! Note what the two arms mean. Standalone, `version`/`db_ready`/
//! `schema_version` describe **this process** and the roll-ups describe the
//! fleet it owns. Remote, all of it is the hub's: its version, its database,
//! its schema. That is the honest answer — the footer is a statement about
//! the fleet in front of you — but it does mean that while a hub is
//! configured the version in the footer is the hub's, not this app's, which
//! is why the footer names the hub beside it.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::health::{self, Health};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn health_check(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Health, IpcError> {
    routed::health_check(&backend, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn health_check(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Health, IpcError> {
        match backend.hub() {
            Some(hub) => hub.fleet_health().await,
            None => Ok(health::health_check(store)),
        }
    }
}
