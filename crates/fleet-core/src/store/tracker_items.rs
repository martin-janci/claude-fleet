//! Tracker items in `work_items` (work graph M3.3): what a sync writes.
//!
//! * **Identity is `(tracker_id, external_id)`** (C24). The key is an
//!   attribute; a key that changes adds the old one to `aliases`, so a moved
//!   issue keeps every link and every retro-bind.
//! * **Missing is not gone** (C25): [`Store::mark_tracker_item_unavailable`]
//!   stamps `unavailable_at` and a reason; nothing is deleted because of a
//!   tracker's answer, and a later sighting clears the stamp.
//! * **Events only on a real change.** [`Store::upsert_tracker_item`]
//!   compares the normalised row it would write with the one stored and
//!   reports whether anything a reader sees moved; a pass that finds the
//!   same tickets writes only `fetched_at` and emits nothing (the replay
//!   ring's 512 slots are for changes).
//! * **Retro-binding** ([`Store::bind_tracker_refs`]): a bare `ref_key` link
//!   whose prefix belongs to exactly one tracker, and which that tracker now
//!   has an item for (by key or alias), gets `item_id`; `ref_key` stays for
//!   history. A prefix two trackers claim is never bound (C28 / §0.3).

use super::work::{map_item, ITEM_COLUMNS};
use super::{now_unix, Store, WorkItemRow};
use crate::events::EventBus as _;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::BTreeSet;

/// One tracker item as a provider normalised it, ready to store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackerItemWrite {
    pub external_id: String,
    pub key: Option<String>,
    pub aliases: Vec<String>,
    pub title: String,
    pub url: Option<String>,
    pub kind: Option<String>,
    pub hierarchy_level: Option<i64>,
    pub status_name: String,
    /// todo | in_progress | done.
    pub status_category: String,
    pub resolution: Option<String>,
    pub parent_external_id: Option<String>,
    pub containers: Vec<String>,
    pub assignees: Vec<String>,
    pub assignee_id: Option<String>,
    pub iteration: Option<String>,
    pub iteration_active: bool,
    pub updated_ext: Option<i64>,
    pub description: Option<String>,
}

/// What an upsert did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertOutcome {
    pub id: i64,
    /// Anything a reader sees changed (a new item counts).
    pub changed: bool,
    /// `(from, to)` status names, when the status moved on an existing item.
    pub status_change: Option<(String, String)>,
}

/// `meta` JSON: the parts of a tracker item only fleet's own reads use.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ItemMeta {
    /// The first 2k characters of the description (third-party text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub iteration_active: bool,
    /// Favourite-filter views this item was last seen in (`filter:<id>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
}

impl ItemMeta {
    pub fn parse(raw: Option<&str>) -> ItemMeta {
        raw.and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_default()
    }
}

fn json_list(v: &[String]) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        serde_json::to_string(v).ok()
    }
}

/// The columns a reader sees, in comparison form.
#[derive(Debug, PartialEq, Eq)]
struct Visible {
    key: Option<String>,
    aliases: Option<String>,
    title: String,
    url: Option<String>,
    kind: Option<String>,
    hierarchy_level: Option<i64>,
    status_name: Option<String>,
    status_category: String,
    resolution: Option<String>,
    parent_id: Option<i64>,
    containers: Option<String>,
    assignees: Option<String>,
    iteration: Option<String>,
    updated_ext: Option<i64>,
    unavailable: bool,
    meta: Option<String>,
}

impl Visible {
    /// What a session row's `work_suggested` summary reads of the item
    /// (`SESSION_COLUMNS` in `rows.rs`): key, title, status and url. The
    /// tracker (for `org_id`) never changes on an update.
    fn suggested_view(
        &self,
    ) -> (
        &Option<String>,
        &str,
        &Option<String>,
        &str,
        &Option<String>,
    ) {
        (
            &self.key,
            &self.title,
            &self.status_name,
            &self.status_category,
            &self.url,
        )
    }

    /// Which parts of a session row a change from `self` to `after` moves.
    fn session_change(&self, after: &Visible) -> SessionChange {
        let suggested = self.suggested_view() != after.suggested_view();
        SessionChange {
            // `work` reads the same plus whether the item is unavailable.
            primary: suggested || self.unavailable != after.unavailable,
            suggested,
            // `work_rejected` lists keys only.
            rejected: self.key != after.key,
        }
    }
}

/// The parts of a session row a tracker item change moves (see
/// `SESSION_COLUMNS` in `rows.rs`): `work` (the primary link), `work_suggested`
/// (the top suggestion) and `work_rejected` (keys).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SessionChange {
    primary: bool,
    suggested: bool,
    rejected: bool,
}

impl SessionChange {
    const ALL: SessionChange = SessionChange {
        primary: true,
        suggested: true,
        rejected: true,
    };

    fn any(self) -> bool {
        self.primary || self.suggested || self.rejected
    }
}

/// The trackers that may answer `key` (M6): a GitHub `owner/repo#n` (or an
/// enterprise `host/owner/repo#n`, M11.4) the GitHub trackers of that
/// instance whose scope covers the repository; an `asana:<gid>` every
/// Asana tracker (the gid is global, the tracker that has it answers); a
/// ticket key the trackers that own its prefix (Jira projects, Linear
/// teams).
pub fn tracker_claims(trackers: &[super::TrackerRow], key: &str) -> Vec<i64> {
    let key = key.trim();
    if let Some((repo, _)) = super::work::github_ref(key) {
        let repo = repo.to_ascii_lowercase();
        return trackers
            .iter()
            .filter(|t| t.provider == "github" && github_covers(t, &repo))
            .map(|t| t.id)
            .collect();
    }
    if key.starts_with("asana:") {
        return trackers
            .iter()
            .filter(|t| t.provider == "asana")
            .map(|t| t.id)
            .collect();
    }
    let Some((prefix, _)) = key.split_once('-') else {
        return Vec::new();
    };
    trackers
        .iter()
        .filter(|t| {
            t.config
                .key_prefixes
                .iter()
                .any(|p| p.eq_ignore_ascii_case(prefix))
        })
        .map(|t| t.id)
        .collect()
}

/// A GitHub tracker's scope covers `repo` (`owner/repo` on github.com, or
/// `host/owner/repo` on an enterprise instance, lower case): the same
/// instance as the tracker's site (M11.4), then its `settings.repos` when
/// set, else the site's owner, else everything on that instance.
pub fn github_covers(t: &super::TrackerRow, repo: &str) -> bool {
    let Some((site_host, site_owner)) = super::trackers::github_site(&t.site_url) else {
        return false;
    };
    let Some((host, repo)) = super::work::split_github_repo(repo) else {
        return false;
    };
    let same_instance = match (site_host.as_deref(), host) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    };
    if !same_instance {
        return false;
    }
    if !t.settings.repos.is_empty() {
        return t
            .settings
            .repos
            .iter()
            .any(|r| r.eq_ignore_ascii_case(repo));
    }
    match site_owner {
        Some(owner) => repo
            .split_once('/')
            .is_some_and(|(o, _)| o.eq_ignore_ascii_case(&owner)),
        None => true,
    }
}

/// `tracker_id` may answer `key`: it claims it, and — for a ticket key,
/// whose prefix is the only evidence — no other tracker does (C28 / §0.3: a
/// prefix two trackers claim is never bound). A GitHub or Asana reference
/// names its tracker by more than a prefix, so every claimant may ask.
fn may_answer(trackers: &[super::TrackerRow], tracker_id: i64, key: &str) -> bool {
    let claims = tracker_claims(trackers, key);
    let by_prefix = super::work::github_ref(key).is_none() && !key.starts_with("asana:");
    claims.contains(&tracker_id) && (!by_prefix || claims.len() == 1)
}

impl Store {
    fn tracker_item_id(&self, tracker_id: i64, external_id: &str) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM work_items WHERE tracker_id = ?1 AND external_id = ?2",
                rusqlite::params![tracker_id, external_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn visible(&self, id: i64) -> Result<(Visible, ItemMeta), IpcError> {
        Ok(self.conn.query_row(
            "SELECT key, aliases, title, url, kind, hierarchy_level, status_name, status_category, \
                    resolution, parent_id, containers, assignees, iteration, updated_ext, \
                    unavailable_at IS NOT NULL, meta \
             FROM work_items WHERE id = ?1",
            rusqlite::params![id],
            |r| {
                let meta: Option<String> = r.get(15)?;
                Ok((
                    Visible {
                        key: r.get(0)?,
                        aliases: r.get(1)?,
                        title: r.get(2)?,
                        url: r.get(3)?,
                        kind: r.get(4)?,
                        hierarchy_level: r.get(5)?,
                        status_name: r.get(6)?,
                        status_category: r.get(7)?,
                        resolution: r.get(8)?,
                        parent_id: r.get(9)?,
                        containers: r.get(10)?,
                        assignees: r.get(11)?,
                        iteration: r.get(12)?,
                        updated_ext: r.get(13)?,
                        unavailable: r.get(14)?,
                        meta: meta.clone(),
                    },
                    ItemMeta::parse(meta.as_deref()),
                ))
            },
        )?)
    }

    /// Insert or update one tracker item by `(tracker_id, external_id)`.
    /// Emits `work:item` (and `session:updated` for the live sessions whose
    /// primary work it is) only when something a reader sees changed.
    pub fn upsert_tracker_item(
        &self,
        tracker_id: i64,
        w: &TrackerItemWrite,
    ) -> Result<UpsertOutcome, IpcError> {
        if w.external_id.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a tracker item needs an id",
            ));
        }
        let now = now_unix();
        let key = w.key.as_deref().map(super::work::canonical_key);
        let parent_id = match &w.parent_external_id {
            Some(p) => self.tracker_item_id(tracker_id, p)?,
            None => None,
        };
        let existing = self.tracker_item_id(tracker_id, &w.external_id)?;
        let before = existing.map(|id| self.visible(id)).transpose()?;

        // Aliases: the provider's, every one already known, and the old key
        // when the key moved (C24). Never the current key itself.
        let mut aliases: BTreeSet<String> = w
            .aliases
            .iter()
            .map(|a| super::work::canonical_key(a))
            .collect();
        if let Some((v, _)) = &before {
            if let Some(old) = &v.aliases {
                aliases.extend(serde_json::from_str::<Vec<String>>(old).unwrap_or_default());
            }
            if let Some(old_key) = &v.key {
                aliases.insert(old_key.clone());
            }
        }
        if let Some(k) = &key {
            aliases.remove(k);
        }
        let aliases: Vec<String> = aliases.into_iter().collect();
        let mut meta = before.as_ref().map(|(_, m)| m.clone()).unwrap_or_default();
        meta.description = w.description.clone();
        meta.assignee_id = w.assignee_id.clone();
        meta.iteration_active = w.iteration_active;
        let meta_json = serde_json::to_string(&meta).ok();
        let after = Visible {
            key: key.clone(),
            aliases: json_list(&aliases),
            title: w.title.clone(),
            url: w.url.clone(),
            kind: w.kind.clone(),
            hierarchy_level: w.hierarchy_level,
            status_name: Some(w.status_name.clone()),
            status_category: w.status_category.clone(),
            resolution: w.resolution.clone(),
            // An unresolved parent (not cached yet) keeps the one known.
            parent_id: parent_id.or_else(|| {
                w.parent_external_id
                    .as_ref()
                    .and(before.as_ref().and_then(|(v, _)| v.parent_id))
            }),
            containers: json_list(&w.containers),
            assignees: json_list(&w.assignees),
            iteration: w.iteration.clone(),
            updated_ext: w.updated_ext,
            unavailable: false,
            meta: meta_json.clone(),
        };

        let (id, changed, status_change, session_change) = match (existing, before) {
            (Some(id), Some((old, _))) => {
                // `meta` holds the description too, which a reader of a
                // lookup sees; it counts.
                let changed = old != after;
                let session_change = if changed {
                    old.session_change(&after)
                } else {
                    SessionChange::default()
                };
                let status_moved = old.status_category != after.status_category
                    || old.status_name != after.status_name;
                if changed {
                    self.conn.execute(
                        "UPDATE work_items SET key = ?1, aliases = ?2, title = ?3, url = ?4, \
                           kind = ?5, hierarchy_level = ?6, status_name = ?7, status_category = ?8, \
                           resolution = ?9, parent_id = ?10, containers = ?11, assignees = ?12, \
                           iteration = ?13, updated_ext = ?14, meta = ?15, fetched_at = ?16, \
                           updated_at = ?16, unavailable_at = NULL, unavailable_reason = NULL, \
                           status_changed_at = CASE WHEN ?17 THEN ?16 ELSE status_changed_at END \
                         WHERE id = ?18",
                        rusqlite::params![
                            after.key,
                            after.aliases,
                            after.title,
                            after.url,
                            after.kind,
                            after.hierarchy_level,
                            after.status_name,
                            after.status_category,
                            after.resolution,
                            after.parent_id,
                            after.containers,
                            after.assignees,
                            after.iteration,
                            after.updated_ext,
                            after.meta,
                            now,
                            status_moved,
                            id
                        ],
                    )?;
                } else {
                    self.conn.execute(
                        "UPDATE work_items SET fetched_at = ?1 WHERE id = ?2",
                        rusqlite::params![now, id],
                    )?;
                }
                // Work graph M7: a transition OUT of done is a reopen (an
                // event, recorded once); done again settles it.
                let reopened = changed
                    && old.status_category == "done"
                    && matches!(after.status_category.as_str(), "todo" | "in_progress");
                if changed && after.status_category == "done" {
                    self.conn.execute(
                        "UPDATE work_items SET reopened_at = NULL WHERE id = ?1",
                        rusqlite::params![id],
                    )?;
                }
                if reopened {
                    self.record_reopened(
                        id,
                        after.key.as_deref(),
                        old.status_name.as_deref().unwrap_or("done"),
                        after
                            .status_name
                            .as_deref()
                            .unwrap_or(&after.status_category),
                    )?;
                }
                let status_change = (changed && status_moved).then(|| {
                    (
                        old.status_name.unwrap_or_default(),
                        after.status_name.clone().unwrap_or_default(),
                    )
                });
                (id, changed, status_change, session_change)
            }
            _ => {
                self.conn.execute(
                    "INSERT INTO work_items (source, tracker_id, external_id, key, aliases, title, \
                       url, kind, hierarchy_level, status_name, status_category, resolution, \
                       parent_id, containers, assignees, iteration, updated_ext, meta, fetched_at, \
                       status_changed_at, created_at, updated_at) \
                     VALUES ((SELECT provider FROM trackers WHERE id = ?1), ?1, ?2, ?3, ?4, ?5, ?6, \
                       ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18, ?18, ?18)",
                    rusqlite::params![
                        tracker_id,
                        w.external_id,
                        after.key,
                        after.aliases,
                        after.title,
                        after.url,
                        after.kind,
                        after.hierarchy_level,
                        after.status_name,
                        after.status_category,
                        after.resolution,
                        after.parent_id,
                        after.containers,
                        after.assignees,
                        after.iteration,
                        after.updated_ext,
                        after.meta,
                        now
                    ],
                )?;
                (
                    self.conn.last_insert_rowid(),
                    true,
                    None,
                    SessionChange::ALL,
                )
            }
        };
        if changed {
            self.emit_work_item(id, session_change)?;
        }
        Ok(UpsertOutcome {
            id,
            changed,
            status_change,
        })
    }

    /// Emit `work:item`, then `session:updated` for every live session
    /// whose row shows this item where `change` says it moved: as its
    /// primary work (`work`: key, title, status, url, availability), as its
    /// top suggestion (`work_suggested`: the same minus availability), or
    /// as a rejected key (`work_rejected`).
    ///
    /// Nothing for the sessions when only what no session row shows moved
    /// (description, assignee, `updated`: a comment bumps it): that frame
    /// would repeat the row with only `row_version` changed, and it was most
    /// of the session frames a sync sent (work graph M10.6, measured in
    /// `docs/superpowers/reviews/2026-09-25-replay-ring-pressure.md`).
    fn emit_work_item(&self, id: i64, change: SessionChange) -> Result<(), IpcError> {
        if let Some(row) = self.get_work_item(id)? {
            self.bus
                .emit(&crate::events::RowChange::WorkItemUpdated(row));
        }
        if !change.any() {
            return Ok(());
        }
        // Every live session with any live link to the item, and whether
        // one of those links is a rejection.
        let candidates: Vec<(i64, bool)> = {
            let mut stmt = self.conn.prepare(
                "SELECT p.session_id, MAX(l.state = 'rejected') FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.item_id = ?1 AND l.ended_at IS NULL AND p.session_id IS NOT NULL \
                 GROUP BY p.session_id ORDER BY p.session_id",
            )?;
            let rows = stmt.query_map(rusqlite::params![id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (sid, rejected) in candidates {
            let Some(row) = self.get_session_by_id(sid)? else {
                continue;
            };
            let on =
                |w: &Option<super::WorkSummary>| w.as_ref().and_then(|w| w.item_id) == Some(id);
            let shows = (change.primary && on(&row.work))
                || (change.suggested && on(&row.work_suggested))
                || (change.rejected && rejected);
            if !shows {
                continue;
            }
            self.conn.execute(
                "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
                rusqlite::params![sid],
            )?;
            self.emit_session(sid)?;
        }
        Ok(())
    }

    /// Mark one tracker item unavailable (deleted, or no longer visible —
    /// the tracker cannot say which). `true` when it was not already.
    pub fn mark_tracker_item_unavailable(
        &self,
        tracker_id: i64,
        external_id: &str,
        reason: &str,
    ) -> Result<bool, IpcError> {
        // Only the stamp shows on a session row (its primary work), not the
        // reason; a suggestion does not show it at all.
        let was_available: bool = self
            .conn
            .query_row(
                "SELECT unavailable_at IS NULL FROM work_items \
                 WHERE tracker_id = ?1 AND external_id = ?2",
                rusqlite::params![tracker_id, external_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let n = self.conn.execute(
            "UPDATE work_items SET unavailable_at = ?1, unavailable_reason = ?2, updated_at = ?1 \
             WHERE tracker_id = ?3 AND external_id = ?4 \
               AND (unavailable_at IS NULL OR unavailable_reason IS NOT ?2)",
            rusqlite::params![now_unix(), reason, tracker_id, external_id],
        )?;
        if n > 0 {
            if let Some(id) = self.tracker_item_id(tracker_id, external_id)? {
                let change = SessionChange {
                    primary: was_available,
                    ..SessionChange::default()
                };
                self.emit_work_item(id, change)?;
            }
        }
        Ok(n > 0)
    }

    /// External ids of this tracker's items that any link references (live
    /// or ended), newest link first, at most `limit`: the by-id refresh set.
    pub fn linked_tracker_item_ids(
        &self,
        tracker_id: i64,
        limit: i64,
    ) -> Result<Vec<String>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT i.external_id FROM work_items i \
             JOIN work_links l ON l.item_id = i.id \
             WHERE i.tracker_id = ?1 AND i.external_id IS NOT NULL \
             GROUP BY i.id \
             ORDER BY MAX(l.ended_at IS NULL) DESC, MAX(COALESCE(l.decided_at, l.created_at)) DESC \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![tracker_id, limit], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Bare `ref_key`s (links with no item) this tracker may answer and
    /// that no item of it carries yet: the keys a sync fetches so they can be
    /// bound. Newest link first, at most `limit`. See [`tracker_claims`] for
    /// which tracker may answer which reference.
    pub fn unbound_ref_keys(&self, tracker_id: i64, limit: usize) -> Result<Vec<String>, IpcError> {
        let trackers = self.list_trackers()?;
        let mut stmt = self.conn.prepare(
            "SELECT ref_key FROM work_links WHERE item_id IS NULL AND ref_key IS NOT NULL \
             GROUP BY ref_key ORDER BY MAX(COALESCE(decided_at, created_at)) DESC",
        )?;
        let keys: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::new();
        for k in keys {
            // Work graph M5: a tracker fetches a key only for a session its
            // org may bind to — never on behalf of another org's session,
            // which would make one company's credentials answer (and so
            // reveal) keys another company's sessions merely mentioned.
            if may_answer(&trackers, tracker_id, &k) && self.ref_bindable_by(tracker_id, &k)? {
                out.push(k);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// May `tracker_id`'s items bind the bare link `link`? Not when the
    /// link's session is in one org and the tracker in another.
    fn link_bindable_to(&self, link: i64, tracker_id: i64) -> Result<bool, IpcError> {
        let Some(l) = self.get_work_link(link)? else {
            return Ok(false);
        };
        let tracker_org: Option<i64> = self
            .conn
            .query_row(
                "SELECT org_id FROM trackers WHERE id = ?1",
                rusqlite::params![tracker_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let link_org = self.link_org(&l)?;
        Ok(!matches!((tracker_org, link_org), (Some(t), Some(o)) if t != o))
    }

    /// Some bare link to `key` may bind to `tracker_id`'s items.
    fn ref_bindable_by(&self, tracker_id: i64, key: &str) -> Result<bool, IpcError> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM work_links WHERE item_id IS NULL AND ref_key = ?1")?;
            let rows = stmt.query_map(rusqlite::params![key], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in ids {
            if self.link_bindable_to(id, tracker_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Bind every bare `ref_key` link this tracker can answer (see the
    /// module docs). Returns the live sessions whose row changed; each gets
    /// `session:updated`.
    ///
    /// A bare link whose session is in another org than the tracker stays
    /// bare (work graph M5): binding it would make a cross-org link nobody
    /// asked for.
    pub fn bind_tracker_refs(&self, tracker_id: i64) -> Result<Vec<i64>, IpcError> {
        let trackers = self.list_trackers()?;
        let candidates: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, ref_key FROM work_links WHERE item_id IS NULL AND ref_key IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut touched = Vec::new();
        let tx = self.conn.unchecked_transaction()?;
        for (link, key) in candidates {
            if !may_answer(&trackers, tracker_id, &key)
                || !self.link_bindable_to(link, tracker_id)?
            {
                continue;
            }
            let key = super::work::canonical_key(&key);
            let item: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM work_items WHERE tracker_id = ?1 AND (key = ?2 OR EXISTS \
                       (SELECT 1 FROM json_each(COALESCE(aliases, '[]')) WHERE value = ?2)) \
                     ORDER BY (key = ?2) DESC, id LIMIT 1",
                    rusqlite::params![tracker_id, key],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(item) = item else { continue };
            self.conn.execute(
                "UPDATE work_links SET item_id = ?1 WHERE id = ?2",
                rusqlite::params![item, link],
            )?;
            let sid: Option<i64> = self
                .conn
                .query_row(
                    "SELECT p.session_id FROM work_links l JOIN participants p \
                       ON p.id = l.participant_id AND p.retired_at IS NULL \
                     WHERE l.id = ?1 AND l.ended_at IS NULL",
                    rusqlite::params![link],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            if let Some(sid) = sid {
                self.conn.execute(
                    "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
                    rusqlite::params![sid],
                )?;
                if !touched.contains(&sid) {
                    touched.push(sid);
                }
            }
        }
        tx.commit()?;
        for sid in &touched {
            self.emit_session(*sid)?;
        }
        Ok(touched)
    }

    /// The one tracker item `key` names (its key or an alias) when exactly
    /// one tracker has it; `None` when none does or two do (never guess).
    pub fn tracker_item_for_key(&self, key: &str) -> Result<Option<WorkItemRow>, IpcError> {
        let key = super::normalize_work_ref(key)?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items WHERE tracker_id IS NOT NULL AND \
               (key = ?1 OR EXISTS (SELECT 1 FROM json_each(COALESCE(aliases, '[]')) \
                                    WHERE value = ?1)) \
             ORDER BY (key = ?1) DESC, id"
        ))?;
        let rows: Vec<WorkItemRow> = stmt
            .query_map(rusqlite::params![key], map_item)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let trackers: BTreeSet<Option<i64>> = rows.iter().map(|r| r.tracker_id).collect();
        Ok(if trackers.len() == 1 {
            rows.into_iter().next()
        } else {
            None
        })
    }

    /// Record which items a favourite-filter view returned. A full listing
    /// (`full`) makes the membership exactly `external_ids`; an incremental
    /// one only adds.
    pub fn set_view_members(
        &self,
        tracker_id: i64,
        view_id: &str,
        external_ids: &[String],
        full: bool,
    ) -> Result<(), IpcError> {
        let rows: Vec<(i64, String, Option<String>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, external_id, meta FROM work_items \
                 WHERE tracker_id = ?1 AND external_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(rusqlite::params![tracker_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let tx = self.conn.unchecked_transaction()?;
        for (id, ext, raw) in rows {
            let mut meta = ItemMeta::parse(raw.as_deref());
            let member = external_ids.contains(&ext);
            let has = meta.views.iter().any(|v| v == view_id);
            let next = match (member, has, full) {
                (true, false, _) => {
                    meta.views.push(view_id.to_string());
                    true
                }
                (false, true, true) => {
                    meta.views.retain(|v| v != view_id);
                    true
                }
                _ => false,
            };
            if next {
                self.conn.execute(
                    "UPDATE work_items SET meta = ?1 WHERE id = ?2",
                    rusqlite::params![serde_json::to_string(&meta).ok(), id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every tracker item (of `tracker_id`, or of every tracker), with its
    /// meta, newest `updated` first.
    pub fn tracker_items(
        &self,
        tracker_id: Option<i64>,
    ) -> Result<Vec<(WorkItemRow, ItemMeta)>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS}, meta FROM work_items \
             WHERE tracker_id IS NOT NULL AND (?1 IS NULL OR tracker_id = ?1) \
             ORDER BY COALESCE(updated_ext, 0) DESC, id DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![tracker_id], |r| {
            let meta: Option<String> = r.get(23)?;
            Ok((map_item(r)?, ItemMeta::parse(meta.as_deref())))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Items linked to sessions on `host`: a live link whose session runs
    /// there, or an ended link that ran there. The per-host token's fence
    /// (M3 plan, decision 6).
    pub fn work_item_ids_on_host(&self, host: &str) -> Result<Vec<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.item_id FROM work_links l \
             LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             LEFT JOIN sessions s ON s.id = p.session_id \
             WHERE l.item_id IS NOT NULL AND l.state = 'confirmed' AND \
               ((l.ended_at IS NULL AND s.host_alias = ?1) OR \
                (l.ended_at IS NOT NULL AND l.snap_host = ?1))",
        )?;
        let rows = stmt.query_map(rusqlite::params![host], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Where work on `prefix`-keys last ran: `(project_id, host)` of the
    /// newest confirmed link, live or ended.
    pub fn last_place_for_prefix(&self, prefix: &str) -> Result<Option<(i64, String)>, IpcError> {
        if prefix.is_empty() {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(s.project_id, l.snap_project_id), COALESCE(s.host_alias, l.snap_host) \
                 FROM work_links l \
                 LEFT JOIN work_items i ON i.id = l.item_id \
                 LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 LEFT JOIN sessions s ON s.id = p.session_id AND l.ended_at IS NULL \
                 WHERE l.state = 'confirmed' \
                   AND UPPER(COALESCE(i.key, l.ref_key)) LIKE ?1 || '-%' \
                   AND COALESCE(s.project_id, l.snap_project_id) IS NOT NULL \
                   AND COALESCE(s.host_alias, l.snap_host) IS NOT NULL \
                 ORDER BY COALESCE(l.ended_at, l.decided_at, l.created_at) DESC, l.id DESC \
                 LIMIT 1",
                rusqlite::params![prefix.to_ascii_uppercase()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// The project whose repository is `repo` (`owner/repo`, any case):
    /// where a GitHub issue's work starts by default.
    pub fn project_for_repo(&self, repo: &str) -> Result<Option<i64>, IpcError> {
        let Some((owner, name)) = repo.split_once('/') else {
            return Ok(None);
        };
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM projects WHERE LOWER(owner) = LOWER(?1) AND LOWER(repo) = LOWER(?2) \
                 ORDER BY id LIMIT 1",
                rusqlite::params![owner, name],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The host that most recently ran a session of `project_id`.
    pub fn last_host_for_project(&self, project_id: i64) -> Result<Option<String>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT host_alias FROM sessions WHERE project_id = ?1 AND host_alias <> 'local' \
                 ORDER BY COALESCE(last_turn_at, started_at, 0) DESC, id DESC LIMIT 1",
                rusqlite::params![project_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// An item's `meta`, for the reads that use its description or view
    /// membership.
    pub fn work_item_meta(&self, id: i64) -> Result<ItemMeta, IpcError> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT meta FROM work_items WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(ItemMeta::parse(raw.as_deref()))
    }

    /// Journal a status move of `item_id` on the current conversation of
    /// every live session working on it (the journal is keyed by
    /// conversation, so the move shows in that work's history).
    pub fn journal_status_change(
        &self,
        item_id: i64,
        key: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<usize, IpcError> {
        let targets: Vec<(String, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT s.claude_session_id, p.id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 JOIN sessions s ON s.id = p.session_id \
                 WHERE l.item_id = ?1 AND l.ended_at IS NULL AND l.state = 'confirmed' \
                   AND s.claude_session_id IS NOT NULL",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![item_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let body = format!("{}: {from} → {to}", key.unwrap_or("item"));
        let meta = serde_json::json!({ "item_id": item_id, "from": from, "to": to }).to_string();
        let mut n = 0;
        for (claude, participant) in targets {
            if self
                .append_journal(
                    Some(&claude),
                    Some(participant),
                    "status_change",
                    "fleet",
                    Some(&body),
                    Some(&meta),
                )?
                .is_some()
            {
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RecordingEventBus;
    use crate::store::WorkTarget;
    use std::sync::Arc;

    fn write(ext: &str, key: &str, status: (&str, &str)) -> TrackerItemWrite {
        TrackerItemWrite {
            external_id: ext.into(),
            key: Some(key.into()),
            title: format!("{key} title"),
            status_name: status.0.into(),
            status_category: status.1.into(),
            updated_ext: Some(100),
            ..Default::default()
        }
    }

    fn with_tracker(prefixes: &[&str]) -> (Store, i64, Arc<RecordingEventBus>) {
        let bus = Arc::new(RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        s.set_tracker_probe(
            t.id,
            None,
            &crate::store::TrackerConfig {
                key_prefixes: prefixes.iter().map(|p| p.to_string()).collect(),
                ..Default::default()
            },
        )
        .unwrap();
        bus.take();
        (s, t.id, bus)
    }

    #[test]
    fn an_unchanged_item_writes_no_event_and_a_change_writes_one() {
        let (s, t, bus) = with_tracker(&["ABC"]);
        let first = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        assert!(first.changed);
        assert_eq!(bus.names(), vec!["work:item"]);
        bus.take();
        let again = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        assert_eq!((again.id, again.changed), (first.id, false));
        assert!(bus.names().is_empty(), "no event on an unchanged pass");
        let moved = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("In Progress", "in_progress")))
            .unwrap();
        assert!(moved.changed);
        assert_eq!(
            moved.status_change,
            Some(("To Do".into(), "In Progress".into()))
        );
        let row = s.get_work_item(first.id).unwrap().unwrap();
        assert_eq!(row.status_category, "in_progress");
        assert!(row.status_changed_at.is_some());
        assert_eq!(row.source, "jira");
    }

    #[test]
    fn a_moved_key_becomes_an_alias_and_identity_stays() {
        let (s, t, _) = with_tracker(&["ABC", "NEW"]);
        let a = s
            .upsert_tracker_item(t, &write("5", "ABC-5", ("To Do", "todo")))
            .unwrap();
        let b = s
            .upsert_tracker_item(t, &write("5", "NEW-9", ("To Do", "todo")))
            .unwrap();
        assert_eq!(a.id, b.id);
        let row = s.get_work_item(a.id).unwrap().unwrap();
        assert_eq!(row.key.as_deref(), Some("NEW-9"));
        assert_eq!(row.aliases, vec!["ABC-5"]);
        assert_eq!(s.tracker_item_for_key("abc-5").unwrap().unwrap().id, a.id);
    }

    #[test]
    fn unavailable_is_recorded_once_and_cleared_by_a_sighting() {
        let (s, t, bus) = with_tracker(&["ABC"]);
        let a = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        bus.take();
        assert!(s
            .mark_tracker_item_unavailable(t, "1", "not_found_or_no_permission")
            .unwrap());
        assert!(!s
            .mark_tracker_item_unavailable(t, "1", "not_found_or_no_permission")
            .unwrap());
        assert_eq!(bus.names(), vec!["work:item"]);
        let row = s.get_work_item(a.id).unwrap().unwrap();
        assert_eq!(
            row.unavailable_reason.as_deref(),
            Some("not_found_or_no_permission")
        );
        let back = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        assert!(back.changed, "coming back is a change");
        assert!(s
            .get_work_item(a.id)
            .unwrap()
            .unwrap()
            .unavailable_at
            .is_none());
    }

    fn session(s: &Store, name: &str) -> i64 {
        s.upsert_host("h").unwrap();
        s.upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap()
    }

    #[test]
    fn keys_typed_before_the_tracker_bind_on_their_own_and_the_row_follows() {
        let (s, t, bus) = with_tracker(&["ABC"]);
        let sid = session(&s, "dev");
        s.link_session_work(sid, WorkTarget::Key("abc-7"), "manual")
            .unwrap();
        // An ended link to the same key (past work) binds too.
        let other = session(&s, "old");
        s.link_session_work(other, WorkTarget::Key("ABC-7"), "manual")
            .unwrap();
        s.conn
            .execute(
                "UPDATE participants SET retired_at = 5 WHERE session_id = ?1",
                [other],
            )
            .unwrap();
        assert_eq!(s.unbound_ref_keys(t, 10).unwrap(), vec!["ABC-7"]);
        let item = s
            .upsert_tracker_item(t, &write("70", "ABC-7", ("Doing", "in_progress")))
            .unwrap();
        bus.take();
        let touched = s.bind_tracker_refs(t).unwrap();
        assert_eq!(touched, vec![sid], "only the live session's row changes");
        assert_eq!(bus.names(), vec!["session:updated"]);
        let row = s.get_session_by_id(sid).unwrap().unwrap();
        let w = row.work.unwrap();
        assert_eq!(w.item_id, Some(item.id));
        assert_eq!(w.title, "ABC-7 title");
        assert_eq!(w.status_category.as_deref(), Some("in_progress"));
        assert!(s.unbound_ref_keys(t, 10).unwrap().is_empty());
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM work_links WHERE item_id = ?1 AND ref_key = 'ABC-7'",
                [item.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "both links bound, ref_key kept for history");
        // A later status change reaches the session row.
        bus.take();
        s.upsert_tracker_item(t, &write("70", "ABC-7", ("Done", "done")))
            .unwrap();
        assert_eq!(bus.names(), vec!["work:item", "session:updated"]);
        // What the session row does not show (description, assignee, a
        // comment's `updated`) moves the item only: no session frame, no
        // `row_version` bump (M10.6).
        let version = s.get_session_by_id(sid).unwrap().unwrap().row_version;
        bus.take();
        let mut quiet = write("70", "ABC-7", ("Done", "done"));
        quiet.description = Some("New acceptance criteria".into());
        quiet.assignees = vec!["Dev B".into()];
        quiet.updated_ext = Some(200);
        assert!(s.upsert_tracker_item(t, &quiet).unwrap().changed);
        assert_eq!(bus.names(), vec!["work:item"]);
        let row = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.row_version, version);
        // Unavailable shows on the row once; a new reason does not.
        bus.take();
        assert!(s.mark_tracker_item_unavailable(t, "70", "gone").unwrap());
        assert_eq!(bus.names(), vec!["work:item", "session:updated"]);
        assert!(
            s.get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work
                .unwrap()
                .unavailable
        );
        bus.take();
        assert!(s.mark_tracker_item_unavailable(t, "70", "other").unwrap());
        assert_eq!(bus.names(), vec!["work:item"]);
        // Seen again: available, and the row follows.
        bus.take();
        s.upsert_tracker_item(t, &quiet).unwrap();
        assert_eq!(bus.names(), vec!["work:item", "session:updated"]);
        assert!(
            !s.get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work
                .unwrap()
                .unavailable
        );
    }

    #[test]
    fn a_suggested_or_rejected_item_reaches_the_rows_that_show_it() {
        let (s, t, bus) = with_tracker(&["ABC"]);
        let item = s
            .upsert_tracker_item(t, &write("80", "ABC-8", ("To Do", "todo")))
            .unwrap()
            .id;
        // `sug`: ABC-8 is its top suggestion. `rej`: it rejected ABC-8.
        // `other`: its primary work is another item.
        let sug = session(&s, "sug");
        let link = s
            .link_session_work(sug, WorkTarget::Item(item), "manual")
            .unwrap();
        s.conn
            .execute(
                "UPDATE work_links SET state = 'suggested', is_primary = 0 WHERE id = ?1",
                [link.id],
            )
            .unwrap();
        let rej = session(&s, "rej");
        s.reject_session_work(rej, WorkTarget::Item(item)).unwrap();
        let other = session(&s, "other");
        s.upsert_tracker_item(t, &write("90", "ABC-9", ("To Do", "todo")))
            .unwrap();
        s.link_session_work(other, WorkTarget::Key("ABC-9"), "manual")
            .unwrap();
        let suggested = |s: &Store| {
            s.get_session_by_id(sug)
                .unwrap()
                .unwrap()
                .work_suggested
                .unwrap()
        };
        assert_eq!(suggested(&s).item_id, Some(item));

        // A title and status move reaches the suggestion; a rejection shows
        // only the key, so it stays quiet.
        bus.take();
        let mut next = write("80", "ABC-8", ("In Progress", "in_progress"));
        next.title = "Renamed".into();
        s.upsert_tracker_item(t, &next).unwrap();
        assert_eq!(
            bus.take(),
            vec![
                format!("work:item:{item}"),
                format!("session:updated:{sug}")
            ]
        );
        let w = suggested(&s);
        assert_eq!(
            (w.title.as_str(), w.status_category.as_deref()),
            ("Renamed", Some("in_progress"))
        );

        // What no row shows: nothing but the item.
        let version = s.get_session_by_id(sug).unwrap().unwrap().row_version;
        let mut quiet = next.clone();
        quiet.description = Some("More".into());
        quiet.updated_ext = Some(300);
        s.upsert_tracker_item(t, &quiet).unwrap();
        assert_eq!(bus.names(), vec!["work:item"]);
        assert_eq!(
            s.get_session_by_id(sug).unwrap().unwrap().row_version,
            version
        );

        // Unavailable is not on a suggestion.
        bus.take();
        assert!(s.mark_tracker_item_unavailable(t, "80", "gone").unwrap());
        assert_eq!(bus.names(), vec!["work:item"]);

        // A moved key reaches both the suggestion and the rejection.
        bus.take();
        let mut moved = quiet.clone();
        moved.key = Some("ABC-80".into());
        s.upsert_tracker_item(t, &moved).unwrap();
        assert_eq!(
            bus.take(),
            vec![
                format!("work:item:{item}"),
                format!("session:updated:{sug}"),
                format!("session:updated:{rej}"),
            ]
        );
        assert_eq!(suggested(&s).key.as_deref(), Some("ABC-80"));
        assert_eq!(
            s.get_session_by_id(rej).unwrap().unwrap().work_rejected,
            vec!["ABC-80".to_string()]
        );
    }

    #[test]
    fn a_prefix_two_trackers_claim_is_never_bound() {
        let (s, t, _) = with_tracker(&["ABC"]);
        let t2 = s
            .add_tracker("jira", "Other", "https://other.atlassian.net")
            .unwrap()
            .id;
        s.set_tracker_probe(
            t2,
            None,
            &crate::store::TrackerConfig {
                key_prefixes: vec!["ABC".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let sid = session(&s, "dev");
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        assert!(s.unbound_ref_keys(t, 10).unwrap().is_empty());
        assert!(s.bind_tracker_refs(t).unwrap().is_empty());
        assert_eq!(
            s.get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work
                .unwrap()
                .item_id,
            None
        );
        // Two trackers with the same key: lookup never guesses.
        s.upsert_tracker_item(t2, &write("99", "ABC-1", ("To Do", "todo")))
            .unwrap();
        assert!(s.tracker_item_for_key("ABC-1").unwrap().is_none());
    }

    #[test]
    fn view_membership_is_exact_on_a_full_listing_and_additive_otherwise() {
        let (s, t, _) = with_tracker(&["ABC"]);
        let a = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        let b = s
            .upsert_tracker_item(t, &write("2", "ABC-2", ("To Do", "todo")))
            .unwrap();
        s.set_view_members(t, "filter:9", &["1".into()], true)
            .unwrap();
        s.set_view_members(t, "filter:9", &["2".into()], false)
            .unwrap();
        assert_eq!(s.work_item_meta(a.id).unwrap().views, vec!["filter:9"]);
        assert_eq!(s.work_item_meta(b.id).unwrap().views, vec!["filter:9"]);
        s.set_view_members(t, "filter:9", &["2".into()], true)
            .unwrap();
        assert!(s.work_item_meta(a.id).unwrap().views.is_empty());
    }

    #[test]
    fn a_status_move_is_journaled_on_the_live_conversation() {
        let (s, t, _) = with_tracker(&["ABC"]);
        let sid = session(&s, "dev");
        s.conn
            .execute(
                "UPDATE sessions SET claude_session_id = 'c-1' WHERE id = ?1",
                [sid],
            )
            .unwrap();
        let item = s
            .upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
            .unwrap();
        s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
            .unwrap();
        assert_eq!(
            s.journal_status_change(item.id, Some("ABC-1"), "To Do", "Done")
                .unwrap(),
            1
        );
        let body: String = s
            .conn
            .query_row(
                "SELECT body FROM work_journal WHERE kind = 'status_change'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(body, "ABC-1: To Do → Done");
    }
}
