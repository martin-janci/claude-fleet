//! Org memberships (org administration phase D, migration 107): who is in
//! which company, with which role, and what that makes of a person's device.
//!
//! Plan `docs/superpowers/plans/2026-10-06-org-administration-phase-d.md`.
//!
//! **A device's org follows its person.** [`effective_device`] is the one
//! rule, applied where the auth layer reads token rows
//! ([`Store::auth_client_tokens`]) and where `/events` re-checks a client
//! ([`Store::client_token_binding`]):
//!
//! * the hub's owner, or a person with no membership row: the device's own
//!   binding, exactly as before migration 107 — an upgraded hub changes
//!   nothing;
//! * a person with live memberships: the device's binding when it is one of
//!   them, else their first one. Never an org they are not in;
//! * a person whose every membership was removed: [`NO_ORG`], which reads
//!   nothing of any org — a departed member does not fall back to every
//!   org's work.
//!
//! A viewer's device, and a former member's, is `readonly` whatever its
//! token says.
//!
//! **Every write moves the auth epoch** (migration 107's triggers), because a
//! membership changes what a cached caller resolves to, and the grant
//! generation, because it changes which org grants reach whom.

use super::{now_unix, ClientBinding, ClientTokenRow, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

/// Administers the org: everything phases A–C let the hub's owner do, for
/// that org only.
pub const ROLE_ADMIN: &str = "admin";
/// Sees the org's work and what is shared with the org.
pub const ROLE_MEMBER: &str = "member";
/// Reads the org's work view and overview; never writes, never receives an
/// org share.
pub const ROLE_VIEWER: &str = "viewer";
/// The three roles, widest first.
pub const ORG_ROLES: [&str; 3] = [ROLE_ADMIN, ROLE_MEMBER, ROLE_VIEWER];

/// The org a former member's device is fenced to: an id no org has
/// (`orgs.id` is AUTOINCREMENT and starts at 1), so it reads nothing of any
/// org and no unassigned data either (`OrgScope::for_client` reads the
/// missing org's `bound_sees_unassigned` as off).
pub const NO_ORG: i64 = 0;

/// `role` as it is stored, or `E_VALIDATE`.
pub fn validate_org_role(role: &str) -> Result<&'static str, IpcError> {
    let r = role.trim();
    ORG_ROLES
        .iter()
        .find(|known| known.eq_ignore_ascii_case(r))
        .copied()
        .ok_or_else(|| {
            IpcError::new(
                codes::E_VALIDATE,
                format!("role must be admin, member or viewer, not {role:?}"),
            )
        })
}

/// Receives org shares: a member or an admin.
pub fn role_receives_shares(role: &str) -> bool {
    role == ROLE_ADMIN || role == ROLE_MEMBER
}

/// One membership row, live or former.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrgMemberRow {
    pub org_id: i64,
    pub person_id: i64,
    pub role: String,
    pub added_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<i64>,
    /// When they last became able to receive org shares; `None` for a
    /// viewer. See migration 107.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares_since: Option<i64>,
    /// They left the company; the row stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<i64>,
}

impl OrgMemberRow {
    pub fn is_live(&self) -> bool {
        self.removed_at.is_none()
    }
}

const MEMBER_COLUMNS: &str =
    "org_id, person_id, role, added_at, added_by, shares_since, removed_at";

fn map_member(r: &rusqlite::Row<'_>) -> rusqlite::Result<OrgMemberRow> {
    Ok(OrgMemberRow {
        org_id: r.get(0)?,
        person_id: r.get(1)?,
        role: r.get(2)?,
        added_at: r.get(3)?,
        added_by: r.get(4)?,
        shares_since: r.get(5)?,
        removed_at: r.get(6)?,
    })
}

/// What a person's device resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceOrg {
    /// The org the device is fenced to; `None` unbound.
    pub org_id: Option<i64>,
    /// Read-only whatever its token mode says: a viewer, or a former member.
    pub readonly: bool,
}

/// THE rule (see the module docs). `memberships` are this person's rows,
/// live and former, in any order.
pub fn effective_device(
    person: Option<i64>,
    stored_org: Option<i64>,
    owner: Option<i64>,
    memberships: &[OrgMemberRow],
) -> DeviceOrg {
    let unchanged = DeviceOrg {
        org_id: stored_org,
        readonly: false,
    };
    let Some(p) = person else {
        return unchanged;
    };
    // `matches!`, not `==`: two `None`s must not make a person-less device
    // the owner's (the `is_the_personal_owner` trap).
    if matches!(owner, Some(o) if o == p) {
        return unchanged;
    }
    let mine: Vec<&OrgMemberRow> = memberships.iter().filter(|m| m.person_id == p).collect();
    if mine.is_empty() {
        return unchanged;
    }
    let mut live: Vec<&OrgMemberRow> = mine.iter().copied().filter(|m| m.is_live()).collect();
    if live.is_empty() {
        return DeviceOrg {
            org_id: Some(NO_ORG),
            readonly: true,
        };
    }
    live.sort_by_key(|m| (m.added_at, m.org_id));
    let chosen = stored_org
        .and_then(|o| live.iter().find(|m| m.org_id == o).copied())
        .unwrap_or(live[0]);
    DeviceOrg {
        org_id: Some(chosen.org_id),
        readonly: chosen.role == ROLE_VIEWER,
    }
}

impl Store {
    /// Add `person` to `org` as `role`, or change their role; a former
    /// member re-joins (a new `added_at`, and org shares from before do not
    /// come back). `added_by` is the person doing it, `None` for the
    /// operator or the desktop's own store.
    ///
    /// `E_NOTFOUND` for an unknown org or person, `E_VALIDATE` for a disabled
    /// person or an unknown role.
    pub fn set_org_member(
        &self,
        org: i64,
        person: i64,
        role: &str,
        added_by: Option<i64>,
    ) -> Result<OrgMemberRow, IpcError> {
        let role = validate_org_role(role)?;
        if self.get_org(org)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("org {org} not found"),
            ));
        }
        let p = self
            .get_person(person)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no person {person}")))?;
        if p.disabled_at.is_some() {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!(
                    "{:?} is disabled; a disabled person joins no company",
                    p.name
                ),
            ));
        }
        let now = now_unix();
        let shares = role_receives_shares(role);
        match self.org_member(org, person)? {
            Some(m) if m.is_live() => {
                // A change of role. Gaining the right to receive shares
                // starts it now; keeping it keeps when it started; losing it
                // clears it.
                let since = match (shares, m.shares_since) {
                    (false, _) => None,
                    (true, Some(t)) => Some(t),
                    (true, None) => Some(now),
                };
                self.conn.execute(
                    "UPDATE org_members SET role = ?3, shares_since = ?4 \
                      WHERE org_id = ?1 AND person_id = ?2",
                    rusqlite::params![org, person, role, since],
                )?;
            }
            Some(_) => {
                self.conn.execute(
                    "UPDATE org_members SET role = ?3, added_at = ?4, added_by = ?5, \
                            shares_since = ?6, removed_at = NULL \
                      WHERE org_id = ?1 AND person_id = ?2",
                    rusqlite::params![org, person, role, now, added_by, shares.then_some(now)],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO org_members \
                       (org_id, person_id, role, added_at, added_by, shares_since) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![org, person, role, now, added_by, shares.then_some(now)],
                )?;
            }
        }
        super::session_grants::bump_grant_generation();
        self.org_member(org, person)?.ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                "the membership vanished right after it was written",
            )
        })
    }

    /// Take `person` out of `org`: the row stays as a former membership.
    /// `false` when they were not a live member.
    pub fn remove_org_member(&self, org: i64, person: i64) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE org_members SET removed_at = ?3 \
              WHERE org_id = ?1 AND person_id = ?2 AND removed_at IS NULL",
            rusqlite::params![org, person, now_unix()],
        )?;
        if n > 0 {
            super::session_grants::bump_grant_generation();
        }
        Ok(n > 0)
    }

    /// One membership row, live or former.
    pub fn org_member(&self, org: i64, person: i64) -> Result<Option<OrgMemberRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {MEMBER_COLUMNS} FROM org_members \
                      WHERE org_id = ?1 AND person_id = ?2"
                ),
                rusqlite::params![org, person],
                map_member,
            )
            .optional()?)
    }

    /// `person`'s LIVE role in `org`, if any.
    pub fn org_role(&self, org: i64, person: i64) -> Result<Option<String>, IpcError> {
        Ok(self
            .org_member(org, person)?
            .filter(OrgMemberRow::is_live)
            .map(|m| m.role))
    }

    /// The live members of `org`: admins first, then by when they joined.
    pub fn org_members(&self, org: i64) -> Result<Vec<OrgMemberRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {MEMBER_COLUMNS} FROM org_members \
              WHERE org_id = ?1 AND removed_at IS NULL \
              ORDER BY CASE role WHEN 'admin' THEN 0 WHEN 'member' THEN 1 ELSE 2 END, \
                       added_at, person_id"
        ))?;
        let rows = st.query_map([org], map_member)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every membership row of `person`, live and former.
    pub fn memberships_of(&self, person: i64) -> Result<Vec<OrgMemberRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {MEMBER_COLUMNS} FROM org_members WHERE person_id = ?1 \
              ORDER BY added_at, org_id"
        ))?;
        let rows = st.query_map([person], map_member)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every membership row, by person.
    fn memberships_by_person(&self) -> Result<BTreeMap<i64, Vec<OrgMemberRow>>, IpcError> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {MEMBER_COLUMNS} FROM org_members"))?;
        let mut out: BTreeMap<i64, Vec<OrgMemberRow>> = BTreeMap::new();
        for m in st.query_map([], map_member)? {
            let m = m?;
            out.entry(m.person_id).or_default().push(m);
        }
        Ok(out)
    }

    /// The org that owns the hub (owner's answer 1), if any.
    pub fn hub_owner_org(&self) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row("SELECT id FROM orgs WHERE owns_hub = 1", [], |r| r.get(0))
            .optional()?)
    }

    /// Make `org` the company that owns the hub, or (`None`) no company.
    pub fn set_hub_owner_org(&self, org: Option<i64>) -> Result<(), IpcError> {
        if let Some(o) = org {
            if self.get_org(o)?.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("org {o} not found"),
                ));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE orgs SET owns_hub = 0 WHERE owns_hub = 1", [])?;
        if let Some(o) = org {
            tx.execute("UPDATE orgs SET owns_hub = 1 WHERE id = ?1", [o])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The hub owner's switch: `org`'s admins see the unclaimed count on its
    /// hosts.
    pub fn set_org_admins_see_unclaimed(
        &self,
        org: i64,
        on: bool,
    ) -> Result<super::OrgRow, IpcError> {
        let n = self.conn.execute(
            "UPDATE orgs SET admins_see_unclaimed = ?2 WHERE id = ?1",
            rusqlite::params![org, on as i64],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("org {org} not found"),
            ));
        }
        self.get_org(org)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("org {org} not found")))
    }

    /// The live client rows as the AUTH layer must see them: each person's
    /// device with its org and mode as its memberships make them
    /// ([`effective_device`]). Listings and admin writes read
    /// [`Store::active_client_tokens`], which is the stored binding.
    pub fn auth_client_tokens(&self) -> Result<Vec<ClientTokenRow>, IpcError> {
        let mut rows = self.active_client_tokens()?;
        let owner = self.personal_owner_id()?;
        let members = self.memberships_by_person()?;
        for r in &mut rows {
            if super::machine_token_kind(&r.mode).is_some() {
                continue;
            }
            let Some(p) = r.person_id else { continue };
            let Some(mine) = members.get(&p) else {
                continue;
            };
            let d = effective_device(Some(p), r.org_id, owner, mine);
            r.org_id = d.org_id;
            if d.readonly && matches!(r.mode.as_str(), "full" | "answer") {
                r.mode = "readonly".into();
            }
        }
        Ok(rows)
    }

    /// A stored binding with the membership rule applied (`/events`' beat
    /// compares it to the one the stream opened as).
    pub(crate) fn effective_binding(
        &self,
        stored: ClientBinding,
    ) -> Result<ClientBinding, IpcError> {
        let Some(p) = stored.person_id else {
            return Ok(stored);
        };
        let d = effective_device(
            Some(p),
            stored.org_id,
            self.personal_owner_id()?,
            &self.memberships_of(p)?,
        );
        Ok(ClientBinding {
            org_id: d.org_id,
            person_id: stored.person_id,
        })
    }
}

#[cfg(test)]
#[path = "org_members_tests.rs"]
mod tests;
