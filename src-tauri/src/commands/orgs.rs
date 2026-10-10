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
use fleet_core::service::orgs::{OrgDetail, OrgSuggestion};
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
    /// Jev evaluation (D48): `on` | `off`, the org's second consent, to
    /// reply text (the org's `decide.jev.reply_consent` row, J2 `turn_outcome`).
    #[serde(default)]
    pub jev_reply: Option<String>,
    /// D31 (work graph M14): the org's bound devices also see unassigned
    /// work and sessions.
    #[serde(default)]
    pub bound_sees_unassigned: Option<bool>,
    /// Org administration phase D: this company owns the hub.
    #[serde(default)]
    pub owns_hub: Option<bool>,
    /// Phase D: its admins see the unclaimed count on its hosts.
    #[serde(default)]
    pub admins_see_unclaimed: Option<bool>,
    /// M15 step G2.10: members see only their own sessions (default on).
    #[serde(default)]
    pub members_own_sessions_only: Option<bool>,
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
    /// M15 step G2.10, the org form's one rule form: what `value` is —
    /// `repository` (owner/name), `path`, `host` or `owner`. Turned into
    /// the fields above here, so a hub of any age takes the rule.
    #[serde(default)]
    pub match_by: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

impl AddOrgRuleArgs {
    /// `org_admin`'s arguments for `action`: the rule's fields, from
    /// `match_by` + `value` when given.
    fn admin_args(&self, action: &str) -> Result<OrgAdminArgs, IpcError> {
        let rule = match self.match_by.as_deref() {
            Some(by) => fleet_core::service::orgs::rule_from_match(
                self.org_id,
                by,
                self.value.as_deref().unwrap_or(""),
            )?,
            None => OrgRuleRow {
                id: 0,
                org_id: self.org_id,
                owner: self.owner.clone(),
                repo: self.repo.clone(),
                path_prefix: self.path_prefix.clone(),
                host_alias: self.host_alias.clone(),
            },
        };
        Ok(OrgAdminArgs {
            org_id: Some(self.org_id),
            owner: rule.owner,
            repo: rule.repo,
            path_prefix: rule.path_prefix,
            host_alias: rule.host_alias,
            ..OrgAdminArgs::new(action)
        })
    }
}

/// M15 step G2.10: an entry of the org's project catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddOrgProjectArgs {
    pub org_id: i64,
    pub name: String,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// Host aliases, comma-separated; empty = every host of the org.
    #[serde(default)]
    pub hosts: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectIdArgs {
    pub project_id: i64,
}

/// One share on one of an org's sessions (M15 step G4.7).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgShareArgs {
    pub org_id: i64,
    pub grant_id: i64,
}

/// One person of an org, current or former (M15 step G4.7).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgPersonArgs {
    pub org_id: i64,
    pub person_id: i64,
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
                jev_reply: args.jev_reply,
                bound_sees_unassigned: args.bound_sees_unassigned,
                owns_hub: args.owns_hub,
                admins_see_unclaimed: args.admins_see_unclaimed,
                members_own_sessions_only: args.members_own_sessions_only,
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
    decode(routed::add_org_rule(&backend, &store, args.admin_args("add_rule")?).await?)
}

/// M15 step G2.10: what a rule would match and move now, before it is
/// added (the rule form's live impact). Writes nothing.
#[tauri::command]
pub async fn org_rule_preview(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddOrgRuleArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::org_rule_preview(&backend, &store, args.admin_args("rule_preview")?).await
}

/// M15 step G2.10: add an entry to the org's project catalog.
#[tauri::command]
pub async fn add_org_project(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddOrgProjectArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::add_org_project(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            name: Some(args.name),
            remote: args.remote,
            path: args.path,
            hosts: args.hosts,
            ..OrgAdminArgs::new("add_project")
        },
    )
    .await
}

#[tauri::command]
pub async fn remove_org_project(
    backend: State<'_, Arc<FleetBackend>>,
    args: ProjectIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::remove_org_project(
        &backend,
        &store,
        OrgAdminArgs {
            project_id: Some(args.project_id),
            ..OrgAdminArgs::new("remove_project")
        },
    )
    .await
}

/// M15 step G4.7: take back one share on the org's sessions (Sharing tab).
#[tauri::command]
pub async fn revoke_org_share(
    backend: State<'_, Arc<FleetBackend>>,
    args: OrgShareArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::revoke_org_share(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            grant_id: Some(args.grant_id),
            ..OrgAdminArgs::new("revoke_share")
        },
    )
    .await
}

/// M15 step G4.7: narrow one share on the org's sessions to watch.
#[tauri::command]
pub async fn narrow_org_share(
    backend: State<'_, Arc<FleetBackend>>,
    args: OrgShareArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::narrow_org_share(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            grant_id: Some(args.grant_id),
            ..OrgAdminArgs::new("narrow_share")
        },
    )
    .await
}

/// M15 step G4.7: take back every share a member, current or former, holds on
/// the org's sessions ("removed 3 d ago · Take back their shares").
#[tauri::command]
pub async fn revoke_org_member_grants(
    backend: State<'_, Arc<FleetBackend>>,
    args: OrgPersonArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::revoke_org_member_grants(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            person_id: Some(args.person_id),
            ..OrgAdminArgs::new("revoke_member_grants")
        },
    )
    .await
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

/// Org administration phase C: an org's own value of a per-org setting;
/// `value: None` inherits the fleet's again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetOrgSettingArgs {
    pub org_id: i64,
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[tauri::command]
pub async fn set_org_setting(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetOrgSettingArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::set_org_setting(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            key: Some(args.key),
            value: args.value,
            ..OrgAdminArgs::new("set_org_setting")
        },
    )
    .await
}

/// Org administration phase D: add a person to an org (a new name becomes
/// a person), or change their role.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetOrgMemberArgs {
    pub org_id: i64,
    #[serde(default)]
    pub person: Option<String>,
    #[serde(default)]
    pub person_id: Option<i64>,
    pub role: String,
}

#[tauri::command]
pub async fn set_org_member(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetOrgMemberArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::set_org_member(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            person: args.person,
            person_id: args.person_id,
            role: Some(args.role),
            ..OrgAdminArgs::new("set_member")
        },
    )
    .await
}

/// Phase D: take a person out of an org; what was shared with them on its
/// sessions goes too unless `keep_grants`. Redesign 11.2's dialog says it
/// with `grants`: `revoke` (the default), `narrow` (each Drive share becomes
/// Watch, then they leave with their shares) or `keep`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoveOrgMemberArgs {
    pub org_id: i64,
    pub person_id: i64,
    #[serde(default)]
    pub keep_grants: Option<bool>,
    #[serde(default)]
    pub grants: Option<String>,
}

#[tauri::command]
pub async fn remove_org_member(
    backend: State<'_, Arc<FleetBackend>>,
    args: RemoveOrgMemberArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::remove_org_member_choosing(&backend, &store, args).await
}

/// Redesign 11.2: how many live shares TO a member stand on the org's
/// sessions, as `{ watch, drive }` — what the remove dialog asks about.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgMemberArgs {
    pub org_id: i64,
    pub person_id: i64,
}

#[tauri::command]
pub async fn org_member_grants(
    backend: State<'_, Arc<FleetBackend>>,
    args: OrgMemberArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::org_member_grants(
        &backend,
        &store,
        OrgAdminArgs {
            org_id: Some(args.org_id),
            person_id: Some(args.person_id),
            ..OrgAdminArgs::new("member_grants")
        },
    )
    .await
}

// --- reads (routed) ------------------------------------------------------------

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

    pub async fn list_orgs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<OrgDetail>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_orgs", &read("orgs")).await,
            None => orgs::org_details(store, &ViewScope::internal(), orgs::AdminView::Admin),
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

    pub async fn set_org_setting(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_org_setting", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn set_org_member(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_org_member", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn remove_org_member(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("remove_org_member", &args).await,
            None => local(&args, store),
        }
    }

    /// [`super::remove_org_member`]: `grants` said with the actions every
    /// hub that has `remove_member` already knows.
    pub async fn remove_org_member_choosing(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: RemoveOrgMemberArgs,
    ) -> Result<serde_json::Value, IpcError> {
        let member = |action: &str| OrgAdminArgs {
            org_id: Some(args.org_id),
            person_id: Some(args.person_id),
            ..OrgAdminArgs::new(action)
        };
        let keep = match args.grants.as_deref() {
            None => args.keep_grants,
            Some("revoke") => Some(false),
            Some("keep") => Some(true),
            Some("narrow") => {
                // Narrowed first, then removed keeping them: an older hub
                // never sees `grants`, only two actions it already has.
                let narrowed =
                    remove_org_member(backend, store, member("narrow_member_grants")).await?;
                let mut out = remove_org_member(
                    backend,
                    store,
                    OrgAdminArgs {
                        keep_grants: Some(true),
                        ..member("remove_member")
                    },
                )
                .await?;
                if let Some(o) = out.as_object_mut() {
                    o.insert("narrowed".into(), narrowed["narrowed"].clone());
                }
                return Ok(out);
            }
            Some(other) => {
                return Err(IpcError::new(
                    fleet_core::ipc_error::codes::E_INVALID,
                    format!("grants is revoke, narrow or keep, not {other:?}"),
                ))
            }
        };
        remove_org_member(
            backend,
            store,
            OrgAdminArgs {
                keep_grants: keep,
                ..member("remove_member")
            },
        )
        .await
    }

    pub async fn org_member_grants(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("org_member_grants", &args).await,
            None => local(&args, store),
        }
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

    pub async fn org_rule_preview(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("org_rule_preview", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn add_org_project(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("add_org_project", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn remove_org_project(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("remove_org_project", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn revoke_org_share(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("revoke_org_share", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn narrow_org_share(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("narrow_org_share", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn revoke_org_member_grants(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("revoke_org_member_grants", &args).await,
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
