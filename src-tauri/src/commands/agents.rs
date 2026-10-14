//! The Automation screen's Agents tab and the Inbox's agent actions (the
//! reviewer and support agents, design step 5): the rules, the newest jobs
//! and the actions waiting for a person, and a person's answer to one.
//! Standalone it reads and writes `service::agents::admin` as the local
//! operator; paired, it routes to the hub's `agents` tool, which only the
//! owner's own device reaches (and only a trusted full one may answer).

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::agents::admin::{self, AgentsArgs, AgentsStatus};
use fleet_core::store::{AgentActionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

/// The rules, the newest jobs (of one agent, or all) and the pending
/// actions.
#[tauri::command]
pub async fn agents_status(
    agent: Option<String>,
    limit: Option<u32>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AgentsStatus, IpcError> {
    routed::agents_status(&backend, &store, agent, limit).await
}

/// Debugging the agents: `explain` (per rule: whether it polls and why
/// not, its last poll error, its jobs by state) or `job` (one job whole,
/// `id`, its proposed actions included). Only reads.
#[tauri::command]
pub async fn agents_debug(
    action: String,
    id: Option<i64>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::agents_debug(&backend, &store, action, id).await
}

/// Approve (`approve: true`) or reject a pending action. An approved one
/// is carried out by its agent on the next run pass.
#[tauri::command]
pub async fn agents_decide(
    id: i64,
    approve: bool,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AgentActionRow, IpcError> {
    routed::agents_decide(&backend, &store, id, approve).await
}

pub(crate) mod routed {
    use super::*;

    pub fn status_args(agent: Option<String>, limit: Option<u32>) -> AgentsArgs {
        AgentsArgs {
            action: "status".into(),
            agent,
            limit,
            ..Default::default()
        }
    }

    pub fn debug_args(action: String, id: Option<i64>) -> AgentsArgs {
        AgentsArgs {
            action,
            id,
            ..Default::default()
        }
    }

    pub async fn agents_debug(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        action: String,
        id: Option<i64>,
    ) -> Result<serde_json::Value, IpcError> {
        // Only the reads: this command never grants, revokes or answers.
        if !matches!(action.as_str(), "explain" | "job") {
            return Err(IpcError::new(
                fleet_core::ipc_error::codes::E_INVALID,
                format!("agents_debug action must be explain | job, got {action:?}"),
            ));
        }
        let args = debug_args(action, id);
        match backend.hub() {
            Some(hub) => hub.route("agents_debug", &args).await,
            None => admin::call(store, &args, "desktop", fleet_core::store::now_unix()),
        }
    }

    pub fn decide_args(id: i64, approve: bool) -> AgentsArgs {
        AgentsArgs {
            action: if approve { "approve" } else { "reject" }.into(),
            id: Some(id),
            ..Default::default()
        }
    }

    pub async fn agents_status(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        agent: Option<String>,
        limit: Option<u32>,
    ) -> Result<AgentsStatus, IpcError> {
        let args = status_args(agent, limit);
        match backend.hub() {
            Some(hub) => hub.route("agents_status", &args).await,
            None => admin::status(store, &args, fleet_core::store::now_unix()),
        }
    }

    pub async fn agents_decide(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        id: i64,
        approve: bool,
    ) -> Result<AgentActionRow, IpcError> {
        let args = decide_args(id, approve);
        match backend.hub() {
            Some(hub) => hub.route("agents_decide", &args).await,
            None => admin::decide(
                store,
                &args,
                approve,
                "desktop",
                fleet_core::store::now_unix(),
            ),
        }
    }
}
