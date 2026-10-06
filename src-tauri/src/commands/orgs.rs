//! Tauri commands for organisations (work graph M5): read the scopes, the
//! orgs and the suggestions (routed to the hub's `work`), and administer
//! orgs, rules and assignments (thin wrappers over the same
//! `fleet_core::service::orgs::admin` the hub's master-only `work_admin`
//! runs).
//!
//! The admin commands route to the hub's `org_admin` (org administration
//! phase B): an org and a host's place in it are the per-host tokens'
//! security boundary, so the hub lets only its owner's own trusted `full`
//! device change them — never a host, an org-bound device or a colleague's.
//! Standalone, they run `service::org_admin` on this desktop's store.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::org_admin::OrgAdminArgs;
use fleet_core::service::orgs::{OrgDetail, OrgSuggestion, ScopeEntry};
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
    /// Jev evaluation (D31): `on` | `off`, the org's consent to decision-
    /// model calls (`orgs.jev_allowed`).
    #[serde(default)]
    pub jev: Option<String>,
    /// D31 (work graph M14): the org's bound devices also see unassigned
    /// work and sessions.
    #[serde(default)]
    pub bound_sees_unassigned: Option<bool>,
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

// --- administration (routed to `org_admin`) -------------------------------------
//
// `async` so the transactional store work runs on the async runtime and not
// on the macOS main thread, where a sync command would run it (CLAUDE.md: no
// blocking I/O on a sync Tauri command).

#[tauri::command]
pub async fn add_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgRow, IpcError> {
    decode(
        routed::add_org(
            &backend,
            &store,
            OrgAdminArgs {
                name: Some(args.name),
                color: args.color,
                isolate_sessions: Some(args.isolate_sessions),
                ..OrgAdminArgs::new("add_org")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn update_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: UpdateOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgRow, IpcError> {
    decode(
        routed::update_org(
            &backend,
            &store,
            OrgAdminArgs {
                org_id: Some(args.org_id),
                name: args.name,
                color: args.color,
                isolate_sessions: args.isolate_sessions,
                auto_tidy: args.auto_tidy,
                jev: args.jev,
                bound_sees_unassigned: args.bound_sees_unassigned,
                ..OrgAdminArgs::new("update_org")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn remove_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: OrgIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    routed::remove_org(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            ..OrgAdminArgs::new("remove_org")
        },
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn add_org_rule(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddOrgRuleArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgRuleRow, IpcError> {
    decode(
        routed::add_org_rule(
            &backend,
            &store,
            OrgAdminArgs {
                org_id: Some(args.org_id),
                owner: args.owner,
                repo: args.repo,
                path_prefix: args.path_prefix,
                host_alias: args.host_alias,
                ..OrgAdminArgs::new("add_rule")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn remove_org_rule(
    backend: State<'_, Arc<FleetBackend>>,
    args: RuleIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    routed::remove_org_rule(
        &backend,
        &store,
        OrgAdminArgs {
            rule_id: Some(args.rule_id),
            ..OrgAdminArgs::new("remove_rule")
        },
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn assign_host_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssignHostOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    let action = if args.org_id.is_some() {
        "assign_host"
    } else {
        "unassign_host"
    };
    routed::assign_host_org(
        &backend,
        &store,
        OrgAdminArgs {
            host_alias: Some(args.host_alias),
            org_id: args.org_id,
            ..OrgAdminArgs::new(action)
        },
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn assign_tracker_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssignTrackerOrgArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TrackerRow, IpcError> {
    decode(
        routed::assign_tracker_org(
            &backend,
            &store,
            OrgAdminArgs {
                tracker_id: Some(args.tracker_id),
                org_id: args.org_id,
                ..OrgAdminArgs::new("assign_tracker")
            },
        )
        .await?,
    )
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
    use fleet_core::service::orgs;
    use fleet_core::service::view_scope::ViewScope;
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
            None => orgs::scopes(store, &ViewScope::internal()),
        }
    }

    pub async fn list_orgs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<OrgDetail>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_orgs", &read("orgs")).await,
            None => orgs::org_details(store, &ViewScope::internal(), orgs::DeviceView::Shown),
        }
    }

    /// This desktop's own store, standalone: `service::org_admin`, as the
    /// hub's `org_admin` tool runs it.
    pub(crate) fn local(
        args: &OrgAdminArgs,
        store: &Mutex<Store>,
    ) -> Result<serde_json::Value, IpcError> {
        fleet_core::service::org_admin::run(args, store, fleet_core::service::org_admin::Me::LOCAL)
    }

    pub async fn add_org(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("add_org", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn update_org(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("update_org", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn remove_org(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("remove_org", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn add_org_rule(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("add_org_rule", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn remove_org_rule(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("remove_org_rule", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn assign_host_org(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("assign_host_org", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn assign_tracker_org(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("assign_tracker_org", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn org_suggestions(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<OrgSuggestion>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("org_suggestions", &read("org_suggestions")).await,
            None => orgs::org_suggestions(store, &ViewScope::internal()),
        }
    }
}
