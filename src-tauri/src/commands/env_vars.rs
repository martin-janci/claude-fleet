//! Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md):
//! Settings › Environment and the hub's `env_vars` tool. Standalone it runs
//! `service::env::run` as this app's own master (references allowed);
//! paired, the hub's tool decides, so a reference from a device is refused
//! there. Every command is fleet-scoped (`backend::fleets::FLEET_SCOPED`):
//! `fleet` names an extra fleet, whose hub then decides the same way.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::env::{self, EnvVarSetArgs, EnvVarUnsetArgs, EnvVarsArgs};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn env_vars(
    args: EnvVarsArgs,
    fleet: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    fleets: State<'_, Arc<crate::backend::fleets::Fleets>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    let backend = fleets.select(backend.inner(), fleet.as_deref())?;
    routed::env_vars(&backend, &store, args).await
}

/// The resource page's list: `env_vars { action: "list" }` with no
/// arguments of its own.
#[tauri::command]
pub async fn env_vars_list(
    fleet: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    fleets: State<'_, Arc<crate::backend::fleets::Fleets>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    let backend = fleets.select(backend.inner(), fleet.as_deref())?;
    routed::env_vars_list(&backend, &store).await
}

/// Settings › Environment's Add: a resource action binds its arguments by
/// name and carries no constant, so the action is this command's.
#[tauri::command]
pub async fn env_var_set(
    args: EnvVarSetArgs,
    fleet: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    fleets: State<'_, Arc<crate::backend::fleets::Fleets>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    let backend = fleets.select(backend.inner(), fleet.as_deref())?;
    routed::env_var_set(&backend, &store, args).await
}

/// Settings › Environment's Remove.
#[tauri::command]
pub async fn env_var_unset(
    args: EnvVarUnsetArgs,
    fleet: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    fleets: State<'_, Arc<crate::backend::fleets::Fleets>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    let backend = fleets.select(backend.inner(), fleet.as_deref())?;
    routed::env_var_unset(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    // Paired, every command routes by its own name (the verdict table maps
    // each to the hub's `env_vars` tool); standalone, the desktop is its
    // own master, so references are allowed, every shared scope is its own,
    // and its `person` scope is this fleet's owner's.

    fn own_master(store: &Mutex<Store>) -> Result<env::EnvCaller, IpcError> {
        let person = fleet_core::ipc_error::lock(store)?.personal_owner_id()?;
        Ok(env::EnvCaller {
            refs_allowed: true,
            admin: true,
            person,
        })
    }

    pub async fn env_vars(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: EnvVarsArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("env_vars", &args).await,
            None => env::run(store, &args, &own_master(store)?),
        }
    }

    pub async fn env_var_set(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: EnvVarSetArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("env_var_set", &args.into_args()).await,
            None => env::run(store, &args.into_args(), &own_master(store)?),
        }
    }

    pub async fn env_var_unset(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: EnvVarUnsetArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("env_var_unset", &args.into_args()).await,
            None => env::run(store, &args.into_args(), &own_master(store)?),
        }
    }

    pub async fn env_vars_list(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<serde_json::Value, IpcError> {
        let args = EnvVarsArgs {
            action: "list".into(),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("env_vars_list", &args).await,
            None => env::run(store, &args, &own_master(store)?),
        }
    }
}
