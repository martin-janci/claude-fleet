//! The Today view's digest (work graph M9.1): `work { action: today, since }`.
//!
//! Live sessions grouped by their primary work into one of three buckets —
//! decided in this order, so a group is in exactly one:
//!
//! * **waiting**: some session needs a person (`attention::needs_attention`,
//!   the hub's one classifier — the phone and the desktop agree);
//! * **stale**: every session has been idle past [`STALE_AFTER_SECS`], or the
//!   ticket is done while a session still runs;
//! * **in_progress**: the rest.
//!
//! Sessions without work form one *no work* group per bucket. **shipped** is
//! what finished since `since`: linked tickets (or tickets assigned to the
//! tracker's account) that moved to done, and work links that ended with a
//! PR. There is no merged-PR signal in fleet, so this is the honest reading.
//!
//! The digest is built from rows, links and the tracker cache only — no
//! network, no journal bodies — under the caller's [`OrgScope`]: a per-host
//! token reads its own host's day inside its org, and nothing else. The
//! standup's words are the desktop's to build from what it shows.

use crate::ipc_error::{lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::store::{ItemMeta, SessionRow, Store, TrackerRow, WorkItemRow, WorkLinkRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

/// A session idle this long, with nobody waiting on it, is stale.
pub const STALE_AFTER_SECS: i64 = 3 * 86_400;
/// Without `since`: the last day.
pub const DEFAULT_WINDOW_SECS: i64 = 86_400;
/// At most this many groups and shipped entries, each.
pub const TODAY_MAX: usize = 200;

pub const BUCKET_WAITING: &str = "waiting";
pub const BUCKET_IN_PROGRESS: &str = "in_progress";
pub const BUCKET_STALE: &str = "stale";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Today {
    pub since: i64,
    pub now: i64,
    #[serde(default)]
    pub groups: Vec<TodayGroup>,
    #[serde(default)]
    pub shipped: Vec<TodayShipped>,
}

/// Live sessions sharing one piece of work (or none), in one bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodayGroup {
    /// waiting | in_progress | stale.
    pub bucket: String,
    /// `None`: the sessions with no work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default)]
    pub sessions: Vec<TodaySession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodaySession {
    pub id: i64,
    /// The friendly name, else the tmux name.
    pub name: String,
    pub host_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// waiting | stuck | failed | lifecycle, when a person is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<String>,
    /// idle | done, when this session is why its group is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_status: Option<String>,
    pub last_activity_at: i64,
}

/// Something that finished since `since`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodayShipped {
    /// done (a ticket moved to done) | pr (work ended with a PR).
    pub how: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
}

/// A ticket that moved to done, as [`digest`] reads it.
#[derive(Debug, Clone)]
pub struct DoneItem {
    pub item: WorkItemRow,
    pub org_id: Option<i64>,
}

/// An ended link with what it was about.
#[derive(Debug, Clone)]
pub struct EndedWork {
    pub link: WorkLinkRow,
    pub key: Option<String>,
    pub title: String,
    pub url: Option<String>,
}

/// Why a session is stale, if it is.
fn stale_reason(row: &SessionRow, now: i64) -> Option<&'static str> {
    if row.work.as_ref().and_then(|w| w.status_category.as_deref()) == Some("done") {
        Some("done")
    } else if row.last_activity_at < now - STALE_AFTER_SECS {
        Some("idle")
    } else {
        None
    }
}

fn session_of(row: &SessionRow, now: i64) -> TodaySession {
    TodaySession {
        id: row.id,
        name: row
            .friendly_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| row.tmux_name.clone()),
        host_alias: row.host_alias.clone(),
        org_id: row.org_id,
        attention: crate::service::attention::needs_attention(row)
            .map(|a| a.reason.as_str().into()),
        stale: stale_reason(row, now).map(str::to_string),
        claude_status: row.claude_status.clone(),
        pr_url: row.pr_url.clone(),
        ci_status: row.ci_status.clone(),
        last_activity_at: row.last_activity_at,
    }
}

/// PURE: the digest of `rows` (already scoped and redacted), `done` tickets
/// and `ended` links (both already scoped).
pub fn digest(
    rows: &[SessionRow],
    done: &[DoneItem],
    ended: &[EndedWork],
    now: i64,
    since: i64,
) -> Today {
    // Group by work key; no-work sessions by bucket.
    let mut by_key: BTreeMap<String, (TodayGroup, Vec<TodaySession>)> = BTreeMap::new();
    let mut no_work: Vec<TodaySession> = Vec::new();
    for row in rows {
        let s = session_of(row, now);
        match row.work.as_ref().and_then(|w| w.key.clone()) {
            Some(key) => {
                let w = row.work.as_ref().cloned().unwrap_or_default();
                by_key
                    .entry(key.clone())
                    .or_insert_with(|| {
                        (
                            TodayGroup {
                                key: Some(key),
                                title: w.title.clone(),
                                item_id: w.item_id,
                                status_category: w.status_category.clone(),
                                status_name: w.status_name.clone(),
                                url: w.url.clone(),
                                org_id: w.org_id,
                                ..Default::default()
                            },
                            Vec::new(),
                        )
                    })
                    .1
                    .push(s);
            }
            None => no_work.push(s),
        }
    }
    let bucket_of = |ss: &[TodaySession]| -> &'static str {
        if ss.iter().any(|s| s.attention.is_some()) {
            BUCKET_WAITING
        } else if !ss.is_empty() && ss.iter().all(|s| s.stale.is_some()) {
            BUCKET_STALE
        } else {
            BUCKET_IN_PROGRESS
        }
    };
    let mut groups: Vec<TodayGroup> = by_key
        .into_values()
        .map(|(mut g, sessions)| {
            g.bucket = bucket_of(&sessions).into();
            g.sessions = sessions;
            g
        })
        .collect();
    // The no-work sessions, one group per bucket they fall in by themselves.
    for bucket in [BUCKET_WAITING, BUCKET_IN_PROGRESS, BUCKET_STALE] {
        let sessions: Vec<TodaySession> = no_work
            .iter()
            .filter(|s| bucket_of(std::slice::from_ref(*s)) == bucket)
            .cloned()
            .collect();
        if !sessions.is_empty() {
            groups.push(TodayGroup {
                bucket: bucket.into(),
                sessions,
                ..Default::default()
            });
        }
    }
    for g in &mut groups {
        g.sessions.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then(a.id.cmp(&b.id))
        });
    }
    // Most recently active first; the no-work group last within its bucket.
    let latest = |g: &TodayGroup| g.sessions.iter().map(|s| s.last_activity_at).max();
    groups.sort_by(|a, b| {
        a.key
            .is_none()
            .cmp(&b.key.is_none())
            .then(latest(b).cmp(&latest(a)))
            .then(a.key.cmp(&b.key))
    });
    groups.truncate(TODAY_MAX);

    // Shipped: done tickets first-class, then PRs of ended work whose key
    // did not already ship as done.
    let mut shipped: Vec<TodayShipped> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for d in done {
        let at = d.item.status_changed_at.unwrap_or(d.item.updated_at);
        if d.item.status_category != "done" || at < since {
            continue;
        }
        if let Some(k) = &d.item.key {
            if !seen.insert(k.clone()) {
                continue;
            }
        }
        shipped.push(TodayShipped {
            how: "done".into(),
            key: d.item.key.clone(),
            title: d.item.title.clone(),
            url: d.item.url.clone(),
            pr_url: None,
            at,
            org_id: d.org_id,
        });
    }
    for e in ended {
        let (Some(at), Some(pr)) = (e.link.ended_at, e.link.snap_pr_url.clone()) else {
            continue;
        };
        if at < since {
            continue;
        }
        let dedup = e.key.clone().unwrap_or_else(|| pr.clone());
        if !seen.insert(dedup) {
            // A done ticket that also has a PR: keep the PR on it.
            if let Some(s) = shipped
                .iter_mut()
                .find(|s| s.key.is_some() && s.key == e.key && s.pr_url.is_none())
            {
                s.pr_url = Some(pr);
            }
            continue;
        }
        shipped.push(TodayShipped {
            how: "pr".into(),
            key: e.key.clone(),
            title: e.title.clone(),
            url: e.url.clone(),
            pr_url: Some(pr),
            at,
            org_id: e.link.org_id,
        });
    }
    shipped.sort_by(|a, b| b.at.cmp(&a.at).then(a.key.cmp(&b.key)));
    shipped.truncate(TODAY_MAX);
    Today {
        since,
        now,
        groups,
        shipped,
    }
}

/// Sessions the Today view is about: the fleet's Claude sessions, not
/// shells, external Claudes or the operator.
fn counts(row: &SessionRow, operator: Option<&crate::service::operator::OperatorRef>) -> bool {
    !matches!(row.kind.as_str(), "shell" | "external")
        && !operator.is_some_and(|o| o.host_alias == row.host_alias && o.tmux_name == row.tmux_name)
}

fn mine(meta: &ItemMeta, tracker: Option<&TrackerRow>) -> bool {
    tracker
        .and_then(|t| t.config.account_id.as_deref())
        .is_some_and(|me| meta.assignee_id.as_deref() == Some(me))
}

/// `work { action: today, since? }`.
pub fn today(
    store: &Mutex<Store>,
    since: Option<i64>,
    scope: &OrgScope,
) -> Result<Today, IpcError> {
    let now = crate::service::catalog::now_secs();
    let since = since.unwrap_or(now - DEFAULT_WINDOW_SECS).min(now);
    let s = lock(store)?;
    let operator = crate::service::operator::operator_ref(&s);

    // Live rows: a per-host token reads its own host's, redacted to its org.
    let mut rows: Vec<SessionRow> = s
        .list_all_sessions()?
        .into_iter()
        .filter(|r| counts(r, operator.as_ref()))
        .filter(|r| match scope.host() {
            Some(h) => r.host_alias == h && scope.sees_row(r),
            None => true,
        })
        .collect();
    for r in &mut rows {
        scope.redact_row(r);
    }

    // Tickets that moved to done since `since`: linked to work, or mine.
    let linked = s.linked_work_item_ids()?;
    let allowed = crate::service::trackers::tickets::allowed(scope, &s)?;
    let trackers = s.list_trackers()?;
    let mut done = Vec::new();
    for (item, meta) in s.tracker_items(None)? {
        if item.status_category != "done"
            || item.status_changed_at.unwrap_or(item.updated_at) < since
        {
            continue;
        }
        if allowed.as_ref().is_some_and(|a| !a.contains(&item.id)) {
            continue;
        }
        let t = trackers.iter().find(|t| Some(t.id) == item.tracker_id);
        if !linked.contains(&item.id) && !mine(&meta, t) {
            continue;
        }
        let org_id = s.item_org(item.id)?;
        done.push(DoneItem { item, org_id });
    }

    // Work that ended since `since`, as the caller may read it.
    let mut links = s.recent_ended_work_links(since, TODAY_MAX as i64)?;
    orgs::scope_links(&s, scope, &mut links)?;
    let mut ended = Vec::new();
    for link in links {
        if link.role != "work" || link.snap_pr_url.is_none() {
            continue;
        }
        let item = link
            .item_id
            .map(|id| s.get_work_item(id))
            .transpose()?
            .flatten();
        ended.push(EndedWork {
            key: item
                .as_ref()
                .and_then(|i| i.key.clone())
                .or_else(|| link.ref_key.clone()),
            title: item.as_ref().map(|i| i.title.clone()).unwrap_or_default(),
            url: item.and_then(|i| i.url),
            link,
        });
    }
    Ok(digest(&rows, &done, &ended, now, since))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WorkSummary;

    const NOW: i64 = 10_000_000;
    const SINCE: i64 = NOW - 3_600;

    fn row(id: i64, key: Option<&str>) -> SessionRow {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let sid = s
            .upsert_session(&format!("s{id}"), "h", None, None, 1, 1, "running", None)
            .unwrap();
        let mut r = s.get_session_by_id(sid).unwrap().unwrap();
        r.id = id;
        r.last_activity_at = NOW - 60;
        r.claude_status = Some("working".into());
        r.work = key.map(|k| WorkSummary {
            link_id: id,
            key: Some(k.into()),
            title: format!("{k} title"),
            source: "manual".into(),
            ..Default::default()
        });
        r
    }

    fn bucket_of(t: &Today, key: Option<&str>) -> Vec<String> {
        t.groups
            .iter()
            .filter(|g| g.key.as_deref() == key)
            .map(|g| g.bucket.clone())
            .collect()
    }

    #[test]
    fn a_group_is_in_one_bucket_decided_waiting_then_stale_then_in_progress() {
        let mut blocked = row(1, Some("PAY-7"));
        blocked.claude_status = Some("blocked".into());
        let working = row(2, Some("PAY-7"));
        let mut idle = row(3, Some("OLD-1"));
        idle.last_activity_at = NOW - STALE_AFTER_SECS - 1;
        idle.claude_status = Some("idle".into());
        let mut recent_idle = row(4, Some("OLD-1"));
        recent_idle.claude_status = Some("idle".into());
        let mut done = row(5, Some("DONE-1"));
        done.work.as_mut().unwrap().status_category = Some("done".into());
        let t = digest(
            &[blocked, working, idle, recent_idle, done],
            &[],
            &[],
            NOW,
            SINCE,
        );
        assert_eq!(bucket_of(&t, Some("PAY-7")), vec!["waiting"]);
        // One session is recent: the group is not stale.
        assert_eq!(bucket_of(&t, Some("OLD-1")), vec!["in_progress"]);
        assert_eq!(bucket_of(&t, Some("DONE-1")), vec!["stale"]);
        let pay = t
            .groups
            .iter()
            .find(|g| g.key.as_deref() == Some("PAY-7"))
            .unwrap();
        assert_eq!(pay.sessions.len(), 2);
        assert_eq!(
            pay.sessions
                .iter()
                .filter(|s| s.attention.is_some())
                .count(),
            1
        );
        assert_eq!(pay.title, "PAY-7 title");
    }

    #[test]
    fn sessions_without_work_form_one_group_per_bucket_after_the_work() {
        let mut a = row(1, None);
        a.claude_status = Some("blocked".into());
        let b = row(2, None);
        let mut c = row(3, None);
        c.last_activity_at = NOW - STALE_AFTER_SECS - 10;
        let d = row(4, Some("K-1"));
        let t = digest(&[a, b, c, d], &[], &[], NOW, SINCE);
        assert_eq!(
            bucket_of(&t, None),
            vec!["waiting", "in_progress", "stale"],
            "{t:#?}"
        );
        assert_eq!(t.groups[0].key.as_deref(), Some("K-1"), "work first");
        assert!(t.groups.iter().all(|g| g.sessions.len() == 1));
    }

    fn item(key: &str, cat: &str, changed: i64) -> DoneItem {
        DoneItem {
            item: WorkItemRow {
                id: 1,
                source: "jira".into(),
                key: Some(key.into()),
                title: format!("{key} title"),
                url: Some(format!("https://x.atlassian.net/browse/{key}")),
                status_category: cat.into(),
                created_at: 1,
                updated_at: 1,
                tracker_id: Some(1),
                external_id: None,
                aliases: vec![],
                kind: None,
                hierarchy_level: None,
                status_name: None,
                resolution: None,
                parent_id: None,
                assignees: vec![],
                iteration: None,
                updated_ext: None,
                status_changed_at: Some(changed),
                fetched_at: None,
                unavailable_at: None,
                unavailable_reason: None,
            },
            org_id: None,
        }
    }

    fn ended(key: Option<&str>, pr: Option<&str>, at: i64) -> EndedWork {
        EndedWork {
            link: WorkLinkRow {
                id: 9,
                item_id: None,
                ref_key: key.map(str::to_string),
                participant_id: None,
                state: "confirmed".into(),
                source: "manual".into(),
                is_primary: true,
                created_at: 1,
                decided_at: None,
                ended_at: Some(at),
                snap_host: Some("h".into()),
                snap_tmux: None,
                snap_name: None,
                snap_project_id: None,
                snap_worktree: None,
                snap_branch: None,
                snap_pr_url: pr.map(str::to_string),
                snap_claude_ids: None,
                role: "work".into(),
                resumable: true,
                claude_session_id: None,
                strength: None,
                rule: None,
                evidence: vec![],
                preselected: false,
                end_reason: None,
                org_id: None,
            },
            key: key.map(str::to_string),
            title: String::new(),
            url: None,
        }
    }

    #[test]
    fn shipped_is_done_tickets_and_ended_prs_since_the_start_deduplicated() {
        let t = digest(
            &[],
            &[
                item("A-1", "done", NOW - 10),
                item("A-2", "done", SINCE - 1),
                item("A-3", "in_progress", NOW - 10),
            ],
            &[
                ended(Some("A-1"), Some("https://gh/pr/1"), NOW - 5),
                ended(Some("B-1"), Some("https://gh/pr/2"), NOW - 20),
                ended(Some("B-2"), Some("https://gh/pr/3"), SINCE - 5),
                ended(Some("B-3"), None, NOW - 5),
            ],
            NOW,
            SINCE,
        );
        let got: Vec<(&str, Option<&str>, Option<&str>)> = t
            .shipped
            .iter()
            .map(|s| (s.how.as_str(), s.key.as_deref(), s.pr_url.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("done", Some("A-1"), Some("https://gh/pr/1")),
                ("pr", Some("B-1"), Some("https://gh/pr/2")),
            ]
        );
    }

    fn store_with_day() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_host("h2").unwrap();
        let a = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("b", "h2", None, None, 1, 1, "running", None)
            .unwrap();
        s.link_session_work(a, crate::store::WorkTarget::Key("PAY-7"), "manual")
            .unwrap();
        (Mutex::new(s), a)
    }

    #[test]
    fn today_reads_the_store_and_a_host_scope_reads_only_its_host() {
        let (st, a) = store_with_day();
        let all = today(&st, None, &OrgScope::All).unwrap();
        let ids: Vec<i64> = all
            .groups
            .iter()
            .flat_map(|g| g.sessions.iter().map(|s| s.id))
            .collect();
        assert_eq!(ids.len(), 2, "{all:#?}");
        let pay = all
            .groups
            .iter()
            .find(|g| g.key.as_deref() == Some("PAY-7"))
            .expect("PAY-7 group");
        assert_eq!(pay.sessions[0].id, a);

        let host = OrgScope::for_host(&st.lock().unwrap(), "h").unwrap();
        let mine = today(&st, None, &host).unwrap();
        let hosts: Vec<&str> = mine
            .groups
            .iter()
            .flat_map(|g| g.sessions.iter().map(|s| s.host_alias.as_str()))
            .collect();
        assert_eq!(hosts, vec!["h"]);
        assert!(mine.since <= mine.now && mine.now - mine.since == DEFAULT_WINDOW_SECS);
    }

    #[test]
    fn a_future_since_is_clamped_to_now() {
        let (st, _) = store_with_day();
        let t = today(&st, Some(i64::MAX), &OrgScope::All).unwrap();
        assert_eq!(t.since, t.now);
    }
}
