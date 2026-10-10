//! Sprints and releases on the MCP surface (design 2026-09-28 §7, §8).
//!
//! * Reads join `work`: `buckets` (the ones in scope, with roll-ups) and
//!   `bucket` (one, with its members — the ones the caller may see).
//! * Membership joins `work_link`: `bucket_add`, `bucket_remove`. A per-host
//!   token is refused: a session does not decide the team's plan.
//! * Creating, changing, closing, deleting and adopting are `work_admin`
//!   (master only), as rules and saved views are: they reshape what every
//!   client sees. A person does the same through `work_link { action:
//!   bucket_admin, bucket_op }` ([`person_admin`], owner decision
//!   2026-10-10): their own personal buckets always, an organisation's team
//!   ones as its admin — or as its member when the org turned
//!   `work.members_plan_sprints` on. Adopting stays `work_admin`'s.
//!
//! The org fence is the bucket's own `org_id` under `OrgScope::sees_org`
//! (an unassigned bucket follows the same rule as unassigned work, D31),
//! and an item's the one `status::set_status` uses. A personal bucket
//! (`owner_person_id`) is its person's alone (`may_own_person_row`). A
//! bucket or item outside the scope answers exactly as an unknown id.

use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::trackers::admin::WorkAdminArgs;
use crate::service::view_scope::ViewScope;
use crate::store::{BucketMemberRow, BucketPatch, BucketRow, NewBucket, Store, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// `work { action: bucket }`: one bucket and its members, past ones too
/// (a closed sprint's carry-over is the point of keeping them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketDetail {
    pub bucket: BucketRow,
    #[serde(default)]
    pub members: Vec<BucketMemberRow>,
}

/// What a bucket admin action answers, with a warning that does not refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketAnswer {
    pub bucket: BucketRow,
    /// Another sprint of the same org is active too. Overlapping sprints
    /// are a real practice, so this warns and does not refuse (§5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

fn not_found(id: i64) -> IpcError {
    orgs::not_found("bucket", id)
}

/// May `scope` read `b`? A team bucket by its org; a personal one by its
/// person alone.
pub fn sees_bucket(scope: &ViewScope, b: &BucketRow) -> bool {
    match b.owner_person_id {
        Some(_) => scope.may_own_person_row(b.org_id, b.owner_person_id),
        None => scope.org.sees_org(b.org_id),
    }
}

/// The bucket, if the scope may see it.
fn visible_bucket(s: &Store, scope: &ViewScope, id: i64) -> Result<BucketRow, IpcError> {
    match s.get_bucket(id)? {
        Some(b) if sees_bucket(scope, &b) => Ok(b),
        _ => Err(not_found(id)),
    }
}

/// May `scope` create or change a bucket of `org` owned by `owner`? A
/// personal one: its person. A team one of an org: the hub itself, the
/// org's admin, or its member when the org lets members plan. A team one
/// of no org: the hub itself, or the one person of a one-person hub.
pub fn may_plan(
    s: &Store,
    scope: &ViewScope,
    org: Option<i64>,
    owner: Option<i64>,
) -> Result<bool, IpcError> {
    if owner.is_some() {
        return Ok(scope.may_own_person_row(org, owner));
    }
    if !scope.org.sees_org(org) {
        return Ok(false);
    }
    if scope.is_internal() {
        return Ok(true);
    }
    let (Some(o), Some(p)) = (org, scope.person) else {
        return Ok(org.is_none() && scope.is_sole_person());
    };
    Ok(match s.org_role(o, p)?.as_deref() {
        Some(crate::store::ROLE_ADMIN) => true,
        Some(crate::store::ROLE_MEMBER) => crate::service::settings::get_bool_for(
            s,
            crate::service::settings::WORK_MEMBERS_PLAN_SPRINTS,
            Some(o),
        ),
        _ => false,
    })
}

/// Whether the scope may see an item: a local one through its links, a
/// tracker one through its tracker's org — `status::set_status`'s fence.
fn item_visible(s: &Store, scope: &OrgScope, item: &WorkItemRow) -> Result<bool, IpcError> {
    if item.source == "local" {
        super::local::local_item_visible(s, scope, item.id)
    } else {
        Ok(scope.sees_org(s.item_org(item.id)?))
    }
}

/// `work { action: buckets, kind? }`.
pub fn buckets(
    store: &Mutex<Store>,
    scope: &ViewScope,
    kind: Option<&str>,
) -> Result<Vec<BucketRow>, IpcError> {
    let s = lock(store)?;
    Ok(s.list_buckets(kind)?
        .into_iter()
        .filter(|b| sees_bucket(scope, b))
        .collect())
}

/// `work { action: bucket, bucket_id }`.
pub fn bucket(store: &Mutex<Store>, scope: &ViewScope, id: i64) -> Result<BucketDetail, IpcError> {
    let s = lock(store)?;
    let bucket = visible_bucket(&s, scope, id)?;
    let mut members = Vec::new();
    for m in s.bucket_members(id, true)? {
        if item_visible(&s, &scope.org, &m.item)? {
            members.push(m);
        }
    }
    Ok(BucketDetail { bucket, members })
}

fn link_ids(args: &WorkLinkArgs) -> Result<(i64, i64), IpcError> {
    let bucket = args.bucket_id.ok_or_else(|| {
        IpcError::new(codes::E_INVALID, format!("{} needs bucket_id", args.action))
    })?;
    let item = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{} needs item_id", args.action)))?;
    Ok((bucket, item))
}

/// The bucket and the item, both visible, or the unknown-id answer.
fn fenced(s: &Store, scope: &ViewScope, bucket: i64, item: i64) -> Result<(), IpcError> {
    visible_bucket(s, scope, bucket)?;
    match s.get_work_item(item)? {
        Some(i) if item_visible(s, &scope.org, &i)? => Ok(()),
        _ => Err(orgs::not_found("work item", item)),
    }
}

/// `work_link { action: bucket_add, bucket_id, item_id }`.
pub fn bucket_add(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<BucketRow, IpcError> {
    let (bucket, item) = link_ids(args)?;
    let s = lock(store)?;
    fenced(&s, scope, bucket, item)?;
    s.add_bucket_item(bucket, item)
}

/// `work_link { action: bucket_remove, bucket_id, item_id }`.
pub fn bucket_remove(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<BucketRow, IpcError> {
    let (bucket, item) = link_ids(args)?;
    let s = lock(store)?;
    fenced(&s, scope, bucket, item)?;
    s.remove_bucket_item(bucket, item)?;
    s.get_bucket(bucket)?.ok_or_else(|| not_found(bucket))
}

/// The `work_admin` bucket actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketAction {
    Create,
    Update,
    Close,
    Delete,
    Adopt,
    Unadopt,
}

impl BucketAction {
    pub const NAMES: &'static [&'static str] = &[
        "bucket_create",
        "bucket_update",
        "bucket_close",
        "bucket_delete",
        "bucket_adopt",
        "bucket_unadopt",
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "bucket_create" => BucketAction::Create,
            "bucket_update" => BucketAction::Update,
            "bucket_close" => BucketAction::Close,
            "bucket_delete" => BucketAction::Delete,
            "bucket_adopt" => BucketAction::Adopt,
            "bucket_unadopt" => BucketAction::Unadopt,
            _ => return None,
        })
    }
}

fn bucket_id(args: &WorkAdminArgs) -> Result<i64, IpcError> {
    args.bucket_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{} needs bucket_id", args.action)))
}

/// `0` clears an optional date, as `org_id: 0` means none elsewhere.
fn date(v: Option<i64>) -> Option<Option<i64>> {
    v.map(|d| (d != 0).then_some(d))
}

/// An empty string clears an optional text.
fn text(v: &Option<String>) -> Option<Option<String>> {
    v.as_ref()
        .map(|t| (!t.trim().is_empty()).then(|| t.clone()))
}

/// One `work_admin` bucket action, under the store lock.
pub fn admin(
    action: BucketAction,
    args: &WorkAdminArgs,
    s: &Store,
) -> Result<serde_json::Value, IpcError> {
    admin_as(action, args, s, None)
}

/// [`admin`], creating a personal bucket of `owner` when one is named.
fn admin_as(
    action: BucketAction,
    args: &WorkAdminArgs,
    s: &Store,
    owner: Option<i64>,
) -> Result<serde_json::Value, IpcError> {
    let answer = |bucket: BucketRow| -> Result<serde_json::Value, IpcError> {
        let warning = if bucket.kind == "sprint" && bucket.state == "active" {
            let others = s.other_active_sprints(bucket.id)?;
            (!others.is_empty()).then(|| {
                format!(
                    "{} is active too; overlapping sprints are allowed",
                    others
                        .iter()
                        .map(|o| o.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        } else {
            None
        };
        json(&BucketAnswer { bucket, warning })
    };
    match action {
        BucketAction::Create => {
            let kind = args.kind.as_deref().ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    "bucket_create needs kind: sprint | release",
                )
            })?;
            let name = args
                .name
                .as_deref()
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "bucket_create needs name"))?;
            let row = s.create_bucket(&NewBucket {
                kind,
                name,
                org_id: args.org_id.filter(|o| *o != 0),
                owner_person_id: owner,
                starts_at: args.starts_at.filter(|d| *d != 0),
                ends_at: args.ends_at.filter(|d| *d != 0),
                goal: args.goal.as_deref(),
            })?;
            answer(row)
        }
        BucketAction::Update => {
            let row = s.update_bucket(
                bucket_id(args)?,
                args.expected_version,
                &BucketPatch {
                    name: args.name.clone(),
                    starts_at: date(args.starts_at),
                    ends_at: date(args.ends_at),
                    goal: text(&args.goal),
                    shipped_ref: text(&args.shipped_ref),
                    state: args.state.clone(),
                },
            )?;
            answer(row)
        }
        BucketAction::Close => json(&s.close_sprint(
            bucket_id(args)?,
            args.expected_version,
            args.carry_to.filter(|c| *c != 0),
            args.carry.as_deref(),
        )?),
        BucketAction::Delete => {
            let id = bucket_id(args)?;
            if !s.delete_bucket(id)? {
                return Err(not_found(id));
            }
            json(&serde_json::json!({ "removed": id }))
        }
        BucketAction::Adopt | BucketAction::Unadopt => {
            let id = bucket_id(args)?;
            let tracker = args.tracker_id.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!("{} needs tracker_id", args.action),
                )
            })?;
            let external = args.external_id.as_deref().ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "{} needs external_id: the tracker's sprint or version name",
                        args.action
                    ),
                )
            })?;
            if action == BucketAction::Unadopt {
                return answer(s.remove_bucket_ref(id, tracker, external)?);
            }
            let b = s.get_bucket(id)?.ok_or_else(|| not_found(id))?;
            let t = s.require_tracker(tracker)?;
            let caps = crate::service::trackers::provider_caps(&t);
            // The degradation rule (§4): without the capability a native
            // bucket works the same, it only has nothing to adopt.
            let able = if b.kind == "sprint" {
                caps.iterations
            } else {
                caps.versions
            };
            if !able {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "{} has no {}s to adopt; {} works as fleet's own without one",
                        t.name,
                        if b.kind == "sprint" {
                            "sprint"
                        } else {
                            "version"
                        },
                        b.name
                    ),
                ));
            }
            answer(s.add_bucket_ref(id, tracker, external, None)?)
        }
    }
}

/// `work_link { action: bucket_admin, bucket_op }`: a person's
/// `bucket_create | bucket_update | bucket_close | bucket_delete`, fenced by
/// [`may_plan`]. Creating with no `org_id` makes a personal bucket of the
/// caller's own; a person-less caller (the hub itself aside) is refused.
pub fn person_admin(
    op: &WorkAdminArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<serde_json::Value, IpcError> {
    let action = BucketAction::parse(&op.action).ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "bucket_op.action is one of bucket_create, bucket_update, bucket_close, \
                 bucket_delete; {:?} is not",
                op.action
            ),
        )
    })?;
    if matches!(action, BucketAction::Adopt | BucketAction::Unadopt) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "linking a sprint or release to a tracker is work_admin's",
        ));
    }
    let s = lock(store)?;
    let refuse = |what: &str| {
        IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{what}: an organisation's sprints and releases are planned by its admins \
                 (and its members when the organisation allows it); a personal one by \
                 its person"
            ),
        )
    };
    if action == BucketAction::Create {
        let org = op.org_id.filter(|o| *o != 0);
        let owner = match (org, scope.person) {
            (None, Some(p)) if !scope.is_internal() => Some(p),
            (None, None) if !scope.is_internal() => {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "a personal sprint or release needs a person; this caller names none",
                ));
            }
            _ => None,
        };
        if !may_plan(&s, scope, org, owner)? {
            return Err(refuse("not yours to create"));
        }
        return admin_as(action, op, &s, owner);
    }
    let id = bucket_id(op)?;
    let b = visible_bucket(&s, scope, id)?;
    if !may_plan(&s, scope, b.org_id, b.owner_person_id)? {
        return Err(refuse(&format!("{} is not yours to change", b.name)));
    }
    if let Some(to) = op.carry_to.filter(|c| *c != 0) {
        visible_bucket(&s, scope, to)?;
    }
    admin_as(action, op, &s, None)
}

fn json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
mod tests;
