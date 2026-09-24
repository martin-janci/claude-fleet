//! Tauri commands for organisations (work graph M5): read the scopes, the
//! orgs and the suggestions (routed to the hub's `work`), and administer
//! orgs, rules and assignments (thin wrappers over the same
//! `fleet_core::service::orgs::admin` the hub's master-only `work_admin`
//! runs).
//!
//! The admin commands are `LocalOnly`: an org and a host's place in it are
//! the per-host tokens' security boundary, fleet administration that a
//! paired desktop — a client, never the master — cannot change. It says
//! "configure on the hub" and names `fleet-hub org …`. The reads route, so a
//! paired desktop shows the hub's orgs read-only.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::orgs::{OrgDetail, OrgSuggestion, ScopeEntry};
use fleet_core::service::trackers::admin::{self, WorkAdminArgs};
use fleet_core::store::{OrgRow, OrgRuleRow, Store, TrackerRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddOrgArgs {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub isolate_sessions: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateOrgArgs {
    pub org_id: i64,
    #[serde(default)]
    pub name: Option<String>,
    /// `""` clears the colour.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub isolate_sessions: Option<bool>,
    /// Work graph M7: `on` | `off` | `inherit` (`work.auto_tidy`).
    #[serde(default)]
    pub auto_tidy: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgIdArgs {
    pub org_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddOrgRuleArgs {
    pub org_id: i64,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub host_alias: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleIdArgs {
    pub rule_id: i64,
}

/// `org_id: None` takes the host out of its org.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignHostOrgArgs {
    pub host_alias: String,
    #[serde(default)]
    pub org_id: Option<i64>,
}

/// `org_id: None` unassigns the tracker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignTrackerOrgArgs {
    pub tracker_id: i64,
    #[serde(default)]
    pub org_id: Option<i64>,
}

fn decode<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, IpcError> {
    serde_json::from_value(v)
        .map_err(|e| IpcError::new(fleet_core::ipc_error::codes::E_SERIALIZE, e.to_string()))
}

fn run(args: WorkAdminArgs, store: &Mutex<Store>) -> Result<serde_json::Value, IpcError> {
    admin::admin_sync(&args, store)
}

// --- administration (LocalOnly) ------------------------------------------------

#[tauri::command]
pub fn add_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgRow, IpcError> {
    backend.refuse_local_only("add_org")?;
    decode(run(
        WorkAdminArgs {
            action: "add_org".into(),
            name: Some(args.name),
            color: args.color,
            isolate_sessions: Some(args.isolate_sessions),
            ..Default::default()
        },
        &store,
    )?)
}

#[tauri::command]
pub fn update_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: UpdateOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgRow, IpcError> {
    backend.refuse_local_only("update_org")?;
    decode(run(
        WorkAdminArgs {
            action: "update_org".into(),
            org_id: Some(args.org_id),
            name: args.name,
            color: args.color,
            isolate_sessions: args.isolate_sessions,
            auto_tidy: args.auto_tidy,
            ..Default::default()
        },
        &store,
    )?)
}

#[tauri::command]
pub fn remove_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: OrgIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("remove_org")?;
    run(
        WorkAdminArgs {
            action: "remove_org".into(),
            org_id: Some(args.org_id),
            ..Default::default()
        },
        &store,
    )?;
    Ok(())
}

#[tauri::command]
pub fn add_org_rule(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddOrgRuleArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgRuleRow, IpcError> {
    backend.refuse_local_only("add_org_rule")?;
    decode(run(
        WorkAdminArgs {
            action: "add_rule".into(),
            org_id: Some(args.org_id),
            owner: args.owner,
            repo: args.repo,
            path_prefix: args.path_prefix,
            host_alias: args.host_alias,
            ..Default::default()
        },
        &store,
    )?)
}

#[tauri::command]
pub fn remove_org_rule(
    backend: State<'_, Arc<FleetBackend>>,
    args: RuleIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("remove_org_rule")?;
    run(
        WorkAdminArgs {
            action: "remove_rule".into(),
            rule_id: Some(args.rule_id),
            ..Default::default()
        },
        &store,
    )?;
    Ok(())
}

#[tauri::command]
pub fn assign_host_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssignHostOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("assign_host_org")?;
    let action = if args.org_id.is_some() {
        "assign_host"
    } else {
        "unassign_host"
    };
    run(
        WorkAdminArgs {
            action: action.into(),
            host_alias: Some(args.host_alias),
            org_id: args.org_id,
            ..Default::default()
        },
        &store,
    )?;
    Ok(())
}

#[tauri::command]
pub fn assign_tracker_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssignTrackerOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TrackerRow, IpcError> {
    backend.refuse_local_only("assign_tracker_org")?;
    decode(run(
        WorkAdminArgs {
            action: "assign_tracker".into(),
            tracker_id: Some(args.tracker_id),
            org_id: args.org_id,
            ..Default::default()
        },
        &store,
    )?)
}

// --- reads (routed) ------------------------------------------------------------

#[tauri::command]
pub async fn work_scopes(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ScopeEntry>, IpcError> {
    routed::work_scopes(&backend, &store).await
}

#[tauri::command]
pub async fn list_orgs(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<OrgDetail>, IpcError> {
    routed::list_orgs(&backend, &store).await
}

#[tauri::command]
pub async fn org_suggestions(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<OrgSuggestion>, IpcError> {
    routed::org_suggestions(&backend, &store).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::service::orgs::{self, OrgScope};
    use fleet_core::service::work::WorkArgs;

    fn read(action: &str) -> WorkArgs {
        WorkArgs {
            action: Some(action.into()),
            ..Default::default()
        }
    }

    pub async fn work_scopes(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ScopeEntry>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("work_scopes", &read("scopes")).await,
            None => orgs::scopes(store, &OrgScope::All),
        }
    }

    pub async fn list_orgs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<OrgDetail>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_orgs", &read("orgs")).await,
            None => orgs::org_details(store, &OrgScope::All),
        }
    }

    pub async fn org_suggestions(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<OrgSuggestion>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("org_suggestions", &read("org_suggestions")).await,
            None => orgs::org_suggestions(store, &OrgScope::All),
        }
    }
}
