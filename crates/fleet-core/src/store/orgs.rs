//! Organisations (work graph M5, migration 050): named orgs, the text-keyed
//! rules that place sessions in them, and the org of a host, a tracker, a
//! session and a link.
//!
//! **Resolution** (first hit wins, plan §3):
//!
//! | Entity | Order |
//! |---|---|
//! | Tracker item | its tracker's `org_id` |
//! | Link | its item's org, else its session's (live), else the session's org when it ended |
//! | Session | the most specific matching rule — path > owner/repo > owner > host-only rule — else its host's `org_id` |
//!
//! A rule matches when EVERY field it sets matches: `owner` / `repo`
//! against the session's project (case-insensitively; an adopted folder's
//! `local` owner never matches), `path_prefix` against the worktree's path
//! (else the project's), on a directory boundary, and `host_alias` against
//! the session's host. Ties go to the lower rule id. Rules are text, so a
//! project row that `refresh_projects` deletes and re-creates keeps its org
//! (review C22). A session with no project resolves by host alone: fleet
//! stores no cwd for it.
//!
//! The session's org is computed in SQL ([`session_org_sql!`], a column of
//! `SESSION_COLUMNS`) so listed and emitted rows agree; [`org_of_session`] is
//! the same rule as a pure function, and a test holds the two equal.

use super::{now_unix, Store};
use crate::events::EventBus;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::BTreeSet;

/// The session's org as one SQL expression over a `sessions` row aliased
/// `$s`. A macro (not a `const`) so `SESSION_COLUMNS` can `concat!` it.
/// Migration 050's snapshot trigger inlines the same expression for
/// `sessions s`; `tests::the_trigger_snapshots_the_session_org` keeps them
/// equal in behaviour.
#[macro_export]
macro_rules! session_org_sql {
    ($s:literal) => {
        concat!(
            "COALESCE((SELECT r.org_id FROM org_rules r \
               LEFT JOIN projects op ON op.id = ",
            $s,
            ".project_id \
               LEFT JOIN worktrees ow ON ow.id = ",
            $s,
            ".worktree_id \
              WHERE (r.host_alias IS NULL OR r.host_alias = ",
            $s,
            ".host_alias) \
                AND (r.owner IS NULL OR (op.owner IS NOT NULL AND op.owner <> 'local' \
                                         AND lower(op.owner) = lower(r.owner))) \
                AND (r.repo IS NULL OR lower(op.repo) = lower(r.repo)) \
                AND (r.path_prefix IS NULL \
                     OR COALESCE(ow.path, op.base_path) = r.path_prefix \
                     OR substr(COALESCE(ow.path, op.base_path), 1, length(r.path_prefix) + 1) \
                        = r.path_prefix || '/') \
              ORDER BY CASE WHEN r.path_prefix IS NOT NULL THEN 3000 + length(r.path_prefix) \
                            WHEN r.repo IS NOT NULL THEN 2000 \
                            WHEN r.owner IS NOT NULL THEN 1000 \
                            ELSE 0 END \
                       + CASE WHEN r.host_alias IS NOT NULL THEN 1 ELSE 0 END DESC, \
                       r.id ASC \
              LIMIT 1), \
             (SELECT h.org_id FROM hosts h WHERE h.alias = ",
            $s,
            ".host_alias))"
        )
    };
}

/// Longest org name.
pub const ORG_NAME_MAX_CHARS: usize = 60;

/// One organisation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrgRow {
    pub id: i64,
    pub name: String,
    /// A CSS colour (`#rrggbb`) for the UI's bar, or none.
    #[serde(default)]
    pub color: Option<String>,
    /// Decision D7: also fence sessions between this org and the others.
    #[serde(default)]
    pub isolate_sessions: bool,
    pub created_at: i64,
    /// Work graph M7: this org's auto-tidy override; `None` inherits
    /// `work.auto_tidy`. Absent from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_tidy: Option<bool>,
}

/// One placement rule. At least one of `owner`, `path_prefix`, `host_alias`
/// is set; `repo` only with `owner`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrgRuleRow {
    #[serde(default)]
    pub id: i64,
    pub org_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
}

/// What [`org_of_session`] reads of a session: its host and, when it has
/// one, its project's owner / repo and its working path (the worktree's,
/// else the project's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionOrgFacts<'a> {
    pub host_alias: &'a str,
    pub owner: Option<&'a str>,
    pub repo: Option<&'a str>,
    pub path: Option<&'a str>,
}

/// Specificity of a rule: path (longer first) > owner/repo > owner > host
/// only; a host qualifier breaks a tie.
fn rank(r: &OrgRuleRow) -> i64 {
    let base = if let Some(p) = &r.path_prefix {
        3000 + p.chars().count() as i64
    } else if r.repo.is_some() {
        2000
    } else if r.owner.is_some() {
        1000
    } else {
        0
    };
    base + i64::from(r.host_alias.is_some())
}

fn rule_matches(r: &OrgRuleRow, f: &SessionOrgFacts<'_>) -> bool {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if let Some(h) = &r.host_alias {
        if h != f.host_alias {
            return false;
        }
    }
    if let Some(o) = &r.owner {
        match f.owner {
            Some(owner) if owner != "local" && eq(owner, o) => {}
            _ => return false,
        }
    }
    if let Some(repo) = &r.repo {
        match f.repo {
            Some(x) if eq(x, repo) => {}
            _ => return false,
        }
    }
    if let Some(p) = &r.path_prefix {
        match f.path {
            Some(path) if path == p || path.starts_with(&format!("{p}/")) => {}
            _ => return false,
        }
    }
    true
}

/// The session's org: the most specific matching rule, else the host's org.
/// The same rule as [`session_org_sql!`], as a pure function.
pub fn org_of_session(
    facts: &SessionOrgFacts<'_>,
    rules: &[OrgRuleRow],
    host_org: Option<i64>,
) -> Option<i64> {
    rules
        .iter()
        .filter(|r| rule_matches(r, facts))
        .max_by(|a, b| rank(a).cmp(&rank(b)).then(b.id.cmp(&a.id)))
        .map(|r| r.org_id)
        .or(host_org)
}

/// Can a person name an org this?
pub fn validate_org_name(name: &str) -> Result<String, IpcError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > ORG_NAME_MAX_CHARS || n.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("an org name is 1–{ORG_NAME_MAX_CHARS} characters, one line"),
        ));
    }
    Ok(n.to_string())
}

/// `#rgb` / `#rrggbb`, or nothing.
pub fn validate_org_color(color: Option<&str>) -> Result<Option<String>, IpcError> {
    let Some(c) = color.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let hex = c.strip_prefix('#').unwrap_or("");
    if !(hex.len() == 3 || hex.len() == 6) || !hex.chars().all(|x| x.is_ascii_hexdigit()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "an org colour is #rgb or #rrggbb",
        ));
    }
    Ok(Some(c.to_lowercase()))
}

/// Normalise a rule before it is stored: trimmed, empty fields dropped, a
/// path without its trailing `/`, and the shape checks the table's CHECKs
/// make, with a sentence instead of a constraint error.
pub fn normalize_rule(mut r: OrgRuleRow) -> Result<OrgRuleRow, IpcError> {
    let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    r.owner = clean(r.owner);
    r.repo = clean(r.repo);
    r.host_alias = clean(r.host_alias);
    r.path_prefix = clean(r.path_prefix).map(|p| {
        let t = p.trim_end_matches('/');
        if t.is_empty() {
            "/".to_string()
        } else {
            t.to_string()
        }
    });
    if r.owner.is_none() && r.path_prefix.is_none() && r.host_alias.is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a rule needs owner, path_prefix or host_alias",
        ));
    }
    if r.owner
        .as_deref()
        .is_some_and(|o| o.eq_ignore_ascii_case("local"))
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            "`local` is the placeholder owner of adopted folders, not an owner; use path_prefix",
        ));
    }
    if r.repo.is_some() && r.owner.is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a repo rule needs its owner",
        ));
    }
    if r.path_prefix.as_deref() == Some("/") {
        return Err(IpcError::new(
            codes::E_INVALID,
            "path_prefix `/` would match everything; use a host rule",
        ));
    }
    for (what, v) in [
        ("owner", &r.owner),
        ("repo", &r.repo),
        ("host_alias", &r.host_alias),
        ("path_prefix", &r.path_prefix),
    ] {
        if let Some(v) = v {
            if v.chars().count() > 512 || v.chars().any(char::is_control) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{what} must be one line of at most 512 characters"),
                ));
            }
        }
    }
    Ok(r)
}

const ORG_COLUMNS: &str = "id, name, color, isolate_sessions, created_at, auto_tidy";
const RULE_COLUMNS: &str = "id, org_id, owner, repo, path_prefix, host_alias";

fn map_org(r: &rusqlite::Row<'_>) -> rusqlite::Result<OrgRow> {
    Ok(OrgRow {
        id: r.get(0)?,
        name: r.get(1)?,
        color: r.get(2)?,
        isolate_sessions: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
        auto_tidy: r.get::<_, Option<i64>>(5)?.map(|v| v != 0),
    })
}

fn map_rule(r: &rusqlite::Row<'_>) -> rusqlite::Result<OrgRuleRow> {
    Ok(OrgRuleRow {
        id: r.get(0)?,
        org_id: r.get(1)?,
        owner: r.get(2)?,
        repo: r.get(3)?,
        path_prefix: r.get(4)?,
        host_alias: r.get(5)?,
    })
}

fn org_not_found(id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("org {id} not found"))
}

impl Store {
    pub fn list_orgs(&self) -> Result<Vec<OrgRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ORG_COLUMNS} FROM orgs ORDER BY name COLLATE NOCASE, id"
        ))?;
        let rows = stmt.query_map([], map_org)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_org(&self, id: i64) -> Result<Option<OrgRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ORG_COLUMNS} FROM orgs WHERE id = ?1"),
                rusqlite::params![id],
                map_org,
            )
            .optional()?)
    }

    /// Create an org. `E_EXISTS` for a taken name (case-insensitively).
    pub fn add_org(
        &self,
        name: &str,
        color: Option<&str>,
        isolate_sessions: bool,
    ) -> Result<OrgRow, IpcError> {
        let name = validate_org_name(name)?;
        let color = validate_org_color(color)?;
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM orgs WHERE lower(name) = lower(?1))",
            rusqlite::params![name],
            |r| r.get(0),
        )?;
        if taken {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("an org named {name:?} already exists"),
            ));
        }
        self.conn.execute(
            "INSERT INTO orgs (name, color, isolate_sessions, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![name, color, isolate_sessions as i64, now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_org(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "org vanished"))
    }

    /// Change what is given. `color: Some("")` clears the colour.
    pub fn update_org(
        &self,
        id: i64,
        name: Option<&str>,
        color: Option<&str>,
        isolate_sessions: Option<bool>,
    ) -> Result<OrgRow, IpcError> {
        let cur = self.get_org(id)?.ok_or_else(|| org_not_found(id))?;
        let name = match name {
            Some(n) => {
                let n = validate_org_name(n)?;
                let taken: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM orgs WHERE lower(name) = lower(?1) AND id <> ?2)",
                    rusqlite::params![n, id],
                    |r| r.get(0),
                )?;
                if taken {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("an org named {n:?} already exists"),
                    ));
                }
                n
            }
            None => cur.name,
        };
        let color = match color {
            Some(c) => validate_org_color(Some(c))?,
            None => cur.color,
        };
        let isolate = isolate_sessions.unwrap_or(cur.isolate_sessions);
        self.conn.execute(
            "UPDATE orgs SET name = ?2, color = ?3, isolate_sessions = ?4 WHERE id = ?1",
            rusqlite::params![id, name, color, isolate as i64],
        )?;
        self.get_org(id)?.ok_or_else(|| org_not_found(id))
    }

    /// Set (or, with `None`, clear) an org's auto-tidy override (work graph
    /// M7): `None` inherits `work.auto_tidy`.
    pub fn set_org_auto_tidy(&self, id: i64, on: Option<bool>) -> Result<OrgRow, IpcError> {
        let n = self.conn.execute(
            "UPDATE orgs SET auto_tidy = ?2 WHERE id = ?1",
            rusqlite::params![id, on.map(|b| b as i64)],
        )?;
        if n == 0 {
            return Err(org_not_found(id));
        }
        self.get_org(id)?.ok_or_else(|| org_not_found(id))
    }

    /// org id → its auto-tidy override, for the orgs that set one.
    pub fn org_auto_tidy_overrides(
        &self,
    ) -> Result<std::collections::HashMap<i64, bool>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, auto_tidy FROM orgs WHERE auto_tidy IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Delete an org: its rules go with it, its hosts become unassigned, and
    /// so do its past links (the `snap_org_id` snapshot, which has no FK):
    /// a removed org's work must not stay fenced from every host forever,
    /// nor name an org that no longer exists. One transaction. Callers
    /// refuse first while trackers reference it ([`Store::trackers_of_org`]).
    /// `false` when there was no such org.
    pub fn remove_org(&self, id: i64) -> Result<bool, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        // Explicit, not only the FK actions: they need `foreign_keys = ON`.
        tx.execute(
            "UPDATE hosts SET org_id = NULL WHERE org_id = ?1",
            rusqlite::params![id],
        )?;
        tx.execute(
            "UPDATE work_links SET snap_org_id = NULL WHERE snap_org_id = ?1",
            rusqlite::params![id],
        )?;
        tx.execute(
            "DELETE FROM org_rules WHERE org_id = ?1",
            rusqlite::params![id],
        )?;
        let removed = tx.execute("DELETE FROM orgs WHERE id = ?1", rusqlite::params![id])? > 0;
        tx.commit()?;
        Ok(removed)
    }

    /// `(id, name)` of the trackers assigned to org `id`.
    pub fn trackers_of_org(&self, id: i64) -> Result<Vec<(i64, String)>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM trackers WHERE org_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map(rusqlite::params![id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_org_rules(&self) -> Result<Vec<OrgRuleRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RULE_COLUMNS} FROM org_rules ORDER BY org_id, id"
        ))?;
        let rows = stmt.query_map([], map_rule)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Add a rule (normalised first). `E_EXISTS` for an identical rule.
    pub fn add_org_rule(&self, rule: OrgRuleRow) -> Result<OrgRuleRow, IpcError> {
        let r = normalize_rule(rule)?;
        if self.get_org(r.org_id)?.is_none() {
            return Err(org_not_found(r.org_id));
        }
        let dup: Option<i64> = self
            .conn
            .query_row(
                "SELECT org_id FROM org_rules WHERE owner IS ?1 AND repo IS ?2 \
                   AND path_prefix IS ?3 AND host_alias IS ?4",
                rusqlite::params![r.owner, r.repo, r.path_prefix, r.host_alias],
                |x| x.get(0),
            )
            .optional()?;
        if let Some(org) = dup {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("that rule already exists (org {org})"),
            ));
        }
        self.conn.execute(
            "INSERT INTO org_rules (org_id, owner, repo, path_prefix, host_alias) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![r.org_id, r.owner, r.repo, r.path_prefix, r.host_alias],
        )?;
        Ok(OrgRuleRow {
            id: self.conn.last_insert_rowid(),
            ..r
        })
    }

    /// `false` when there was no such rule.
    pub fn remove_org_rule(&self, id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .execute("DELETE FROM org_rules WHERE id = ?1", rusqlite::params![id])?
            > 0)
    }

    /// Place a host in an org (`None`: unassign). `E_NOTFOUND` for an
    /// unknown host or org.
    pub fn set_host_org(&self, alias: &str, org: Option<i64>) -> Result<(), IpcError> {
        if let Some(id) = org {
            if self.get_org(id)?.is_none() {
                return Err(org_not_found(id));
            }
        }
        let n = self.conn.execute(
            "UPDATE hosts SET org_id = ?2 WHERE alias = ?1",
            rusqlite::params![alias, org],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("host {alias} not found"),
            ));
        }
        Ok(())
    }

    /// The host's org; `None` for no org or an unknown host.
    pub fn host_org(&self, alias: &str) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT org_id FROM hosts WHERE alias = ?1",
                rusqlite::params![alias],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Place a tracker (and so its items) in an org (`None`: unassign).
    pub fn set_tracker_org(&self, tracker_id: i64, org: Option<i64>) -> Result<(), IpcError> {
        if let Some(id) = org {
            if self.get_org(id)?.is_none() {
                return Err(org_not_found(id));
            }
        }
        let n = self.conn.execute(
            "UPDATE trackers SET org_id = ?2 WHERE id = ?1",
            rusqlite::params![tracker_id, org],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("tracker {tracker_id} not found"),
            ));
        }
        Ok(())
    }

    /// The orgs with `isolate_sessions` on.
    pub fn isolated_orgs(&self) -> Result<BTreeSet<i64>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM orgs WHERE isolate_sessions = 1")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<BTreeSet<_>>>()?)
    }

    /// A live session's org (`None` for unassigned or an unknown id).
    pub fn session_org(&self, session_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                concat!(
                    "SELECT ",
                    crate::session_org_sql!("sessions"),
                    " FROM sessions WHERE id = ?1"
                ),
                rusqlite::params![session_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// A work item's org: its tracker's; `None` for a local item, an
    /// unassigned tracker or an unknown id.
    pub fn item_org(&self, item_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT t.org_id FROM work_items i JOIN trackers t ON t.id = i.tracker_id \
                 WHERE i.id = ?1",
                rusqlite::params![item_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// The org of an ended link whose snapshot predates migration 050: the
    /// rules over what the snapshot kept (host, project), else the host's.
    fn snapshot_org(&self, l: &super::WorkLinkRow) -> Result<Option<i64>, IpcError> {
        let Some(host) = l.snap_host.as_deref() else {
            return Ok(None);
        };
        let project: Option<(String, String, String)> = match l.snap_project_id {
            Some(pid) => self
                .conn
                .query_row(
                    "SELECT owner, repo, base_path FROM projects WHERE id = ?1",
                    rusqlite::params![pid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?,
            None => None,
        };
        let facts = SessionOrgFacts {
            host_alias: host,
            owner: project.as_ref().map(|p| p.0.as_str()),
            repo: project.as_ref().map(|p| p.1.as_str()),
            path: project.as_ref().map(|p| p.2.as_str()),
        };
        Ok(org_of_session(
            &facts,
            &self.list_org_rules()?,
            self.host_org(host)?,
        ))
    }

    /// A link's org: its item's, else its live session's, else the org its
    /// session had when the link ended.
    pub fn link_org(&self, l: &super::WorkLinkRow) -> Result<Option<i64>, IpcError> {
        if let Some(org) = l.item_id.map(|i| self.item_org(i)).transpose()?.flatten() {
            return Ok(Some(org));
        }
        if l.ended_at.is_none() {
            if let Some(p) = l.participant_id {
                let sid: Option<i64> = self
                    .conn
                    .query_row(
                        "SELECT session_id FROM participants WHERE id = ?1 AND retired_at IS NULL",
                        rusqlite::params![p],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                if let Some(sid) = sid {
                    return self.session_org(sid);
                }
            }
        }
        // `org_id` holds `snap_org_id` straight from the row (see `map_link`).
        if l.org_id.is_some() {
            return Ok(l.org_id);
        }
        self.snapshot_org(l)
    }

    /// The org a work target belongs to: an item's (its tracker's); a key's
    /// through the item it resolves to (`resolve_work_key`), else none.
    pub fn work_target_org(&self, target: super::WorkTarget<'_>) -> Result<Option<i64>, IpcError> {
        let item = match target {
            super::WorkTarget::Item(id) => Some(id),
            super::WorkTarget::Key(raw) => {
                self.resolve_work_key(&super::normalize_work_ref(raw)?)?.0
            }
            super::WorkTarget::Ref(_) => None,
        };
        Ok(item.map(|i| self.item_org(i)).transpose()?.flatten())
    }

    /// The org a session would have if created now on `host` in
    /// `project_id` (no worktree yet: the project's path).
    pub fn org_for_new_session(
        &self,
        host: &str,
        project_id: i64,
    ) -> Result<Option<i64>, IpcError> {
        let project: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT owner, repo, base_path FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let facts = SessionOrgFacts {
            host_alias: host,
            owner: project.as_ref().map(|p| p.0.as_str()),
            repo: project.as_ref().map(|p| p.1.as_str()),
            path: project.as_ref().map(|p| p.2.as_str()),
        };
        Ok(org_of_session(
            &facts,
            &self.list_org_rules()?,
            self.host_org(host)?,
        ))
    }

    /// Every session's org, by id (the snapshot `announce_org_moves` diffs).
    pub fn session_orgs(&self) -> Result<std::collections::HashMap<i64, Option<i64>>, IpcError> {
        let mut stmt = self.conn.prepare(concat!(
            "SELECT id, ",
            crate::session_org_sql!("sessions"),
            " FROM sessions"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// After an org change: every session whose org moved gets its
    /// `row_version` bumped (the frontend's merge guard orders by it) and a
    /// `session:updated`, so the sidebar regroups without a re-list.
    pub fn announce_org_moves(
        &self,
        before: &std::collections::HashMap<i64, Option<i64>>,
    ) -> Result<usize, IpcError> {
        let after = self.session_orgs()?;
        let mut moved = 0;
        for (id, org) in after {
            if before.get(&id).copied().flatten() == org {
                continue;
            }
            // A no-op UPDATE: migration 042's trigger bumps `row_version`.
            self.conn.execute(
                "UPDATE sessions SET status = status WHERE id = ?1",
                rusqlite::params![id],
            )?;
            if let Some(row) = self.get_session_by_id(id)? {
                self.bus.session_updated(&row);
                moved += 1;
            }
        }
        Ok(moved)
    }

    /// Resolve every link's `org_id` in place ([`Store::link_org`]).
    pub fn fill_link_orgs(&self, links: &mut [super::WorkLinkRow]) -> Result<(), IpcError> {
        for l in links.iter_mut() {
            l.org_id = self.link_org(l)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "orgs_tests.rs"]
mod tests;

#[cfg(test)]
impl Store {
    /// Test-only: put a row in a Claude status directly.
    pub(crate) fn set_session_claude_status_for_test(&self, id: i64, status: &str) {
        self.conn
            .execute(
                "UPDATE sessions SET claude_status = ?2 WHERE id = ?1",
                rusqlite::params![id, status],
            )
            .expect("status");
    }
}
