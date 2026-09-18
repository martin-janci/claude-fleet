//! Tauri IPC wrapper for the health-check command. The logic lives in
//! `service::health`; this file only adapts `tauri::State` to plain references.
//!
//! # Not routed, and this is a deferral rather than a decision
//!
//! `fleet_health` is a hub tool and `HubBackend::fleet_health` exists, but
//! this command **cannot** call it: it returns a bare `Health`, not a
//! `Result`, so it has nowhere to put `E_HUB_UNREACHABLE`. Giving it one
//! would change what `App.svelte`'s `health = await healthCheck()` can
//! receive — a frontend-visible contract change, which Task 3 is not allowed
//! to make and which no `.ts` change is permitted to absorb.
//!
//! So in remote mode this still answers from the local database, whose fleet
//! roll-ups (`sessions_total`, `ghosts`, `stuck`, `context_red`) are all
//! zero because nothing fills it. `version`, `db_ready` and `schema_version`
//! — what the startup readiness check actually reads — stay correct.
//!
//! Task 5 owns the frontend and should finish this: make the command
//! `Result<Health, IpcError>`, route it, and let the Hub banner render the
//! failure. `HubBackend::fleet_health` is already written and tested.

use fleet_core::service::health::{self, Health};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn health_check(store: State<'_, Arc<Mutex<Store>>>) -> Health {
    health::health_check(&store)
}
