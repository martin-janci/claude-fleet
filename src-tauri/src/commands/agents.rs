//! The Automation screen's Agents tab and the Inbox's agent actions (the
//! reviewer and support agents, design step 5): the rules, the newest jobs
//! and the actions waiting for a person, and a person's answer to one.
//! Standalone it reads and writes `service::agents::admin` as the local
//! operator; paired, it routes to the hub's `agents` tool, which only the
//! owner's own device reaches (and only a trusted full one may answer).

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::agents::admin::{self, AgentsArgs, AgentsStatus};
use fleet_core::ssh::SshClient;
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

/// The catalog's fleet agents (configurable agents, step 5):
/// `definitions` (the list), `form` (a definition's grant form,
/// `definition`, prefilled from rule `id` for Edit), `grant` (the form's
/// `answers`; each MCP binding is asked for its tools on the host first),
/// `revoke` (rule `id`) and `run` (a manual fleet agent's rule `id` now,
/// with the person's `request` and the `run_scope` they narrowed it to).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn agents_fleet(
    action: String,
    definition: Option<String>,
    id: Option<i64>,
    answers: Option<serde_json::Value>,
    request: Option<String>,
    run_scope: Option<serde_json::Value>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<serde_json::Value, IpcError> {
    let args = routed::FleetCall {
        action,
        definition,
        id,
        answers,
        request,
        run_scope,
    };
    routed::agents_fleet(&backend, &store, &ssh, args).await
}

pub(crate) mod routed {
    use super::*;

    /// What `agents_fleet` was asked, as the frontend sends it.
    #[derive(Debug, Clone, Default)]
    pub struct FleetCall {
        pub action: String,
        pub definition: Option<String>,
        pub id: Option<i64>,
        pub answers: Option<serde_json::Value>,
        pub request: Option<String>,
        pub run_scope: Option<serde_json::Value>,
    }

    pub fn fleet_args(c: FleetCall) -> AgentsArgs {
        AgentsArgs {
            agent: (c.action == "grant").then(|| "fleet".to_string()),
            action: c.action,
            definition: c.definition,
            id: c.id,
            answers: c.answers,
            request: c.request,
            run_scope: c.run_scope,
            ..Default::default()
        }
    }

    pub async fn agents_fleet(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        ssh: &SshClient,
        call: FleetCall,
    ) -> Result<serde_json::Value, IpcError> {
        let action = &call.action;
        if !matches!(
            action.as_str(),
            "definitions" | "gallery" | "form" | "grant" | "install" | "revoke" | "run"
        ) {
            return Err(IpcError::new(
                fleet_core::ipc_error::codes::E_INVALID,
                format!("agents_fleet action must be definitions | gallery | form | grant | install | revoke | run, got {action:?}"),
            ));
        }
        let args = fleet_args(call);
        if let Some(hub) = backend.hub() {
            return hub.route("agents_fleet", &args).await;
        }
        let now = fleet_core::store::now_unix();
        if args.action == "grant" {
            let done = fleet_core::service::fleet_agents::grant::grant_or_answers(
                store, ssh, &args, "desktop", now,
            )
            .await?;
            return serde_json::to_value(done).map_err(|e| {
                IpcError::new(fleet_core::ipc_error::codes::E_INTERNAL, e.to_string())
            });
        }
        // `install` writes the catalog: under the authoring lock, off the
        // async workers.
        admin::call_async(store, &args, "desktop", now).await
    }

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
