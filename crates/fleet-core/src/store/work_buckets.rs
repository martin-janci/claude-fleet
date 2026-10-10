//! Sprints and releases (design 2026-09-28 §1, §5): `work_buckets`, their
//! members and their links to a tracker's sprint or version.
//!
//! * **Fleet owns the bucket** (E1). A `work_bucket_refs` row lets a tracker
//!   *adopt* synced items into it ([`Store::adopt_tracker_item`]); the
//!   tracker never renames, dates or closes it.
//! * **A person outranks a machine.** Adoption writes `source = 'adopted'`
//!   and withdraws only its own rows when the tracker stops reporting the
//!   sprint or version; a `manual` membership is never touched by a sync,
//!   and a membership a person ended is not re-adopted.
//! * **One current sprint per item**, several current releases (Jira's
//!   `fixVersions` is a list). Enforced here rather than by a trigger, so
//!   the refusal names the sprint that holds the item.
//! * **History is kept.** Removing an item, or closing its sprint, stamps
//!   `removed_at`; the row stays, so "this did not finish in sprint 23" is
//!   still a fact afterwards.
//!
//! Visibility is the caller's: these methods know no scope, and
//! `service::work::buckets` fences every read and write by the bucket's and
//! the item's org.

use super::work::{map_item, ITEM_COLUMNS};
use super::{now_unix, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// `sprint` (when) or `release` (which version) — E4's two typed axes.
pub const BUCKET_KINDS: [&str; 2] = ["sprint", "release"];
/// A bucket's name, in characters.
pub const BUCKET_NAME_MAX_CHARS: usize = 120;
/// A sprint's goal, in characters.
pub const BUCKET_GOAL_MAX_CHARS: usize = 2000;
/// A release's `shipped_ref` (a tag or a URL), in characters.
pub const BUCKET_REF_MAX_CHARS: usize = 512;

/// One sprint or release, with its members' roll-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketRow {
    pub id: i64,
    /// sprint | release
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// sprint: planned | active | closed; release: planned | released.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<i64>,
    /// A sprint's end; a release's target date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipped_at: Option<i64>,
    /// Typed in by a person; read by nothing automatically (E3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipped_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// The `expected_version` guard.
    pub version: i64,
    /// Current members.
    #[serde(default)]
    pub total: i64,
    /// Current members whose stored status is `done` (a person's, a
    /// tracker's or the merged-PR stamp; the live `in_progress` lift never
    /// makes anything done, so the stored column is the whole answer).
    #[serde(default)]
    pub done: i64,
    /// The tracker sprints or versions this bucket adopts from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<BucketRefRow>,
}

/// A link from a bucket to a tracker's sprint or version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketRefRow {
    pub tracker_id: i64,
    /// What the tracker's snapshot reports for it (its name, today).
    pub external_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
}

/// One member of a bucket, current or past.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketMemberRow {
    pub item: WorkItemRow,
    /// manual | adopted
    pub source: String,
    pub added_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<i64>,
}

/// One current membership of an item, for the item's own read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemBucketRow {
    pub bucket_id: i64,
    pub kind: String,
    pub name: String,
    pub state: String,
    pub source: String,
}

/// What `create_bucket` writes.
#[derive(Debug, Clone, Default)]
pub struct NewBucket<'a> {
    pub kind: &'a str,
    pub name: &'a str,
    pub org_id: Option<i64>,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
    pub goal: Option<&'a str>,
}

/// One current membership, as [`Store::bucket_membership`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketMembership {
    pub item_id: i64,
    pub bucket_id: i64,
    /// The bucket's name.
    pub name: String,
    /// The bucket's organisation.
    pub org_id: Option<i64>,
}

/// What `update_bucket` changes. `None` leaves a field; for the optional
/// columns `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct BucketPatch {
    pub name: Option<String>,
    pub starts_at: Option<Option<i64>>,
    pub ends_at: Option<Option<i64>>,
    pub goal: Option<Option<String>>,
    pub shipped_ref: Option<Option<String>>,
    /// sprint: `active`; release: `released`. `closed` is
    /// [`Store::close_sprint`]'s, because it decides the carry-over.
    pub state: Option<String>,
}

/// What closing a sprint did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SprintClosed {
    pub bucket: BucketRow,
    /// Members whose membership the close ended.
    pub ended: Vec<i64>,
    /// Unfinished members added to `carry_to`.
    #[serde(default)]
    pub carried: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_to: Option<i64>,
}

const BUCKET_COLUMNS: &str = "b.id, b.kind, b.name, b.org_id, b.state, b.starts_at, b.ends_at, \
     b.shipped_at, b.shipped_ref, b.goal, b.created_at, b.updated_at, b.version, \
     (SELECT COUNT(*) FROM work_bucket_items m WHERE m.bucket_id = b.id AND m.removed_at IS NULL), \
     (SELECT COUNT(*) FROM work_bucket_items m JOIN work_items i ON i.id = m.item_id \
       WHERE m.bucket_id = b.id AND m.removed_at IS NULL AND i.status_category = 'done')";

fn map_bucket(r: &rusqlite::Row<'_>) -> rusqlite::Result<BucketRow> {
    Ok(BucketRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        org_id: r.get(3)?,
        state: r.get(4)?,
        starts_at: r.get(5)?,
        ends_at: r.get(6)?,
        shipped_at: r.get(7)?,
        shipped_ref: r.get(8)?,
        goal: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        version: r.get(12)?,
        total: r.get(13)?,
        done: r.get(14)?,
        refs: Vec::new(),
    })
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

/// The states a kind may be in, in lifecycle order.
pub fn bucket_states(kind: &str) -> &'static [&'static str] {
    match kind {
        "sprint" => &["planned", "active", "closed"],
        _ => &["planned", "released"],
    }
}

fn check_kind(kind: &str) -> Result<(), IpcError> {
    if BUCKET_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(invalid(format!(
            "a bucket is a {}; {kind:?} is neither",
            BUCKET_KINDS.join(" or ")
        )))
    }
}

fn check_name(name: &str) -> Result<String, IpcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("a sprint or release needs a name"));
    }
    if name.chars().count() > BUCKET_NAME_MAX_CHARS {
        return Err(invalid(format!(
            "a sprint or release name is at most {BUCKET_NAME_MAX_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a sprint or release name is one line of text"));
    }
    Ok(name.to_string())
}

fn check_text(what: &str, v: Option<&str>, max: usize) -> Result<Option<String>, IpcError> {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if v.chars().count() > max => {
            Err(invalid(format!("{what} is at most {max} characters")))
        }
        v => Ok(v.map(str::to_string)),
    }
}

fn check_dates(starts_at: Option<i64>, ends_at: Option<i64>) -> Result<(), IpcError> {
    match (starts_at, ends_at) {
        (Some(s), Some(e)) if e < s => Err(invalid("a sprint cannot end before it starts")),
        _ => Ok(()),
    }
}

/// A UNIQUE violation on `ux_work_buckets_name`, as a sentence.
fn name_taken(e: rusqlite::Error, kind: &str, name: &str) -> IpcError {
    match e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            IpcError::new(
                codes::E_EXISTS,
                format!("this organisation already has a {kind} named {name:?}"),
            )
        }
        e => e.into(),
    }
}

impl Store {
    /// Create a sprint or release, `planned`.
    pub fn create_bucket(&self, b: &NewBucket<'_>) -> Result<BucketRow, IpcError> {
        check_kind(b.kind)?;
        let name = check_name(b.name)?;
        let goal = check_text("a goal", b.goal, BUCKET_GOAL_MAX_CHARS)?;
        check_dates(b.starts_at, b.ends_at)?;
        if b.kind == "release" && b.starts_at.is_some() {
            return Err(invalid(
                "a release has a target date (ends_at), not a start",
            ));
        }
        if let Some(org) = b.org_id {
            self.require_org(org)?;
        }
        let now = now_unix();
        self.conn
            .execute(
                "INSERT INTO work_buckets (kind, name, org_id, state, starts_at, ends_at, goal, \
                   created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 'planned', ?4, ?5, ?6, ?7, ?7)",
                rusqlite::params![b.kind, name, b.org_id, b.starts_at, b.ends_at, goal, now],
            )
            .map_err(|e| name_taken(e, b.kind, &name))?;
        self.require_bucket(self.conn.last_insert_rowid())
    }

    fn require_org(&self, org: i64) -> Result<(), IpcError> {
        let known: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM orgs WHERE id = ?1)",
            [org],
            |r| r.get(0),
        )?;
        if known {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("organisation {org} not found"),
            ))
        }
    }

    /// One bucket with its roll-up and refs.
    pub fn get_bucket(&self, id: i64) -> Result<Option<BucketRow>, IpcError> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {BUCKET_COLUMNS} FROM work_buckets b WHERE b.id = ?1"),
                [id],
                map_bucket,
            )
            .optional()?;
        let Some(mut row) = row else {
            return Ok(None);
        };
        row.refs = self.bucket_refs(id)?;
        Ok(Some(row))
    }

    fn require_bucket(&self, id: i64) -> Result<BucketRow, IpcError> {
        self.get_bucket(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("bucket {id} not found")))
    }

    fn bucket_refs(&self, id: i64) -> Result<Vec<BucketRefRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT tracker_id, external_id, external_name, last_seen_at FROM work_bucket_refs \
             WHERE bucket_id = ?1 ORDER BY tracker_id, external_id",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok(BucketRefRow {
                tracker_id: r.get(0)?,
                external_id: r.get(1)?,
                external_name: r.get(2)?,
                last_seen_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every bucket, optionally of one kind, newest plan first: open ones
    /// (not closed, not released) before finished ones, then by date. The
    /// caller fences by org.
    pub fn list_buckets(&self, kind: Option<&str>) -> Result<Vec<BucketRow>, IpcError> {
        if let Some(k) = kind {
            check_kind(k)?;
        }
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {BUCKET_COLUMNS} FROM work_buckets b \
             WHERE ?1 IS NULL OR b.kind = ?1 \
             ORDER BY b.state IN ('closed', 'released') ASC, \
                      COALESCE(b.starts_at, b.ends_at, b.created_at) DESC, b.id DESC"
        ))?;
        let rows = stmt.query_map([kind], map_bucket)?;
        let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for b in &mut out {
            b.refs = self.bucket_refs(b.id)?;
        }
        Ok(out)
    }

    /// Change a bucket's fields or move it forward in its lifecycle. With
    /// `expected_version`, a change made meanwhile answers `E_CONFLICT` and
    /// writes nothing.
    pub fn update_bucket(
        &self,
        id: i64,
        expected_version: Option<i64>,
        p: &BucketPatch,
    ) -> Result<BucketRow, IpcError> {
        let before = self.require_bucket(id)?;
        check_version(&before, expected_version)?;
        let name = p.name.as_deref().map(check_name).transpose()?;
        let starts_at = p.starts_at.unwrap_or(before.starts_at);
        let ends_at = p.ends_at.unwrap_or(before.ends_at);
        check_dates(starts_at, ends_at)?;
        if before.kind == "release" && starts_at.is_some() {
            return Err(invalid(
                "a release has a target date (ends_at), not a start",
            ));
        }
        let goal = match &p.goal {
            Some(g) => check_text("a goal", g.as_deref(), BUCKET_GOAL_MAX_CHARS)?,
            None => before.goal.clone(),
        };
        let shipped_ref = match &p.shipped_ref {
            Some(_) if before.kind != "release" => {
                return Err(invalid("only a release has a shipped_ref"));
            }
            Some(r) => check_text("a shipped_ref", r.as_deref(), BUCKET_REF_MAX_CHARS)?,
            None => before.shipped_ref.clone(),
        };
        let now = now_unix();
        let (state, shipped_at) = match p.state.as_deref() {
            None => (before.state.clone(), before.shipped_at),
            Some(s) if s == before.state => (before.state.clone(), before.shipped_at),
            Some(s) => {
                if !bucket_states(&before.kind).contains(&s) {
                    return Err(invalid(format!(
                        "a {}'s state is one of {}",
                        before.kind,
                        bucket_states(&before.kind).join(", ")
                    )));
                }
                match (before.kind.as_str(), before.state.as_str(), s) {
                    ("sprint", "planned", "active") => (s.to_string(), None),
                    ("sprint", _, "closed") => {
                        return Err(invalid(
                            "close a sprint with bucket_close: it decides what happens \
                             to its unfinished work",
                        ));
                    }
                    ("release", "planned", "released") => (s.to_string(), Some(now)),
                    (kind, from, to) => {
                        return Err(invalid(format!(
                            "a {kind} does not go from {from} back to {to}"
                        )));
                    }
                }
            }
        };
        let name = name.unwrap_or_else(|| before.name.clone());
        self.conn
            .execute(
                "UPDATE work_buckets SET name = ?1, starts_at = ?2, ends_at = ?3, goal = ?4, \
                   shipped_ref = ?5, state = ?6, shipped_at = ?7, updated_at = ?8, \
                   version = version + 1 \
                 WHERE id = ?9",
                rusqlite::params![
                    name,
                    starts_at,
                    ends_at,
                    goal,
                    shipped_ref,
                    state,
                    shipped_at,
                    now,
                    id
                ],
            )
            .map_err(|e| name_taken(e, &before.kind, &name))?;
        self.require_bucket(id)
    }

    /// Other sprints of the same org that are `active` — overlapping
    /// sprints are a real practice, so the caller warns rather than refuses
    /// (§5).
    pub fn other_active_sprints(&self, id: i64) -> Result<Vec<BucketRow>, IpcError> {
        let b = self.require_bucket(id)?;
        Ok(self
            .list_buckets(Some("sprint"))?
            .into_iter()
            .filter(|o| o.id != id && o.state == "active" && o.org_id == b.org_id)
            .collect())
    }

    /// Close a sprint. Every current membership ends (`removed_at`), so the
    /// carry-over stays visible afterwards. The unfinished members in
    /// `carry` — every unfinished one when `None`, the dialog's preselection
    /// (E9) — are added to the sprint `carry_to` as a person's membership;
    /// with no `carry_to` they are simply left with no sprint.
    pub fn close_sprint(
        &self,
        id: i64,
        expected_version: Option<i64>,
        carry_to: Option<i64>,
        carry: Option<&[i64]>,
    ) -> Result<SprintClosed, IpcError> {
        let before = self.require_bucket(id)?;
        check_version(&before, expected_version)?;
        if before.kind != "sprint" {
            return Err(invalid("only a sprint closes; a release is released"));
        }
        if before.state == "closed" {
            return Err(invalid(format!("{} is already closed", before.name)));
        }
        if let Some(to) = carry_to {
            let target = self.require_bucket(to)?;
            if to == id || target.kind != "sprint" || target.state == "closed" {
                return Err(invalid(
                    "carry unfinished work to another sprint that is not closed",
                ));
            }
            if target.org_id != before.org_id {
                return Err(invalid(
                    "carry unfinished work to a sprint of the same organisation",
                ));
            }
        } else if carry.is_some_and(|c| !c.is_empty()) {
            return Err(invalid("carry names items but no carry_to sprint"));
        }
        let members: Vec<(i64, bool)> = {
            let mut stmt = self.conn.prepare(
                "SELECT m.item_id, i.status_category = 'done' FROM work_bucket_items m \
                 JOIN work_items i ON i.id = m.item_id \
                 WHERE m.bucket_id = ?1 AND m.removed_at IS NULL ORDER BY m.item_id",
            )?;
            let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let unfinished: BTreeSet<i64> = members
            .iter()
            .filter(|(_, done)| !done)
            .map(|(i, _)| *i)
            .collect();
        let chosen: BTreeSet<i64> = match (carry_to, carry) {
            (None, _) => BTreeSet::new(),
            (Some(_), None) => unfinished.clone(),
            (Some(_), Some(list)) => {
                let list: BTreeSet<i64> = list.iter().copied().collect();
                if let Some(stray) = list.difference(&unfinished).next() {
                    return Err(invalid(format!(
                        "item {stray} is not unfinished work of this sprint"
                    )));
                }
                list
            }
        };
        let now = now_unix();
        self.in_savepoint("close_sprint", |conn| -> Result<(), IpcError> {
            conn.execute(
                "UPDATE work_bucket_items SET removed_at = ?1 \
                 WHERE bucket_id = ?2 AND removed_at IS NULL",
                rusqlite::params![now, id],
            )?;
            conn.execute(
                "UPDATE work_buckets SET state = 'closed', updated_at = ?1, \
                   version = version + 1 WHERE id = ?2",
                rusqlite::params![now, id],
            )?;
            if let Some(to) = carry_to {
                for item in &chosen {
                    self.put_member(to, *item, "manual", now)?;
                }
            }
            Ok(())
        })?;
        Ok(SprintClosed {
            bucket: self.require_bucket(id)?,
            ended: members.into_iter().map(|(i, _)| i).collect(),
            carried: chosen.into_iter().collect(),
            carry_to,
        })
    }

    /// Delete a bucket with its memberships and refs. `false` when unknown.
    pub fn delete_bucket(&self, id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .execute("DELETE FROM work_buckets WHERE id = ?1", [id])?
            == 1)
    }

    /// A person puts an item in a bucket. Idempotent for a current
    /// membership (an adopted one becomes the person's); a past one is
    /// reopened.
    ///
    /// Refused: a closed sprint; a second current sprint (the refusal names
    /// the one that holds the item); an item of another organisation than
    /// the bucket's — unassigned on either side is never a conflict, as for
    /// links.
    pub fn add_bucket_item(&self, bucket_id: i64, item_id: i64) -> Result<BucketRow, IpcError> {
        let b = self.require_bucket(bucket_id)?;
        let item = self.get_work_item(item_id)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("work item {item_id} not found"))
        })?;
        if b.kind == "sprint" && b.state == "closed" {
            return Err(invalid(format!(
                "{} is closed; plan the work into an open sprint",
                b.name
            )));
        }
        let item_org = self.item_org(item_id)?;
        if let (Some(w), Some(o)) = (item_org, b.org_id) {
            if w != o {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    format!(
                        "{} belongs to organisation {w} and {} to organisation {o}; \
                         fleet does not plan work across organisations",
                        item_label(&item),
                        b.name
                    ),
                )
                .with_details(serde_json::json!({
                    "work_org_id": w, "bucket_org_id": o, "cross_org": true
                })));
            }
        }
        if b.kind == "sprint" {
            if let Some(other) = self.current_sprint_of(item_id)? {
                if other.bucket_id != bucket_id {
                    return Err(IpcError::new(
                        codes::E_CONFLICT,
                        format!(
                            "{} is already in sprint {:?}; remove it from that sprint first \
                             — an item is in one sprint at a time",
                            item_label(&item),
                            other.name
                        ),
                    )
                    .with_details(serde_json::json!({ "sprint_id": other.bucket_id })));
                }
            }
        }
        self.put_member(bucket_id, item_id, "manual", now_unix())?;
        self.require_bucket(bucket_id)
    }

    /// Insert or reopen a membership as `source`. A current row becomes a
    /// person's when a person puts it there, and stays as it is otherwise.
    fn put_member(
        &self,
        bucket_id: i64,
        item_id: i64,
        source: &str,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO work_bucket_items (bucket_id, item_id, source, added_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(bucket_id, item_id) DO UPDATE SET \
               source = CASE WHEN removed_at IS NULL AND ?3 = 'adopted' THEN source ELSE ?3 END, \
               added_at = CASE WHEN removed_at IS NULL THEN added_at ELSE ?4 END, \
               removed_at = NULL",
            rusqlite::params![bucket_id, item_id, source, now],
        )?;
        Ok(())
    }

    /// End an item's membership. `false` when it had none. The row becomes
    /// `manual` whoever added it: ending it was a person's decision, which
    /// adoption must not undo on the tracker's next sync.
    pub fn remove_bucket_item(&self, bucket_id: i64, item_id: i64) -> Result<bool, IpcError> {
        self.require_bucket(bucket_id)?;
        Ok(self.conn.execute(
            "UPDATE work_bucket_items SET removed_at = ?1, source = 'manual' \
             WHERE bucket_id = ?2 AND item_id = ?3 AND removed_at IS NULL",
            rusqlite::params![now_unix(), bucket_id, item_id],
        )? == 1)
    }

    /// A bucket's members, current first; past ones too with `with_past`.
    pub fn bucket_members(
        &self,
        bucket_id: i64,
        with_past: bool,
    ) -> Result<Vec<BucketMemberRow>, IpcError> {
        let cols = ITEM_COLUMNS
            .split(',')
            .map(|c| format!("i.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols}, m.source, m.added_at, m.removed_at FROM work_bucket_items m \
             JOIN work_items i ON i.id = m.item_id \
             WHERE m.bucket_id = ?1 AND (?2 OR m.removed_at IS NULL) \
             ORDER BY m.removed_at IS NOT NULL, m.added_at, i.id"
        ))?;
        let n = super::work::ITEM_COLUMN_COUNT;
        let rows = stmt.query_map(rusqlite::params![bucket_id, with_past], |r| {
            Ok(BucketMemberRow {
                item: map_item(r)?,
                source: r.get(n)?,
                added_at: r.get(n + 1)?,
                removed_at: r.get(n + 2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every current membership of one kind: the Work view's group by
    /// sprint or release reads it once per tree. An item in several
    /// releases comes first under the one still planned, then the nearest
    /// target date, then the oldest.
    pub fn bucket_membership(&self, kind: &str) -> Result<Vec<BucketMembership>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT m.item_id, b.id, b.name, b.org_id FROM work_bucket_items m \
             JOIN work_buckets b ON b.id = m.bucket_id \
             WHERE b.kind = ?1 AND m.removed_at IS NULL \
             ORDER BY m.item_id, b.state <> 'planned', b.ends_at IS NULL, b.ends_at, b.id",
        )?;
        let rows = stmt.query_map([kind], |r| {
            Ok(BucketMembership {
                item_id: r.get(0)?,
                bucket_id: r.get(1)?,
                name: r.get(2)?,
                org_id: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// An item's current memberships, sprints first.
    pub fn item_buckets(&self, item_id: i64) -> Result<Vec<ItemBucketRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT b.id, b.kind, b.name, b.state, m.source FROM work_bucket_items m \
             JOIN work_buckets b ON b.id = m.bucket_id \
             WHERE m.item_id = ?1 AND m.removed_at IS NULL \
             ORDER BY b.kind = 'release', b.id",
        )?;
        let rows = stmt.query_map([item_id], |r| {
            Ok(ItemBucketRow {
                bucket_id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                state: r.get(3)?,
                source: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn current_sprint_of(&self, item_id: i64) -> Result<Option<ItemBucketRow>, IpcError> {
        Ok(self
            .item_buckets(item_id)?
            .into_iter()
            .find(|b| b.kind == "sprint"))
    }

    /// Link a bucket to a tracker's sprint (by the name its items report as
    /// their iteration) or version (one of their `versions`), then adopt
    /// every cached item that reports it. The tracker's organisation must
    /// match the bucket's; unassigned on either side is no conflict.
    /// Whether the provider has sprints or versions at all is the caller's
    /// check (`Caps`), the store knows no providers.
    pub fn add_bucket_ref(
        &self,
        bucket_id: i64,
        tracker_id: i64,
        external_id: &str,
        external_name: Option<&str>,
    ) -> Result<BucketRow, IpcError> {
        let b = self.require_bucket(bucket_id)?;
        let external_id = external_id.trim();
        if external_id.is_empty() {
            return Err(invalid("adopt needs the tracker's sprint or version name"));
        }
        if external_id.chars().count() > BUCKET_NAME_MAX_CHARS {
            return Err(invalid(format!(
                "a sprint or version name is at most {BUCKET_NAME_MAX_CHARS} characters"
            )));
        }
        let tracker_org: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT org_id FROM trackers WHERE id = ?1",
                [tracker_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(tracker_org) = tracker_org else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("tracker {tracker_id} not found"),
            ));
        };
        if let (Some(t), Some(o)) = (tracker_org, b.org_id) {
            if t != o {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    format!(
                        "tracker {tracker_id} belongs to organisation {t} and {} to \
                         organisation {o}",
                        b.name
                    ),
                ));
            }
        }
        let external_name = check_text("a name", external_name, BUCKET_NAME_MAX_CHARS)?;
        let now = now_unix();
        self.in_savepoint("bucket_adopt", |conn| -> Result<(), IpcError> {
            conn.execute(
                "INSERT INTO work_bucket_refs (bucket_id, tracker_id, external_id, external_name) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(bucket_id, tracker_id, external_id) DO UPDATE SET \
                   external_name = COALESCE(excluded.external_name, external_name)",
                rusqlite::params![bucket_id, tracker_id, external_id, external_name],
            )?;
            self.readopt_tracker(tracker_id, now)
        })?;
        self.require_bucket(bucket_id)
    }

    /// Unlink a bucket from a tracker's sprint or version, and withdraw the
    /// memberships only that link justified. A person's are kept.
    pub fn remove_bucket_ref(
        &self,
        bucket_id: i64,
        tracker_id: i64,
        external_id: &str,
    ) -> Result<BucketRow, IpcError> {
        self.require_bucket(bucket_id)?;
        let now = now_unix();
        self.in_savepoint("bucket_unadopt", |conn| -> Result<(), IpcError> {
            let gone = conn.execute(
                "DELETE FROM work_bucket_refs \
                 WHERE bucket_id = ?1 AND tracker_id = ?2 AND external_id = ?3",
                rusqlite::params![bucket_id, tracker_id, external_id.trim()],
            )?;
            if gone == 0 {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("bucket {bucket_id} does not adopt {external_id:?} from tracker {tracker_id}"),
                ));
            }
            // Adopted members of this bucket from this tracker, re-judged
            // against the refs that remain. While another ref of this bucket
            // to the tracker remains, adoption withdraws what it no longer
            // justifies and keeps the rest with their `added_at`; with none
            // left the bucket is no longer linked, so withdraw here.
            // A ref adoption ignores (another org's tracker) does not count.
            let still_linked = self
                .tracker_bucket_refs(tracker_id)?
                .iter()
                .any(|r| r.0 == bucket_id);
            if !still_linked {
                conn.execute(
                    "UPDATE work_bucket_items SET removed_at = ?1 \
                     WHERE bucket_id = ?2 AND source = 'adopted' AND removed_at IS NULL \
                       AND item_id IN (SELECT id FROM work_items WHERE tracker_id = ?3)",
                    rusqlite::params![now, bucket_id, tracker_id],
                )?;
            }
            self.readopt_tracker(tracker_id, now)
        })?;
        self.require_bucket(bucket_id)
    }

    /// Run adoption over every cached item of a tracker (a ref added or
    /// removed). One read of the items, then the per-item rule.
    fn readopt_tracker(&self, tracker_id: i64, now: i64) -> Result<(), IpcError> {
        let items: Vec<(i64, Option<String>, Option<String>)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, iteration, meta FROM work_items WHERE tracker_id = ?1")?;
            let rows = stmt.query_map([tracker_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let refs = self.tracker_bucket_refs(tracker_id)?;
        for (id, iteration, meta) in items {
            let versions = super::tracker_items::ItemMeta::parse(meta.as_deref()).versions;
            self.adopt_with(tracker_id, &refs, id, iteration.as_deref(), &versions, now)?;
        }
        Ok(())
    }

    /// `(bucket_id, kind, state, external_id)` of every ref to a tracker
    /// whose bucket may hold its items. `add_bucket_ref` checks the org
    /// match, but a tracker can move to another org afterwards: a ref
    /// across organisations is then dormant, not a way to plan one org's
    /// work in another's sprint.
    fn tracker_bucket_refs(
        &self,
        tracker_id: i64,
    ) -> Result<Vec<(i64, String, String, String)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.bucket_id, b.kind, b.state, r.external_id FROM work_bucket_refs r \
             JOIN work_buckets b ON b.id = r.bucket_id \
             JOIN trackers t ON t.id = r.tracker_id \
             WHERE r.tracker_id = ?1 \
               AND (b.org_id IS NULL OR t.org_id IS NULL OR b.org_id = t.org_id) \
             ORDER BY r.bucket_id",
        )?;
        let rows = stmt.query_map([tracker_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Adoption for one synced item (§5), called by the tracker upsert when
    /// the item changed: buckets linked to the sprint it reports (its
    /// `iteration`) or to a version it reports gain it as `adopted`; adopted
    /// memberships it no longer justifies end. Never touches a `manual`
    /// row, never re-adopts a membership a person ended, never adopts into
    /// a closed sprint, and never gives an item a second current sprint.
    /// Nothing at all when no bucket links to this tracker — the common
    /// case costs one indexed read.
    pub(super) fn adopt_tracker_item(
        &self,
        tracker_id: i64,
        item_id: i64,
        iteration: Option<&str>,
        versions: &[String],
    ) -> Result<(), IpcError> {
        let refs = self.tracker_bucket_refs(tracker_id)?;
        if refs.is_empty() {
            return Ok(());
        }
        self.adopt_with(tracker_id, &refs, item_id, iteration, versions, now_unix())
    }

    fn adopt_with(
        &self,
        tracker_id: i64,
        refs: &[(i64, String, String, String)],
        item_id: i64,
        iteration: Option<&str>,
        versions: &[String],
        now: i64,
    ) -> Result<(), IpcError> {
        let linked: BTreeSet<i64> = refs.iter().map(|r| r.0).collect();
        let seen: Vec<&(i64, String, String, String)> = refs
            .iter()
            .filter(|(_, kind, _, ext)| match kind.as_str() {
                "release" => versions.iter().any(|v| v.trim() == ext),
                _ => iteration.map(str::trim) == Some(ext.as_str()),
            })
            .collect();
        // The tracker still reports these, whether or not this item is new
        // to the bucket.
        for (bucket, _, _, ext) in &seen {
            self.conn.execute(
                "UPDATE work_bucket_refs SET last_seen_at = ?1 \
                 WHERE bucket_id = ?2 AND tracker_id = ?3 AND external_id = ?4 \
                   AND IFNULL(last_seen_at, 0) < ?1",
                rusqlite::params![now, bucket, tracker_id, ext],
            )?;
        }
        let mut wanted: BTreeSet<i64> = seen.iter().map(|r| r.0).collect();
        // One sprint: the lowest-numbered open one when several link to the
        // same name.
        let sprint = refs
            .iter()
            .filter(|(id, kind, state, _)| {
                kind == "sprint" && state != "closed" && wanted.contains(id)
            })
            .map(|r| r.0)
            .min();
        wanted.retain(|id| {
            refs.iter()
                .any(|(b, kind, _, _)| b == id && (kind == "release" || Some(*id) == sprint))
        });
        let current: Vec<(i64, String, Option<i64>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT bucket_id, source, removed_at FROM work_bucket_items WHERE item_id = ?1",
            )?;
            let rows = stmt.query_map([item_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        // Withdraw: an adopted, current membership of a linked bucket the
        // tracker no longer reports.
        for (bucket, source, removed) in &current {
            if removed.is_none()
                && source == "adopted"
                && linked.contains(bucket)
                && !wanted.contains(bucket)
            {
                self.conn.execute(
                    "UPDATE work_bucket_items SET removed_at = ?1 \
                     WHERE bucket_id = ?2 AND item_id = ?3",
                    rusqlite::params![now, bucket, item_id],
                )?;
            }
        }
        for bucket in wanted {
            match current.iter().find(|(b, _, _)| *b == bucket) {
                // Already a member, whoever put it there.
                Some((_, _, None)) => continue,
                // A person ended it: their decision stands.
                Some((_, source, Some(_))) if source == "manual" => continue,
                _ => {}
            }
            let is_sprint = refs
                .iter()
                .any(|(b, kind, _, _)| *b == bucket && kind == "sprint");
            if is_sprint {
                if let Some(other) = self.current_sprint_of(item_id)? {
                    if other.bucket_id != bucket {
                        continue;
                    }
                }
            }
            self.put_member(bucket, item_id, "adopted", now)?;
        }
        Ok(())
    }
}

fn check_version(b: &BucketRow, expected: Option<i64>) -> Result<(), IpcError> {
    match expected {
        Some(v) if v != b.version => Err(IpcError::new(
            codes::E_CONFLICT,
            format!(
                "{} changed meanwhile (version {} now, {v} expected); reload it",
                b.name, b.version
            ),
        )
        .with_details(serde_json::json!({ "version": b.version }))),
        _ => Ok(()),
    }
}

fn item_label(item: &WorkItemRow) -> String {
    item.key
        .clone()
        .unwrap_or_else(|| format!("{:?}", item.title))
}

#[cfg(test)]
mod tests;
