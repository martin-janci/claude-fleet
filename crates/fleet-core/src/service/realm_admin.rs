//! Realm administration (identity sub-project 1): users, the group tree and
//! its members, realm and client roles with their composites, role
//! mappings, effective roles and the admin audit log, over the realm model
//! of `store/realm.rs`.
//!
//! The MCP tool `realm_admin` runs it for a person's device (`fleet-hub
//! realm …` for the operator). Who may do what is [`Authority`]:
//!
//! * [`Authority::Fleet`] — a fleet admin's unbound device, or the desktop's
//!   own store — reaches every action.
//! * [`Authority::Org`] — an org admin's device fenced to their org — reads
//!   that org's group subtree (`/orgs/<org>/…`) and its live members, and
//!   changes one thing: who is in `/orgs/<org>/{admins,members,viewers}`,
//!   through the org's own membership rules (`Store::set_org_member`'s), so
//!   the projection and grant rules apply. Never their own membership, nor
//!   a fleet admin's (`org_admin::require_other_member`).
//!
//! Two rules every write keeps on top of the store's own (`fleet-admin`
//! never leaves the personal owner nor its last holder):
//!
//! * **Nobody removes their own `fleet-admin`** (spec §4): a write that
//!   would take it from the caller's person — an unmapping, leaving a group,
//!   deleting or moving one, a composite — is `E_FORBIDDEN`; another fleet
//!   admin does it. Nobody disables themselves either.
//! * **Every write that succeeds leaves one admin event**, inside the
//!   change's own transaction where the store offers an `_in_tx` writer,
//!   otherwise right after it (logged on failure, as `org_admin::run`).
//!
//! Role references are `"fleet-admin"` (a realm role) or
//! `"<client>:<name>"` (`"fleet:org-admin"`); groups are named by path
//! (`/orgs/acme/members`).

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::org_admin::{self, Authority, Me};
use crate::store::{
    record_admin_event_in_tx, AdminEvent, AdminEventQuery, GroupRow, PersonProfile, PersonRow,
    Principal, RoleRef, RoleRow, Store, ROLE_FLEET_ADMIN,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RealmAdminParams")]
pub struct RealmAdminArgs {
    /// list_users|get_user|update_user|enable_user|disable_user|list_groups|get_group|create_group|rename_group|move_group|delete_group|add_member|remove_member|list_roles|create_role|update_role|delete_role|add_composite|remove_composite|map_role|unmap_role|effective_roles|audit
    pub action: String,
    /// A person's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// A group's path, e.g. /orgs/acme/members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Parent group's path; "" = top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// New group or role name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// fleet-admin, or client:role (fleet:org-admin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The composite's included role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    /// create_role: client id; absent = realm role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// Role description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// update_user: email, email_verified, first_name, last_name, attributes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
    /// audit: from this Unix second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// audit: max rows (100; ≤500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl RealmAdminArgs {
    /// The audit line: the action and the names it touches. Never the
    /// profile (an email address) nor a description's text.
    pub fn audit_summary(&self) -> String {
        let mut out = format!("action={}", self.action.escape_debug());
        for (k, v) in [
            ("user", &self.user),
            ("group", &self.group),
            ("parent", &self.parent),
            ("name", &self.name),
            ("role", &self.role),
            ("child", &self.child),
            ("client", &self.client),
        ] {
            if let Some(v) = v {
                out.push_str(&format!(" {k}={}", v.escape_debug()));
            }
        }
        if self.profile.is_some() {
            out.push_str(" profile=<set>");
        }
        if let Some(v) = self.since {
            out.push_str(&format!(" since={v}"));
        }
        if let Some(v) = self.limit {
            out.push_str(&format!(" limit={v}"));
        }
        out
    }
}

/// One `realm_admin` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ListUsers,
    GetUser,
    UpdateUser,
    EnableUser,
    DisableUser,
    ListGroups,
    GetGroup,
    CreateGroup,
    RenameGroup,
    MoveGroup,
    DeleteGroup,
    AddMember,
    RemoveMember,
    ListRoles,
    CreateRole,
    UpdateRole,
    DeleteRole,
    AddComposite,
    RemoveComposite,
    MapRole,
    UnmapRole,
    EffectiveRoles,
    Audit,
}

const ACTIONS: [(&str, Action); 23] = [
    ("list_users", Action::ListUsers),
    ("get_user", Action::GetUser),
    ("update_user", Action::UpdateUser),
    ("enable_user", Action::EnableUser),
    ("disable_user", Action::DisableUser),
    ("list_groups", Action::ListGroups),
    ("get_group", Action::GetGroup),
    ("create_group", Action::CreateGroup),
    ("rename_group", Action::RenameGroup),
    ("move_group", Action::MoveGroup),
    ("delete_group", Action::DeleteGroup),
    ("add_member", Action::AddMember),
    ("remove_member", Action::RemoveMember),
    ("list_roles", Action::ListRoles),
    ("create_role", Action::CreateRole),
    ("update_role", Action::UpdateRole),
    ("delete_role", Action::DeleteRole),
    ("add_composite", Action::AddComposite),
    ("remove_composite", Action::RemoveComposite),
    ("map_role", Action::MapRole),
    ("unmap_role", Action::UnmapRole),
    ("effective_roles", Action::EffectiveRoles),
    ("audit", Action::Audit),
];

impl Action {
    pub fn parse(s: &str) -> Result<Self, IpcError> {
        ACTIONS
            .iter()
            .find(|(n, _)| *n == s)
            .map(|(_, a)| *a)
            .ok_or_else(|| {
                let names: Vec<&str> = ACTIONS.iter().map(|(n, _)| *n).collect();
                IpcError::new(
                    codes::E_VALIDATE,
                    format!(
                        "realm_admin has no action {s:?}; one of: {}",
                        names.join(", ")
                    ),
                )
            })
    }

    pub fn name(self) -> &'static str {
        ACTIONS
            .iter()
            .find(|(_, a)| *a == self)
            .map(|(n, _)| *n)
            .unwrap_or_default()
    }

    /// Changes nothing: any device the tool serves may run it.
    pub fn is_read(self) -> bool {
        matches!(
            self,
            Action::ListUsers
                | Action::GetUser
                | Action::ListGroups
                | Action::GetGroup
                | Action::ListRoles
                | Action::EffectiveRoles
                | Action::Audit
        )
    }

    /// What an org admin may run: their org's reads, and its members.
    fn org_admin_may(self) -> bool {
        matches!(
            self,
            Action::ListUsers
                | Action::GetUser
                | Action::ListGroups
                | Action::GetGroup
                | Action::EffectiveRoles
                | Action::AddMember
                | Action::RemoveMember
        )
    }
}

/// Run one action as `me`. The caller already decided `me` may write when
/// the action is not a read (the tool: a trusted full device).
pub fn run(args: &RealmAdminArgs, store: &Mutex<Store>, me: Me<'_>) -> Result<Value, IpcError> {
    let action = Action::parse(&args.action)?;
    if matches!(me.authority, Authority::Org { .. }) && !action.org_admin_may() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{} is a fleet admin's: an org admin reads their org's groups and \
                 members and changes who is in /orgs/<org>/admins, members or viewers",
                action.name()
            ),
        ));
    }
    let s = lock(store)?;
    let fence = match me.authority {
        Authority::Fleet => None,
        Authority::Org { org, .. } => Some(OrgFence::of(&s, org)?),
    };
    if action.is_read() {
        return read(action, args, &s, fence.as_ref());
    }
    let w = Writer {
        s: &s,
        me,
        action: action.name(),
        args,
        actor_device: match me.device {
            Some(d) => s
                .active_client_tokens()?
                .into_iter()
                .find(|c| c.name == d)
                .map(|c| c.id),
            None => None,
        },
    };
    write(action, args, &w, fence.as_ref())
}

// ---- the org fence -----------------------------------------------------------

/// What an org admin sees: the org and its group tree, by ancestry
/// ([`crate::store::OrgGroupFence`], shared with `org_admin { list_people }`).
struct OrgFence {
    org: i64,
    groups: crate::store::OrgGroupFence,
}

impl OrgFence {
    fn of(s: &Store, org: i64) -> Result<Self, IpcError> {
        Ok(OrgFence {
            org,
            groups: s.org_group_fence(org)?,
        })
    }

    fn covers(&self, g: &GroupRow) -> bool {
        self.groups.covers(g.id)
    }

    /// Fence a role's `via` list to what an org admin may see: not a group
    /// outside their org. False when the role is then not shown at all.
    fn keep_role_vias(&self, via: &mut Vec<String>) -> bool {
        self.groups.keep_role_vias(via)
    }

    fn member(&self, s: &Store, person: i64) -> Result<(), IpcError> {
        if s.org_role(self.org, person)?.is_some() {
            return Ok(());
        }
        Err(IpcError::new(
            codes::E_FORBIDDEN,
            "an org admin sees their own org's members only",
        ))
    }

    fn group(&self, g: &GroupRow) -> Result<(), IpcError> {
        if self.covers(g) {
            return Ok(());
        }
        Err(IpcError::new(
            codes::E_FORBIDDEN,
            "an org admin sees their own org's groups only",
        ))
    }
}

// ---- lookups ---------------------------------------------------------------

fn need<'a>(v: &'a Option<String>, action: &str, field: &str) -> Result<&'a str, IpcError> {
    v.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| IpcError::new(codes::E_VALIDATE, format!("{action} needs {field}")))
}

/// A live person by name, else the one disabled person of that name.
fn person(s: &Store, name: &str) -> Result<PersonRow, IpcError> {
    if let Some(p) = s.get_person_by_name(name)? {
        return Ok(p);
    }
    let mut gone: Vec<PersonRow> = s
        .list_people()?
        .into_iter()
        .filter(|p| p.name == name && p.disabled_at.is_some())
        .collect();
    match gone.len() {
        0 => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no person named {name:?}"),
        )),
        1 => Ok(gone.remove(0)),
        _ => Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "several disabled people are named {name:?}; use org_admin's \
                 person_id"
            ),
        )),
    }
}

fn live_person(s: &Store, name: &str) -> Result<PersonRow, IpcError> {
    s.get_person_by_name(name)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("this hub knows no live person named {name:?}"),
        )
    })
}

fn group(s: &Store, path: &str) -> Result<GroupRow, IpcError> {
    s.group_by_path(path)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no group {path:?}")))
}

/// `""` or absent: the top level.
fn parent(s: &Store, path: &Option<String>) -> Result<Option<GroupRow>, IpcError> {
    match path.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(p) => group(s, p).map(Some),
    }
}

fn role(s: &Store, r: &str) -> Result<RoleRow, IpcError> {
    let found = match r.split_once(':') {
        Some((client, name)) => s.role(RoleRef::Client(client, name))?,
        None => s.role(RoleRef::Realm(r))?,
    };
    found.ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no role {r:?}")))
}

fn role_ref(r: &RoleRow) -> String {
    crate::store::role_ref(r.client.as_deref(), &r.name)
}

fn managed_err() -> IpcError {
    IpcError::new(codes::E_FORBIDDEN, "this group is managed by its org")
}

/// A managed `/orgs/<org>/{admins,members,viewers}` group: its org and the
/// membership role it stands for.
fn org_slot(s: &Store, g: &GroupRow) -> Result<Option<(i64, &'static str)>, IpcError> {
    if !g.managed {
        return Ok(None);
    }
    let role = match g.name.as_str() {
        "admins" => "admin",
        "members" => "member",
        "viewers" => "viewer",
        _ => return Ok(None),
    };
    let Some(parent) = g.parent_id else {
        return Ok(None);
    };
    Ok(s.group(parent)?
        .and_then(|p| p.org_id.filter(|_| p.managed))
        .map(|org| (org, role)))
}

// ---- output ----------------------------------------------------------------

fn group_json(s: &Store, g: &GroupRow) -> Result<Value, IpcError> {
    let mut members = Vec::new();
    for id in s.group_members(g.id)? {
        if let Some(p) = s.get_person(id)? {
            members.push(p.name);
        }
    }
    let roles: Vec<String> = s.group_roles(g.id)?.iter().map(role_ref).collect();
    Ok(json!({
        "path": g.path,
        "name": g.name,
        "managed": g.managed,
        "org_id": g.org_id,
        "members": members,
        "roles": roles,
    }))
}

fn roles_json(s: &Store, person: i64, fence: Option<&OrgFence>) -> Result<Vec<Value>, IpcError> {
    let mut out = Vec::new();
    for e in s.effective_roles(person)? {
        let mut via: Vec<String> = e.via.clone();
        if let Some(f) = fence {
            if !f.keep_role_vias(&mut via) {
                continue;
            }
        }
        out.push(json!({ "name": e.role.name, "client": e.role.client, "via": via }));
    }
    Ok(out)
}

fn user_json(
    s: &Store,
    p: &PersonRow,
    fence: Option<&OrgFence>,
    full: bool,
) -> Result<Value, IpcError> {
    let groups: Vec<String> = s
        .groups_of(p.id)?
        .into_iter()
        .filter(|g| fence.is_none_or(|f| f.covers(g)))
        .map(|g| g.path)
        .collect();
    let mut v = json!({
        "name": p.name,
        "uuid": p.uuid,
        "email": p.email,
        "enabled": p.disabled_at.is_none(),
        "groups": groups,
        "roles": roles_json(s, p.id, fence)?,
    });
    if full {
        v["display_name"] = json!(p.display_name);
        v["email_verified"] = json!(p.email_verified);
        v["first_name"] = json!(p.first_name);
        v["last_name"] = json!(p.last_name);
        v["attributes"] = p.attributes.clone();
        v["owner"] = json!(p.is_personal_owner);
        v["created_at"] = json!(p.created_at);
    }
    Ok(v)
}

fn role_json(s: &Store, r: &RoleRow) -> Result<Value, IpcError> {
    let composites: Vec<String> = s.composites_of(r.id)?.iter().map(role_ref).collect();
    Ok(json!({
        "name": r.name,
        "client": r.client,
        "description": r.description,
        "seeded": r.seeded,
        "composites": composites,
    }))
}

// ---- reads -----------------------------------------------------------------

fn read(
    action: Action,
    args: &RealmAdminArgs,
    s: &Store,
    fence: Option<&OrgFence>,
) -> Result<Value, IpcError> {
    let name = action.name();
    match action {
        Action::ListUsers => {
            let people = match fence {
                None => s.list_people()?,
                Some(f) => {
                    let mut v = Vec::new();
                    for m in s.org_members(f.org)? {
                        if let Some(p) = s.get_person(m.person_id)? {
                            v.push(p);
                        }
                    }
                    v
                }
            };
            let rows = people
                .iter()
                .map(|p| user_json(s, p, fence, false))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
        Action::GetUser | Action::EffectiveRoles => {
            let p = person(s, need(&args.user, name, "user")?)?;
            if let Some(f) = fence {
                f.member(s, p.id)?;
            }
            if action == Action::GetUser {
                user_json(s, &p, fence, true)
            } else {
                Ok(Value::Array(roles_json(s, p.id, fence)?))
            }
        }
        Action::ListGroups => {
            let rows = s
                .list_groups()?
                .iter()
                .filter(|g| fence.is_none_or(|f| f.covers(g)))
                .map(|g| group_json(s, g))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
        Action::GetGroup => {
            let g = group(s, need(&args.group, name, "group")?)?;
            if let Some(f) = fence {
                f.group(&g)?;
            }
            let mut v = group_json(s, &g)?;
            v["uuid"] = json!(g.uuid);
            v["attributes"] = g.attributes.clone();
            v["parent"] = match g.parent_id {
                Some(p) => json!(s.group(p)?.map(|p| p.path)),
                None => Value::Null,
            };
            Ok(v)
        }
        Action::ListRoles => {
            let rows = s
                .list_roles(None)?
                .iter()
                .map(|r| role_json(s, r))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
        Action::Audit => {
            let rows = s.admin_events(&AdminEventQuery {
                since: args.since,
                limit: args.limit.unwrap_or(100).min(500),
                ..Default::default()
            })?;
            serde_json::to_value(rows).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
        }
        _ => Err(IpcError::new(
            codes::E_INTERNAL,
            format!("{name} is not a read"),
        )),
    }
}

// ---- writes ----------------------------------------------------------------

struct Writer<'a> {
    s: &'a Store,
    me: Me<'a>,
    action: &'static str,
    args: &'a RealmAdminArgs,
    actor_device: Option<i64>,
}

impl Writer<'_> {
    fn event<'e>(&self, resource_type: &'e str, resource_id: &'e str) -> AdminEvent<'e> {
        AdminEvent {
            actor_person: self.me.person,
            actor_device: self.actor_device,
            action: self.action,
            resource_type,
            resource_id,
            representation: serde_json::to_value(self.args).unwrap_or_default(),
        }
    }

    /// Whether the caller's person holds `fleet-admin` through a mapping
    /// (read on the store's connection, so inside a transaction it sees the
    /// change in flight).
    fn mine(&self) -> Result<bool, IpcError> {
        Ok(match self.me.person {
            Some(p) => self
                .s
                .people_with_realm_role(ROLE_FLEET_ADMIN)?
                .contains(&p),
            None => false,
        })
    }

    /// Run `change` in one transaction with its admin event, refusing it
    /// when it takes `fleet-admin` from the caller. `resource_id` may read
    /// the change's result (a new group's path).
    fn in_tx<T>(
        &self,
        resource_type: &str,
        change: impl FnOnce(&Connection) -> Result<T, IpcError>,
        resource_id: impl FnOnce(&T) -> String,
    ) -> Result<T, IpcError> {
        self.s.with_tx(|c| {
            let before = self.mine()?;
            let v = change(c)?;
            if before && !self.mine()? {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "nobody removes their own fleet-admin: another fleet admin does",
                ));
            }
            let id = resource_id(&v);
            record_admin_event_in_tx(c, &self.event(resource_type, &id))?;
            Ok(v)
        })
    }

    /// The event for a change a store call has already committed: a failure
    /// to record is logged, never a refusal of what happened.
    fn after(&self, resource_type: &str, resource_id: &str) {
        self.s
            .record_admin_event_or_log(&self.event(resource_type, resource_id));
    }
}

fn write(
    action: Action,
    args: &RealmAdminArgs,
    w: &Writer<'_>,
    fence: Option<&OrgFence>,
) -> Result<Value, IpcError> {
    let s = w.s;
    let name = action.name();
    match action {
        Action::UpdateUser => {
            let p = person(s, need(&args.user, name, "user")?)?;
            let profile =
                profile_of(args.profile.as_ref().ok_or_else(|| {
                    IpcError::new(codes::E_VALIDATE, "update_user needs profile")
                })?)?;
            let row = s.update_person_profile(p.id, &profile)?;
            w.after("user", &row.name);
            user_json(s, &row, None, true)
        }
        Action::EnableUser | Action::DisableUser => {
            let p = person(s, need(&args.user, name, "user")?)?;
            let row = if action == Action::EnableUser {
                s.enable_person(p.id)?
            } else {
                if w.me.person == Some(p.id) {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "nobody disables themselves: another fleet admin does",
                    ));
                }
                s.disable_person(p.id)?
            };
            w.after("user", &row.name);
            user_json(s, &row, None, true)
        }
        Action::CreateGroup => {
            let new = need(&args.name, name, "name")?;
            let parent = parent(s, &args.parent)?;
            if parent.as_ref().is_some_and(|p| p.managed) {
                return Err(managed_err());
            }
            let g = w.in_tx(
                "group",
                |c| s.create_group_in_tx(c, parent.map(|p| p.id), new, false, None, w.me.person),
                |g| g.path.clone(),
            )?;
            group_json(s, &g)
        }
        Action::RenameGroup | Action::MoveGroup => {
            let path = need(&args.group, name, "group")?;
            let g = group(s, path)?;
            let g = if action == Action::RenameGroup {
                let new = need(&args.name, name, "name")?;
                if g.managed {
                    return Err(managed_err());
                }
                w.in_tx(
                    "group",
                    |c| s.rename_group_in_tx(c, g.id, new),
                    |_| path.to_string(),
                )?
            } else {
                let to = parent(s, &args.parent)?.map(|p| p.id);
                w.in_tx(
                    "group",
                    |c| s.move_group_in_tx(c, g.id, to),
                    |_| path.to_string(),
                )?
            };
            group_json(s, &g)
        }
        Action::DeleteGroup => {
            let path = need(&args.group, name, "group")?;
            let g = group(s, path)?;
            let deleted = w.in_tx(
                "group",
                |c| s.delete_unmanaged_group_in_tx(c, g.id),
                |_| path.to_string(),
            )?;
            Ok(json!({ "deleted": deleted }))
        }
        Action::AddMember | Action::RemoveMember => member(action, args, w, fence),
        Action::CreateRole => {
            let new = need(&args.name, name, "name")?;
            let client = args
                .client
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty());
            let r = s.create_role(client, new, args.description.as_deref())?;
            w.after("role", &role_ref(&r));
            role_json(s, &r)
        }
        Action::UpdateRole => {
            let r = role(s, need(&args.role, name, "role")?)?;
            let r = s.update_role(r.id, args.description.as_deref())?;
            w.after("role", &role_ref(&r));
            role_json(s, &r)
        }
        Action::DeleteRole => {
            let r = role(s, need(&args.role, name, "role")?)?;
            let deleted = w.in_tx("role", |c| s.delete_role_in_tx(c, r.id), |_| role_ref(&r))?;
            Ok(json!({ "deleted": deleted }))
        }
        Action::AddComposite | Action::RemoveComposite => {
            let parent = role(s, need(&args.role, name, "role")?)?;
            let child = role(s, need(&args.child, name, "child")?)?;
            if action == Action::AddComposite {
                s.add_composite(parent.id, child.id)?;
                w.after("composite", &role_ref(&parent));
                role_json(s, &parent)
            } else {
                let removed = w.in_tx(
                    "composite",
                    |c| s.remove_composite_in_tx(c, parent.id, child.id),
                    |_| role_ref(&parent),
                )?;
                Ok(json!({ "removed": removed }))
            }
        }
        Action::MapRole | Action::UnmapRole => {
            let r = role(s, need(&args.role, name, "role")?)?;
            let (principal, id) = match (
                args.user
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty()),
                args.group
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty()),
            ) {
                (Some(u), None) => (Principal::Person(person(s, u)?.id), u.to_string()),
                (None, Some(g)) => (Principal::Group(group(s, g)?.id), g.to_string()),
                _ => {
                    return Err(IpcError::new(
                        codes::E_VALIDATE,
                        format!("{name} needs either user or group"),
                    ))
                }
            };
            if action == Action::MapRole {
                w.in_tx(
                    "role_mapping",
                    |c| s.map_role_in_tx(c, principal, r.id, false),
                    |_| id.clone(),
                )?;
                Ok(json!({ "mapped": role_ref(&r) }))
            } else {
                let removed = w.in_tx(
                    "role_mapping",
                    |c| s.unmap_role_in_tx(c, principal, r.id, false),
                    |_| id.clone(),
                )?;
                Ok(json!({ "removed": removed }))
            }
        }
        _ => Err(IpcError::new(
            codes::E_INTERNAL,
            format!("{name} is not a write"),
        )),
    }
}

/// `add_member` / `remove_member`. An org's managed subgroups go through the
/// org's membership (the role from the subgroup's name), so the projection,
/// the grant rules and an org admin's limits apply; another managed group
/// is the org's own; any other group is a plain membership.
fn member(
    action: Action,
    args: &RealmAdminArgs,
    w: &Writer<'_>,
    fence: Option<&OrgFence>,
) -> Result<Value, IpcError> {
    let s = w.s;
    let name = action.name();
    let path = need(&args.group, name, "group")?;
    let user = need(&args.user, name, "user")?;
    let g = group(s, path)?;
    let slot = org_slot(s, &g)?;
    if let Some(f) = fence {
        if slot.map(|(o, _)| o) != Some(f.org) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "an org admin changes the members of their own org's admins, members \
                 and viewers groups only",
            ));
        }
    }
    let adding = action == Action::AddMember;
    let p = if adding {
        live_person(s, user)?
    } else {
        person(s, user)?
    };
    let Some((org, role)) = slot else {
        if g.managed {
            return Err(managed_err());
        }
        return if adding {
            w.in_tx(
                "group",
                |c| s.add_group_member_in_tx(c, g.id, p.id, w.me.person),
                |_| path.to_string(),
            )?;
            group_json(s, &g)
        } else {
            let removed = w.in_tx(
                "group",
                |c| s.remove_group_member_in_tx(c, g.id, p.id),
                |_| path.to_string(),
            )?;
            Ok(json!({ "removed": removed }))
        };
    };
    org_admin::require_other_member(s, p.id, w.me)?;
    if adding {
        // Pulling in a person who already has a device or another company
        // is the fleet's call, as in org_admin's set_member.
        if fence.is_some()
            && s.org_role(org, p.id)?.is_none()
            && org_admin::has_identity_beyond(s, p.id, org)?
        {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "which company a person who already has a device belongs to is a fleet \
                 admin's to change",
            ));
        }
        w.in_tx(
            "group",
            |c| s.set_org_member_in_tx(c, org, p.id, role, w.me.person),
            |_| path.to_string(),
        )?;
        crate::store::org_membership_changed();
        group_json(s, &g)
    } else {
        // Only from the subgroup named: removing an admin "from members"
        // must not end their membership.
        if !s.group_members(g.id)?.contains(&p.id) {
            return Ok(json!({ "removed": false }));
        }
        let ended = w.in_tx(
            "group",
            |c| s.remove_org_member_in_tx(c, org, p.id),
            |_| path.to_string(),
        )?;
        let mut revoked = 0;
        if ended {
            crate::store::org_membership_changed();
            // org_admin's remove_member: what was shared with them on the
            // org's sessions goes too.
            revoked = s.revoke_person_grants_in_org(p.id, org)?;
        }
        Ok(json!({ "removed": ended, "revoked_grants": revoked }))
    }
}

/// `update_user`'s `profile`: a partial update, `null` clears a text field.
fn profile_of(v: &Value) -> Result<PersonProfile, IpcError> {
    let bad = |m: String| IpcError::new(codes::E_VALIDATE, m);
    let obj = v
        .as_object()
        .ok_or_else(|| bad("profile is an object".into()))?;
    let text = |k: &str, v: &Value| -> Result<Option<String>, IpcError> {
        match v {
            Value::Null => Ok(None),
            Value::String(t) => Ok(Some(t.trim().to_string()).filter(|t| !t.is_empty())),
            _ => Err(bad(format!("profile.{k} is a string or null"))),
        }
    };
    let mut p = PersonProfile::default();
    for (k, v) in obj {
        match k.as_str() {
            "email" => p.email = Some(text(k, v)?),
            "first_name" => p.first_name = Some(text(k, v)?),
            "last_name" => p.last_name = Some(text(k, v)?),
            "email_verified" => {
                p.email_verified = Some(
                    v.as_bool()
                        .ok_or_else(|| bad("profile.email_verified is a boolean".into()))?,
                )
            }
            "attributes" => p.attributes = Some(v.clone()),
            other => {
                return Err(bad(format!(
                    "profile has no field {other:?}: it takes email, email_verified, \
                     first_name, last_name and attributes"
                )))
            }
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests;
