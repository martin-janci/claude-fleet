//! The Automation screen's Routines (Orbit Fleet redesign 8.6; the backend is
//! 8.5's `service::routines`): one command over the `routines` tool's
//! actions, so the desktop and the phone ask the same thing. Standalone it
//! runs the service itself (the desktop IS the local operator); paired, it
//! routes to the hub's `routines` tool, which cuts it to this device's
//! person.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::routines::RoutineInput;
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

/// The `routines` tool's arguments, as the tool reads them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutinesArgs {
    /// list | get | runs | failing | budget | save | preview | delete | set_enabled | skip_next | run_now | set_run_outcome
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<RoutineInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// One `routines` action; the answer is the tool's JSON.
#[tauri::command]
pub async fn routines(
    args: RoutinesArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<serde_json::Value, IpcError> {
    routed::routines(&backend, &store, &ssh, &reg, args).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::ipc_error::codes;
    use fleet_core::service::routines::{self as svc, Deps};
    use fleet_core::service::view_scope::ViewScope;

    fn json<T: Serialize>(v: T) -> Result<serde_json::Value, IpcError> {
        serde_json::to_value(v)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("routines: {e}")))
    }

    pub async fn routines(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
        args: RoutinesArgs,
    ) -> Result<serde_json::Value, IpcError> {
        if let Some(hub) = backend.hub() {
            return hub.route("routines", &args).await;
        }
        let scope = ViewScope::internal();
        let id = || {
            args.routine_id.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!("{} needs routine_id", args.action),
                )
            })
        };
        match args.action.as_str() {
            "list" => json(svc::list(store, &scope)?),
            "get" => json(svc::get(store, &scope, id()?)?),
            "runs" => json(svc::runs(store, &scope, id()?, args.limit)?),
            "failing" => json(svc::failing(store, &scope)?),
            "budget" => json(svc::budget(store, fleet_core::store::now_unix())?),
            "save" => {
                let input = args
                    .routine
                    .as_ref()
                    .ok_or_else(|| IpcError::new(codes::E_INVALID, "save needs routine"))?;
                json(svc::save(store, &scope, args.routine_id, input)?)
            }
            "preview" => {
                let input = args
                    .routine
                    .as_ref()
                    .ok_or_else(|| IpcError::new(codes::E_INVALID, "preview needs routine"))?;
                json(svc::preview(
                    store,
                    &scope,
                    args.routine_id,
                    input,
                    fleet_core::store::now_unix(),
                )?)
            }
            "delete" => json(serde_json::json!({ "removed": svc::delete(store, &scope, id()?)? })),
            "set_enabled" => {
                let on = args
                    .enabled
                    .ok_or_else(|| IpcError::new(codes::E_INVALID, "set_enabled needs enabled"))?;
                json(svc::set_enabled(store, &scope, id()?, on)?)
            }
            "skip_next" => json(svc::skip_next(
                store,
                &scope,
                id()?,
                args.skip.unwrap_or(true),
            )?),
            "run_now" => {
                let deps = Deps::live(Arc::clone(store), Arc::clone(ssh), Arc::clone(reg));
                json(svc::run_now(&deps, &scope, id()?, fleet_core::store::now_unix()).await?)
            }
            "set_run_outcome" => {
                let run_id = args
                    .run_id
                    .ok_or_else(|| IpcError::new(codes::E_INVALID, "set_run_outcome needs run_id"))?;
                let outcome = args
                    .outcome
                    .as_deref()
                    .ok_or_else(|| IpcError::new(codes::E_INVALID, "set_run_outcome needs outcome"))?;
                json(svc::set_run_outcome(
                    store,
                    &scope,
                    id()?,
                    run_id,
                    outcome,
                    fleet_core::store::now_unix(),
                )?)
            }
            other => Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "action must be list | get | runs | failing | budget | save | preview | delete | \
                     set_enabled | skip_next | run_now | set_run_outcome, got {other:?}"
                ),
            )),
        }
    }
}
