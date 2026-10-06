//! Organisation administration, phase B
//! (`docs/superpowers/specs/2026-10-06-org-administration-design.md`): the
//! company's devices and people, and its orgs, administered from the
//! desktop.
//!
//! Before this module the hub's `fleet-hub client|person|org …` commands
//! were the only way to do any of it, so a desktop paired to a hub showed
//! Settings → Organisations read-only. The hub tool `org_admin`
//! (`Access::PersonDevice`: the hub owner's own device bound to no org)
//! runs these actions, and a write needs that device to be trusted and
//! `full` — the same person `settings_writer` lets change the fleet's
//! settings. A standalone desktop runs them on its own store.
//!
//! The org actions are `orgs::admin`'s, unchanged: this module only lets a
//! second caller reach them. What it adds is the device and people half,
//! and three rules every write keeps:
//!
//! * **No lock-out.** The device a caller is using cannot be revoked,
//!   untrusted, bound to an org or handed to someone else through it — any
//!   of those would take away the very access it is using. `fleet-hub` on
//!   the hub machine still can.
//! * **Machine tokens are not devices.** A peer hub link and an updater
//!   token (`store::machine_token_kind`) are not listed and refused by every
//!   device action; `fleet-hub peer` and `fleet-hub pair --mode updater`
//!   manage them.
//! * **The owner cannot be disabled** (`Store::disable_person` refuses it).

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgAction};
use crate::service::trackers::admin::WorkAdminArgs;
use crate::store::{ClientTokenRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "OrgAdminParams")]
pub struct OrgAdminArgs {
    /// list_orgs|add_org|update_org|remove_org|add_rule|remove_rule|assign_host|unassign_host|assign_tracker|set_org_setting|list_devices|pair_device|revoke_device|set_device_trust|bind_device|set_device_person|grant_catalog|list_people|rename_person|disable_person
    pub action: String,
    /// Org name (add/update_org), or a person's new name (rename_person).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The org acted on (or bound to).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// An org by name, instead of org_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// `#rrggbb`; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Fence sessions between this org and the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolate_sessions: Option<bool>,
    /// on|off|inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_tidy: Option<String>,
    /// on|off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<String>,
    /// Its bound devices also see unassigned work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_sees_unassigned: Option<bool>,
    /// remove_rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// Rule: a GitHub owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Rule: one repository of `owner`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Rule: a path prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    /// Rule, or assign_host / unassign_host: a host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// assign_tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// A paired device, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// full|readonly (pair_device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Its prompts reach agents unmarked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
    /// Whose device, by name; created when new. Absent: the hub's owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// rename_person / disable_person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    /// rename_person; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// A catalog, by name (grant_catalog).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// Grant (true) or take back (false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// Pairing code lifetime, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_s: Option<u64>,
    /// set_org_setting: a setting an org may set for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// set_org_setting: the org's own value; absent inherits the fleet's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl OrgAdminArgs {
    pub fn new(action: &str) -> Self {
        OrgAdminArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    /// The audit line: names and ids only (there is no secret here).
    pub fn audit_summary(&self) -> String {
        let mut out = format!("action={}", self.action.escape_debug());
        for (k, v) in [
            ("name", &self.name),
            ("org", &self.org),
            ("device", &self.device),
            ("person", &self.person),
            ("catalog", &self.catalog),
            ("mode", &self.mode),
            ("key", &self.key),
            ("value", &self.value),
        ] {
            if let Some(v) = v {
                out.push_str(&format!(" {k}={}", v.escape_debug()));
            }
        }
        for (k, v) in [
            ("org_id", self.org_id),
            ("rule_id", self.rule_id),
            ("tracker_id", self.tracker_id),
            ("person_id", self.person_id),
        ] {
            if let Some(v) = v {
                out.push_str(&format!(" {k}={v}"));
            }
        }
        for (k, v) in [("trusted", self.trusted), ("on", self.on)] {
            if let Some(v) = v {
                out.push_str(&format!(" {k}={v}"));
            }
        }
        out
    }
}

/// One `org_admin` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// One of `orgs::admin`'s, run unchanged.
    Org(OrgAction),
    /// An org's own value of a per-org setting (phase C).
    SetOrgSetting,
    ListDevices,
    /// Minted by the hub's pairing registry, so only the MCP tool runs it.
    PairDevice,
    RevokeDevice,
    SetDeviceTrust,
    BindDevice,
    SetDevicePerson,
    GrantCatalog,
    ListPeople,
    RenamePerson,
    DisablePerson,
}

impl Action {
    pub fn parse(s: &str) -> Result<Self, IpcError> {
        Ok(match s {
            "set_org_setting" => Action::SetOrgSetting,
            "list_devices" => Action::ListDevices,
            "pair_device" => Action::PairDevice,
            "revoke_device" => Action::RevokeDevice,
            "set_device_trust" => Action::SetDeviceTrust,
            "bind_device" => Action::BindDevice,
            "set_device_person" => Action::SetDevicePerson,
            "grant_catalog" => Action::GrantCatalog,
            "list_people" => Action::ListPeople,
            "rename_person" => Action::RenamePerson,
            "disable_person" => Action::DisablePerson,
            // A device's org is `bind_device`, which keeps the lock-out rule
            // `assign_client` does not know about.
            other => match OrgAction::parse(other) {
                Some(OrgAction::AssignClient) | None => {
                    return Err(IpcError::new(
                        codes::E_INVALID,
                        format!("org_admin has no action {other:?}"),
                    ))
                }
                Some(a) => Action::Org(a),
            },
        })
    }

    /// Changes nothing: any of the owner's devices may run it, trusted or
    /// not.
    pub fn is_read(self) -> bool {
        matches!(
            self,
            Action::Org(OrgAction::ListOrgs) | Action::ListDevices | Action::ListPeople
        )
    }
}

/// Who is asking, as far as the lock-out rule cares: the name of the device
/// the call comes through, or `None` for the desktop's own store.
#[derive(Debug, Clone, Copy)]
pub struct Me<'a> {
    pub device: Option<&'a str>,
}

impl Me<'_> {
    pub const LOCAL: Me<'static> = Me { device: None };
}

/// A person's paired device, as Settings → Devices lists it. Never carries
/// the token digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub name: String,
    /// `full` or `readonly`.
    pub mode: String,
    pub trusted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// The org's name, when bound to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    /// Whose device it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
    /// The asset catalogs it may change, by name.
    #[serde(default)]
    pub catalogs: Vec<String>,
    /// The device this list was read through.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub this_device: bool,
}

/// A person this hub knows, as Settings → People lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonSummary {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// This hub's owner, who cannot be disabled.
    pub owner: bool,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<i64>,
    /// Their live devices, by name.
    #[serde(default)]
    pub devices: Vec<String>,
}

/// Whose hosts' unclaimed counts `person` is served (phase D): every host
/// for a host administrator — the hub's owner, or an admin of the company
/// that owns the hub (owner's answer 1) — else the hosts of each org they
/// administer whose `admins_see_unclaimed` the hub's owner turned on
/// (answer 3).
pub fn unclaimed_reach(
    s: &Store,
    person: i64,
) -> Result<crate::service::view_scope::UnclaimedReach, IpcError> {
    use crate::service::view_scope::UnclaimedReach;
    if matches!(s.personal_owner_id()?, Some(o) if o == person) {
        return Ok(UnclaimedReach::Every);
    }
    let admin_of: Vec<i64> = s
        .memberships_of(person)?
        .into_iter()
        .filter(|m| m.is_live() && m.role == crate::store::ROLE_ADMIN)
        .map(|m| m.org_id)
        .collect();
    if admin_of.is_empty() {
        return Ok(UnclaimedReach::None);
    }
    if let Some(h) = s.hub_owner_org()? {
        if admin_of.contains(&h) {
            return Ok(UnclaimedReach::Every);
        }
    }
    let mut orgs = std::collections::BTreeSet::new();
    for o in admin_of {
        if s.get_org(o)?.is_some_and(|r| r.admins_see_unclaimed) {
            orgs.insert(o);
        }
    }
    Ok(if orgs.is_empty() {
        UnclaimedReach::None
    } else {
        UnclaimedReach::Orgs(orgs)
    })
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

fn need<'a, T>(v: &'a Option<T>, action: &str, field: &str) -> Result<&'a T, IpcError> {
    v.as_ref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{action} needs {field}")))
}

/// The org `args` names, by id or by name; `None` when it names none.
fn org_of(s: &Store, args: &OrgAdminArgs) -> Result<Option<i64>, IpcError> {
    if let Some(id) = args.org_id {
        return Ok(Some(id));
    }
    let Some(name) = args.org.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    s.list_orgs()?
        .into_iter()
        .find(|o| o.name == name)
        .map(|o| Some(o.id))
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no org named {name:?}")))
}

/// The live person's device `name`, or why it is not one this module may
/// change: unknown, a machine token, or the device `me` is using (`what`
/// says what that would have done; pass [`Me::LOCAL`] for a change that
/// only widens).
fn device_row(s: &Store, name: &str, me: Me<'_>, what: &str) -> Result<ClientTokenRow, IpcError> {
    let wanted = name.trim();
    let row = s
        .active_client_tokens()?
        .into_iter()
        .find(|c| c.name == wanted)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("no paired device named {wanted:?}"),
            )
        })?;
    if let Some(kind) = crate::store::machine_token_kind(&row.mode) {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "{:?} is {kind}, not a person's device; manage it on the hub (fleet-hub peer, fleet-hub pair --mode updater)",
                row.name
            ),
        ));
    }
    if me.device == Some(row.name.as_str()) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "{:?} is the device you are using: {what} it from here would take away the \
                 access you are using. Do it from another trusted device, or on the hub \
                 (fleet-hub client …)",
                row.name
            ),
        ));
    }
    Ok(row)
}

/// Every live person's device, newest first.
pub fn list_devices(s: &Store, me: Me<'_>) -> Result<Vec<DeviceSummary>, IpcError> {
    let orgs: std::collections::BTreeMap<i64, String> =
        s.list_orgs()?.into_iter().map(|o| (o.id, o.name)).collect();
    let people: std::collections::BTreeMap<i64, String> = s
        .list_people()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut grants: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for c in s.list_catalogs()? {
        for holder in s.catalog_grantees(c.id)? {
            grants.entry(holder).or_default().push(c.name.clone());
        }
    }
    Ok(s.active_client_tokens()?
        .into_iter()
        .filter(|c| crate::store::machine_token_kind(&c.mode).is_none())
        .map(|c| DeviceSummary {
            org: c.org_id.and_then(|o| orgs.get(&o).cloned()),
            person: c.person_id.and_then(|p| people.get(&p).cloned()),
            catalogs: grants.remove(&c.name).unwrap_or_default(),
            this_device: me.device == Some(c.name.as_str()),
            name: c.name,
            mode: c.mode,
            trusted: c.trusted_at.is_some(),
            org_id: c.org_id,
            person_id: c.person_id,
            created_at: c.created_at,
            last_seen_at: c.last_seen_at,
        })
        .collect())
}

/// Every person this hub knows, the owner first.
pub fn list_people(s: &Store) -> Result<Vec<PersonSummary>, IpcError> {
    let devices = s.active_client_tokens()?;
    let mut out: Vec<PersonSummary> = s
        .list_people()?
        .into_iter()
        .map(|p| PersonSummary {
            devices: devices
                .iter()
                .filter(|d| d.person_id == Some(p.id))
                .map(|d| d.name.clone())
                .collect(),
            id: p.id,
            name: p.name,
            display_name: p.display_name,
            owner: p.is_personal_owner,
            created_at: p.created_at,
            disabled_at: p.disabled_at,
        })
        .collect();
    out.sort_by_key(|p| (!p.owner, p.disabled_at.is_some(), p.id));
    Ok(out)
}

fn device_json(s: &Store, name: &str, me: Me<'_>) -> Result<serde_json::Value, IpcError> {
    let row = list_devices(s, me)?
        .into_iter()
        .find(|d| d.name == name)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("no paired device named {name:?}"),
            )
        })?;
    to_json(&row)
}

/// Run one action except `pair_device` (the MCP tool mints codes through the
/// hub's pairing registry). The caller already decided `me` may write when
/// the action is not a read.
pub fn run(
    args: &OrgAdminArgs,
    store: &Mutex<Store>,
    me: Me<'_>,
) -> Result<serde_json::Value, IpcError> {
    let action = Action::parse(&args.action)?;
    let s = lock(store)?;
    let name = args.action.as_str();
    match action {
        Action::Org(a) => {
            let org_id = match a {
                OrgAction::AddOrg => None,
                _ => org_of(&s, args)?,
            };
            orgs::admin(
                a,
                &WorkAdminArgs {
                    action: args.action.clone(),
                    name: args.name.clone(),
                    org_id,
                    color: args.color.clone(),
                    isolate_sessions: args.isolate_sessions,
                    auto_tidy: args.auto_tidy.clone(),
                    jev: args.jev.clone(),
                    bound_sees_unassigned: args.bound_sees_unassigned,
                    rule_id: args.rule_id,
                    owner: args.owner.clone(),
                    repo: args.repo.clone(),
                    path_prefix: args.path_prefix.clone(),
                    host_alias: args.host_alias.clone(),
                    tracker_id: args.tracker_id,
                    ..Default::default()
                },
                &s,
            )
        }
        Action::SetOrgSetting => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "set_org_setting needs org_id or org")
            })?;
            let key = need(&args.key, name, "key")?;
            let actor = match me.device {
                Some(d) => crate::service::settings::Actor::PersonVia(d),
                None => crate::service::settings::Actor::Person,
            };
            crate::service::settings::set_for_org(&s, org, key, args.value.as_deref(), actor)?;
            to_json(&crate::service::settings::org_settings(&s, org))
        }
        Action::ListDevices => to_json(&list_devices(&s, me)?),
        Action::ListPeople => to_json(&list_people(&s)?),
        Action::PairDevice => Err(IpcError::new(
            codes::E_INVALID_STATE,
            "pairing codes are minted by a hub (fleet-hub pair); pair this desktop with one first",
        )),
        Action::RevokeDevice => {
            let row = device_row(&s, need(&args.device, name, "device")?, me, "revoking")?;
            s.revoke_client_token(&row.name)?;
            tracing::info!(client = %row.name, "[org_admin] revoked a device");
            Ok(serde_json::json!({ "revoked": row.name }))
        }
        Action::SetDeviceTrust => {
            let on = *need(&args.trusted, name, "trusted")?;
            let device = need(&args.device, name, "device")?;
            // Trusting the device in hand takes nothing away; only the
            // withdrawal is a lock-out.
            let row = if on {
                device_row(&s, device, Me::LOCAL, "")?
            } else {
                device_row(&s, device, me, "untrusting")?
            };
            s.set_client_trust(&row.name, on)?;
            device_json(&s, &row.name, me)
        }
        Action::BindDevice => {
            let row = device_row(&s, need(&args.device, name, "device")?, me, "binding")?;
            let org = org_of(&s, args)?;
            // Through `orgs::admin`, so the org generation moves and the
            // device's streams re-read their scope at once.
            orgs::admin(
                OrgAction::AssignClient,
                &WorkAdminArgs {
                    action: "assign_client".into(),
                    name: Some(row.name.clone()),
                    org_id: org,
                    ..Default::default()
                },
                &s,
            )?;
            device_json(&s, &row.name, me)
        }
        Action::SetDevicePerson => {
            let row = device_row(&s, need(&args.device, name, "device")?, me, "handing over")?;
            let person = match args
                .person
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                Some(p) => {
                    let p = crate::store::validate_person_name(p)?;
                    Some(match s.get_person_by_name(&p)? {
                        Some(existing) => existing,
                        None => s.create_person(&p, None)?,
                    })
                }
                None => {
                    return Err(IpcError::new(
                        codes::E_INVALID,
                        "set_device_person needs person: a device belongs to somebody \
                         (revoke it to end its access)",
                    ))
                }
            };
            s.set_client_person(&row.name, person.as_ref().map(|p| p.id))?;
            device_json(&s, &row.name, me)
        }
        Action::GrantCatalog => {
            let on = *need(&args.on, name, "on")?;
            let device = need(&args.device, name, "device")?;
            // A grant only widens; taking back the caller's own is a
            // lock-out like any other.
            let row = if on {
                device_row(&s, device, Me::LOCAL, "")?
            } else {
                device_row(&s, device, me, "taking a grant from")?
            };
            let catalog = need(&args.catalog, name, "catalog")?.trim().to_string();
            if catalog == crate::service::catalog::catalogs::PERSONAL {
                s.set_client_assets_admin(&row.name, on)?;
            } else {
                let id = s
                    .get_catalog_by_name(&catalog)?
                    .ok_or_else(|| {
                        IpcError::new(codes::E_NOTFOUND, format!("no catalog named {catalog:?}"))
                    })?
                    .id;
                s.set_client_catalog_grant(&row.name, id, on)?;
            }
            device_json(&s, &row.name, me)
        }
        Action::RenamePerson => {
            let id = *need(&args.person_id, name, "person_id")?;
            if args.name.is_none() && args.display_name.is_none() {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "rename_person needs name and/or display_name",
                ));
            }
            to_json(&s.rename_person(id, args.name.as_deref(), args.display_name.as_deref())?)
        }
        Action::DisablePerson => {
            let id = *need(&args.person_id, name, "person_id")?;
            if let Some(mine) = me.device {
                let owns = s
                    .active_client_tokens()?
                    .into_iter()
                    .any(|c| c.name == mine && c.person_id == Some(id));
                if owns {
                    return Err(IpcError::new(
                        codes::E_INVALID_STATE,
                        "that is the person this device belongs to: disabling them would \
                         revoke the device you are using",
                    ));
                }
            }
            to_json(&s.disable_person(id)?)
        }
    }
}

#[cfg(test)]
#[path = "org_admin_tests.rs"]
mod tests;
