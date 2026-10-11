//! Jira-style ticket keys administered by fleet (migration 168).
//!
//! A ticket reads `FLT-12`, not `owner/repo#12` or `asana:1209876543210987`.
//! Each **ticket space** has a short upper-case prefix and its own counter:
//!
//! * a fleet project's (`project:<id>`), drawn on by the native tasks written
//!   in it and by the GitHub issues of its repository, so a project's work is
//!   one numbering whichever side it was written on;
//! * a GitHub repository no project claims (`github:<owner/repo>`);
//! * an Asana tracker (`tracker:<id>`).
//!
//! Jira and Linear keys are already human (`ABC-12`) and draw nothing.
//!
//! A native item's `key` IS its ticket key (the start path and branch
//! detection then see `FLT-12`). A tracker item keeps its tracker's key as
//! identity and carries the ticket key in `display_key`, which the key
//! lookups answer to as well. A native item written outside any project keeps
//! the fleet-wide `TASK-<id>`, and items keyed before this migration keep the
//! keys they have: a key may already sit in a branch name or a commit.
//!
//! The prefix is derived from the space's name the first time it is needed
//! (`claude-fleet` → `CF`, `payments` → `PAY`), never one another space,
//! a tracker's probed prefixes, an existing key or `TASK` already uses.
//! [`Store::rename_ticket_prefix`] changes it, Jira-like: every key in the
//! space follows, and the old prefix keeps resolving.

use super::work::{map_item, ITEM_COLUMNS};
use super::{now_unix, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// Shortest and longest prefix: the shape `canonical_key` upper-cases
/// (`is_ticket_key`).
pub const TICKET_PREFIX_MIN: usize = 2;
pub const TICKET_PREFIX_MAX: usize = 10;

/// One ticket space: the numbering behind a prefix.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TicketSpace {
    pub id: i64,
    /// `project:<id>` | `github:<owner/repo>` | `tracker:<id>`.
    pub scope: String,
    pub prefix: String,
    /// The number the next ticket draws.
    pub next_seq: i64,
    /// Prefixes a rename replaced; their keys still resolve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_prefixes: Vec<String>,
}

/// A prefix fleet accepts: a letter, then letters, digits or `_`, 2–10 long,
/// upper case. What a person types is upper-cased first.
pub fn valid_ticket_prefix(p: &str) -> bool {
    let mut chars = p.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && (TICKET_PREFIX_MIN..=TICKET_PREFIX_MAX).contains(&p.len())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// PURE: the prefix a name suggests. Words are split on anything that is not
/// a letter or digit and on lower→upper case changes (`fleetMobile`). Several
/// words give their initials (at most four: `claude-fleet` → `CF`); one word
/// its first three letters, or all of it when it is four or fewer
/// (`payments` → `PAY`, `web` → `WEB`). A name that gives fewer than two
/// letters is padded from its first word, else falls back to `TKT`.
pub fn derive_ticket_prefix(name: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_ascii_lowercase();
        cur.push(c.to_ascii_uppercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    // A prefix starts with a letter: leading digits of a word do not count,
    // and a word of digits only (`v2`'s `2` after the split) is no initial.
    let words: Vec<String> = words
        .into_iter()
        .map(|w| {
            w.trim_start_matches(|c: char| c.is_ascii_digit())
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let mut p = match words.as_slice() {
        [] => String::new(),
        [one] if one.len() <= 4 => one.clone(),
        [one] => one.chars().take(3).collect(),
        many => many
            .iter()
            .take(4)
            .filter_map(|w| w.chars().next())
            .collect(),
    };
    if p.len() < TICKET_PREFIX_MIN {
        if let Some(first) = words.first() {
            p = first.chars().take(3).collect();
        }
    }
    if valid_ticket_prefix(&p) {
        p
    } else {
        "TKT".to_string()
    }
}

/// What a person typed as a prefix, upper-cased, or `E_INVALID` saying
/// what a prefix is.
pub fn parse_ticket_prefix(raw: &str) -> Result<String, IpcError> {
    let p = raw.trim().to_ascii_uppercase();
    if valid_ticket_prefix(&p) {
        Ok(p)
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "a ticket prefix is a letter then letters, digits or _, \
                 {TICKET_PREFIX_MIN}–{TICKET_PREFIX_MAX} long"
            ),
        ))
    }
}

/// `PREFIX-12` → `("PREFIX", 12)` for a valid prefix; else `None`.
fn split_ticket_key(key: &str) -> Option<(String, i64)> {
    let (p, n) = key.rsplit_once('-')?;
    let p = p.to_ascii_uppercase();
    if !valid_ticket_prefix(&p) || n.is_empty() || n.len() > 9 {
        return None;
    }
    if !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((p, n.parse().ok()?))
}

fn map_space(r: &rusqlite::Row<'_>) -> rusqlite::Result<TicketSpace> {
    let former: Option<String> = r.get(4)?;
    Ok(TicketSpace {
        id: r.get(0)?,
        scope: r.get(1)?,
        prefix: r.get(2)?,
        next_seq: r.get(3)?,
        former_prefixes: former
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
    })
}

/// One row of Settings → Ticket keys (`work_admin { ticket_prefixes }`): a
/// ticket space, or a project that has none yet with the prefix it would
/// get.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TicketPrefixRow {
    /// `None` for a project whose space is not made yet.
    #[serde(default)]
    pub space_id: Option<i64>,
    #[serde(default)]
    pub project_id: Option<i64>,
    /// `project:<id>` | `github:<owner/repo>` | `tracker:<id>`.
    pub scope: String,
    /// What a person reads: `acme/claude-fleet`, `GitHub acme/web`,
    /// `Asana · Company B`.
    pub label: String,
    pub prefix: String,
    /// The prefix is only suggested: nothing has drawn on it yet, and a
    /// first ticket may still find it taken.
    #[serde(default)]
    pub suggested: bool,
    /// Tickets drawn so far.
    #[serde(default)]
    pub tickets: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_prefixes: Vec<String>,
}

const SPACE_COLUMNS: &str = "id, scope, prefix, next_seq, former_prefixes";

/// The ticket space a GitHub issue key draws on before any project claims
/// its repository: `owner/repo#12` → `github:owner/repo`.
fn github_scope(key: &str) -> Option<(String, String)> {
    let (repo, _) = super::work::github_ref(key)?;
    let name = repo.rsplit('/').next().unwrap_or(repo).to_string();
    Some((format!("github:{}", repo.to_ascii_lowercase()), name))
}

impl Store {
    /// Every ticket space, by prefix.
    pub fn list_ticket_spaces(&self) -> Result<Vec<TicketSpace>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SPACE_COLUMNS} FROM ticket_spaces ORDER BY prefix"
        ))?;
        let rows = stmt.query_map([], map_space)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn ticket_space(&self, id: i64) -> Result<Option<TicketSpace>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SPACE_COLUMNS} FROM ticket_spaces WHERE id = ?1"),
                rusqlite::params![id],
                map_space,
            )
            .optional()?)
    }

    fn ticket_space_by_scope(&self, scope: &str) -> Result<Option<TicketSpace>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SPACE_COLUMNS} FROM ticket_spaces WHERE scope = ?1"),
                rusqlite::params![scope],
                map_space,
            )
            .optional()?)
    }

    /// A project's ticket space, created (with a prefix from its repository
    /// name) the first time it is asked for. `E_NOTFOUND` for no project.
    pub fn project_ticket_space(&self, project_id: i64) -> Result<TicketSpace, IpcError> {
        let scope = format!("project:{project_id}");
        if let Some(s) = self.ticket_space_by_scope(&scope)? {
            return Ok(s);
        }
        let repo: Option<String> = self
            .conn
            .query_row(
                "SELECT repo FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(repo) = repo else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("project {project_id} not found"),
            ));
        };
        self.create_ticket_space(&scope, &repo)
    }

    /// Whether `prefix` is free for a new space or a rename: no space has it
    /// (now or before a rename), no tracker probed it, it is not `TASK`, and
    /// no work item's key or display key already starts with it.
    fn ticket_prefix_free(
        &self,
        prefix: &str,
        except_space: Option<i64>,
    ) -> Result<bool, IpcError> {
        if prefix.eq_ignore_ascii_case(super::TASK_KEY_PREFIX) {
            return Ok(false);
        }
        for s in self.list_ticket_spaces()? {
            if Some(s.id) == except_space {
                continue;
            }
            if s.prefix.eq_ignore_ascii_case(prefix)
                || s.former_prefixes
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(prefix))
            {
                return Ok(false);
            }
        }
        for t in self.list_trackers()? {
            if t.config
                .key_prefixes
                .iter()
                .any(|p| p.eq_ignore_ascii_case(prefix))
            {
                return Ok(false);
            }
        }
        // `_` is a LIKE wildcard; a prefix holds no `\`, so it escapes.
        let pattern = format!("{}-%", prefix.to_ascii_uppercase().replace('_', "\\_"));
        let used: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_items \
               WHERE (upper(key) LIKE ?1 ESCAPE '\\' \
                      OR upper(display_key) LIKE ?1 ESCAPE '\\') \
                 AND (?2 IS NULL OR ticket_space_id IS NOT ?2))",
            rusqlite::params![pattern, except_space],
            |r| r.get(0),
        )?;
        Ok(!used)
    }

    /// A new space for `scope` whose prefix `name` suggests: the derived
    /// prefix, else it with 2, 3 … appended.
    fn create_ticket_space(&self, scope: &str, name: &str) -> Result<TicketSpace, IpcError> {
        let base = derive_ticket_prefix(name);
        let mut prefix = base.clone();
        let mut n = 2;
        while !self.ticket_prefix_free(&prefix, None)? {
            let suffix = n.to_string();
            let keep = TICKET_PREFIX_MAX
                .saturating_sub(suffix.len())
                .min(base.len());
            prefix = format!("{}{suffix}", &base[..keep]);
            n += 1;
            if n > 999 {
                return Err(IpcError::new(
                    codes::E_INTERNAL,
                    format!("no free ticket prefix for {name:?}"),
                ));
            }
        }
        self.conn.execute(
            "INSERT INTO ticket_spaces (scope, prefix, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![scope, prefix, now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.ticket_space(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "ticket space vanished after insert"))
    }

    /// Draw the next number of `space`: `(seq, PREFIX-seq)`. A number whose
    /// key some item already carries (a person named a local item `FLT-7`
    /// by hand) is skipped. The caller writes the item in the same
    /// transaction.
    pub(super) fn draw_ticket_number(
        &self,
        space: &TicketSpace,
    ) -> Result<(i64, String), IpcError> {
        let mut seq: i64 = self.conn.query_row(
            "SELECT next_seq FROM ticket_spaces WHERE id = ?1",
            rusqlite::params![space.id],
            |r| r.get(0),
        )?;
        loop {
            let key = format!("{}-{seq}", space.prefix);
            let taken: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM work_items WHERE key = ?1 OR display_key = ?1)",
                rusqlite::params![key],
                |r| r.get(0),
            )?;
            if !taken {
                self.conn.execute(
                    "UPDATE ticket_spaces SET next_seq = ?1 WHERE id = ?2",
                    rusqlite::params![seq + 1, space.id],
                )?;
                return Ok((seq, key));
            }
            seq += 1;
        }
    }

    /// Give tracker item `item_id` (key `key`) its ticket key when its
    /// tracker has no human keys (GitHub, Asana) and it has none yet.
    /// Returns whether it drew one. Jira / Linear keys draw nothing.
    pub(super) fn ensure_tracker_ticket_key(
        &self,
        item_id: i64,
        tracker_id: i64,
        key: Option<&str>,
    ) -> Result<bool, IpcError> {
        let Some(key) = key else {
            return Ok(false);
        };
        if !key.starts_with("asana:") && super::work::github_ref(key).is_none() {
            return Ok(false);
        }
        let has: bool = self.conn.query_row(
            "SELECT display_key IS NOT NULL FROM work_items WHERE id = ?1",
            rusqlite::params![item_id],
            |r| r.get(0),
        )?;
        if has {
            return Ok(false);
        }
        let space = if key.starts_with("asana:") {
            let scope = format!("tracker:{tracker_id}");
            match self.ticket_space_by_scope(&scope)? {
                Some(s) => s,
                None => {
                    let name: String = self
                        .conn
                        .query_row(
                            "SELECT name FROM trackers WHERE id = ?1",
                            rusqlite::params![tracker_id],
                            |r| r.get(0),
                        )
                        .optional()?
                        .unwrap_or_else(|| "Asana".to_string());
                    self.create_ticket_space(&scope, &name)?
                }
            }
        } else if let Some((scope, name)) = github_scope(key) {
            // A project whose repository this is numbers its issues.
            let repo = scope.trim_start_matches("github:");
            let project: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM projects \
                     WHERE lower(owner || '/' || repo) = ?1 ORDER BY id LIMIT 1",
                    rusqlite::params![repo],
                    |r| r.get(0),
                )
                .optional()?;
            match project {
                Some(pid) => self.project_ticket_space(pid)?,
                None => match self.ticket_space_by_scope(&scope)? {
                    Some(s) => s,
                    None => self.create_ticket_space(&scope, &name)?,
                },
            }
        } else {
            return Ok(false);
        };
        let (seq, display) = self.draw_ticket_number(&space)?;
        self.conn.execute(
            "UPDATE work_items SET ticket_space_id = ?1, ticket_seq = ?2, display_key = ?3 \
             WHERE id = ?4",
            rusqlite::params![space.id, seq, display, item_id],
        )?;
        Ok(true)
    }

    /// `key` with a former prefix rewritten to its space's current one
    /// (`OLD-7` → `NEW-7` after a rename); `None` when it names none.
    pub(super) fn current_ticket_key(&self, key: &str) -> Result<Option<String>, IpcError> {
        let Some((prefix, seq)) = split_ticket_key(key) else {
            return Ok(None);
        };
        for s in self.list_ticket_spaces()? {
            if s.former_prefixes
                .iter()
                .any(|f| f.eq_ignore_ascii_case(&prefix))
            {
                return Ok(Some(format!("{}-{seq}", s.prefix)));
            }
        }
        Ok(None)
    }

    /// The item whose ticket key is `key` (current or a former prefix's).
    pub fn item_by_ticket_key(&self, key: &str) -> Result<Option<WorkItemRow>, IpcError> {
        let key = super::normalize_work_ref(key)?;
        let find = |k: &str| -> Result<Option<WorkItemRow>, IpcError> {
            Ok(self
                .conn
                .query_row(
                    &format!(
                        "SELECT {ITEM_COLUMNS} FROM work_items WHERE display_key = ?1 \
                         ORDER BY id LIMIT 1"
                    ),
                    rusqlite::params![k],
                    map_item,
                )
                .optional()?)
        };
        if let Some(item) = find(&key)? {
            return Ok(Some(item));
        }
        match self.current_ticket_key(&key)? {
            Some(k) => find(&k),
            None => Ok(None),
        }
    }

    /// Every ticket space, with a label and its ticket count, then every
    /// project (not fleet's own) that has none yet with the prefix its name
    /// suggests — by label.
    pub fn ticket_prefix_overview(&self) -> Result<Vec<TicketPrefixRow>, IpcError> {
        let mut out = Vec::new();
        let mut with_space = std::collections::HashSet::new();
        for sp in self.list_ticket_spaces()? {
            let (label, project_id) = self.ticket_scope_label(&sp.scope)?;
            if let Some(pid) = project_id {
                with_space.insert(pid);
            }
            let tickets: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM work_items WHERE ticket_space_id = ?1",
                rusqlite::params![sp.id],
                |r| r.get(0),
            )?;
            out.push(TicketPrefixRow {
                space_id: Some(sp.id),
                project_id,
                scope: sp.scope,
                label,
                prefix: sp.prefix,
                suggested: false,
                tickets,
                former_prefixes: sp.former_prefixes,
            });
        }
        for p in self.list_projects()? {
            if p.system || with_space.contains(&p.id) {
                continue;
            }
            out.push(TicketPrefixRow {
                space_id: None,
                project_id: Some(p.id),
                scope: format!("project:{}", p.id),
                label: format!("{}/{}", p.owner, p.repo),
                prefix: derive_ticket_prefix(&p.repo),
                suggested: true,
                tickets: 0,
                former_prefixes: Vec::new(),
            });
        }
        out.sort_by_key(|a| a.label.to_lowercase());
        Ok(out)
    }

    /// A scope as a person reads it, and the project it is, if one.
    fn ticket_scope_label(&self, scope: &str) -> Result<(String, Option<i64>), IpcError> {
        if let Some(id) = scope
            .strip_prefix("project:")
            .and_then(|i| i.parse::<i64>().ok())
        {
            let name: Option<String> = self
                .conn
                .query_row(
                    "SELECT owner || '/' || repo FROM projects WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .optional()?;
            return Ok((name.unwrap_or_else(|| format!("project {id}")), Some(id)));
        }
        if let Some(repo) = scope.strip_prefix("github:") {
            return Ok((format!("GitHub {repo}"), None));
        }
        if let Some(id) = scope
            .strip_prefix("tracker:")
            .and_then(|i| i.parse::<i64>().ok())
        {
            let name: Option<(String, String)> = self
                .conn
                .query_row(
                    "SELECT provider, name FROM trackers WHERE id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            return Ok((
                match name {
                    Some((provider, name)) => {
                        let mut p = provider;
                        if let Some(c) = p.get_mut(..1) {
                            c.make_ascii_uppercase();
                        }
                        format!("{p} · {name}")
                    }
                    None => format!("tracker {id} (removed)"),
                },
                None,
            ));
        }
        Ok((scope.to_string(), None))
    }

    /// Set the prefix of a space (`space_id`) or of a project's
    /// (`project_id`, its space made first when it has none): a rename by
    /// [`Self::rename_ticket_prefix`]'s rules. `E_INVALID` when neither is
    /// given.
    pub fn set_ticket_prefix(
        &self,
        space_id: Option<i64>,
        project_id: Option<i64>,
        prefix: &str,
    ) -> Result<TicketSpace, IpcError> {
        let space_id = match (space_id, project_id) {
            (Some(id), _) => id,
            (None, Some(pid)) => {
                // Checked before the space is made, so a refused prefix
                // leaves no space behind.
                let wanted = parse_ticket_prefix(prefix)?;
                let existing = self.ticket_space_by_scope(&format!("project:{pid}"))?;
                if existing.is_none() && !self.ticket_prefix_free(&wanted, None)? {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("ticket prefix {wanted} is already in use"),
                    ));
                }
                self.project_ticket_space(pid)?.id
            }
            (None, None) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "set_ticket_prefix needs space_id or project_id",
                ))
            }
        };
        self.rename_ticket_prefix(space_id, prefix)
    }

    /// Rename a space's prefix, Jira-like: every native key and display key
    /// in it follows (`OLD-7` → `NEW-7`, emitted), and the old prefix keeps
    /// resolving. `E_INVALID` for a malformed prefix, `E_EXISTS` for one in
    /// use, `E_NOTFOUND` for no such space. Renaming to its own prefix is a
    /// no-op; the old prefix is kept for lookups only when a ticket drew on it.
    pub fn rename_ticket_prefix(
        &self,
        space_id: i64,
        prefix: &str,
    ) -> Result<TicketSpace, IpcError> {
        let prefix = parse_ticket_prefix(prefix)?;
        let Some(space) = self.ticket_space(space_id)? else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("ticket space {space_id} not found"),
            ));
        };
        if space.prefix == prefix {
            return Ok(space);
        }
        // Taking back one of its own former prefixes is allowed.
        let own_former = space.former_prefixes.iter().any(|f| f == &prefix);
        if !own_former && !self.ticket_prefix_free(&prefix, Some(space_id))? {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("ticket prefix {prefix} is already in use"),
            ));
        }
        // The old prefix keeps resolving only when a ticket ever used it; a
        // prefix nothing drew on is simply let go.
        let mut former = space.former_prefixes.clone();
        former.retain(|f| f != &prefix);
        if space.next_seq > 1 && !former.contains(&space.prefix) {
            former.push(space.prefix.clone());
        }
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE ticket_spaces SET prefix = ?1, former_prefixes = ?2 WHERE id = ?3",
            rusqlite::params![prefix, serde_json::to_string(&former).ok(), space_id],
        )?;
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM work_items WHERE ticket_space_id = ?1")?;
            let rows = stmt.query_map(rusqlite::params![space_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = now_unix();
        // A native item's key is its display key; a tracker item's is not.
        self.conn.execute(
            "UPDATE work_items SET \
               key = CASE WHEN source = 'local' AND key = display_key \
                          THEN ?1 || '-' || ticket_seq ELSE key END, \
               display_key = ?1 || '-' || ticket_seq, updated_at = ?2 \
             WHERE ticket_space_id = ?3",
            rusqlite::params![prefix, now, space_id],
        )?;
        tx.commit()?;
        for id in ids {
            self.emit_work_item(
                id,
                super::tracker_items::SessionChange {
                    primary: true,
                    suggested: true,
                    rejected: true,
                },
            )?;
        }
        self.ticket_space(space_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "ticket space vanished after rename"))
    }
}

#[cfg(test)]
mod tests;
