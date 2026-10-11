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

//!
//! **Phase D: an org's admin, for that org.** [`Authority`] is who is
//! asking: [`Authority::Fleet`] (the desktop's own store, the hub owner's
//! unbound device) reaches every action, as above; [`Authority::Org`] — a
//! person's device fenced to an org they administer — reaches its own org
//! only, and [`check`] refuses everything else before a row is touched:
//! rules, tracker routing, `bound_sees_unassigned`, which company owns the
//! hub, the unclaimed-count switch, other orgs, people's names and disabling
//! stay the hub owner's. Routing a host into an org is a host
//! administrator's (owner's answer 1): the hub's owner, or an admin of the
//! company that owns the hub. The lists are narrowed to the org's own
//! devices and members.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgAction};
use crate::service::trackers::admin::WorkAdminArgs;
use crate::store::{ClientTokenRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "OrgAdminParams")]
pub struct OrgAdminArgs {
    /// list_orgs|add_org|update_org|remove_org|add_rule|remove_rule|assign_host|unassign_host|assign_tracker|set_org_setting|list_devices|pair_device|revoke_device|set_device_trust|rename_device|set_device_mode|bind_device|set_device_person|grant_catalog|list_people|add_person|rename_person|disable_person|list_members|set_member|remove_member|member_grants|revoke_member_grants|narrow_member_grants|set_hub_org|set_admins_see_unclaimed|rule_preview|add_project|remove_project|revoke_share|narrow_share
    pub action: String,
    /// Org name (add/update_org), or a person's or device's new name
    /// (rename_person, rename_device).
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
    /// on|off: the org's reply-text consent (D48), on top of jev.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_reply: Option<String>,
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
    /// full|readonly (pair_device, set_device_mode).
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
    /// set_member: admin|member|viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// remove_member: keep what was shared with them on the org's sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_grants: Option<bool>,
    /// update_org: this company owns the hub (hub owner only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owns_hub: Option<bool>,
    /// update_org: its admins see the unclaimed count (hub owner only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admins_see_unclaimed: Option<bool>,
    /// update_org: members see only their own sessions (default on); off,
    /// they watch each other's sessions in the org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members_own_sessions_only: Option<bool>,
    /// add_project: its git remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// add_project: where it is checked out on the hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// add_project: host aliases it may run on, comma-separated; empty =
    /// every host of the org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<String>,
    /// remove_project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// revoke_share / narrow_share: the share (a session grant) on one of
    /// the org's sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<i64>,
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
            ("role", &self.role),
            ("remote", &self.remote),
            ("path", &self.path),
            ("hosts", &self.hosts),
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
            ("project_id", self.project_id),
            ("grant_id", self.grant_id),
        ] {
            if let Some(v) = v {
                out.push_str(&format!(" {k}={v}"));
            }
        }
        for (k, v) in [
            ("trusted", self.trusted),
            ("on", self.on),
            ("keep_grants", self.keep_grants),
            ("owns_hub", self.owns_hub),
            ("admins_see_unclaimed", self.admins_see_unclaimed),
            ("members_own_sessions_only", self.members_own_sessions_only),
        ] {
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
    RenameDevice,
    SetDeviceMode,
    BindDevice,
    SetDevicePerson,
    GrantCatalog,
    ListPeople,
    /// M15 step G7.14: "+ Person" — someone the hub knows before a device
    /// is paired to them (a member added ahead of their phone).
    AddPerson,
    RenamePerson,
    DisablePerson,
    // Phase D: members and roles.
    ListMembers,
    /// Add a person to the org, or change their role.
    SetMember,
    RemoveMember,
    /// How many live grants TO a member stand on the org's sessions.
    MemberGrants,
    RevokeMemberGrants,
    NarrowMemberGrants,
    /// Which company owns the hub (owner's answer 1).
    SetHubOrg,
    /// The org's admins see the unclaimed count on its hosts (answer 3).
    SetAdminsSeeUnclaimed,
    /// M15 step G2.10: what a rule would match and move, before it is added.
    RulePreview,
    /// M15 step G2.10: the org's project catalog.
    AddProject,
    RemoveProject,
    /// M15 step G4.7: take back or narrow to watch one share on the org's
    /// sessions, from its Sharing tab. Downward only.
    RevokeShare,
    NarrowShare,
}

impl Action {
    pub fn parse(s: &str) -> Result<Self, IpcError> {
        Ok(match s {
            "set_org_setting" => Action::SetOrgSetting,
            "list_devices" => Action::ListDevices,
            "pair_device" => Action::PairDevice,
            "revoke_device" => Action::RevokeDevice,
            "set_device_trust" => Action::SetDeviceTrust,
            "rename_device" => Action::RenameDevice,
            "set_device_mode" => Action::SetDeviceMode,
            "bind_device" => Action::BindDevice,
            "set_device_person" => Action::SetDevicePerson,
            "grant_catalog" => Action::GrantCatalog,
            "list_people" => Action::ListPeople,
            "add_person" => Action::AddPerson,
            "rename_person" => Action::RenamePerson,
            "disable_person" => Action::DisablePerson,
            "list_members" => Action::ListMembers,
            "set_member" => Action::SetMember,
            "remove_member" => Action::RemoveMember,
            "member_grants" => Action::MemberGrants,
            "revoke_member_grants" => Action::RevokeMemberGrants,
            "narrow_member_grants" => Action::NarrowMemberGrants,
            "set_hub_org" => Action::SetHubOrg,
            "set_admins_see_unclaimed" => Action::SetAdminsSeeUnclaimed,
            "rule_preview" => Action::RulePreview,
            "add_project" => Action::AddProject,
            "remove_project" => Action::RemoveProject,
            "revoke_share" => Action::RevokeShare,
            "narrow_share" => Action::NarrowShare,
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
            Action::Org(OrgAction::ListOrgs)
                | Action::ListDevices
                | Action::ListPeople
                | Action::ListMembers
                | Action::MemberGrants
                | Action::RulePreview
        )
    }
}

/// Who is asking, as far as WHAT they may administer goes (phase D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// The desktop's own store, or the hub owner's own unbound device:
    /// every action.
    Fleet,
    /// An admin of `org`, through a device fenced to it. `hub`: that org
    /// owns the hub, so they also route hosts (owner's answer 1).
    Org { org: i64, hub: bool },
}

/// The authority of a person's device, or `None` when it has none:
/// `fleet` is the hub owner's unbound device (`Access::PersonDevice`'s
/// rule); otherwise the device's (effective) org, when its person
/// administers it — the hub's owner counts as every org's admin. A former
/// member's device ([`crate::store::NO_ORG`]) administers nothing.
pub fn authority_for(
    s: &Store,
    fleet: bool,
    person: Option<i64>,
    device_org: Option<i64>,
) -> Result<Option<Authority>, IpcError> {
    if fleet {
        return Ok(Some(Authority::Fleet));
    }
    let (Some(p), Some(o)) = (person, device_org) else {
        return Ok(None);
    };
    if o == crate::store::NO_ORG || s.get_org(o)?.is_none() {
        return Ok(None);
    }
    let owner = matches!(s.personal_owner_id()?, Some(x) if x == p);
    let admin = owner || s.org_role(o, p)?.as_deref() == Some(crate::store::ROLE_ADMIN);
    Ok(admin.then(|| Authority::Org {
        org: o,
        hub: s.hub_owner_org().ok().flatten() == Some(o),
    }))
}

/// Who is asking, as far as the lock-out rule cares: the name of the device
/// the call comes through, or `None` for the desktop's own store.
#[derive(Debug, Clone, Copy)]
pub struct Me<'a> {
    pub device: Option<&'a str>,
    /// The device's person (phase D: an admin does not change their own
    /// membership through it).
    pub person: Option<i64>,
    pub authority: Authority,
}

impl Me<'_> {
    pub const LOCAL: Me<'static> = Me {
        device: None,
        person: None,
        authority: Authority::Fleet,
    };
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
    /// M15 step G7.14: what it is, `desktop` or `phone`, from the app's last
    /// `X-Fleet-Client` header. Absent until it has sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// G7.14: what it runs, "phone · fleet-mobile 0.5.4", as the People &
    /// devices table shows it under the name. Absent with `kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

/// A device's kind and app line from what its app last said it runs
/// (`update_observed`, `client:<token id>`): `None` for a component that is
/// not a person's app.
pub fn device_app(component: &str, version: &str) -> Option<(String, String)> {
    let (kind, app) = match component {
        "desktop" => ("desktop", "fleet desktop"),
        "android" | "ios" => ("phone", "fleet-mobile"),
        _ => return None,
    };
    Some((kind.to_string(), format!("{kind} · {app} {version}")))
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

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Fill each member's live and private counts (redesign 11.7c) as `viewer`
/// may learn them: every live session of `org` the member owns, and of those
/// the ones `viewer` neither owns nor holds a grant on. `None` is the
/// desktop's own store, which reads every row, so nothing is private to it.
pub fn count_member_sessions(
    s: &Store,
    org: i64,
    viewer: Option<i64>,
    members: &mut [MemberSummary],
) -> Result<(), IpcError> {
    let grants = match viewer {
        Some(p) => s.grants_for_person(p)?,
        None => Default::default(),
    };
    // M15 step G2.10: a teammate's session is open to a fellow member.
    let team = match viewer {
        Some(p) => team_reach(s, p)?,
        None => Default::default(),
    };
    let mut by_owner: std::collections::HashMap<i64, (u32, u32)> = Default::default();
    for (owner, session) in s.live_owned_sessions_in_org(org)? {
        let open = viewer.is_none()
            || viewer == Some(owner)
            || grants.contains_key(&session)
            || team.covers(Some(org), Some(owner));
        let e = by_owner.entry(owner).or_default();
        e.0 += 1;
        if !open {
            e.1 += 1;
        }
    }
    for m in members.iter_mut() {
        let (live, private) = by_owner.get(&m.person_id).copied().unwrap_or_default();
        m.live_sessions = live;
        m.private_sessions = private;
    }
    Ok(())
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

/// Whose sessions `person` watches as a teammate (M15 step G2.10): for each
/// org they are a live member of whose "members see only their own
/// sessions" switch is off, the org's other live members. Nobody's for a
/// disabled person.
pub fn team_reach(
    s: &Store,
    person: i64,
) -> Result<crate::service::view_scope::TeamReach, IpcError> {
    let mut out = std::collections::BTreeMap::new();
    let live_person = |p: i64| -> Result<bool, IpcError> {
        Ok(s.get_person(p)?.is_some_and(|p| p.disabled_at.is_none()))
    };
    if !live_person(person)? {
        return Ok(Default::default());
    }
    for m in s.memberships_of(person)? {
        if !m.is_live()
            || s.get_org(m.org_id)?
                .is_none_or(|o| o.members_own_sessions_only)
        {
            continue;
        }
        let mut others = std::collections::BTreeSet::new();
        for t in s.org_members(m.org_id)? {
            if t.person_id != person && live_person(t.person_id)? {
                others.insert(t.person_id);
            }
        }
        if !others.is_empty() {
            out.insert(m.org_id, others);
        }
    }
    Ok(crate::service::view_scope::TeamReach::from_map(out))
}

/// One live member of an org, as the org page lists them (phase D).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSummary {
    pub person_id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// admin | member | viewer.
    pub role: String,
    pub added_at: i64,
    /// Since when an org share reaches them (redesign 11.2); absent for a
    /// viewer, who receives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares_since: Option<i64>,
    /// The hub's owner (who administers every org anyway).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner: bool,
    /// Their live devices, by name.
    #[serde(default)]
    pub devices: Vec<String>,
    /// Redesign 11.7c: their live sessions in this org, counted for whoever
    /// administers it. Absent (zero) from `set_member`'s answer.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub live_sessions: u32,
    /// Of [`Self::live_sessions`], how many the asking admin may not open:
    /// shown as existing, never named — a session's metadata is content.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub private_sessions: u32,
}

/// The live members of `org`, admins first.
pub fn list_members(s: &Store, org: i64) -> Result<Vec<MemberSummary>, IpcError> {
    let owner = s.personal_owner_id()?;
    let devices = s.active_client_tokens()?;
    let mut out = Vec::new();
    for m in s.org_members(org)? {
        let Some(p) = s.get_person(m.person_id)? else {
            continue;
        };
        if p.disabled_at.is_some() {
            continue;
        }
        out.push(MemberSummary {
            person_id: p.id,
            devices: devices
                .iter()
                .filter(|d| d.person_id == Some(p.id))
                .map(|d| d.name.clone())
                .collect(),
            name: p.name,
            display_name: p.display_name,
            role: m.role,
            added_at: m.added_at,
            shares_since: m.shares_since,
            owner: owner == Some(m.person_id),
            live_sessions: 0,
            private_sessions: 0,
        });
    }
    Ok(out)
}

fn forbidden(what: &str) -> IpcError {
    IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{what} is the hub owner's to change, not an org admin's: it decides which \
             company something belongs to, or reaches beyond one org"
        ),
    )
}

/// The refusals an org admin gets before any row is touched (phase D; see
/// the module docs). [`Authority::Fleet`] passes everything.
fn check(s: &Store, action: Action, args: &OrgAdminArgs, me: Me<'_>) -> Result<(), IpcError> {
    let Authority::Org { org, hub } = me.authority else {
        return Ok(());
    };
    let own_org = || -> Result<(), IpcError> {
        match org_of(s, args)? {
            Some(o) if o == org => Ok(()),
            Some(_) => Err(IpcError::new(
                codes::E_FORBIDDEN,
                "an org admin administers their own org only",
            )),
            None => Err(IpcError::new(
                codes::E_INVALID,
                format!("{} needs org_id or org", args.action),
            )),
        }
    };
    match action {
        Action::Org(OrgAction::ListOrgs) | Action::ListDevices | Action::ListPeople => Ok(()),
        Action::Org(OrgAction::UpdateOrg) => {
            own_org()?;
            if args.bound_sees_unassigned.is_some() {
                return Err(forbidden("whether its devices see unassigned work"));
            }
            if args.owns_hub.is_some() {
                return Err(forbidden("which company owns the hub"));
            }
            if args.admins_see_unclaimed.is_some() {
                return Err(forbidden("who sees the unclaimed count"));
            }
            // Off lets every member read the others' sessions, an org admin
            // included: wider than the counts phase D gives an admin, so it
            // is the hub owner's call. Turning it back on only narrows.
            if args.members_own_sessions_only == Some(false) {
                return Err(forbidden("whether members see each other's sessions"));
            }
            Ok(())
        }
        Action::Org(OrgAction::AssignHost | OrgAction::UnassignHost) if hub => Ok(()),
        Action::Org(OrgAction::AssignHost | OrgAction::UnassignHost) => Err(IpcError::new(
            codes::E_FORBIDDEN,
            "routing a host into an org is a host administrator's: the hub's owner, or \
             an admin of the company that owns the hub",
        )),
        Action::Org(OrgAction::AddOrg | OrgAction::RemoveOrg) => {
            Err(forbidden("adding or removing an org"))
        }
        Action::Org(OrgAction::AddRule | OrgAction::RemoveRule) | Action::RulePreview => {
            Err(forbidden("an org's routing rules"))
        }
        Action::Org(OrgAction::AssignTracker | OrgAction::AssignClient) => {
            Err(forbidden("which org a tracker or a device belongs to"))
        }
        Action::SetOrgSetting
        | Action::ListMembers
        | Action::SetMember
        | Action::RemoveMember
        | Action::MemberGrants
        | Action::RevokeMemberGrants
        | Action::NarrowMemberGrants
        | Action::AddProject
        | Action::RevokeShare
        | Action::NarrowShare => own_org(),
        // The entry's org is checked in the arm, against the entry.
        Action::RemoveProject => Ok(()),
        // The device's org is checked in the arm, against the device.
        Action::PairDevice
        | Action::RevokeDevice
        | Action::SetDeviceTrust
        | Action::RenameDevice
        | Action::SetDeviceMode
        | Action::GrantCatalog => Ok(()),
        Action::BindDevice => Err(forbidden("which org a device is bound to")),
        Action::SetDevicePerson => Err(forbidden("whose device it is")),
        Action::AddPerson | Action::RenamePerson => Err(forbidden("a person's name")),
        Action::DisablePerson => Err(forbidden("disabling a person")),
        Action::SetHubOrg => Err(forbidden("which company owns the hub")),
        Action::SetAdminsSeeUnclaimed => Err(forbidden("who sees the unclaimed count")),
    }
}

/// For an org admin: `row` is a device of their org (fenced to it, by its
/// membership) and not the hub owner's.
fn require_org_device(s: &Store, row: &ClientTokenRow, me: Me<'_>) -> Result<(), IpcError> {
    let Authority::Org { org, .. } = me.authority else {
        return Ok(());
    };
    let effective = s
        .auth_client_tokens()?
        .into_iter()
        .find(|c| c.id == row.id)
        .and_then(|c| c.org_id);
    let owners = row.person_id.is_some() && row.person_id == s.personal_owner_id()?;
    if effective != Some(org) || owners {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!("{:?} is not a device of your org's members", row.name),
        ));
    }
    Ok(())
}

/// Whether `person` already has an identity on this hub beyond `org`: a
/// device (live or revoked, so sessions it owned stay theirs) or a live
/// membership of another org. An org admin may invite a new person and pair
/// that person's first device; taking over someone who already exists would
/// let them mint a token that IS that person (review r04 F1, F2).
pub fn has_identity_beyond(s: &Store, person: i64, org: i64) -> Result<bool, IpcError> {
    let devices = s
        .list_client_tokens(true)?
        .iter()
        .any(|c| c.person_id == Some(person));
    let elsewhere = s
        .memberships_of(person)?
        .iter()
        .any(|m| m.is_live() && m.org_id != org);
    Ok(devices || elsewhere)
}

/// A person named in `args` (`person_id`, else `person` by name). `create`:
/// a new name becomes a person (an admin inviting a colleague).
fn person_of(s: &Store, args: &OrgAdminArgs, create: bool) -> Result<i64, IpcError> {
    if let Some(id) = args.person_id {
        return s
            .get_person(id)?
            .map(|p| p.id)
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no person {id}")));
    }
    let name = args
        .person
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("{} needs person or person_id", args.action),
            )
        })?;
    let name = crate::store::validate_person_name(name)?;
    match s.get_person_by_name(&name)? {
        Some(p) => Ok(p.id),
        None if create => Ok(s.create_person(&name, None)?.id),
        None => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("this hub knows no live person named {name:?}"),
        )),
    }
}

/// An org admin's own membership, and the hub owner's, are not theirs to
/// change: the first would take away the access they are using, the second
/// is the hub's.
fn require_other_member(s: &Store, person: i64, me: Me<'_>) -> Result<(), IpcError> {
    if !matches!(me.authority, Authority::Org { .. }) {
        return Ok(());
    }
    if me.person == Some(person) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "that is your own membership: another admin of the org, or the hub's owner, \
             changes it",
        ));
    }
    if s.personal_owner_id()? == Some(person) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "the hub's owner administers every org; their membership is theirs",
        ));
    }
    Ok(())
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
    // The org a device is fenced to is the one its person's memberships make
    // it (phase D), so that is the org shown — and an org admin lists only
    // their org's devices, never the hub owner's.
    let effective: std::collections::BTreeMap<i64, Option<i64>> = s
        .auth_client_tokens()?
        .into_iter()
        .map(|c| (c.id, c.org_id))
        .collect();
    let owner = s.personal_owner_id()?;
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
    let apps: std::collections::BTreeMap<String, (String, String)> = s
        .update_observed_all()?
        .into_iter()
        .filter_map(|o| Some((o.target.clone(), device_app(&o.component, &o.version)?)))
        .collect();
    Ok(s.active_client_tokens()?
        .into_iter()
        .filter(|c| crate::store::machine_token_kind(&c.mode).is_none())
        .map(|mut c| {
            c.org_id = effective.get(&c.id).copied().flatten();
            c
        })
        .filter(|c| match me.authority {
            Authority::Fleet => true,
            Authority::Org { org, .. } => {
                c.org_id == Some(org) && !(c.person_id.is_some() && c.person_id == owner)
            }
        })
        .map(|c| {
            let (kind, app) = apps
                .get(&format!("client:{}", c.id))
                .cloned()
                .map_or((None, None), |(k, a)| (Some(k), Some(a)));
            DeviceSummary {
                kind,
                app,
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
            }
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

/// `orgs::admin`'s arguments out of `org_admin`'s.
fn org_work_args(args: &OrgAdminArgs, org_id: Option<i64>) -> WorkAdminArgs {
    WorkAdminArgs {
        action: args.action.clone(),
        name: args.name.clone(),
        org_id,
        color: args.color.clone(),
        isolate_sessions: args.isolate_sessions,
        auto_tidy: args.auto_tidy.clone(),
        jev: args.jev.clone(),
        jev_reply: args.jev_reply.clone(),
        bound_sees_unassigned: args.bound_sees_unassigned,
        rule_id: args.rule_id,
        owner: args.owner.clone(),
        repo: args.repo.clone(),
        path_prefix: args.path_prefix.clone(),
        host_alias: args.host_alias.clone(),
        tracker_id: args.tracker_id,
        ..Default::default()
    }
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
    check(&s, action, args, me)?;
    match action {
        Action::Org(OrgAction::ListOrgs) if matches!(me.authority, Authority::Org { .. }) => {
            let Authority::Org { org, .. } = me.authority else {
                unreachable!()
            };
            let mut all = orgs::admin(OrgAction::ListOrgs, &WorkAdminArgs::default(), &s)?;
            if let Some(list) = all.as_array_mut() {
                list.retain(|o| o["id"].as_i64() == Some(org));
            }
            Ok(all)
        }
        Action::Org(a) => {
            let org_id = match a {
                OrgAction::AddOrg => None,
                _ => org_of(&s, args)?,
            };
            // Phase D's two switches ride update_org, so the org page edits
            // them like any other (`check` keeps them the hub owner's).
            if let (OrgAction::UpdateOrg, Some(o)) = (a, org_id) {
                match args.owns_hub {
                    Some(true) => s.set_hub_owner_org(Some(o))?,
                    Some(false) if s.hub_owner_org()? == Some(o) => s.set_hub_owner_org(None)?,
                    _ => {}
                }
                if let Some(on) = args.admins_see_unclaimed {
                    s.set_org_admins_see_unclaimed(o, on)?;
                }
                if let Some(on) = args.members_own_sessions_only {
                    s.set_org_members_own_sessions_only(o, on)?;
                    tracing::info!(
                        org = o,
                        on,
                        "[org_admin] set members see only their own sessions"
                    );
                }
            }
            if let (OrgAction::AddOrg, Some(on)) = (a, args.members_own_sessions_only) {
                let out = orgs::admin(a, &org_work_args(args, None), &s)?;
                if let Some(id) = out["id"].as_i64() {
                    return to_json(&s.set_org_members_own_sessions_only(id, on)?);
                }
                return Ok(out);
            }
            orgs::admin(a, &org_work_args(args, org_id), &s)
        }
        Action::RulePreview => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "rule_preview needs org_id or org")
            })?;
            to_json(&orgs::rule_preview(
                &s,
                org,
                crate::store::OrgRuleRow {
                    id: 0,
                    org_id: org,
                    owner: args.owner.clone(),
                    repo: args.repo.clone(),
                    path_prefix: args.path_prefix.clone(),
                    host_alias: args.host_alias.clone(),
                },
            )?)
        }
        Action::AddProject => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "add_project needs org_id or org")
            })?;
            let row = s.add_org_project(
                org,
                &crate::store::NewOrgProject {
                    name: need(&args.name, name, "name")?,
                    remote: args.remote.as_deref(),
                    path: args.path.as_deref(),
                    hosts: args.hosts.as_deref(),
                },
            )?;
            tracing::info!(org, project = row.id, "[org_admin] added a project");
            to_json(&row)
        }
        Action::RemoveProject => {
            let id = *need(&args.project_id, name, "project_id")?;
            let row = s
                .get_org_project(id)?
                .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no project {id}")))?;
            if let Authority::Org { org, .. } = me.authority {
                if row.org_id != org {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "an org admin administers their own org only",
                    ));
                }
            }
            Ok(serde_json::json!({ "removed": s.remove_org_project(id)? }))
        }
        Action::RevokeShare | Action::NarrowShare => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, format!("{name} needs org_id or org"))
            })?;
            let grant = *need(&args.grant_id, name, "grant_id")?;
            let row = s.org_admin_change_grant(grant, org, action == Action::NarrowShare)?;
            tracing::info!(org, grant, level = %row.level, revoked = row.revoked_at.is_some(), "[org_admin] {name}");
            to_json(&row)
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
        Action::ListPeople => match me.authority {
            Authority::Fleet => to_json(&list_people(&s)?),
            Authority::Org { org, .. } => {
                let members: std::collections::BTreeSet<i64> = s
                    .org_members(org)?
                    .into_iter()
                    .map(|m| m.person_id)
                    .collect();
                let mut people = list_people(&s)?;
                people.retain(|p| members.contains(&p.id));
                to_json(&people)
            }
        },
        Action::ListMembers => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "list_members needs org_id or org")
            })?;
            let mut members = list_members(&s, org)?;
            count_member_sessions(&s, org, me.person, &mut members)?;
            to_json(&members)
        }
        Action::SetMember => {
            let org = org_of(&s, args)?
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "set_member needs org_id or org"))?;
            let role = need(&args.role, name, "role")?;
            let person = person_of(&s, args, true)?;
            require_other_member(&s, person, me)?;
            // Changing a member's role is the admin's; pulling in a person
            // who already has devices or another company is the hub owner's.
            if matches!(me.authority, Authority::Org { .. })
                && !s.org_member(org, person)?.is_some_and(|m| m.is_live())
                && has_identity_beyond(&s, person, org)?
            {
                return Err(forbidden(
                    "which company a person who already has a device belongs to",
                ));
            }
            s.set_org_member(org, person, role, me.person)?;
            tracing::info!(org, person, role = %role, "[org_admin] set a member");
            to_json(&list_members(&s, org)?)
        }
        Action::RemoveMember => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "remove_member needs org_id or org")
            })?;
            let person = person_of(&s, args, false)?;
            require_other_member(&s, person, me)?;
            let removed = s.remove_org_member(org, person)?;
            // Owner's answer 2: what was shared with them on the org's
            // sessions goes too, unless the admin keeps it.
            let revoked = if removed && !args.keep_grants.unwrap_or(false) {
                s.revoke_person_grants_in_org(person, org)?
            } else {
                0
            };
            tracing::info!(org, person, revoked, "[org_admin] removed a member");
            Ok(serde_json::json!({ "removed": removed, "revoked_grants": revoked }))
        }
        Action::MemberGrants | Action::RevokeMemberGrants | Action::NarrowMemberGrants => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(codes::E_INVALID, format!("{name} needs org_id or org"))
            })?;
            let person = person_of(&s, args, false)?;
            if matches!(me.authority, Authority::Org { .. }) && s.org_member(org, person)?.is_none()
            {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "an org admin acts on the grants of the org's own members only",
                ));
            }
            match action {
                Action::RevokeMemberGrants => Ok(serde_json::json!({
                    "revoked": s.revoke_person_grants_in_org(person, org)?
                })),
                Action::NarrowMemberGrants => Ok(serde_json::json!({
                    "narrowed": s.narrow_person_grants_in_org(person, org)?
                })),
                _ => {
                    let (watch, answer, drive) = s.person_grants_in_org(person, org)?;
                    Ok(serde_json::json!({ "watch": watch, "answer": answer, "drive": drive }))
                }
            }
        }
        Action::SetHubOrg => {
            let org = org_of(&s, args)?;
            s.set_hub_owner_org(org)?;
            Ok(serde_json::json!({ "owns_hub": org }))
        }
        Action::SetAdminsSeeUnclaimed => {
            let org = org_of(&s, args)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    "set_admins_see_unclaimed needs org_id or org",
                )
            })?;
            to_json(&s.set_org_admins_see_unclaimed(org, *need(&args.on, name, "on")?)?)
        }
        Action::PairDevice => Err(IpcError::new(
            codes::E_INVALID_STATE,
            "pairing codes are minted by a hub (fleet-hub pair); pair this desktop with one first",
        )),
        Action::RevokeDevice => {
            let row = device_row(&s, need(&args.device, name, "device")?, me, "revoking")?;
            require_org_device(&s, &row, me)?;
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
            require_org_device(&s, &row, me)?;
            s.set_client_trust(&row.name, on)?;
            device_json(&s, &row.name, me)
        }
        Action::RenameDevice => {
            let to = need(&args.name, name, "name")?;
            // A new name takes nothing away, so the device in hand may be
            // renamed too: its streams re-read who they are.
            let row = device_row(&s, need(&args.device, name, "device")?, Me::LOCAL, "")?;
            require_org_device(&s, &row, me)?;
            let renamed = s.rename_client_token(&row.name, to)?;
            tracing::info!(from = %row.name, to = %renamed.name, "[org_admin] renamed a device");
            device_json(&s, &renamed.name, me)
        }
        Action::SetDeviceMode => {
            let mode = need(&args.mode, name, "mode")?.trim();
            let device = need(&args.device, name, "device")?;
            // As with trust: only read-only takes access away, so only that
            // is refused for the device in hand.
            let row = if mode == "full" {
                device_row(&s, device, Me::LOCAL, "")?
            } else {
                device_row(&s, device, me, "making read-only")?
            };
            require_org_device(&s, &row, me)?;
            s.set_client_mode(&row.name, mode)?;
            tracing::info!(client = %row.name, mode, "[org_admin] set a device's mode");
            device_json(&s, &row.name, me)
        }
        Action::BindDevice => {
            let row = device_row(&s, need(&args.device, name, "device")?, me, "binding")?;
            let org = org_of(&s, args)?;
            // Phase D: a device's org is one of its person's memberships.
            if let (Some(p), Some(o)) = (row.person_id, org) {
                let owner = s.personal_owner_id()? == Some(p);
                let live: Vec<i64> = s
                    .memberships_of(p)?
                    .into_iter()
                    .filter(|m| m.is_live())
                    .map(|m| m.org_id)
                    .collect();
                if !owner && !live.is_empty() && !live.contains(&o) {
                    return Err(IpcError::new(
                        codes::E_VALIDATE,
                        format!(
                            "{:?} belongs to a person who is not in that org; add them to it \
                             first (set_member)",
                            row.name
                        ),
                    ));
                }
            }
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
            require_org_device(&s, &row, me)?;
            let catalog = need(&args.catalog, name, "catalog")?.trim().to_string();
            if let Authority::Org { org, .. } = me.authority {
                let ours = s
                    .get_catalog_by_name(&catalog)?
                    .is_some_and(|c| c.org_id == Some(org));
                if !ours {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "an org admin grants their org's own catalogs only",
                    ));
                }
            }
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
        Action::AddPerson => {
            let person = need(&args.name, name, "name")?;
            let display = args
                .display_name
                .as_deref()
                .filter(|d| !d.trim().is_empty());
            to_json(&s.create_person(person, display)?)
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
