//! The realm model (identity sub-project 1, migration 178): clients, roles
//! (realm and per-client, with composites), a group tree, group membership,
//! and role mappings to people and groups. Groups and mappings are the
//! source of truth; `org_members` is a projection of them (Task 5).
//!
//! Every write here moves the `auth_epoch` through the table triggers of
//! migration 178, so a cached caller re-resolves.
//!
//! Two layers: the public methods open their own transaction; the `_in_tx`
//! helpers take the caller's `&Connection` and never open one, so the org
//! projection can compose several of them atomically. The `_in_tx` helpers
//! that create / rename / delete groups skip the "managed by its org" check
//! the public ones apply.

use super::{breaks_a_line, now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};

pub const CLIENT_FLEET: &str = "fleet";
pub const ROLE_FLEET_ADMIN: &str = "fleet-admin";
pub const ROLE_FLEET_TRUSTED: &str = "fleet-trusted";
pub const ROLE_ASSETS_ADMIN: &str = "assets-admin";
/// (subgroup of an org's root group, the `fleet` client role it maps to).
pub const ORG_ROLES: [(&str, &str); 3] = [
    ("admins", "org-admin"),
    ("members", "org-member"),
    ("viewers", "org-viewer"),
];

/// Longest role / group name and deepest group nesting.
const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 512;
const MAX_GROUP_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRow {
    pub id: i64,
    pub uuid: String,
    /// The client's text `client_id` (e.g. `"fleet"`); `None` for a realm role.
    pub client: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub seeded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupRow {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    /// `/orgs/acme/admins`: the names from the root down.
    pub path: String,
    pub parent_id: Option<i64>,
    pub managed: bool,
    pub org_id: Option<i64>,
    pub attributes: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRef<'a> {
    Realm(&'a str),
    Client(&'a str, &'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Principal {
    Person(i64),
    Group(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveRole {
    pub role: RoleRow,
    /// `"direct"`, `"group:/orgs/acme/admins"`, `"composite:org-admin"`.
    pub via: Vec<String>,
}

const ROLE_SELECT: &str = "SELECT r.id, r.uuid, c.client_id, r.name, r.description, r.seeded \
     FROM roles r LEFT JOIN clients c ON c.id = r.client_id";

const GROUP_COLUMNS: &str = "id, uuid, name, parent_id, attributes, managed, org_id";

/// Every role a person holds, directly, through a group (or an ancestor of
/// one) or through a composite. `UNION`, not `UNION ALL`, so a cycle written
/// by hand still terminates. A disabled person holds none.
const EFFECTIVE_ROLE_IDS: &str = "
WITH RECURSIVE
  g(id) AS (SELECT group_id FROM group_members WHERE person_id = ?1
            UNION SELECT gr.parent_id FROM groups gr JOIN g ON gr.id = g.id WHERE gr.parent_id IS NOT NULL),
  r(id) AS (SELECT role_id FROM user_role_mappings WHERE person_id = ?1
            UNION SELECT role_id FROM group_role_mappings WHERE group_id IN (SELECT id FROM g)
            UNION SELECT rc.child_id FROM role_composites rc JOIN r ON rc.parent_id = r.id)
SELECT id FROM r
WHERE EXISTS (SELECT 1 FROM people p WHERE p.id = ?1 AND p.disabled_at IS NULL)";

const PEOPLE_WITH_REALM_ROLE: &str = "
WITH RECURSIVE
  holders(role_id) AS (SELECT id FROM roles WHERE client_id IS NULL AND name = ?1
                       UNION SELECT rc.parent_id FROM role_composites rc JOIN holders h ON rc.child_id = h.role_id),
  grp(id) AS (SELECT group_id FROM group_role_mappings WHERE role_id IN (SELECT role_id FROM holders)
              UNION SELECT c.id FROM groups c JOIN grp ON c.parent_id = grp.id)
SELECT p.id FROM people p WHERE p.disabled_at IS NULL AND (
  EXISTS (SELECT 1 FROM user_role_mappings u WHERE u.person_id = p.id AND u.role_id IN (SELECT role_id FROM holders))
  OR EXISTS (SELECT 1 FROM group_members m WHERE m.person_id = p.id AND m.group_id IN (SELECT id FROM grp)))
ORDER BY p.id";

fn invalid(m: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_VALIDATE, m)
}

fn managed_err() -> IpcError {
    IpcError::new(codes::E_FORBIDDEN, "this group is managed by its org")
}

/// Live holders of realm role `name`, read through `conn`.
fn holders_in_tx(conn: &Connection, name: &str) -> Result<Vec<i64>, IpcError> {
    Ok(realm_role_holders(conn, name)?)
}

/// [`holders_in_tx`] with SQLite's own error, for a reader that answers in
/// `rusqlite::Error` (`Store::client_is_assets_admin`). Ascending ids.
pub(crate) fn realm_role_holders(conn: &Connection, name: &str) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare(PEOPLE_WITH_REALM_ROLE)?;
    let ids = stmt.query_map(rusqlite::params![name], |r| r.get(0))?;
    ids.collect()
}

/// The id of the seeded realm role `name` (`fleet-trusted`, `assets-admin`,
/// …). `E_INTERNAL` when it is missing: migration 178 seeds them all.
pub(crate) fn realm_role_id_in_tx(conn: &Connection, name: &str) -> Result<i64, IpcError> {
    conn.query_row(
        "SELECT id FROM roles WHERE client_id IS NULL AND name = ?1",
        rusqlite::params![name],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| IpcError::new(codes::E_INTERNAL, format!("realm role {name} is missing")))
}

fn personal_owner_in_tx(conn: &Connection) -> Result<Option<i64>, IpcError> {
    Ok(conn
        .query_row(
            "SELECT id FROM people WHERE is_personal_owner = 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

/// Run `change` through `conn` and refuse it (`E_FORBIDDEN`) when it takes
/// `fleet-admin` away from the personal owner, or from its last live holder
/// (identity sub-project 1: nobody can lock the fleet out of its own
/// administration). Judged as a transition — who held the role before the
/// change against who holds it after — so a database where the owner was
/// never mapped does not refuse every unrelated write. The owner's
/// unconditional place in `Store::realm_sets` is not counted: at least one
/// live person must still hold the mapping, directly or through a group.
///
/// On `Err` the change has already run through `conn`: the caller's
/// transaction must roll back, which every caller does by returning the
/// error out of `with_tx` / its own transaction.
pub(crate) fn keeping_fleet_admin<T>(
    conn: &Connection,
    change: impl FnOnce() -> Result<T, IpcError>,
) -> Result<T, IpcError> {
    let before = holders_in_tx(conn, ROLE_FLEET_ADMIN)?;
    let v = change()?;
    if before.is_empty() {
        return Ok(v);
    }
    let after = holders_in_tx(conn, ROLE_FLEET_ADMIN)?;
    if after.is_empty() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "this would leave nobody holding fleet-admin; grant it to someone else first",
        ));
    }
    if let Some(owner) = personal_owner_in_tx(conn)? {
        if before.contains(&owner) && !after.contains(&owner) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "the personal owner keeps fleet-admin",
            ));
        }
    }
    Ok(v)
}

/// Trim and check a role or group name: non-empty, bounded, one line, and
/// for a group no `/` (it is the path separator).
fn validate_realm_name(kind: &str, name: &str, group: bool) -> Result<String, IpcError> {
    let t = name.trim();
    let len = t.chars().count();
    if len == 0 || len > MAX_NAME_LEN {
        return Err(invalid(format!(
            "a {kind} name is 1-{MAX_NAME_LEN} characters, got {len}"
        )));
    }
    if t.chars().any(breaks_a_line) {
        return Err(invalid(format!(
            "a {kind} name must not contain control characters or line separators"
        )));
    }
    if group && t.contains('/') {
        return Err(invalid("a group name must not contain '/'"));
    }
    Ok(t.to_string())
}

fn validate_description(d: Option<&str>) -> Result<Option<String>, IpcError> {
    let Some(d) = d.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(None);
    };
    if d.chars().count() > MAX_DESCRIPTION_LEN || d.chars().any(breaks_a_line) {
        return Err(invalid(format!(
            "a role description is at most {MAX_DESCRIPTION_LEN} characters, one line"
        )));
    }
    Ok(Some(d.to_string()))
}

fn map_role_row(r: &rusqlite::Row) -> rusqlite::Result<RoleRow> {
    Ok(RoleRow {
        id: r.get(0)?,
        uuid: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        client: r.get(2)?,
        name: r.get(3)?,
        description: r.get(4)?,
        seeded: r.get::<_, i64>(5)? != 0,
    })
}

fn load_role(conn: &Connection, id: i64) -> Result<Option<RoleRow>, IpcError> {
    Ok(conn
        .query_row(
            &format!("{ROLE_SELECT} WHERE r.id = ?1"),
            rusqlite::params![id],
            map_role_row,
        )
        .optional()?)
}

fn role_or_404(conn: &Connection, id: i64) -> Result<RoleRow, IpcError> {
    load_role(conn, id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("role {id} not found")))
}

fn load_group(conn: &Connection, id: i64) -> Result<Option<GroupRow>, IpcError> {
    let row = conn
        .query_row(
            &format!("SELECT {GROUP_COLUMNS} FROM groups WHERE id = ?1"),
            rusqlite::params![id],
            map_group_row,
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some(mut g) => {
            g.path = group_path_in_tx(conn, g.id)?;
            Ok(Some(g))
        }
    }
}

fn group_or_404(conn: &Connection, id: i64) -> Result<GroupRow, IpcError> {
    load_group(conn, id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("group {id} not found")))
}

/// A group row with its `path` still empty.
fn map_group_row(r: &rusqlite::Row) -> rusqlite::Result<GroupRow> {
    Ok(GroupRow {
        id: r.get(0)?,
        uuid: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        name: r.get(2)?,
        path: String::new(),
        parent_id: r.get(3)?,
        attributes: serde_json::from_str(&r.get::<_, String>(4)?)
            .unwrap_or_else(|_| serde_json::json!({})),
        managed: r.get::<_, i64>(5)? != 0,
        org_id: r.get(6)?,
    })
}

/// `/a/b/c` for a group, walking `parent_id` (at most 64 levels).
pub(crate) fn group_path_in_tx(conn: &Connection, id: i64) -> Result<String, IpcError> {
    let mut names = Vec::new();
    let mut cur = Some(id);
    while let Some(g) = cur {
        if names.len() >= MAX_GROUP_DEPTH {
            return Err(invalid("group nesting too deep"));
        }
        let (name, parent): (String, Option<i64>) = conn
            .query_row(
                "SELECT name, parent_id FROM groups WHERE id = ?1",
                rusqlite::params![g],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("group {g} not found")))?;
        names.push(name);
        cur = parent;
    }
    names.reverse();
    Ok(format!("/{}", names.join("/")))
}

fn person_exists(conn: &Connection, id: i64) -> Result<bool, IpcError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM people WHERE id = ?1)",
        rusqlite::params![id],
        |r| r.get(0),
    )?)
}

impl Store {
    // ---- roles -----------------------------------------------------------

    /// Roles of one client (`Some("fleet")`), or every role (`None`), ordered
    /// realm roles first, then by client and name.
    pub fn list_roles(&self, client: Option<&str>) -> Result<Vec<RoleRow>, IpcError> {
        let sql = format!(
            "{ROLE_SELECT} WHERE (?1 IS NULL OR c.client_id = ?1) \
             ORDER BY c.client_id IS NOT NULL, c.client_id, r.name"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![client], map_role_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn role(&self, r: RoleRef) -> Result<Option<RoleRow>, IpcError> {
        let row = match r {
            RoleRef::Realm(name) => self.conn.query_row(
                &format!("{ROLE_SELECT} WHERE r.client_id IS NULL AND r.name = ?1"),
                rusqlite::params![name],
                map_role_row,
            ),
            RoleRef::Client(client, name) => self.conn.query_row(
                &format!("{ROLE_SELECT} WHERE c.client_id = ?1 AND r.name = ?2"),
                rusqlite::params![client, name],
                map_role_row,
            ),
        };
        Ok(row.optional()?)
    }

    /// A new, non-seeded role of a client (`Some`) or of the realm (`None`).
    pub fn create_role(
        &self,
        client: Option<&str>,
        name: &str,
        description: Option<&str>,
    ) -> Result<RoleRow, IpcError> {
        let name = validate_realm_name("role", name, false)?;
        let description = validate_description(description)?;
        self.with_tx(|c| {
            let client_row: Option<i64> = match client {
                None => None,
                Some(cid) => Some(
                    c.query_row(
                        "SELECT id FROM clients WHERE client_id = ?1",
                        rusqlite::params![cid],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| {
                        IpcError::new(codes::E_NOTFOUND, format!("client {cid:?} not found"))
                    })?,
                ),
            };
            let taken: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM roles WHERE ifnull(client_id, 0) = ifnull(?1, 0) AND name = ?2)",
                rusqlite::params![client_row, name],
                |r| r.get(0),
            )?;
            if taken {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!("a role named {name:?} already exists there"),
                ));
            }
            c.execute(
                "INSERT INTO roles (uuid, client_id, name, description, seeded, created_at) \
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    client_row,
                    name,
                    description,
                    now_unix()
                ],
            )?;
            role_or_404(c, c.last_insert_rowid())
        })
    }

    pub fn update_role(&self, id: i64, description: Option<&str>) -> Result<RoleRow, IpcError> {
        let description = validate_description(description)?;
        self.with_tx(|c| {
            role_or_404(c, id)?;
            c.execute(
                "UPDATE roles SET description = ?2 WHERE id = ?1",
                rusqlite::params![id, description],
            )?;
            role_or_404(c, id)
        })
    }

    /// Delete a role and, by cascade, its composites and mappings. A seeded
    /// role is `E_FORBIDDEN`; `false` when there is no such role.
    pub fn delete_role(&self, id: i64) -> Result<bool, IpcError> {
        self.with_tx(|c| {
            let Some(role) = load_role(c, id)? else {
                return Ok(false);
            };
            if role.seeded {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    format!("role {:?} is seeded and cannot be deleted", role.name),
                ));
            }
            keeping_fleet_admin(c, || {
                Ok(c.execute("DELETE FROM roles WHERE id = ?1", rusqlite::params![id])? > 0)
            })
        })
    }

    /// `parent` now includes `child`. Refused (`E_VALIDATE`) when `child`
    /// already reaches `parent`, which would make a cycle.
    pub fn add_composite(&self, parent: i64, child: i64) -> Result<(), IpcError> {
        self.with_tx(|c| {
            let prole = role_or_404(c, parent)?;
            let crole = role_or_404(c, child)?;
            if parent == child {
                return Err(invalid("a role cannot include itself"));
            }
            // Org roles stay inside the org model: nothing outside it may
            // reach one through a composite, and one may not reach out.
            if is_org_role(&prole) && !is_org_role(&crole) {
                return Err(invalid("an org role can only include org roles"));
            }
            if !is_org_role(&prole) {
                let reaches_org: bool = c.query_row(
                    "WITH RECURSIVE d(id) AS (SELECT ?1 \
                       UNION SELECT rc.child_id FROM role_composites rc JOIN d ON rc.parent_id = d.id) \
                     SELECT EXISTS(SELECT 1 FROM d JOIN roles r ON r.id = d.id \
                       JOIN clients cl ON cl.id = r.client_id \
                       WHERE cl.client_id = ?2 AND substr(r.name, 1, 4) = 'org-')",
                    rusqlite::params![child, CLIENT_FLEET],
                    |r| r.get(0),
                )?;
                if reaches_org {
                    return Err(org_role_err());
                }
            }
            let cycle: bool = c.query_row(
                "WITH RECURSIVE d(id) AS (SELECT ?1 \
                   UNION SELECT rc.child_id FROM role_composites rc JOIN d ON rc.parent_id = d.id) \
                 SELECT EXISTS(SELECT 1 FROM d WHERE id = ?2)",
                rusqlite::params![child, parent],
                |r| r.get(0),
            )?;
            if cycle {
                return Err(invalid("that composite would make a cycle"));
            }
            c.execute(
                "INSERT OR IGNORE INTO role_composites (parent_id, child_id) VALUES (?1, ?2)",
                rusqlite::params![parent, child],
            )?;
            Ok(())
        })
    }

    /// Remove an edge. An edge between two seeded roles is `E_FORBIDDEN`.
    pub fn remove_composite(&self, parent: i64, child: i64) -> Result<bool, IpcError> {
        self.with_tx(|c| {
            if let (Some(p), Some(ch)) = (load_role(c, parent)?, load_role(c, child)?) {
                if p.seeded && ch.seeded {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "a composite between seeded roles cannot be removed",
                    ));
                }
            }
            keeping_fleet_admin(c, || {
                Ok(c.execute(
                    "DELETE FROM role_composites WHERE parent_id = ?1 AND child_id = ?2",
                    rusqlite::params![parent, child],
                )? > 0)
            })
        })
    }

    // ---- groups ----------------------------------------------------------

    pub fn list_groups(&self) -> Result<Vec<GroupRow>, IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {GROUP_COLUMNS} FROM groups ORDER BY id"))?;
        let mut rows = stmt
            .query_map([], map_group_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for g in &mut rows {
            g.path = group_path_in_tx(&self.conn, g.id)?;
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(rows)
    }

    pub fn group(&self, id: i64) -> Result<Option<GroupRow>, IpcError> {
        load_group(&self.conn, id)
    }

    /// The group at `/a/b/c`, or `None`.
    pub fn group_by_path(&self, path: &str) -> Result<Option<GroupRow>, IpcError> {
        let Some(rest) = path.strip_prefix('/') else {
            return Ok(None);
        };
        let mut cur: Option<i64> = None;
        for seg in rest.split('/') {
            if seg.is_empty() {
                return Ok(None);
            }
            let next: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM groups WHERE ifnull(parent_id, 0) = ifnull(?1, 0) AND name = ?2",
                    rusqlite::params![cur, seg],
                    |r| r.get(0),
                )
                .optional()?;
            match next {
                Some(id) => cur = Some(id),
                None => return Ok(None),
            }
        }
        match cur {
            Some(id) => load_group(&self.conn, id),
            None => Ok(None),
        }
    }

    /// A new, unmanaged group under `parent` (or at the top). A managed
    /// parent is `E_FORBIDDEN`.
    pub fn create_group(
        &self,
        parent: Option<i64>,
        name: &str,
        by: Option<i64>,
    ) -> Result<GroupRow, IpcError> {
        self.with_tx(|c| {
            if let Some(p) = parent {
                if group_or_404(c, p)?.managed {
                    return Err(managed_err());
                }
            }
            self.create_group_in_tx(c, parent, name, false, None, by)
        })
    }

    /// The raw writer: no managed check. `_by` is reserved (no column yet).
    pub fn create_group_in_tx(
        &self,
        conn: &Connection,
        parent: Option<i64>,
        name: &str,
        managed: bool,
        org_id: Option<i64>,
        _by: Option<i64>,
    ) -> Result<GroupRow, IpcError> {
        let name = validate_realm_name("group", name, true)?;
        if let Some(p) = parent {
            group_or_404(conn, p)?;
        }
        if sibling_named(conn, parent, &name, None)? {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a group named {name:?} already exists there"),
            ));
        }
        conn.execute(
            "INSERT INTO groups (uuid, name, parent_id, managed, org_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                name,
                parent,
                managed,
                org_id,
                now_unix()
            ],
        )?;
        // Also bounds the depth: the path walk refuses past 64 levels.
        group_or_404(conn, conn.last_insert_rowid())
    }

    pub fn rename_group(&self, id: i64, name: &str) -> Result<GroupRow, IpcError> {
        self.with_tx(|c| {
            if group_or_404(c, id)?.managed {
                return Err(managed_err());
            }
            self.rename_group_in_tx(c, id, name)
        })
    }

    /// The raw writer: no managed check.
    pub fn rename_group_in_tx(
        &self,
        conn: &Connection,
        id: i64,
        name: &str,
    ) -> Result<GroupRow, IpcError> {
        let name = validate_realm_name("group", name, true)?;
        let g = group_or_404(conn, id)?;
        if sibling_named(conn, g.parent_id, &name, Some(id))? {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a group named {name:?} already exists there"),
            ));
        }
        conn.execute(
            "UPDATE groups SET name = ?2 WHERE id = ?1",
            rusqlite::params![id, name],
        )?;
        group_or_404(conn, id)
    }

    /// Move a group under `parent` (`None`: to the top). Refused when the
    /// new parent is the group or one of its descendants.
    pub fn move_group(&self, id: i64, parent: Option<i64>) -> Result<GroupRow, IpcError> {
        self.with_tx(|c| {
            let g = group_or_404(c, id)?;
            if g.managed {
                return Err(managed_err());
            }
            if let Some(p) = parent {
                if group_or_404(c, p)?.managed {
                    return Err(managed_err());
                }
                let mut cur = Some(p);
                let mut steps = 0;
                while let Some(x) = cur {
                    if x == id {
                        return Err(invalid("a group cannot move under itself"));
                    }
                    steps += 1;
                    if steps > MAX_GROUP_DEPTH {
                        return Err(invalid("group nesting too deep"));
                    }
                    cur = c.query_row(
                        "SELECT parent_id FROM groups WHERE id = ?1",
                        rusqlite::params![x],
                        |r| r.get(0),
                    )?;
                }
            }
            if sibling_named(c, parent, &g.name, Some(id))? {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!("a group named {:?} already exists there", g.name),
                ));
            }
            c.execute(
                "UPDATE groups SET parent_id = ?2 WHERE id = ?1",
                rusqlite::params![id, parent],
            )?;
            group_or_404(c, id)
        })
    }

    /// Delete a group and, by cascade, its subgroups, memberships and
    /// mappings. A managed group is `E_FORBIDDEN`; `false` when absent.
    pub fn delete_group(&self, id: i64) -> Result<bool, IpcError> {
        self.with_tx(|c| {
            if let Some(g) = load_group(c, id)? {
                if g.managed {
                    return Err(managed_err());
                }
            }
            keeping_fleet_admin(c, || self.delete_group_in_tx(c, id))
        })
    }

    /// The raw writer: no managed check.
    pub fn delete_group_in_tx(&self, conn: &Connection, id: i64) -> Result<bool, IpcError> {
        Ok(conn.execute("DELETE FROM groups WHERE id = ?1", rusqlite::params![id])? > 0)
    }

    /// Direct members, ascending person ids.
    pub fn group_members(&self, id: i64) -> Result<Vec<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT person_id FROM group_members WHERE group_id = ?1 ORDER BY person_id",
        )?;
        let ids = stmt.query_map(rusqlite::params![id], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The groups a person is a direct member of, by path.
    pub fn groups_of(&self, person: i64) -> Result<Vec<GroupRow>, IpcError> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT group_id FROM group_members WHERE person_id = ?1 ORDER BY group_id",
            )?;
            let ids = stmt.query_map(rusqlite::params![person], |r| r.get(0))?;
            ids.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if let Some(g) = load_group(&self.conn, id)? {
                out.push(g);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    // ---- membership and mappings -------------------------------------------

    /// Add `person` to `group` (idempotent). Both must exist.
    pub fn add_group_member_in_tx(
        &self,
        conn: &Connection,
        group: i64,
        person: i64,
        by: Option<i64>,
    ) -> Result<(), IpcError> {
        group_or_404(conn, group)?;
        if !person_exists(conn, person)? {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("person {person} not found"),
            ));
        }
        conn.execute(
            "INSERT OR IGNORE INTO group_members (group_id, person_id, added_at, added_by) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![group, person, now_unix(), by],
        )?;
        Ok(())
    }

    /// Remove `person` from `group`; `false` when they were not a member.
    /// `E_FORBIDDEN` when it would take `fleet-admin` from the personal
    /// owner or its last holder (the caller's transaction must roll back).
    pub fn remove_group_member_in_tx(
        &self,
        conn: &Connection,
        group: i64,
        person: i64,
    ) -> Result<bool, IpcError> {
        keeping_fleet_admin(conn, || {
            Ok(conn.execute(
                "DELETE FROM group_members WHERE group_id = ?1 AND person_id = ?2",
                rusqlite::params![group, person],
            )? > 0)
        })
    }

    /// Grant `role` to a person or a group. The `fleet` client's `org-*`
    /// roles are always refused (`E_VALIDATE`): only the org projection
    /// grants them, through `map_role_in_tx(.., true)`.
    pub fn map_role(&self, p: Principal, role: i64) -> Result<(), IpcError> {
        self.with_tx(|c| self.map_role_in_tx(c, p, role, false))
    }

    /// The raw writer; `allow_org_roles` is for the org projection's seeding.
    pub fn map_role_in_tx(
        &self,
        conn: &Connection,
        p: Principal,
        role: i64,
        allow_org_roles: bool,
    ) -> Result<(), IpcError> {
        let r = role_or_404(conn, role)?;
        if is_org_role(&r) && !allow_org_roles {
            return Err(org_role_err());
        }
        match p {
            Principal::Person(person) => {
                if !person_exists(conn, person)? {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("person {person} not found"),
                    ));
                }
                conn.execute(
                    "INSERT OR IGNORE INTO user_role_mappings (person_id, role_id) VALUES (?1, ?2)",
                    rusqlite::params![person, role],
                )?;
            }
            Principal::Group(group) => {
                group_or_404(conn, group)?;
                conn.execute(
                    "INSERT OR IGNORE INTO group_role_mappings (group_id, role_id) VALUES (?1, ?2)",
                    rusqlite::params![group, role],
                )?;
            }
        }
        Ok(())
    }

    /// Revoke a mapping. A managed group's mappings are `E_FORBIDDEN`, and
    /// so is taking `fleet-admin` from the personal owner or its last holder.
    pub fn unmap_role(&self, p: Principal, role: i64) -> Result<bool, IpcError> {
        self.with_tx(|c| self.unmap_role_in_tx(c, p, role, false))
    }

    /// The raw writer; `allow_managed` is for the org projection.
    pub fn unmap_role_in_tx(
        &self,
        conn: &Connection,
        p: Principal,
        role: i64,
        allow_managed: bool,
    ) -> Result<bool, IpcError> {
        keeping_fleet_admin(conn, || {
            self.unmap_role_raw_in_tx(conn, p, role, allow_managed)
        })
    }

    fn unmap_role_raw_in_tx(
        &self,
        conn: &Connection,
        p: Principal,
        role: i64,
        allow_managed: bool,
    ) -> Result<bool, IpcError> {
        let n = match p {
            Principal::Person(person) => conn.execute(
                "DELETE FROM user_role_mappings WHERE person_id = ?1 AND role_id = ?2",
                rusqlite::params![person, role],
            )?,
            Principal::Group(group) => {
                if !allow_managed {
                    if let Some(g) = load_group(conn, group)? {
                        if g.managed {
                            return Err(managed_err());
                        }
                    }
                }
                conn.execute(
                    "DELETE FROM group_role_mappings WHERE group_id = ?1 AND role_id = ?2",
                    rusqlite::params![group, role],
                )?
            }
        };
        Ok(n > 0)
    }

    // ---- resolution ----------------------------------------------------------

    /// Every role a live person holds and why, sorted by (client, name).
    pub fn effective_roles(&self, person: i64) -> Result<Vec<EffectiveRole>, IpcError> {
        let conn = &self.conn;
        let ids: BTreeSet<i64> = {
            let mut stmt = conn.prepare(EFFECTIVE_ROLE_IDS)?;
            let ids = stmt.query_map(rusqlite::params![person], |r| r.get(0))?;
            ids.collect::<rusqlite::Result<BTreeSet<_>>>()?
        };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut roles: BTreeMap<i64, RoleRow> = BTreeMap::new();
        for id in &ids {
            roles.insert(*id, role_or_404(conn, *id)?);
        }
        let mut via: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
        // Direct mappings.
        {
            let mut stmt =
                conn.prepare("SELECT role_id FROM user_role_mappings WHERE person_id = ?1")?;
            let rows = stmt.query_map(rusqlite::params![person], |r| r.get::<_, i64>(0))?;
            for r in rows {
                via.entry(r?).or_default().insert("direct".into());
            }
        }
        // Groups of the person and their ancestors.
        let mut group_ids: BTreeSet<i64> = BTreeSet::new();
        {
            let mut stmt =
                conn.prepare("SELECT group_id FROM group_members WHERE person_id = ?1")?;
            let rows = stmt.query_map(rusqlite::params![person], |r| r.get::<_, i64>(0))?;
            for g in rows {
                let mut cur = Some(g?);
                let mut steps = 0;
                while let Some(x) = cur {
                    if !group_ids.insert(x) || steps > MAX_GROUP_DEPTH {
                        break;
                    }
                    steps += 1;
                    cur = conn.query_row(
                        "SELECT parent_id FROM groups WHERE id = ?1",
                        rusqlite::params![x],
                        |r| r.get(0),
                    )?;
                }
            }
        }
        for g in &group_ids {
            let path = group_path_in_tx(conn, *g)?;
            let mut stmt =
                conn.prepare("SELECT role_id FROM group_role_mappings WHERE group_id = ?1")?;
            let rows = stmt.query_map(rusqlite::params![g], |r| r.get::<_, i64>(0))?;
            for r in rows {
                via.entry(r?).or_default().insert(format!("group:{path}"));
            }
        }
        // Roles reached only through a composite.
        let mut edges = conn.prepare("SELECT parent_id, child_id FROM role_composites")?;
        let edges: Vec<(i64, i64)> = edges
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for id in &ids {
            if via.contains_key(id) {
                continue;
            }
            let entry = via.entry(*id).or_default();
            for (parent, child) in &edges {
                if child == id {
                    if let Some(p) = roles.get(parent) {
                        entry.insert(format!("composite:{}", p.name));
                    }
                }
            }
        }
        let mut out: Vec<EffectiveRole> = roles
            .into_values()
            .map(|role| EffectiveRole {
                via: via
                    .remove(&role.id)
                    .map(|v| v.into_iter().collect())
                    .unwrap_or_default(),
                role,
            })
            .collect();
        out.sort_by(|a, b| (&a.role.client, &a.role.name).cmp(&(&b.role.client, &b.role.name)));
        Ok(out)
    }

    /// Live people holding the realm role `name` (directly, through a group
    /// or its ancestors, or through a composite), ascending ids.
    pub fn people_with_realm_role(&self, name: &str) -> Result<Vec<i64>, IpcError> {
        holders_in_tx(&self.conn, name)
    }

    /// The holders of the three seeded realm roles — `fleet-admin`,
    /// `fleet-trusted`, `assets-admin`, in that order — for the auth layer
    /// (`token_cache::RealmSets`). The personal owner is added to the
    /// fleet admins unconditionally, so an owner is never locked out by a
    /// missing mapping.
    #[allow(clippy::type_complexity)]
    pub fn realm_sets(&self) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>), IpcError> {
        let mut admins = holders_in_tx(&self.conn, ROLE_FLEET_ADMIN)?;
        if let Some(owner) = personal_owner_in_tx(&self.conn)? {
            if let Err(at) = admins.binary_search(&owner) {
                admins.insert(at, owner);
            }
        }
        Ok((
            admins,
            holders_in_tx(&self.conn, ROLE_FLEET_TRUSTED)?,
            holders_in_tx(&self.conn, ROLE_ASSETS_ADMIN)?,
        ))
    }

    /// Whether `person` administers the fleet: the personal owner, or a live
    /// holder of the realm role `fleet-admin`.
    pub fn is_fleet_admin(&self, person: i64) -> Result<bool, IpcError> {
        if personal_owner_in_tx(&self.conn)? == Some(person) {
            return Ok(true);
        }
        Ok(holders_in_tx(&self.conn, ROLE_FLEET_ADMIN)?.contains(&person))
    }
}

// ---- the org projection -------------------------------------------------
//
// An org's membership lives in its managed groups: `/orgs/<org>` (the root,
// `org_id` set) and its `admins` / `members` / `viewers` subgroups, mapped to
// the `fleet` client roles `org-admin` / `org-member` / `org-viewer`.
// `org_members` is their materialised projection, so its readers
// (`ViewScope`, the org admin's authority, grant checks) are unchanged, and
// nothing but the writers below touches it.

/// The `org_members` role (`admin` | `member` | `viewer`) and its rank for a
/// `fleet` client org role; `None` for any other role.
fn member_role_of(client_role: &str) -> Option<(&'static str, u8)> {
    use super::org_members::{ROLE_ADMIN, ROLE_MEMBER, ROLE_VIEWER};
    match client_role {
        "org-admin" => Some((ROLE_ADMIN, 3)),
        "org-member" => Some((ROLE_MEMBER, 2)),
        "org-viewer" => Some((ROLE_VIEWER, 1)),
        _ => None,
    }
}

/// The subgroup of an org's root that carries `role` (`admin` → `admins`).
fn subgroup_for(role: &str) -> Result<&'static str, IpcError> {
    ORG_ROLES
        .iter()
        .find(|(_, r)| member_role_of(r).map(|(m, _)| m) == Some(role))
        .map(|(g, _)| *g)
        .ok_or_else(|| invalid(format!("unknown org role {role:?}")))
}

/// The id of org `org`'s root group `/orgs/<org>`, if it has one.
pub(crate) fn org_root_group_in_tx(conn: &Connection, org: i64) -> Result<Option<i64>, IpcError> {
    Ok(conn
        .query_row(
            "SELECT id FROM groups WHERE org_id = ?1",
            rusqlite::params![org],
            |r| r.get(0),
        )
        .optional()?)
}

/// The child of `parent` named `name`, if any.
fn child_group_in_tx(conn: &Connection, parent: i64, name: &str) -> Result<Option<i64>, IpcError> {
    Ok(conn
        .query_row(
            "SELECT id FROM groups WHERE parent_id = ?1 AND name = ?2",
            rusqlite::params![parent, name],
            |r| r.get(0),
        )
        .optional()?)
}

/// The org roles a person holds through the org groups, as `org_members`
/// would read them: per org the highest of admin > member > viewer. A
/// disabled person holds none. Ascending org ids.
pub(crate) fn org_roles_from_groups_in_tx(
    conn: &Connection,
    person: i64,
) -> Result<Vec<(i64, String)>, IpcError> {
    let mut stmt = conn.prepare(
        "SELECT root.org_id, r.name FROM group_members gm
         JOIN groups sg ON sg.id = gm.group_id AND sg.managed = 1
         JOIN groups root ON root.id = sg.parent_id AND root.org_id IS NOT NULL
         JOIN group_role_mappings m ON m.group_id = sg.id
         JOIN roles r ON r.id = m.role_id
         WHERE gm.person_id = ?1
           AND EXISTS (SELECT 1 FROM people p WHERE p.id = ?1 AND p.disabled_at IS NULL)",
    )?;
    let rows = stmt.query_map(rusqlite::params![person], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut best: BTreeMap<i64, (&'static str, u8)> = BTreeMap::new();
    for row in rows {
        let (org, role) = row?;
        let Some((member_role, rank)) = member_role_of(&role) else {
            continue;
        };
        let e = best.entry(org).or_insert((member_role, rank));
        if rank > e.1 {
            *e = (member_role, rank);
        }
    }
    Ok(best
        .into_iter()
        .map(|(org, (role, _))| (org, role.to_string()))
        .collect())
}

/// Write `person`'s live membership of `org` as `role`: a change of role, a
/// re-join of a former member (a new `added_at`; org shares from before do
/// not come back) or a first join. Gaining the right to receive shares
/// starts it now; keeping it keeps when it started; losing it clears it.
/// The caller bumps the grant generation after its commit.
fn write_org_member_row_in_tx(
    conn: &Connection,
    org: i64,
    person: i64,
    role: &str,
    by: Option<i64>,
) -> Result<(), IpcError> {
    let now = now_unix();
    let shares = super::org_members::role_receives_shares(role);
    let cur: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT shares_since, removed_at FROM org_members \
              WHERE org_id = ?1 AND person_id = ?2",
            rusqlite::params![org, person],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match cur {
        Some((shares_since, None)) => {
            let since = match (shares, shares_since) {
                (false, _) => None,
                (true, Some(t)) => Some(t),
                (true, None) => Some(now),
            };
            conn.execute(
                "UPDATE org_members SET role = ?3, shares_since = ?4 \
                  WHERE org_id = ?1 AND person_id = ?2",
                rusqlite::params![org, person, role, since],
            )?;
        }
        Some(_) => {
            conn.execute(
                "UPDATE org_members SET role = ?3, added_at = ?4, added_by = ?5, \
                        shares_since = ?6, removed_at = NULL \
                  WHERE org_id = ?1 AND person_id = ?2",
                rusqlite::params![org, person, role, now, by, shares.then_some(now)],
            )?;
        }
        None => {
            conn.execute(
                "INSERT INTO org_members \
                   (org_id, person_id, role, added_at, added_by, shares_since) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![org, person, role, now, by, shares.then_some(now)],
            )?;
        }
    }
    Ok(())
}

/// End `person`'s live membership of `org`: the row stays as a former
/// membership. `false` when there was no live one.
fn end_org_member_row_in_tx(conn: &Connection, org: i64, person: i64) -> Result<bool, IpcError> {
    Ok(conn.execute(
        "UPDATE org_members SET removed_at = ?3 \
          WHERE org_id = ?1 AND person_id = ?2 AND removed_at IS NULL",
        rusqlite::params![org, person, now_unix()],
    )? > 0)
}

/// Every live member of `org` becomes a former member at `at`: the
/// projection of an org whose groups are gone.
pub(crate) fn end_all_org_member_rows_in_tx(
    conn: &Connection,
    org: i64,
    at: i64,
) -> Result<usize, IpcError> {
    Ok(conn.execute(
        "UPDATE org_members SET removed_at = ?2 WHERE org_id = ?1 AND removed_at IS NULL",
        rusqlite::params![org, at],
    )?)
}

/// Delete org `org`'s root group with its subgroups, their memberships and
/// role mappings. Explicit rather than only the FK cascades, which need
/// `foreign_keys = ON` (as `remove_org` does for its other tables).
pub(crate) fn delete_org_groups_in_tx(conn: &Connection, org: i64) -> Result<(), IpcError> {
    let Some(root) = org_root_group_in_tx(conn, org)? else {
        return Ok(());
    };
    const TREE: &str = "SELECT id FROM groups WHERE id = ?1 OR parent_id = ?1";
    conn.execute(
        &format!("DELETE FROM group_members WHERE group_id IN ({TREE})"),
        rusqlite::params![root],
    )?;
    conn.execute(
        &format!("DELETE FROM group_role_mappings WHERE group_id IN ({TREE})"),
        rusqlite::params![root],
    )?;
    conn.execute(
        "DELETE FROM groups WHERE parent_id = ?1",
        rusqlite::params![root],
    )?;
    conn.execute("DELETE FROM groups WHERE id = ?1", rusqlite::params![root])?;
    Ok(())
}

/// Is `person` a live member of `org` in `org_members`?
pub(crate) fn live_org_member_in_tx(
    conn: &Connection,
    org: i64,
    person: i64,
) -> Result<bool, IpcError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM org_members \
          WHERE org_id = ?1 AND person_id = ?2 AND removed_at IS NULL)",
        rusqlite::params![org, person],
        |r| r.get(0),
    )?)
}

/// Bring `person`'s live `org_members` rows in line with their org groups:
/// a desired (org, role) that is absent or different is written, a live
/// org that is no longer desired ends. `by` is recorded on a (re-)join.
pub(crate) fn reproject_org_memberships_in_tx(
    conn: &Connection,
    person: i64,
    by: Option<i64>,
) -> Result<(), IpcError> {
    let desired: BTreeMap<i64, String> = org_roles_from_groups_in_tx(conn, person)?
        .into_iter()
        .collect();
    let current: BTreeMap<i64, String> = {
        let mut stmt = conn.prepare(
            "SELECT org_id, role FROM org_members WHERE person_id = ?1 AND removed_at IS NULL",
        )?;
        let rows = stmt.query_map(rusqlite::params![person], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (org, role) in &desired {
        if current.get(org) != Some(role) {
            write_org_member_row_in_tx(conn, *org, person, role, by)?;
        }
    }
    for org in current.keys() {
        if !desired.contains_key(org) {
            end_org_member_row_in_tx(conn, *org, person)?;
        }
    }
    Ok(())
}

impl Store {
    /// The direct computation the `org_members` projection must equal: the
    /// org roles `person` holds through the org groups, per org the highest.
    pub fn org_roles_from_groups(&self, person: i64) -> Result<Vec<(i64, String)>, IpcError> {
        org_roles_from_groups_in_tx(&self.conn, person)
    }

    /// Make sure org `org_id` has its groups: `/orgs`, the root
    /// `/orgs/<org_name>` (managed, `org_id` set) and the three managed
    /// subgroups mapped to their org roles. Idempotent, and agrees with what
    /// migration 178 seeded. Returns the root group's id.
    ///
    /// The root is written by hand rather than through `create_group_in_tx`:
    /// its name is the org's, already validated by the org (which allows a
    /// few characters a group name does not), exactly as migration 178 does.
    pub(crate) fn ensure_org_groups_in_tx(
        &self,
        conn: &Connection,
        org_id: i64,
        org_name: &str,
    ) -> Result<i64, IpcError> {
        let root = match org_root_group_in_tx(conn, org_id)? {
            Some(id) => id,
            None => {
                let orgs: i64 = match conn
                    .query_row(
                        "SELECT id FROM groups WHERE parent_id IS NULL AND name = 'orgs'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?
                {
                    Some(id) => id,
                    None => {
                        self.create_group_in_tx(conn, None, "orgs", true, None, None)?
                            .id
                    }
                };
                if sibling_named(conn, Some(orgs), org_name, None)? {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("a group named /orgs/{org_name} already exists"),
                    ));
                }
                conn.execute(
                    "INSERT INTO groups (uuid, name, parent_id, managed, org_id, created_at) \
                     VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        org_name,
                        orgs,
                        org_id,
                        now_unix()
                    ],
                )?;
                conn.last_insert_rowid()
            }
        };
        for (sub, role) in ORG_ROLES {
            let g = match child_group_in_tx(conn, root, sub)? {
                Some(id) => id,
                None => {
                    self.create_group_in_tx(conn, Some(root), sub, true, None, None)?
                        .id
                }
            };
            let role_id: i64 = conn
                .query_row(
                    "SELECT r.id FROM roles r JOIN clients c ON c.id = r.client_id \
                      WHERE c.client_id = ?1 AND r.name = ?2",
                    rusqlite::params![CLIENT_FLEET, role],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_INTERNAL,
                        format!("the seeded role {role:?} is missing"),
                    )
                })?;
            self.map_role_in_tx(conn, Principal::Group(g), role_id, true)?;
        }
        Ok(root)
    }

    /// Rename org `org_id`'s root group to the org's new name (validated by
    /// the org). A no-op when it has none.
    pub(crate) fn rename_org_groups_in_tx(
        &self,
        conn: &Connection,
        org_id: i64,
        name: &str,
    ) -> Result<(), IpcError> {
        let Some(root) = org_root_group_in_tx(conn, org_id)? else {
            return Ok(());
        };
        let parent: Option<i64> = conn.query_row(
            "SELECT parent_id FROM groups WHERE id = ?1",
            rusqlite::params![root],
            |r| r.get(0),
        )?;
        if sibling_named(conn, parent, name, Some(root))? {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a group named /orgs/{name} already exists"),
            ));
        }
        conn.execute(
            "UPDATE groups SET name = ?2 WHERE id = ?1 AND name <> ?2",
            rusqlite::params![root, name],
        )?;
        Ok(())
    }

    /// Put `person` in org `org`'s subgroup for `role` (and out of the other
    /// two), then reproject. The caller validated org, person and role.
    pub(crate) fn place_in_org_in_tx(
        &self,
        conn: &Connection,
        org: i64,
        org_name: &str,
        person: i64,
        role: &str,
        by: Option<i64>,
    ) -> Result<(), IpcError> {
        let target = subgroup_for(role)?;
        let root = self.ensure_org_groups_in_tx(conn, org, org_name)?;
        for (sub, _) in ORG_ROLES {
            if let Some(g) = child_group_in_tx(conn, root, sub)? {
                if sub == target {
                    self.add_group_member_in_tx(conn, g, person, by)?;
                } else {
                    self.remove_group_member_in_tx(conn, g, person)?;
                }
            }
        }
        reproject_org_memberships_in_tx(conn, person, by)
    }

    /// Take `person` out of every subgroup of org `org`, then reproject.
    pub(crate) fn take_out_of_org_in_tx(
        &self,
        conn: &Connection,
        org: i64,
        person: i64,
    ) -> Result<(), IpcError> {
        if let Some(root) = org_root_group_in_tx(conn, org)? {
            for (sub, _) in ORG_ROLES {
                if let Some(g) = child_group_in_tx(conn, root, sub)? {
                    self.remove_group_member_in_tx(conn, g, person)?;
                }
            }
        }
        reproject_org_memberships_in_tx(conn, person, None)
    }
}

fn is_org_role(r: &RoleRow) -> bool {
    r.client.as_deref() == Some(CLIENT_FLEET) && r.name.starts_with("org-")
}

fn org_role_err() -> IpcError {
    invalid("org roles are granted through the org's group `/orgs/<org>`")
}

/// Is there a group named `name` under `parent`, other than `except`?
fn sibling_named(
    conn: &Connection,
    parent: Option<i64>,
    name: &str,
    except: Option<i64>,
) -> Result<bool, IpcError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM groups WHERE ifnull(parent_id, 0) = ifnull(?1, 0) \
         AND name = ?2 AND id <> ifnull(?3, 0))",
        rusqlite::params![parent, name, except],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests;
