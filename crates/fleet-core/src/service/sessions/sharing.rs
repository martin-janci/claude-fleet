//! Sharing one session with one person, and reading back who holds what
//! (multi-user M1, T12).
//!
//! The rules live one layer down, in `store/session_grants.rs`, where each of
//! them is a clause of the writing statement rather than a check a caller
//! could forget: only the owner creates a grant, a grant moves downward only,
//! an org recipient is refused, and there is no function at all that raises a
//! level. This module is the thin layer above it that the MCP tools and the
//! desktop commands share — it resolves a person NAME to a row, and it turns
//! the store's grant rows into the answer a Share sheet draws.
//!
//! **Who may call any of this is NOT decided here.** The reach gate
//! (`mcp/tools/support.rs::require_person_sees` with `Reach::Own`) refuses a
//! caller who is not the owner before the call arrives, so that a stranger
//! gets `E_NOTFOUND` for a row they cannot see rather than an `E_FORBIDDEN`
//! that tells them it exists. The store's own owner comparison then refuses it
//! again, because two layers each needing the rule is the normal shape here
//! (spec §4.3, *Where `owns` lives, and where it does not*) and because the
//! gate above is the one that can be forgotten by a new call site.

use super::*;
use crate::ipc_error::codes;
use crate::store::{GrantRecipient, SessionGrantRow};

/// One live grant as a Share sheet draws it: the store row plus the
/// recipient's current name.
///
/// The name is resolved here rather than stored on the grant: `people.name`
/// is renameable (`Store::rename_person`), so a name copied into the grant
/// row at `granted_at` would be the name the person had then.
///
/// `person_id` stays on the answer beside the name because the name is a
/// label and the id is the identity: the owner's next `session_unshare` is
/// addressed by name (that is what they typed), but a client that has to tell
/// two grants apart — or match one against `my_grants` — needs the id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionGrantView {
    pub session_id: i64,
    /// The recipient: a person, or (org administration phase D) an org.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    /// The recipient's current name, or `None` for a grant whose `people` row
    /// has gone (which `session_grants`' foreign key makes unlikely and not
    /// impossible). A client falls back to the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_display_name: Option<String>,
    /// An org recipient: its members and admins (from when they could
    /// receive shares).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    /// `watch` or `drive`.
    pub level: String,
    pub granted_by: i64,
    pub granted_at: i64,
}

impl SessionGrantView {
    fn of(row: SessionGrantRow, s: &Store) -> Result<Self, IpcError> {
        // A failed read propagates; it never becomes a missing name, because
        // a sheet that shows "person 7" where it should show "jane" is a
        // worse answer than an error the owner can retry.
        let person = match row.person_id {
            Some(p) => s.get_person(p)?,
            None => None,
        };
        let org = match row.org_id {
            Some(o) => s.get_org(o)?,
            None => None,
        };
        Ok(SessionGrantView {
            session_id: row.session_id,
            person_id: row.person_id,
            person_name: person.as_ref().map(|p| p.name.clone()),
            person_display_name: person.and_then(|p| p.display_name),
            org_id: row.org_id,
            org_name: org.map(|o| o.name),
            level: row.level,
            granted_by: row.granted_by,
            granted_at: row.granted_at,
        })
    }
}

/// This caller's own person and every live grant TO them — `my_grants`.
///
/// Per-caller by construction, which is the whole reason it is a tool answer
/// and not a field on a `SessionRow`: the event bus serialises one row for
/// every recipient, `strip_nulls` makes an absent per-caller field
/// indistinguishable from "no restriction", and the frontend's row store
/// replaces a held row wholesale — so a per-caller field would be erased by
/// the next routine `session:updated` (spec §5.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MyGrants {
    /// Who the caller is, as a `people` row id. `None` for a caller that
    /// proves no person — which answers an EMPTY grant list, never every
    /// grant.
    pub person_id: Option<i64>,
    pub grants: Vec<MyGrant>,
    /// Gap plan G4.2: the caller's own open asks for a wider level, so the
    /// recipient's header says "Asked for Answer · waiting for Martin".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<MyAccessRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MyGrant {
    pub session_id: i64,
    pub level: String,
    /// Gap plan G4.2, the recipient's header ("Shared by Martin · Read ·
    /// since 13:20", "via 32bit"): who granted the share in force, by id and
    /// by current name, when, and the org it came through. Optional, so an
    /// older hub's answer still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_by: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_by_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_org: Option<String>,
}

/// One of the caller's own open asks (`my_grants`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MyAccessRequest {
    pub id: i64,
    pub session_id: i64,
    /// The level asked for.
    pub level: String,
    pub requested_at: i64,
}

/// One open ask, as the owner's Share sheet draws it (gap plan G4.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessRequestView {
    pub id: i64,
    pub session_id: i64,
    /// The session's caption, for a list across sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub person_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_display_name: Option<String>,
    /// The level asked for.
    pub level: String,
    pub requested_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

impl AccessRequestView {
    fn of(r: crate::store::AccessRequestRow, s: &Store) -> Result<Self, IpcError> {
        let person = s.get_person(r.person_id)?;
        let session_name = s
            .get_session_by_id(r.session_id)?
            .map(|row| row.friendly_name.unwrap_or(row.tmux_name));
        Ok(AccessRequestView {
            id: r.id,
            session_id: r.session_id,
            session_name,
            person_id: r.person_id,
            person_name: person.as_ref().map(|p| p.name.clone()),
            person_display_name: person.and_then(|p| p.display_name),
            level: r.level,
            requested_at: r.requested_at,
            resolution: r.resolution,
        })
    }
}

/// The caller asks the owner of `session_id` for `level` (gap plan G4.2).
/// The store holds the rules; a caller that proves no person cannot ask.
pub fn ask_access(
    s: &Store,
    session_id: i64,
    level: &str,
    asker: Option<i64>,
) -> Result<AccessRequestView, IpcError> {
    let asker = asker.ok_or_else(|| {
        IpcError::new(
            codes::E_FORBIDDEN,
            "only a person a session is shared with can ask for more, and this caller \
             proves no person",
        )
    })?;
    let row = s.request_access(session_id, asker, level)?;
    AccessRequestView::of(row, s)
}

/// The open asks on the owner's sessions (`session_id` only, when given).
/// A caller that proves no person owns nothing, so it gets an empty list.
pub fn access_requests(
    s: &Store,
    owner: Option<i64>,
    session_id: Option<i64>,
) -> Result<Vec<AccessRequestView>, IpcError> {
    let Some(owner) = owner else {
        return Ok(Vec::new());
    };
    s.open_access_requests_for_owner(owner, session_id)?
        .into_iter()
        .map(|r| AccessRequestView::of(r, s))
        .collect()
}

/// The owner grants or declines ask `id`.
pub fn resolve_access_request(
    s: &Store,
    id: i64,
    grant: bool,
    owner: Option<i64>,
) -> Result<AccessRequestView, IpcError> {
    let owner = require_granter(owner, "answer an ask on")?;
    let row = s.resolve_access_request(id, owner, grant)?;
    AccessRequestView::of(row, s)
}

/// The live person holding `name`, for a grant or a claim to be addressed to.
///
/// Live only ([`Store::get_person_by_name`]), so a name a departed colleague
/// used and a new one now holds resolves to the person who holds it. A
/// disabled person is `E_NOTFOUND` here rather than a row that would then be
/// refused deeper down with a different word.
pub fn person_named(s: &Store, name: &str) -> Result<i64, IpcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            "name a person: a grant and a claim are both addressed to someone",
        ));
    }
    s.get_person_by_name(name)?.map(|p| p.id).ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!(
                "this hub knows no live person named {name:?}; see fleet-hub client list, \
                 or pair their device with fleet-hub pair --person {name}"
            ),
        )
    })
}

/// Who a share is addressed to, by name: a person, or (org administration
/// phase D) an org — its members and admins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareTo<'a> {
    Person(&'a str),
    Org(&'a str),
}

impl<'a> ShareTo<'a> {
    /// From a tool's or a command's two fields: exactly one names someone.
    pub fn from_fields(person: &'a str, org: Option<&'a str>) -> Result<Self, IpcError> {
        let org = org.map(str::trim).filter(|o| !o.is_empty());
        match (person.trim(), org) {
            ("", Some(o)) => Ok(ShareTo::Org(o)),
            (p, None) if !p.is_empty() => Ok(ShareTo::Person(p)),
            ("", None) => Err(IpcError::new(
                codes::E_VALIDATE,
                "name a person or an org: a grant is addressed to someone",
            )),
            _ => Err(IpcError::new(
                codes::E_VALIDATE,
                "a grant is addressed to a person or an org, not both",
            )),
        }
    }

    fn resolve(self, s: &Store) -> Result<GrantRecipient, IpcError> {
        Ok(match self {
            ShareTo::Person(name) => GrantRecipient::Person(person_named(s, name)?),
            ShareTo::Org(name) => GrantRecipient::Org(org_named(s, name)?),
        })
    }
}

/// The org named `name` (case-insensitively, as org names are unique).
pub fn org_named(s: &Store, name: &str) -> Result<i64, IpcError> {
    let name = name.trim();
    s.list_orgs()?
        .into_iter()
        .find(|o| o.name.eq_ignore_ascii_case(name))
        .map(|o| o.id)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no org named {name:?}")))
}

/// [`share_session`] to a person or an org.
pub fn share_session_to(
    s: &Store,
    session_id: i64,
    to: ShareTo<'_>,
    level: &str,
    granter: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let granter = require_granter(granter, "share")?;
    let to = to.resolve(s)?;
    s.grant_session(session_id, to, level, granter)?;
    row_after(s, session_id)
}

/// [`unshare_session`] for a person or an org recipient.
pub fn unshare_session_to(
    s: &Store,
    session_id: i64,
    to: ShareTo<'_>,
    granter: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let granter = require_granter(granter, "revoke a grant on")?;
    let to = to.resolve(s)?;
    s.revoke_session_grant_to(session_id, to, granter)?;
    row_after(s, session_id)
}

/// [`narrow_session_share`] for a person or an org recipient.
pub fn narrow_session_share_to(
    s: &Store,
    session_id: i64,
    to: ShareTo<'_>,
    granter: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let granter = require_granter(granter, "narrow a grant on")?;
    let to = to.resolve(s)?;
    s.narrow_session_grant_to(session_id, to, granter)?;
    row_after(s, session_id)
}

/// Share `session_id` with the person named `person`, at `level`, as
/// `granter`.
///
/// `granter` is an `Option` because a caller may prove no person at all (a
/// device no pairing bound, a hub that cannot say who owns it), and the
/// answer for that caller is a refusal rather than a widening — so the
/// `None` arm is the first thing this function writes. Taking an
/// `Option<i64>` and refusing it here is deliberate: the alternative shape,
/// an `i64` the caller unwraps, puts the decision at each call site.
///
/// Returns the session row, so the desktop's optimistic patch has the
/// `row_version` that `announce_grant_change`'s explicit bump just moved.
pub fn share_session(
    s: &Store,
    session_id: i64,
    person: &str,
    level: &str,
    granter: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let granter = require_granter(granter, "share")?;
    let to = person_named(s, person)?;
    s.grant_session(session_id, GrantRecipient::Person(to), level, granter)?;
    row_after(s, session_id)
}

/// Revoke the live grant of `session_id` to `person`, as `granter`.
pub fn unshare_session(
    s: &Store,
    session_id: i64,
    person: &str,
    granter: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let granter = require_granter(granter, "revoke a grant on")?;
    let to = person_named(s, person)?;
    s.revoke_session_grant(session_id, to, granter)?;
    row_after(s, session_id)
}

/// Lower the live grant of `session_id` to `person` from `drive` to `watch`.
///
/// There is deliberately no twin that raises one, here or in the store: a
/// grant only ever moves downward (spec §4.3 invariant 3), and widening is
/// the owner revoking and sharing again — two of their own operations.
pub fn narrow_session_share(
    s: &Store,
    session_id: i64,
    person: &str,
    granter: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let granter = require_granter(granter, "narrow a grant on")?;
    let to = person_named(s, person)?;
    s.narrow_session_grant(session_id, to, granter)?;
    row_after(s, session_id)
}

/// Who holds a grant on `session_id` — the Share sheet's list, live grants
/// only.
///
/// A revoked row stays in `session_grants` for the audit trail and is nobody's
/// access, so it is in no answer this module gives.
pub fn session_access(s: &Store, session_id: i64) -> Result<Vec<SessionGrantView>, IpcError> {
    s.grants_for_session(session_id)?
        .into_iter()
        .map(|g| SessionGrantView::of(g, s))
        .collect()
}

/// `person`'s own grant set, for the client that derives access from it.
pub fn my_grants(s: &Store, person: Option<i64>) -> Result<MyGrants, IpcError> {
    // `None` answers an empty list, which is the honest answer for a caller
    // that is nobody — and the opposite of what a `match` with a widening
    // fall-through would have done.
    let Some(person) = person else {
        return Ok(MyGrants::default());
    };
    let details = s.grant_details_for_person(person)?;
    let mut grants = Vec::new();
    for (session_id, level) in s.grants_for_person(person)? {
        let d = details.get(&session_id).filter(|d| d.level == level);
        let by = match d {
            Some(d) => s.get_person(d.granted_by)?,
            None => None,
        };
        let via_org = match d.and_then(|d| d.org_id) {
            Some(o) => s.get_org(o)?.map(|o| o.name),
            None => None,
        };
        grants.push(MyGrant {
            session_id,
            level,
            shared_by: d.map(|d| d.granted_by),
            shared_by_name: by.map(|p| p.display_name.unwrap_or(p.name)),
            granted_at: d.map(|d| d.granted_at),
            via_org,
        });
    }
    let requests = s
        .open_access_requests_by(person)?
        .into_iter()
        .map(|r| MyAccessRequest {
            id: r.id,
            session_id: r.session_id,
            level: r.level,
            requested_at: r.requested_at,
        })
        .collect();
    Ok(MyGrants {
        person_id: Some(person),
        grants,
        requests,
    })
}

/// A caller that proves no person cannot be a granter. The refusal names the
/// operation so the message is actionable, and it is `E_FORBIDDEN` rather
/// than `E_VALIDATE` because nothing about the ARGUMENTS is wrong.
fn require_granter(granter: Option<i64>, what: &str) -> Result<i64, IpcError> {
    granter.ok_or_else(|| {
        IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "only the owner of a session may {what} it, and this caller proves no \
                 person: a per-host token speaks for a machine, and a paired device \
                 needs fleet-hub client bind-person"
            ),
        )
    })
}

/// The session row as it stands after a grant write, for the caller's
/// optimistic patch.
///
/// `E_NOTFOUND` rather than an `Option`: the write above succeeded against
/// this row, so a row that is gone by now is a session reaped mid-call, and
/// the caller has nothing to patch.
fn row_after(s: &Store, session_id: i64) -> Result<SessionRow, IpcError> {
    s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {session_id} was removed while its sharing was being changed"),
        )
    })
}
