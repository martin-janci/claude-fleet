//! The Work view's read model (work graph M14.1b), one test per use case of
//! the spec (`docs/superpowers/specs/2026-09-27-work-view-design.md`) where
//! the hub owns the behaviour. Some rows the reads answer are seeded
//! (`Store::seed_*`); the writes a person makes through the hub (M14.1c)
//! are `view_write_tests.rs`. The org boundary per caller is the isolation
//! matrix's (`mcp/tools/tests_isolation.rs`).

use super::*;
use crate::service::work::structure::{self, RuleInput};
use crate::service::work::{work_link, WorkLinkArgs};
use crate::store::{NativeItem, RuleConditions, TrackerConfig, TrackerItemWrite, WorkTarget};

struct W {
    st: Mutex<Store>,
    tracker: i64,
    org_a: i64,
    org_b: i64,
    s1: i64,
    s2: i64,
    t1: i64,
    t2: i64,
    t3: i64,
}

/// Multi-user M1 (T7): these reads take the caller's whole scope now. Every
/// case here is about the ORG half, so the person half is the hub's own
/// unrestricted reader — `ViewScope::internal()` — with the org under test
/// put back on it. The person half has its own tests (`view_scope_tests`, and
/// the behavioural matrix in `mcp::tools::tests`).
fn vs(scope: &OrgScope) -> crate::service::view_scope::ViewScope {
    crate::service::view_scope::ViewScope::internal().with_org(scope.clone())
}

fn item(s: &Store, tracker: i64, ext: &str, key: &str, title: &str, project: &str) -> i64 {
    s.upsert_tracker_item(
        tracker,
        &TrackerItemWrite {
            external_id: ext.into(),
            key: Some(key.into()),
            title: title.into(),
            status_name: "In Progress".into(),
            status_category: "in_progress".into(),
            containers: vec![project.into()],
            assignee_id: Some("me".into()),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

/// Org A owns tracker `T` (items TK-1, TK-2 in project TP, TK-3 with no
/// session); two sessions on h1 (org A).
fn world() -> W {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h1").unwrap();
    let a = s.add_org("Acme", None, false).unwrap();
    let b = s.add_org("Beta", None, false).unwrap();
    s.set_host_org("h1", Some(a.id)).unwrap();
    let t = s
        .add_tracker("jira", "Jira (acme)", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_org(t.id, Some(a.id)).unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["TK".into()],
            account_id: Some("me".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let t1 = item(&s, t.id, "1", "TK-1", "Login fails", "TP");
    let t2 = item(&s, t.id, "2", "TK-2", "Audit log", "TP");
    let t3 = item(&s, t.id, "3", "TK-3", "Nobody on it yet", "TP");
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let s1 = s
        .upsert_session("one", "h1", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    let s2 = s
        .upsert_session("two", "h1", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    W {
        st: Mutex::new(s),
        tracker: t.id,
        org_a: a.id,
        org_b: b.id,
        s1,
        s2,
        t1,
        t2,
        t3,
    }
}

fn page(w: &W, scope: &OrgScope, filters: WorkTreeFilters) -> TreePage {
    tree(
        &w.st,
        &vs(scope),
        &TreeArgs {
            filters,
            limit: Some(200),
            ..Default::default()
        },
    )
    .unwrap()
}

fn task_of<'a>(p: &'a TreePage, key: &str) -> &'a WorkTask {
    p.tasks
        .iter()
        .find(|t| t.key.as_deref() == Some(key))
        .unwrap_or_else(|| {
            panic!(
                "no task {key} in {:?}",
                p.tasks.iter().map(|t| &t.key).collect::<Vec<_>>()
            )
        })
}

fn wl(w: &W, action: &str, sid: i64) -> WorkLinkArgs {
    let _ = w;
    WorkLinkArgs {
        session_id: Some(sid),
        action: action.into(),
        ..Default::default()
    }
}

/// Link `sid` to `item`; `primary: false` makes it a secondary link
/// (M14.1c's `link { primary: false }`).
fn link(w: &W, sid: i64, item: i64, primary: bool) -> SessionRow {
    work_link(
        &WorkLinkArgs {
            item_id: Some(item),
            primary: Some(primary),
            ..wl(w, "link", sid)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap()
}

fn bound(org: i64) -> OrgScope {
    OrgScope::Org {
        org,
        sees_unassigned: true,
    }
}

fn strict(org: i64) -> OrgScope {
    OrgScope::Org {
        org,
        sees_unassigned: false,
    }
}

fn links_of(w: &W, sid: i64) -> SessionTasks {
    session_tasks(&w.st, &vs(&OrgScope::All), sid).unwrap()
}

/// UC1 + UC2: one session on tasks A and B shows under both, with ONE
/// session id; exactly one link is primary; the other is a full link.
#[test]
fn a_session_on_two_tasks_is_one_identity_under_both() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    let row = link(&w, w.s1, w.t2, false);
    assert_eq!(
        row.work.unwrap().item_id,
        Some(w.t1),
        "primary:false keeps the primary"
    );

    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let a = task_of(&p, "TK-1");
    let b = task_of(&p, "TK-2");
    assert_eq!(a.sessions[0].session_id, Some(w.s1));
    assert_eq!(
        b.sessions[0].session_id,
        Some(w.s1),
        "the same identity, not a copy"
    );
    assert!(a.sessions[0].primary && !b.sessions[0].primary);
    assert_eq!(
        (a.sessions[0].state.as_str(), b.sessions[0].state.as_str()),
        ("active", "active")
    );
    assert_eq!(a.sessions[0].other_tasks, 1, "also on one other task");

    let st = links_of(&w, w.s1);
    assert_eq!(st.links.len(), 2, "the Sessions view lists every task");
    assert_eq!(st.links.iter().filter(|l| l.link.primary).count(), 1);
    assert_eq!(st.primary_link_id, Some(a.sessions[0].link_id));
}

/// UC3 + UC9 (part): a task lists its active and past sessions; a past one
/// is never active, never primary, and names its snapshot.
#[test]
fn a_task_shows_active_and_past_sessions_apart() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    link(&w, w.s2, w.t1, true);
    w.st.lock().unwrap().delete_session(w.s2).unwrap();
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "TK-1");
    assert_eq!(
        t.counts,
        TaskCounts {
            active: 1,
            ended: 1,
            suggested: 0
        }
    );
    let past = t.sessions.iter().find(|l| l.state == "ended").unwrap();
    assert!(!past.primary && past.session_id.is_none());
    assert_eq!(past.name, "two");
    assert_eq!(t.sessions[0].state, "active", "active first");
    let d = task(&w.st, &vs(&OrgScope::All), "item:1").unwrap();
    assert_eq!(d.last_outcome.unwrap().name, "two");
    // Filters: with an active session / past only.
    let has = |h: &str| WorkTreeFilters {
        has: Some(h.into()),
        ..Default::default()
    };
    assert!(page(&w, &OrgScope::All, has("active"))
        .tasks
        .iter()
        .any(|t| t.key.as_deref() == Some("TK-1")));
    assert!(page(&w, &OrgScope::All, has("past_only")).tasks.is_empty());
    w.st.lock().unwrap().delete_session(w.s1).unwrap();
    let past_only = page(&w, &OrgScope::All, has("past_only"));
    assert_eq!(task_of(&past_only, "TK-1").counts.active, 0);
}

/// A task counts sessions, not links: a session whose link to the task
/// ended on a branch change and was made and ended again is one past
/// session — while it lives and after it is gone.
#[test]
fn a_session_with_two_past_links_is_one_past_session() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    link(&w, w.s2, w.t1, true);
    let first = links_of(&w, w.s1).links[0].link.link_id;
    {
        let s = w.st.lock().unwrap();
        s.seed_end_link(first, "branch_changed");
        s.seed_duplicate_link(first);
    }
    let one_past = TaskCounts {
        active: 1,
        ended: 1,
        suggested: 0,
    };
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "TK-1");
    assert_eq!(t.counts, one_past, "a live session's two past links");
    assert_eq!(
        t.sessions.iter().filter(|l| l.state == "ended").count(),
        2,
        "both links are still listed"
    );
    w.st.lock().unwrap().delete_session(w.s1).unwrap();
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(
        task_of(&p, "TK-1").counts,
        one_past,
        "a gone session's two past links"
    );
}

/// Two suggestions of one session for one task are one suggested session.
#[test]
fn two_suggestions_of_one_session_count_once() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        crate::service::work::detect::on_prompt(&s, w.s1, "please look at TK-3", false).unwrap();
    }
    let sug = links_of(&w, w.s1)
        .links
        .iter()
        .find(|l| l.link.state == "suggested" && l.task.key.as_deref() == Some("TK-3"))
        .expect("a suggestion for TK-3")
        .link
        .link_id;
    w.st.lock().unwrap().seed_duplicate_link(sug);
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t3 = task_of(&p, "TK-3");
    assert_eq!(t3.counts.suggested, 1);
    assert_eq!(t3.sessions.len(), 2, "both links are listed");
}

/// The Sessions view lists a live session's link that ended on a branch
/// change (history) beside its live one, and only that session's links.
#[test]
fn session_tasks_lists_a_live_sessions_past_link_beside_its_live_one() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    let first = links_of(&w, w.s1).links[0].link.link_id;
    w.st.lock().unwrap().seed_end_link(first, "branch_changed");
    link(&w, w.s1, w.t2, true);
    link(&w, w.s2, w.t3, true);
    let st = links_of(&w, w.s1);
    let rows: Vec<(&str, Option<&str>, bool)> = st
        .links
        .iter()
        .map(|l| (l.link.state.as_str(), l.task.key.as_deref(), l.link.primary))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("active", Some("TK-2"), true),
            ("ended", Some("TK-1"), false)
        ]
    );
    assert_eq!(st.primary_link_id, Some(st.links[0].link.link_id));
    assert_eq!(st.links[1].link.link_id, first);
    assert_eq!(
        st.links[1].link.end_reason.as_deref(),
        Some("branch_changed")
    );
}

/// UC4: a synced ticket with no session is a task, found by `has: none`,
/// and it is not the same as a tracker that is down.
#[test]
fn a_ticket_without_a_session_is_a_task() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    let none = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            has: Some("none".into()),
            ..Default::default()
        },
    );
    let keys: Vec<_> = none.tasks.iter().filter_map(|t| t.key.clone()).collect();
    assert!(
        keys.contains(&"TK-3".to_string()) && !keys.contains(&"TK-1".to_string()),
        "{keys:?}"
    );
    let t3 = task_of(&none, "TK-3");
    assert_eq!(t3.counts, TaskCounts::default());
    assert_eq!(
        t3.tracker_state.as_deref(),
        Some("unconfigured"),
        "the tracker's own state rides along"
    );
    assert!(t3.mine, "assigned to the tracker's account");
    assert_eq!(
        (t3.org_id, t3.org_source.as_str(), t3.org_fenced),
        (Some(w.org_a), "tracker", true)
    );
}

/// UC5: a suggestion is listed apart, never active, with its reason; a
/// rejection is final and does not come back.
#[test]
fn a_suggestion_is_distinct_explained_and_decided() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        crate::service::work::detect::on_prompt(&s, w.s1, "please look at TK-3 and TK-2", false)
            .unwrap();
    }
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t3 = task_of(&p, "TK-3");
    assert_eq!(t3.counts.suggested, 1);
    assert_eq!(
        t3.counts.active, 0,
        "a guess is never a session of the task"
    );
    assert_eq!(t3.sessions[0].state, "suggested");
    assert!(
        t3.sessions[0].why.contains("prompt"),
        "{}",
        t3.sessions[0].why
    );
    assert!(t3.review);

    let r = review(&w.st, &vs(&OrgScope::All), None, None).unwrap();
    let it = r
        .items
        .iter()
        .find(|i| i.task.key.as_deref() == Some("TK-3"))
        .unwrap();
    assert_eq!(it.kind, "suggestion");
    assert!(!it.why.is_empty());
    assert_eq!(it.alternatives.len(), 1, "TK-2 is the other guess");
    // Redesign 6.5: detection's confidence, as Review shows it — a key in
    // passing (R6) is a low guess, well under "Confirm all high-confidence".
    assert_eq!(it.rule.as_deref(), Some("R6"));
    assert_eq!(it.confidence, Some(35));

    assert_eq!(it.link_version, 1, "a fresh link");
    work_link(
        &WorkLinkArgs {
            link_id: Some(it.link_id),
            ..wl(&w, "reject", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    {
        let s = w.st.lock().unwrap();
        crate::service::work::detect::on_prompt(&s, w.s1, "TK-3 again", false).unwrap();
    }
    let r = review(&w.st, &vs(&OrgScope::All), None, None).unwrap();
    assert!(
        !r.items
            .iter()
            .any(|i| i.task.key.as_deref() == Some("TK-3")),
        "R9: a rejected pair is not proposed again"
    );
    // The decision moved the link's version (the compare-and-set token of
    // M14.1c), and the rejection is listed with the session's links.
    let st = links_of(&w, w.s1);
    let rejected = st
        .links
        .iter()
        .find(|l| l.link.link_id == it.link_id)
        .unwrap();
    assert_eq!(rejected.link.state, "rejected");
    assert!(rejected.link.link_version > it.link_version);
}

/// UC9: a sync that renames a ticket keeps the person's placement; a
/// ticket that disappears is marked, never reassigned.
#[test]
fn a_sync_updates_the_ticket_and_keeps_local_decisions() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    {
        let s = w.st.lock().unwrap();
        s.seed_placement("item:1", Some("Mine"), None);
        s.upsert_tracker_item(
            w.tracker,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: Some("TK-1".into()),
                title: "Login fails on Safari".into(),
                status_name: "Done".into(),
                status_category: "done".into(),
                containers: vec!["TP".into()],
                ..Default::default()
            },
        )
        .unwrap();
        s.mark_tracker_item_unavailable(w.tracker, "1", "not_found_or_no_permission")
            .unwrap();
    }
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "TK-1");
    assert_eq!(t.title, "Login fails on Safari");
    assert_eq!(t.status_category.as_deref(), Some("done"));
    assert_eq!(
        t.group.label, "Mine",
        "the local placement survives the sync"
    );
    assert!(t.unavailable);
    assert_eq!(
        t.sessions[0].session_id,
        Some(w.s1),
        "still the same task's session"
    );
    let r = review(&w.st, &vs(&OrgScope::All), None, None).unwrap();
    assert!(r
        .items
        .iter()
        .any(|i| i.kind == "unavailable" && i.task.key.as_deref() == Some("TK-1")));
}

/// A bound client's review and tree never show the other org's task on a
/// session they share (the spec's multi-org session behaviour).
#[test]
fn a_session_with_tasks_of_two_orgs_shows_each_side_only_its_own() {
    let w = world();
    let local_b = {
        let s = w.st.lock().unwrap();
        let (it, _) = s
            .name_session_work(w.s2, Some("SECRET-9"), "Beta secret")
            .unwrap();
        s.seed_local_item_org(it.id, Some(w.org_b));
        it.id
    };
    let tk1 = link(&w, w.s1, w.t1, true).work.unwrap().link_id;
    // A person forces B's task onto A's session s1 as a secondary link (the
    // store takes it; M14.1c's `link { primary: false, force_cross_org }`).
    let forced = {
        let s = w.st.lock().unwrap();
        let l = s
            .link_session_work(w.s1, WorkTarget::Item(local_b), "manual")
            .unwrap();
        s.seed_link_primary(tk1, true);
        l.id
    };
    let a = bound(w.org_a);
    let b = bound(w.org_b);
    fn dump<T: serde::Serialize>(x: &T) -> String {
        serde_json::to_string(x).unwrap()
    }

    let pa = page(&w, &a, WorkTreeFilters::default());
    let ra = review(&w.st, &vs(&a), None, None).unwrap();
    let sa = session_tasks(&w.st, &vs(&a), w.s1).unwrap();
    for text in [dump(&pa), dump(&ra), dump(&sa)] {
        assert!(
            !text.contains("SECRET-9") && !text.contains("Beta secret"),
            "{text}"
        );
    }
    assert_eq!(
        task_of(&pa, "TK-1").sessions[0].other_tasks,
        0,
        "B's task is not counted"
    );

    let pb = page(&w, &b, WorkTreeFilters::default());
    let secret = task_of(&pb, "SECRET-9");
    assert!(
        secret.sessions.iter().all(|l| l.session_id != Some(w.s1)),
        "A's session is never named to B"
    );
    assert!(!dump(&pb).contains("\"name\":\"one\""));
    // Unrestricted: both, and the forced link flagged for review.
    let all = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let s = task_of(&all, "SECRET-9");
    assert!(s
        .sessions
        .iter()
        .any(|l| l.session_id == Some(w.s1) && l.cross_org));
    let r = review(&w.st, &vs(&OrgScope::All), None, None).unwrap();
    assert!(r
        .items
        .iter()
        .any(|i| i.kind == "cross_org" && i.link_id == forced));
    // A kept conflict (M14.1c's `ack`) leaves the inbox.
    w.st.lock().unwrap().seed_review_ack(forced);
    let r = review(&w.st, &vs(&OrgScope::All), None, None).unwrap();
    assert!(!r.items.iter().any(|i| i.link_id == forced));
}

/// UC13: pages are a stable keyset — every task once, in order; a cursor
/// is refused under other filters; section headers count every task.
#[test]
fn pages_cover_every_task_once_and_the_cursor_is_bound_to_its_filters() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        for i in 0..30 {
            s.create_local_work_item(Some(&format!("LOC-{i}")), &format!("local {i}"))
                .unwrap();
        }
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let p = tree(
            &w.st,
            &vs(&OrgScope::All),
            &TreeArgs {
                cursor: cursor.clone(),
                limit: Some(7),
                per_task: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(p.total, 33);
        assert_eq!(p.groups.iter().map(|g| g.count).sum::<u32>(), 33);
        seen.extend(p.tasks.iter().map(|t| t.task_id.clone()));
        pages += 1;
        match p.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(pages, 5);
    let unique: BTreeSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 33, "no task twice, none missing");
    let first = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            limit: Some(7),
            ..Default::default()
        },
    )
    .unwrap();
    let err = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            filters: WorkTreeFilters {
                status: Some("open".into()),
                ..Default::default()
            },
            cursor: first.next_cursor,
            limit: Some(7),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    let bad = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            filters: WorkTreeFilters {
                has: Some("everything".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        bad.code,
        codes::E_INVALID,
        "a filter is never silently ignored"
    );
    // One section at a time.
    let tp = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            filters: WorkTreeFilters {
                group: Some(format!("tracker:{}:TP", w.tracker)),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(tp.tasks.len(), 3);
}

/// A bare key a sync later bound to an item still opens (the old id is an
/// alias of the item's).
#[test]
fn a_bare_key_bound_by_a_sync_still_opens() {
    let w = world();
    work_link(
        &WorkLinkArgs {
            key: Some("ZZ-5".into()),
            ..wl(&w, "link", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    assert!(task(&w.st, &vs(&OrgScope::All), "ref:ZZ-5").is_ok());
    let local =
        w.st.lock()
            .unwrap()
            .create_local_work_item(Some("ZZ-5"), "Now named")
            .unwrap();
    let d = task(&w.st, &vs(&OrgScope::All), "ref:zz-5").unwrap();
    assert!(d.task.task_id == format!("item:{}", local.id) || d.task.task_id == "ref:ZZ-5");
    assert!(parse_task_id("bogus").is_err());
}

/// The in-memory org rule is the store's (`Store::item_org`).
///
/// Covers the native-subtask fallback too: `insert_native` writes no
/// `org_id`, so a subtask's org is its PARENT's — under a tracker parent the
/// tracker's org, under a native parent that parent's own. Before that
/// fallback existed every native subtask read as unassigned and the org gates
/// on a start passed everything.
#[test]
fn the_graph_and_the_store_agree_on_item_orgs() {
    let w = world();
    let (local, under_tracker, under_local, orphan) = {
        let s = w.st.lock().unwrap();
        let (it, _) = s.name_session_work(w.s1, None, "Unkeyed").unwrap();
        s.seed_local_item_org(it.id, Some(w.org_b));
        // a native subtask of the org_a tracker item t1
        let under_tracker = s
            .create_native_item(&NativeItem {
                title: "Subtask of a tracker ticket",
                parent_id: Some(w.t1),
                project_id: None,
                notes: None,
            })
            .unwrap();
        // a native subtask of the org_b local item
        let under_local = s
            .create_native_item(&NativeItem {
                title: "Subtask of a local item",
                parent_id: Some(it.id),
                project_id: None,
                notes: None,
            })
            .unwrap();
        // a top-level native item with no org anywhere: still None
        let orphan = s
            .create_native_item(&NativeItem {
                title: "Top-level, no org",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        (it.id, under_tracker.id, under_local.id, orphan.id)
    };
    let s = w.st.lock().unwrap();
    let g = Graph::load(&s, &OrgScope::All).unwrap();
    for id in [w.t1, w.t2, w.t3, local, under_tracker, under_local, orphan] {
        assert_eq!(
            g.item_org(&g.items[&id]),
            s.item_org(id).unwrap(),
            "item {id}"
        );
    }
    // and the fallback resolves to the parent's org, not to None
    assert_eq!(
        s.item_org(under_tracker).unwrap(),
        Some(w.org_a),
        "a subtask of a tracker ticket is in the tracker's org"
    );
    assert_eq!(
        s.item_org(under_local).unwrap(),
        Some(w.org_b),
        "a subtask of a local item is in that item's org"
    );
    assert_eq!(
        s.item_org(orphan).unwrap(),
        None,
        "a top-level native item with no org is still unassigned"
    );
}

/// Writes the real wire shapes of every Work view read to
/// `$WORK_VIEW_FIXTURE_DIR` (when set), so the desktop's and the phone's
/// fixtures can be checked against what the hub serialises.
#[test]
fn dump_wire_samples_when_asked() {
    let Ok(dir) = std::env::var("WORK_VIEW_FIXTURE_DIR") else {
        return;
    };
    let w = world();
    link(&w, w.s1, w.t1, true);
    link(&w, w.s1, w.t2, false);
    {
        let s = w.st.lock().unwrap();
        crate::service::work::detect::on_prompt(&s, w.s2, "look at TK-3", false).unwrap();
    }
    w.st.lock()
        .unwrap()
        .seed_placement("item:2", Some("Security"), Some("why"));
    let write = |name: &str, v: serde_json::Value| {
        std::fs::write(
            format!("{dir}/{name}.json"),
            serde_json::to_string_pretty(&v).unwrap(),
        )
        .unwrap();
    };
    write(
        "tree",
        serde_json::to_value(page(&w, &OrgScope::All, WorkTreeFilters::default())).unwrap(),
    );
    write(
        "task",
        serde_json::to_value(task(&w.st, &vs(&OrgScope::All), "item:2").unwrap()).unwrap(),
    );
    write(
        "session_tasks",
        serde_json::to_value(links_of(&w, w.s1)).unwrap(),
    );
    write(
        "review",
        serde_json::to_value(review(&w.st, &vs(&OrgScope::All), None, None).unwrap()).unwrap(),
    );
    let local =
        w.st.lock()
            .unwrap()
            .name_session_work(w.s2, Some("LOC-1"), "Local")
            .unwrap()
            .0
            .id;
    write(
        "org_impact",
        serde_json::to_value(
            structure::org_impact(
                &w.st,
                &vs(&OrgScope::All),
                &format!("item:{local}"),
                Some(w.org_b),
            )
            .unwrap(),
        )
        .unwrap(),
    );
}

/// UC6: where a task sits and why — a person's placement beats a rule, a
/// rule beats the tracker; a rule's preview changes nothing; a disabled
/// rule and a cleared placement fall back.
#[test]
fn placement_and_rules_explain_the_group() {
    let w = world();
    let group = |w: &W, key: &str| {
        task_of(&page(w, &OrgScope::All, WorkTreeFilters::default()), key)
            .group
            .clone()
    };
    let g = group(&w, "TK-1");
    assert_eq!((g.source.as_str(), g.label.as_str()), ("tracker", "TP"));
    assert_eq!(g.id, format!("tracker:{}:TP", w.tracker));

    let conditions = RuleConditions {
        title_contains: Some("audit".into()),
        ..Default::default()
    };
    let draft = RuleInput {
        name: "Audit".into(),
        conditions: conditions.clone(),
        group: "Compliance".into(),
        ..Default::default()
    };
    let pv = structure::rule_preview(&w.st, &OrgScope::All, &draft).unwrap();
    assert_eq!(pv.total, 1);
    assert_eq!(pv.affected[0].key.as_deref(), Some("TK-2"));
    assert_eq!(pv.affected[0].to.label, "Compliance");
    assert_eq!(
        group(&w, "TK-2").source,
        "tracker",
        "a preview changes nothing"
    );
    let rule =
        w.st.lock()
            .unwrap()
            .seed_rule("Audit", &conditions, "Compliance");
    let g = group(&w, "TK-2");
    assert_eq!((g.source.as_str(), g.rule_id), ("rule", Some(rule)));
    assert_eq!(
        g.tracker_value.as_deref(),
        Some("TP"),
        "what the tracker says stays visible"
    );
    assert_eq!(
        task(&w.st, &vs(&OrgScope::All), "item:2").unwrap().rules,
        vec![rule]
    );
    let rules = structure::rules(&w.st, &OrgScope::All).unwrap();
    assert_eq!(rules.len(), 1);

    // A person's placement of TK-2 only.
    w.st.lock()
        .unwrap()
        .seed_placement("item:2", Some("Security"), None);
    let d = task(&w.st, &vs(&OrgScope::All), "item:2").unwrap();
    assert_eq!(
        (d.task.group.source.as_str(), d.task.group.label.as_str()),
        ("manual", "Security")
    );
    assert_eq!(d.task.placement_version, 1);
    assert_eq!(d.placement.unwrap().group.as_deref(), Some("Security"));
    assert_eq!(group(&w, "TK-1").source, "tracker", "the others stay");
    // The manual placement is kept by the preview of another matching rule.
    let pv = structure::rule_preview(&w.st, &OrgScope::All, &draft).unwrap();
    assert_eq!((pv.total, pv.kept_manual), (0, 1));
    // A cleared placement falls back to the rule; a disabled rule to the
    // tracker.
    w.st.lock().unwrap().seed_placement("item:2", None, None);
    assert_eq!(group(&w, "TK-2").source, "rule");
    w.st.lock().unwrap().seed_rule_enabled(rule, false);
    assert_eq!(group(&w, "TK-2").source, "tracker");
    // An empty rule is refused, even as a preview.
    let err = structure::rule_preview(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            name: "all".into(),
            group: "x".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
}

/// UC7's preview: what moving a local task to another org would change,
/// named for the unrestricted caller only (D33: a bound client or a host
/// may not move an org, so it does not read the impact either). A tracker
/// item's org is its tracker's.
#[test]
fn the_org_impact_names_the_move_for_an_unrestricted_caller_only() {
    let w = world();
    let local = {
        let s = w.st.lock().unwrap();
        s.name_session_work(w.s1, Some("LOC-9"), "Refactor billing")
            .unwrap()
            .0
            .id
    };
    let tid = format!("item:{local}");
    assert!(
        task(&w.st, &vs(&bound(w.org_a)), &tid).is_ok(),
        "unassigned: visible"
    );
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_b)).unwrap();
    assert!(imp.allowed);
    assert_eq!((imp.from_org, imp.to_org), (None, Some(w.org_b)));
    assert!(imp.links[0].becomes_cross_org, "s1 is org A's");
    assert_eq!(imp.hosts_losing, vec!["h1".to_string()]);
    assert!(!imp.impact_token.is_empty());
    for scope in [bound(w.org_a), strict(w.org_a)] {
        let err = structure::org_impact(&w.st, &vs(&scope), &tid, Some(w.org_b)).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }
    let err = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, None).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID, "org_id is required (0: none)");
    let t = structure::org_impact(&w.st, &vs(&OrgScope::All), "item:1", Some(w.org_b)).unwrap();
    assert_eq!(
        (t.allowed, t.reason.as_deref()),
        (false, Some("tracker_controlled"))
    );
    // Once the item is B's (M14.1c's `assign_org`, seeded), A no longer
    // receives it, nor its link on A's own session.
    w.st.lock()
        .unwrap()
        .seed_local_item_org(local, Some(w.org_b));
    let err = task(&w.st, &vs(&bound(w.org_a)), &tid).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND, "answered as unknown");
    let st = session_tasks(&w.st, &vs(&bound(w.org_a)), w.s1).unwrap();
    assert!(st.links.is_empty(), "A's session no longer names B's task");
}

/// A client bound to org A whose session's primary is another org's task (a
/// forced link) is never told that link: no primary is named, and the
/// review inbox does not call the session "without a primary" either.
#[test]
fn a_hidden_primary_is_neither_named_nor_reported_missing_to_a_bound_client() {
    let w = world();
    let local_b = {
        let s = w.st.lock().unwrap();
        let (it, _) = s.name_session_work(w.s2, Some("HID-1"), "Hidden").unwrap();
        s.seed_local_item_org(it.id, Some(w.org_b));
        it.id
    };
    // s1 (org A): B's task is its primary; A's TK-1 a secondary.
    let hidden =
        w.st.lock()
            .unwrap()
            .link_session_work(w.s1, WorkTarget::Item(local_b), "manual")
            .unwrap()
            .id;
    link(&w, w.s1, w.t1, false);
    let a = bound(w.org_a);
    let st = session_tasks(&w.st, &vs(&a), w.s1).unwrap();
    assert_eq!(st.primary_link_id, None, "the hidden primary is not named");
    assert_eq!(st.links.len(), 1);
    let text = serde_json::to_string(&st).unwrap();
    assert!(!text.contains("HID-1") && !text.contains(&format!("\"link_id\":{hidden}")));
    let r = review(&w.st, &vs(&a), None, None).unwrap();
    assert!(
        !r.items.iter().any(|i| i.kind == "no_primary"),
        "{:?}",
        r.items
    );
    let all = links_of(&w, w.s1);
    assert_eq!(all.primary_link_id, Some(hidden));
}

/// D31: an org's bound clients see unassigned work and sessions while its
/// `bound_sees_unassigned` is on (the default), and only rows assigned to
/// the org while it is off — never another org's either way.
#[test]
fn d31_decides_whether_a_bound_client_sees_unassigned_work() {
    let w = world();
    let (h2_session, loose, bare) = {
        let s = w.st.lock().unwrap();
        // An unassigned host and its session; a local item with no org and
        // no work; a bare key on the unassigned session.
        s.upsert_host("h2").unwrap();
        let sid = s
            .upsert_session("free", "h2", None, None, 1, 1, "running", None)
            .unwrap();
        let loose = s
            .create_local_work_item(Some("LOOSE-1"), "Nobody's")
            .unwrap();
        s.link_session_work(sid, WorkTarget::Ref("FREE-7"), "manual")
            .unwrap();
        (sid, loose.id, "ref:FREE-7")
    };
    link(&w, w.s1, w.t1, true);
    let keys = |scope: &OrgScope| -> BTreeSet<String> {
        page(&w, scope, WorkTreeFilters::default())
            .tasks
            .iter()
            .filter_map(|t| t.key.clone())
            .collect()
    };
    let on = keys(&bound(w.org_a));
    assert!(on.contains("TK-1") && on.contains("LOOSE-1") && on.contains("FREE-7"));
    assert!(session_tasks(&w.st, &vs(&bound(w.org_a)), h2_session).is_ok());
    assert!(task(&w.st, &vs(&bound(w.org_a)), bare).is_ok());

    let off = keys(&strict(w.org_a));
    assert!(off.contains("TK-1") && off.contains("TK-3"), "{off:?}");
    assert!(
        !off.contains("LOOSE-1") && !off.contains("FREE-7"),
        "unassigned work is hidden: {off:?}"
    );
    let p = page(&w, &strict(w.org_a), WorkTreeFilters::default());
    assert_eq!(p.groups.iter().map(|g| g.count).sum::<u32>(), p.total);
    assert!(p.orgs.iter().all(|o| o.id == w.org_a));
    for (tid, what) in [
        (format!("item:{loose}"), "a loose item"),
        (bare.into(), "a bare key"),
    ] {
        let err = task(&w.st, &vs(&strict(w.org_a)), &tid).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND, "{what}");
    }
    let err = session_tasks(&w.st, &vs(&strict(w.org_a)), h2_session).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND, "an unassigned session");
    // Org B's client, either way, sees none of A's.
    for b in [bound(w.org_b), strict(w.org_b)] {
        let k = keys(&b);
        assert!(!k.iter().any(|k| k.starts_with("TK-")), "{k:?}");
    }
}

/// Rules and saved views are navigation, but what they name is not every
/// caller's: a bound client reads the rules that place a task it sees and
/// its own org's views; a host reads neither.
#[test]
fn rules_and_views_are_fenced_per_caller() {
    let w = world();
    let (mine, theirs) = {
        let s = w.st.lock().unwrap();
        let b_tracker = s
            .add_tracker("jira", "Jira (beta)", "https://beta.atlassian.net")
            .unwrap();
        s.set_tracker_org(b_tracker.id, Some(w.org_b)).unwrap();
        let mine = s.seed_rule(
            "Audit",
            &RuleConditions {
                title_contains: Some("audit".into()),
                ..Default::default()
            },
            "Compliance",
        );
        let theirs = s.seed_rule(
            "Beta payments",
            &RuleConditions {
                tracker_id: Some(b_tracker.id),
                container: Some("PAY".into()),
                ..Default::default()
            },
            "Payments",
        );
        s.seed_view("Everything", &serde_json::json!({}), None);
        s.seed_view(
            "A open",
            &serde_json::json!({"status": "open"}),
            Some(w.org_a),
        );
        s.seed_view(
            "B secret",
            &serde_json::json!({"query": "beta"}),
            Some(w.org_b),
        );
        (mine, theirs)
    };
    let ids = |scope: &OrgScope| -> Vec<i64> {
        structure::rules(&w.st, scope)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(ids(&OrgScope::All), vec![mine, theirs]);
    assert_eq!(ids(&bound(w.org_a)), vec![mine]);
    assert!(ids(&bound(w.org_b)).is_empty(), "B sees none of A's tasks");
    let host = OrgScope::for_host(&w.st.lock().unwrap(), "h1").unwrap();
    assert!(ids(&host).is_empty());
    let names = |scope: &OrgScope| -> Vec<String> {
        structure::views(&w.st, scope)
            .unwrap()
            .into_iter()
            .map(|v| v.name)
            .collect()
    };
    assert_eq!(names(&OrgScope::All).len(), 3);
    assert_eq!(names(&bound(w.org_a)), vec!["A open".to_string()]);
    assert_eq!(names(&strict(w.org_b)), vec!["B secret".to_string()]);
    assert!(names(&host).is_empty());
    let err = structure::rule_preview(
        &w.st,
        &host,
        &RuleInput {
            name: "x".into(),
            group: "x".into(),
            conditions: RuleConditions {
                key_prefix: Some("TK".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN);
}

/// UC13 under change: a page's cursor stays valid while tasks are added
/// and moved — no task is repeated across pages, and a later page never
/// goes back before the cursor.
#[test]
fn a_cursor_is_stable_under_concurrent_change() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        for i in 0..20 {
            s.create_local_work_item(Some(&format!("LOC-{i}")), &format!("local {i}"))
                .unwrap();
        }
    }
    let args = |cursor: Option<String>| TreeArgs {
        cursor,
        limit: Some(5),
        per_task: Some(0),
        ..Default::default()
    };
    let first = tree(&w.st, &vs(&OrgScope::All), &args(None)).unwrap();
    let seen_first: BTreeSet<String> = first.tasks.iter().map(|t| t.task_id.clone()).collect();
    // Meanwhile: new tasks arrive and one on the first page gains a session
    // (it moves up the order).
    {
        let s = w.st.lock().unwrap();
        for i in 20..25 {
            s.create_local_work_item(Some(&format!("LOC-{i}")), &format!("local {i}"))
                .unwrap();
        }
    }
    link(&w, w.s1, w.t1, true);
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = first.next_cursor.clone();
    while let Some(c) = cursor {
        let p = tree(&w.st, &vs(&OrgScope::All), &args(Some(c))).unwrap();
        seen.extend(p.tasks.iter().map(|t| t.task_id.clone()));
        cursor = p.next_cursor;
    }
    let unique: BTreeSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), seen.len(), "no task twice across later pages");
    assert!(
        seen.iter().all(|t| !seen_first.contains(t)),
        "a later page never repeats the first page"
    );
    // The same cursor, replayed, answers the same page.
    let again = tree(&w.st, &vs(&OrgScope::All), &args(first.next_cursor.clone())).unwrap();
    let again2 = tree(&w.st, &vs(&OrgScope::All), &args(first.next_cursor)).unwrap();
    assert_eq!(again.tasks, again2.tasks);
    // A cursor that is not the hub's is refused, never read as a start.
    let err = tree(&w.st, &vs(&OrgScope::All), &args(Some("bogus".into()))).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    // The review inbox pages the same way.
    let err = review(&w.st, &vs(&OrgScope::All), Some("bogus"), None).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
}

/// Re-cache TK-1 with `description` as the excerpt the sync kept and
/// `description_chars` as the tracker's true length beside it.
fn recache_tk1_description(w: &W, description: &str, description_chars: Option<i64>) {
    w.st.lock()
        .unwrap()
        .upsert_tracker_item(
            w.tracker,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: Some("TK-1".into()),
                title: "Login fails".into(),
                status_name: "In Progress".into(),
                status_category: "in_progress".into(),
                containers: vec!["TP".into()],
                assignee_id: Some("me".into()),
                description: Some(description.to_string()),
                description_chars,
                ..Default::default()
            },
        )
        .unwrap();
}

/// `work { action: task }` is the FOURTH agent-facing path that carries a
/// ticket's description, and it used to cut at `DESCRIPTION_MAX_CHARS` in
/// silence through the bare `fence_untrusted` the rest of this branch
/// replaced — an agent that read the Work view's task detail instead of
/// `lookup` was exactly as blind as before the branch. It now uses the same
/// `fence_ticket`: same audience, same cap, same marker, plus the notice and
/// the `describe` offer. Mirrors `tests_tickets.rs`'s
/// `every_path_that_carries_a_description_says_it_cut`.
#[test]
fn a_host_token_is_told_when_the_task_detail_cut_the_description() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    recache_tk1_description(&w, &"x".repeat(DESCRIPTION_MAX_CHARS), Some(6812));
    let tid = format!("item:{}", w.t1);
    let host = OrgScope::for_host(&w.st.lock().unwrap(), "h1").unwrap();
    let d = task(&w.st, &vs(&host), &tid).unwrap().description.unwrap();
    // This path's cap is the Work view's own (`DESCRIPTION_MAX_CHARS` = 600
    // here, not the trackers' 2000), so the notice names what THIS answer
    // shows of the tracker's 6812.
    assert!(d.contains("shown 600 of 6812 chars"), "{d}");
    assert!(
        d.contains(r#"work { action: describe, key: "TK-1" }"#),
        "{d}"
    );
    // The notice lives OUTSIDE the untrusted fence, as on every other path.
    let end = d.find(crate::mcp::guard::UNTRUSTED_END).expect("fenced");
    assert!(d.find("shown 600 of").unwrap() > end, "{d}");
    // A person (the desktop, a phone, bound or not) still reads it plain:
    // no fence, no notice.
    let plain = task(&w.st, &vs(&OrgScope::All), &tid)
        .unwrap()
        .description
        .unwrap();
    assert!(!plain.contains("shown"), "{plain}");
    assert!(!plain.contains("claude-fleet"), "{plain}");
}

/// The other half, C1's on this path: a description the tracker holds WHOLE
/// reaches the same agent with no notice at all — byte-equal to the plain
/// fence.
#[test]
fn a_whole_description_reaches_the_task_detail_without_a_notice() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    let text = "Login fails on partial captures.";
    recache_tk1_description(&w, text, Some(text.chars().count() as i64));
    let host = OrgScope::for_host(&w.st.lock().unwrap(), "h1").unwrap();
    let d = task(&w.st, &vs(&host), &format!("item:{}", w.t1))
        .unwrap()
        .description
        .unwrap();
    assert!(!d.contains("shown"), "{d}");
    assert!(!d.contains("open the ticket"), "{d}");
    assert_eq!(
        d,
        crate::mcp::guard::fence_untrusted(text, "a tracker ticket", DESCRIPTION_MAX_CHARS)
    );
}

/// A person's screen has no fence notice, so the task detail says in fields
/// what it cut: `description_chars` is the tracker's true length (else the
/// excerpt's) and `description_truncated` is set when the excerpt shown is
/// shorter — for the 600-char cap and for a cache that kept less than the
/// tracker holds. A whole description is marked whole, and serialises
/// without the flag so an older client sees the shape it knew.
#[test]
fn the_task_detail_says_how_much_of_the_description_it_shows() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    let tid = format!("item:{}", w.t1);
    // Longer than the cap, with no count from the tracker: the excerpt's.
    recache_tk1_description(&w, &"y".repeat(900), None);
    let d = task(&w.st, &vs(&OrgScope::All), &tid).unwrap();
    assert_eq!(
        d.description.as_deref().map(|t| t.chars().count()),
        Some(DESCRIPTION_MAX_CHARS)
    );
    assert_eq!(d.description_chars, Some(900));
    assert!(d.description_truncated);
    // Under the cap, but the cache kept less than the tracker holds.
    recache_tk1_description(&w, "short excerpt", Some(6812));
    let d = task(&w.st, &vs(&OrgScope::All), &tid).unwrap();
    assert_eq!(d.description_chars, Some(6812));
    assert!(d.description_truncated);
    // Whole: counted, not flagged, and the flag stays off the wire.
    let text = "Login fails on partial captures.";
    recache_tk1_description(&w, text, Some(text.chars().count() as i64));
    let d = task(&w.st, &vs(&OrgScope::All), &tid).unwrap();
    assert_eq!(d.description_chars, Some(text.chars().count()));
    assert!(!d.description_truncated);
    let wire = serde_json::to_value(&d).unwrap();
    assert!(wire.get("description_truncated").is_none(), "{wire}");
    // An older hub's answer (neither field) still reads.
    let mut old = wire.clone();
    old.as_object_mut().unwrap().remove("description_chars");
    let back: TaskDetail = serde_json::from_value(old).unwrap();
    assert_eq!(back.description_chars, None);
    assert!(!back.description_truncated);
}

#[path = "view_write_tests.rs"]
mod writes;

/// A ticket in a done state (`status_category: done`).
fn done_item(w: &W, ext: &str, key: &str) -> i64 {
    w.st.lock()
        .unwrap()
        .upsert_tracker_item(
            w.tracker,
            &TrackerItemWrite {
                external_id: ext.into(),
                key: Some(key.into()),
                title: format!("{key} shipped"),
                status_name: "Done".into(),
                status_category: "done".into(),
                containers: vec!["TP".into()],
                ..Default::default()
            },
        )
        .unwrap()
        .id
}

fn keys(p: &TreePage) -> Vec<String> {
    p.tasks.iter().filter_map(|t| t.key.clone()).collect()
}

/// The filters a client that hides archived tasks sends (the desktop's
/// default): `archived: false`, asked for explicitly.
fn hiding() -> WorkTreeFilters {
    WorkTreeFilters {
        archived: Some(false),
        ..Default::default()
    }
}

/// Archived tasks: a done task with no active session is out of the tree
/// when the caller asks (`archived: false`, counted in `archived_hidden`),
/// in it with `archived: true` or `status: done`, and still answered by a
/// direct read.
#[test]
fn a_done_task_without_an_active_session_is_archived() {
    let w = world();
    let t9 = done_item(&w, "9", "TK-9");
    let p = page(&w, &OrgScope::All, hiding());
    assert!(!keys(&p).contains(&"TK-9".to_string()), "{:?}", keys(&p));
    assert_eq!(p.archived_hidden, 1);
    assert!(p.tasks.iter().all(|t| !t.archived));

    let shown = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            archived: Some(true),
            ..Default::default()
        },
    );
    assert!(task_of(&shown, "TK-9").archived);
    assert_eq!(shown.archived_hidden, 0);
    assert_eq!(shown.total, p.total + 1);

    let done = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            status: Some("done".into()),
            archived: Some(false),
            ..Default::default()
        },
    );
    assert_eq!(keys(&done), vec!["TK-9".to_string()]);
    assert_eq!(done.archived_hidden, 0);

    // Only the tree hides it.
    let d = task(&w.st, &vs(&OrgScope::All), &format!("item:{t9}")).unwrap();
    assert!(d.task.archived);
}

/// A client from before the archive (a fleet-mobile that never sends
/// `archived` and has no "N hidden" row) keeps seeing every task: absent
/// is not a request to hide.
#[test]
fn a_client_that_never_sends_archived_sees_archived_tasks() {
    let w = world();
    done_item(&w, "9", "TK-9");
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert!(task_of(&p, "TK-9").archived);
    assert_eq!(p.archived_hidden, 0);
}

/// A done task someone is still working in is not archived.
#[test]
fn a_done_task_with_an_active_session_stays_in_the_tree() {
    let w = world();
    let t9 = done_item(&w, "9", "TK-9");
    link(&w, w.s1, t9, true);
    let p = page(&w, &OrgScope::All, hiding());
    let t = task_of(&p, "TK-9");
    assert!(!t.archived);
    assert_eq!(t.counts.active, 1);
    assert_eq!(p.archived_hidden, 0);
}

/// A task whose every link is archived (one of them ended) and that has no
/// active session is archived; one un-archived link keeps it in the tree.
#[test]
fn a_task_with_every_link_archived_is_hidden() {
    let w = world();
    link(&w, w.s2, w.t2, true);
    work_link(&wl(&w, "archive", w.s2), &w.st, &OrgScope::All).unwrap();
    // Still live (active): not archived.
    let p = page(&w, &OrgScope::All, hiding());
    assert!(!task_of(&p, "TK-2").archived);
    w.st.lock().unwrap().delete_session(w.s2).unwrap();
    let p = page(&w, &OrgScope::All, hiding());
    assert!(!keys(&p).contains(&"TK-2".to_string()), "{:?}", keys(&p));
    assert_eq!(p.archived_hidden, 1);
    // `has: past_only` asks for past work, which is archived work: it
    // shows it even while hiding, as the sidebar's "Past only" does.
    let past = |archived: Option<bool>| WorkTreeFilters {
        has: Some("past_only".into()),
        archived,
        ..Default::default()
    };
    let hidden = page(&w, &OrgScope::All, past(Some(false)));
    assert!(task_of(&hidden, "TK-2").archived);
    assert_eq!(hidden.archived_hidden, 0);
    let shown = page(&w, &OrgScope::All, past(Some(true)));
    assert!(task_of(&shown, "TK-2").archived);

    // A second, un-archived past session: not every link is archived.
    link(&w, w.s1, w.t2, true);
    w.st.lock().unwrap().delete_session(w.s1).unwrap();
    let p = page(&w, &OrgScope::All, hiding());
    assert!(!task_of(&p, "TK-2").archived);
    assert_eq!(p.archived_hidden, 0);
}

/// Archived is the task's, not the caller's: a host token sees only its
/// own host's past work (M2's fence), so its own archived past link alone
/// must not make the task archived while another host's past session on
/// it is not archived (and it learns nothing more about that one than the
/// boolean).
#[test]
fn a_task_worked_on_elsewhere_is_not_archived_for_a_fenced_caller() {
    let w = world();
    let s3 = {
        let s = w.st.lock().unwrap();
        s.upsert_host("h2").unwrap();
        s.set_host_org("h2", Some(w.org_a)).unwrap();
        s.upsert_session("three", "h2", None, None, 1, 1, "running", None)
            .unwrap()
    };
    link(&w, w.s2, w.t2, true);
    work_link(&wl(&w, "archive", w.s2), &w.st, &OrgScope::All).unwrap();
    w.st.lock().unwrap().delete_session(w.s2).unwrap();
    link(&w, s3, w.t2, true);
    w.st.lock().unwrap().delete_session(s3).unwrap();

    let host = OrgScope::for_host(&w.st.lock().unwrap(), "h1").unwrap();
    let p = page(&w, &host, hiding());
    let t = task_of(&p, "TK-2");
    assert!(!t.archived, "h2's past session on it is not archived");
    assert_eq!(p.archived_hidden, 0);
    assert_eq!(t.counts.ended, 1, "h2's past work stays fenced");
    assert!(t.sessions.iter().all(|l| l.host.as_deref() == Some("h1")));
    // The master, seeing both, agrees.
    assert!(!task_of(&page(&w, &OrgScope::All, hiding()), "TK-2").archived);
}

/// `archived_hidden` counts over the whole result, not the page, and
/// honours the other filters.
#[test]
fn archived_hidden_counts_the_whole_result() {
    let w = world();
    for n in 0..3 {
        done_item(&w, &format!("d{n}"), &format!("TK-{}", 10 + n));
    }
    let p = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            filters: hiding(),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(p.tasks.len(), 1);
    assert_eq!(p.archived_hidden, 3);
    let q = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            query: Some("TK-11".into()),
            ..hiding()
        },
    );
    assert!(q.tasks.is_empty());
    assert_eq!(q.archived_hidden, 1);
    let open = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            status: Some("open".into()),
            ..hiding()
        },
    );
    assert_eq!(open.archived_hidden, 0, "open already excludes done");
}

/// One refresh is one read: the sections a client has open are paged from
/// the same built tasks exactly as their own reads page them (tasks and
/// cursor), and `review_total` is the inbox's total under the same scope.
#[test]
fn a_tree_read_pages_its_sections_and_review_total_as_their_own_reads() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    {
        let s = w.st.lock().unwrap();
        for i in 0..5 {
            s.create_local_work_item(Some(&format!("LOC-{i}")), &format!("local {i}"))
                .unwrap();
        }
        crate::service::work::detect::on_prompt(&s, w.s2, "please look at TK-3 and TK-2", false)
            .unwrap();
    }
    let filters = WorkTreeFilters {
        archived: Some(false),
        ..Default::default()
    };
    for (n, scope) in [OrgScope::All, bound(w.org_a), strict(w.org_a)]
        .into_iter()
        .enumerate()
    {
        let head = page(&w, &scope, filters.clone());
        let asks: Vec<SectionAsk> = head
            .groups
            .iter()
            .map(|g| SectionAsk {
                org_id: g.org_id,
                group_id: g.group.id.clone(),
                limit: Some(2),
            })
            .collect();
        assert!(!asks.is_empty(), "scope {n}");
        let batched = tree(
            &w.st,
            &vs(&scope),
            &TreeArgs {
                filters: filters.clone(),
                limit: Some(1),
                sections: asks.clone(),
                with_review_total: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(batched.sections.len(), asks.len(), "scope {n}");
        for (ask, got) in asks.iter().zip(&batched.sections) {
            let own_filters = section_filters(&filters, ask.org_id, &ask.group_id);
            let own = tree(
                &w.st,
                &vs(&scope),
                &TreeArgs {
                    filters: own_filters.clone(),
                    limit: ask.limit,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                (got.org_id, got.group_id.as_str()),
                (ask.org_id, ask.group_id.as_str())
            );
            assert_eq!(got.tasks, own.tasks, "scope {n}: {ask:?}");
            assert_eq!(got.next_cursor, own.next_cursor, "scope {n}: {ask:?}");
            // The section's cursor pages on in the section's own read.
            if let Some(c) = &got.next_cursor {
                let next = tree(
                    &w.st,
                    &vs(&scope),
                    &TreeArgs {
                        filters: own_filters,
                        cursor: Some(c.clone()),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(!next.tasks.is_empty(), "scope {n}: {ask:?}");
            }
        }
        let inbox = review(&w.st, &vs(&scope), None, Some(1)).unwrap();
        assert_eq!(batched.review_total, Some(inbox.total), "scope {n}");
        if n == 0 {
            assert!(inbox.total > 0, "the suggestions are in the inbox");
        }
    }
    // Asked for neither, a read answers as before.
    let plain = tree(&w.st, &vs(&OrgScope::All), &TreeArgs::default()).unwrap();
    assert!(plain.sections.is_empty());
    assert_eq!(plain.review_total, None);
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("sections").is_none() && json.get("review_total").is_none());
    // Too many sections is refused, never cut short.
    let err = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            sections: vec![
                SectionAsk {
                    org_id: None,
                    group_id: "none".into(),
                    limit: None,
                };
                TREE_MAX_SECTIONS + 1
            ],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
}

// --- native item status (design 2026-09-28 §2): the live precedence the
// tree projects through `service::work::status::effective_status`, given
// `has_working_session` from one join over the whole page. ---------------

/// Mark `sid`'s session as presently working, the way a live Claude turn
/// does (`crate::service::work::tidy::tests::seed_session_and_item`'s
/// pattern): a `claude_session_id`, then its `claude_status`.
fn mark_working(w: &W, sid: i64, claude_session_id: &str) {
    let s = w.st.lock().unwrap();
    s.set_claude_session_id(sid, claude_session_id).unwrap();
    s.set_claude_status_by_session_id(claude_session_id, "working")
        .unwrap();
}

/// A working session lifts a LOCAL item's `todo` to `in_progress` in the
/// tree — the live signal `effective_status` computes from one join, never
/// a query per row.
#[test]
fn a_working_session_shows_a_local_item_as_in_progress() {
    let w = world();
    w.st.lock()
        .unwrap()
        .name_session_work(w.s1, Some("LOC-77"), "Refactor billing")
        .unwrap();
    mark_working(&w, w.s1, "c-loc-77");

    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(
        task_of(&p, "LOC-77").status_category.as_deref(),
        Some("in_progress")
    );
}

/// A person's status is final: a working session does not lift it back to
/// `in_progress` (§2 rule 1 outranks rule 3).
#[test]
fn a_persons_status_is_not_lifted_by_a_working_session() {
    let w = world();
    let (item, _) =
        w.st.lock()
            .unwrap()
            .name_session_work(w.s1, Some("LOC-78"), "Something else")
            .unwrap();
    w.st.lock()
        .unwrap()
        .set_item_status(item.id, "todo")
        .unwrap();
    mark_working(&w, w.s1, "c-loc-78");

    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(
        task_of(&p, "LOC-78").status_category.as_deref(),
        Some("todo")
    );
}

/// A tracker item's status is its tracker's: a working session must never
/// move it (§2, "who may be overridden").
#[test]
fn a_working_session_never_lifts_a_tracker_item() {
    let w = world();
    let t4 =
        w.st.lock()
            .unwrap()
            .upsert_tracker_item(
                w.tracker,
                &TrackerItemWrite {
                    external_id: "4".into(),
                    key: Some("TK-4".into()),
                    title: "Not started".into(),
                    status_name: "To Do".into(),
                    status_category: "todo".into(),
                    containers: vec!["TP".into()],
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
    link(&w, w.s1, t4, true);
    mark_working(&w, w.s1, "c-tk-4");

    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(task_of(&p, "TK-4").status_category.as_deref(), Some("todo"));
}

/// Fix round 1: the tree (`to_task`) and a session's own task list
/// (`brief_of`, via `session_tasks`) must not disagree about the same
/// item's status — both project through `effective_status` with the same
/// page-wide working set, never a query per caller.
#[test]
fn the_tree_and_a_sessions_own_tasks_agree_on_a_working_items_status() {
    let w = world();
    w.st.lock()
        .unwrap()
        .name_session_work(w.s1, Some("LOC-79"), "Agree with me")
        .unwrap();
    mark_working(&w, w.s1, "c-loc-79");

    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let tree_status = task_of(&p, "LOC-79").status_category.clone();
    assert_eq!(tree_status.as_deref(), Some("in_progress"));

    let links = links_of(&w, w.s1);
    assert_eq!(links.links.len(), 1);
    assert_eq!(
        links.links[0].task.status_category, tree_status,
        "the tree and the session's own task list must agree"
    );
}

/// Fix round 2 (C2): a working session on a bare `ref_key` link (no item
/// at all, `l.item_id IS NULL`) must not break `Store::
/// work_items_with_working_session`'s one-join query. The clause `AND
/// l.item_id IS NOT NULL` is load-bearing for a stronger reason than "a
/// NULL would slip into the set": `r.get::<_, i64>` on that NULL column
/// would **error**, and every reader that loads a `Graph` — `tree`,
/// `task`, `session_tasks`, `review` — would fail outright, not just admit
/// a bogus id.
#[test]
fn a_working_session_on_a_bare_ref_key_link_does_not_break_the_view() {
    let w = world();
    work_link(
        &WorkLinkArgs {
            key: Some("BARE-9".into()),
            ..wl(&w, "link", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    mark_working(&w, w.s1, "c-bare-9");

    // None of these may error.
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert!(!p.tasks.is_empty());
    assert!(task(&w.st, &vs(&OrgScope::All), "ref:BARE-9").is_ok());
    assert!(session_tasks(&w.st, &vs(&OrgScope::All), w.s1).is_ok());
    assert!(review(&w.st, &vs(&OrgScope::All), None, None).is_ok());
}

/// Fix round 2 (C3): the live signal must not leak "someone is working on
/// this" through a session the caller cannot see. A local item owned by
/// org A, ALSO worked by an org B session, lifts to `in_progress` for
/// `OrgScope::All` (which sees every session) but stays at its stored
/// value for org A's own bound scope, which cannot see org B's session.
#[test]
fn the_live_lift_is_fenced_by_org_scope() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        s.upsert_host("h2").unwrap();
        s.set_host_org("h2", Some(w.org_b)).unwrap();
        let s2b = s
            .upsert_session("two-b", "h2", None, None, 1, 1, "running", None)
            .unwrap();
        let (it, _) = s
            .name_session_work(w.s1, Some("LOC-90"), "Cross-org watch")
            .unwrap();
        s.seed_local_item_org(it.id, Some(w.org_a));
        s.link_session_work(s2b, WorkTarget::Item(it.id), "manual")
            .unwrap();
        s.set_claude_session_id(s2b, "c-loc-90-b").unwrap();
        s.set_claude_status_by_session_id("c-loc-90-b", "working")
            .unwrap();
    }

    let all = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(
        task_of(&all, "LOC-90").status_category.as_deref(),
        Some("in_progress"),
        "All sees org B's session working on it"
    );

    let a = bound(w.org_a);
    let pa = page(&w, &a, WorkTreeFilters::default());
    assert_eq!(
        task_of(&pa, "LOC-90").status_category.as_deref(),
        Some("todo"),
        "org A must not see org B's session lift this to in_progress"
    );
}

/// Fix round 3 (item 5): `to_task`'s `archived` now reads `item_status`
/// (the live-lifted value), not the raw stored `status_category` directly
/// — checked here rather than assumed safe. A "legacy" done item
/// (`status_category = 'done'`, `status_set_by` NULL — written before that
/// column existed, or by hand) that a session resumes work on is lifted to
/// `in_progress` by the same live rule that protects a tracked done/derived
/// stamp from a fresh working session; either way `archived` stays false,
/// because the very link that lifts the status also makes
/// `counts.active >= 1` for the same task. This pins that interaction.
#[test]
fn a_legacy_done_item_a_session_resumes_is_lifted_and_stays_unarchived() {
    let w = world();
    let item =
        w.st.lock()
            .unwrap()
            .create_local_work_item(Some("LOC-91"), "Legacy done")
            .unwrap();
    // A narrow, deliberate raw UPDATE (not the banned pattern of faking a
    // person's/tracker's status through one): simulates a `done` stamped
    // before `status_set_by` existed, which `stamp_derived_done`/
    // `set_item_status` — the store's only real writers — always pair with
    // one, so there is no other way to reach this state through the store.
    w.st.lock()
        .unwrap()
        .conn_ref()
        .execute(
            "UPDATE work_items SET status_category = 'done' WHERE id = ?1",
            rusqlite::params![item.id],
        )
        .unwrap();
    link(&w, w.s1, item.id, true);
    mark_working(&w, w.s1, "c-loc-91");

    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "LOC-91");
    assert_eq!(
        t.status_category.as_deref(),
        Some("in_progress"),
        "nobody tracked this done, so the live lift applies"
    );
    assert!(
        !t.archived,
        "a session working it now is not archived, lifted status or not"
    );
}

// ---------------------------------------------------------------------------
// Shared work context: tree fields, hidden proposals, the task page's data
// ---------------------------------------------------------------------------

fn step(
    text: &str,
    state: crate::service::work::steps::StepState,
) -> crate::service::work::steps::StepEvent {
    crate::service::work::steps::StepEvent {
        native_id: format!("task:{text}"),
        text: Some(text.into()),
        state: Some(state),
        agent: "claude_code",
    }
}

#[test]
fn proposals_are_not_tasks_and_native_children_name_their_parent() {
    let w = world();
    let (parent, sub, prop, rejected) = {
        let s = w.st.lock().unwrap();
        let pid = s.upsert_project("acme", "web", "/src/web").unwrap();
        let parent = s
            .create_native_item(&crate::store::NativeItem {
                title: "Ship v1",
                project_id: Some(pid),
                notes: Some("n"),
                ..Default::default()
            })
            .unwrap();
        let sub = s
            .create_native_item(&crate::store::NativeItem {
                title: "Changelog",
                parent_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
        let prop = s
            .propose_subtask(&crate::store::Proposal {
                parent_id: parent.id,
                title: "Idea",
                notes: None,
                why: Some("w"),
                proposed_by: "x",
            })
            .unwrap();
        let rejected = s
            .propose_subtask(&crate::store::Proposal {
                parent_id: parent.id,
                title: "Bad idea",
                notes: None,
                why: None,
                proposed_by: "x",
            })
            .unwrap();
        s.decide_proposal(rejected.id, false).unwrap();
        (parent, sub, prop, rejected)
    };
    let p = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            archived: Some(true),
            ..Default::default()
        },
    );
    assert!(
        p.tasks
            .iter()
            .all(|t| t.item_id != Some(prop.id) && t.item_id != Some(rejected.id)),
        "a proposal is not a task"
    );
    let pt = p
        .tasks
        .iter()
        .find(|t| t.item_id == Some(parent.id))
        .unwrap();
    assert_eq!(
        (
            pt.origin.as_str(),
            pt.project_label.as_deref(),
            pt.open_proposals
        ),
        ("manual", Some("acme/web"), 1)
    );
    assert_eq!(pt.parent_task_id, None);
    let st = p.tasks.iter().find(|t| t.item_id == Some(sub.id)).unwrap();
    assert_eq!(
        st.parent_task_id.as_deref(),
        Some(format!("item:{}", parent.id).as_str())
    );
    let tracker = task_of(&p, "TK-1");
    assert_eq!(
        (tracker.origin.as_str(), tracker.parent_task_id.as_deref()),
        ("detected", None)
    );
    let d = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.notes.as_deref(), Some("n"));
    assert_eq!(
        d.subtasks.iter().map(|x| x.item_id).collect::<Vec<_>>(),
        vec![sub.id]
    );
    assert_eq!(
        d.proposals.iter().map(|x| x.item_id).collect::<Vec<_>>(),
        vec![prop.id]
    );
    assert_eq!(d.proposals[0].why.as_deref(), Some("w"));
    assert_eq!(
        d.rejected_proposals
            .iter()
            .map(|x| x.item_id)
            .collect::<Vec<_>>(),
        vec![rejected.id]
    );

    // Accepted, the proposal is an ordinary subtask (and a task of the tree).
    w.st.lock().unwrap().decide_proposal(prop.id, true).unwrap();
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert!(p.tasks.iter().any(|t| t.item_id == Some(prop.id)));
    let d = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", parent.id)).unwrap();
    assert_eq!(
        d.subtasks.iter().map(|x| x.item_id).collect::<Vec<_>>(),
        vec![sub.id, prop.id]
    );
    assert_eq!(d.subtasks[1].origin, "proposed");
    assert!(d.proposals.is_empty());
    let pt = page(&w, &OrgScope::All, WorkTreeFilters::default())
        .tasks
        .into_iter()
        .find(|t| t.item_id == Some(parent.id))
        .unwrap();
    assert_eq!(pt.open_proposals, 0);
}

#[test]
fn a_task_page_shows_the_jobs_result_and_its_sessions_steps() {
    let w = world();
    let (parent, job_item) = {
        let s = w.st.lock().unwrap();
        let parent = s
            .create_native_item(&crate::store::NativeItem {
                title: "Ship v1",
                ..Default::default()
            })
            .unwrap();
        s.set_claude_session_id(w.s1, "c-s1").unwrap();
        s.link_session_work(w.s1, WorkTarget::Item(parent.id), "manual")
            .unwrap();
        s.record_steps(
            "c-s1",
            None,
            "hook",
            &[step(
                "Read it",
                crate::service::work::steps::StepState::Completed,
            )],
        )
        .unwrap();
        let job = s.insert_task(None, Some(w.s2), "Changelog", "n").unwrap();
        let job_item = s
            .create_agent_task_item(&job, Some(parent.id), None)
            .unwrap();
        s.finish_task(job.id, "done", Some("CHANGELOG.md written"), None)
            .unwrap();
        (parent, job_item)
    };
    let d = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.jobs.len(), 1);
    assert_eq!(d.jobs[0].item_id, job_item.id);
    assert_eq!(d.jobs[0].state, "done");
    assert_eq!(d.jobs[0].result.as_deref(), Some("CHANGELOG.md written"));
    assert_eq!(d.jobs[0].worker.as_deref(), Some("two"));
    assert_eq!(
        (
            d.subtasks[0].origin.as_str(),
            d.subtasks[0].job_state.as_deref()
        ),
        ("agent", Some("done"))
    );
    assert_eq!(
        d.steps
            .iter()
            .flat_map(|g| g.steps.iter())
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Read it"]
    );
    assert_eq!(d.steps[0].label, "one");
    assert_eq!(d.steps[0].claude_session_id, "c-s1");

    // The job mirror's own page carries its result; the tree its state.
    let jd = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", job_item.id)).unwrap();
    assert_eq!(jd.job_result.as_deref(), Some("CHANGELOG.md written"));
    assert_eq!(jd.task.job_state.as_deref(), Some("done"));
    assert_eq!(jd.task.origin, "agent");
}

#[test]
fn a_subtasks_steps_roll_up_to_its_parent_and_count_its_live_sessions() {
    let w = world();
    let (parent, sub) = {
        let s = w.st.lock().unwrap();
        let parent = s
            .create_native_item(&crate::store::NativeItem {
                title: "Ship v1",
                ..Default::default()
            })
            .unwrap();
        let sub = s
            .create_native_item(&crate::store::NativeItem {
                title: "Changelog",
                parent_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
        s.set_claude_session_id(w.s2, "c-s2").unwrap();
        s.link_session_work(w.s2, WorkTarget::Item(sub.id), "manual")
            .unwrap();
        s.record_steps(
            "c-s2",
            None,
            "hook",
            &[step(
                "Write it",
                crate::service::work::steps::StepState::InProgress,
            )],
        )
        .unwrap();
        (parent, sub)
    };
    let d = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.subtasks[0].item_id, sub.id);
    assert_eq!(d.subtasks[0].live_sessions, 1);
    assert_eq!(d.steps.len(), 1);
    assert_eq!(
        (
            d.steps[0].label.as_str(),
            d.steps[0].steps[0].state.as_str()
        ),
        ("two", "in_progress")
    );
}

#[test]
fn a_per_host_token_reads_agent_text_fenced() {
    let w = world();
    let parent = {
        let s = w.st.lock().unwrap();
        let parent = s
            .create_native_item(&crate::store::NativeItem {
                title: "Ship v1",
                notes: Some("the notes"),
                ..Default::default()
            })
            .unwrap();
        s.propose_subtask(&crate::store::Proposal {
            parent_id: parent.id,
            title: "Idea",
            notes: None,
            why: Some("the why"),
            proposed_by: "x",
        })
        .unwrap();
        s.set_claude_session_id(w.s1, "c-s1").unwrap();
        s.link_session_work(w.s1, WorkTarget::Item(parent.id), "manual")
            .unwrap();
        s.record_steps(
            "c-s1",
            None,
            "hook",
            &[step(
                "the step",
                crate::service::work::steps::StepState::Pending,
            )],
        )
        .unwrap();
        parent
    };
    let end = crate::mcp::guard::UNTRUSTED_END;
    let host = OrgScope::for_host(&w.st.lock().unwrap(), "h1").unwrap();
    let d = task(&w.st, &vs(&host), &format!("item:{}", parent.id)).unwrap();
    let notes = d.notes.unwrap();
    assert!(
        notes.contains("the notes") && notes.contains(end),
        "{notes}"
    );
    let why = d.proposals[0].why.clone().unwrap();
    assert!(why.contains("the why") && why.contains(end), "{why}");
    let text = &d.steps[0].steps[0].text;
    assert!(text.contains("the step") && text.contains(end), "{text}");

    // A person reads it as is.
    let d = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.notes.as_deref(), Some("the notes"));
    assert_eq!(d.steps[0].steps[0].text, "the step");
}

#[test]
fn an_untitled_task_borrows_its_first_sessions_name() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        s.link_session_work(w.s1, WorkTarget::Ref("#333"), "manual")
            .unwrap();
    }
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "#333");
    assert!(t.title_derived);
    assert_eq!(t.title, "one");
    // A titled task never borrows.
    assert!(!task_of(&p, "TK-1").title_derived);
}

/// Redesign step 6.3: a task with an unmet dependency is Blocked, outside a
/// mission too, and stops being Blocked once what it waits for is done.
#[test]
fn a_task_waiting_on_unfinished_work_is_blocked_until_it_is_done() {
    let w = world();
    lock(&w.st)
        .unwrap()
        .add_item_dep(w.t1, w.t2, "person", "test")
        .unwrap();
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t1 = task_of(&p, "TK-1");
    assert!(t1.blocked);
    assert_eq!(t1.blocked_by, vec![format!("item:{}", w.t2)]);
    assert!(
        !task_of(&p, "TK-2").blocked,
        "the dependency itself waits on nothing"
    );
    assert!(!task_of(&p, "TK-3").blocked);

    // TK-2 is done: nothing blocks TK-1 any more.
    lock(&w.st)
        .unwrap()
        .upsert_tracker_item(
            w.tracker,
            &TrackerItemWrite {
                external_id: "2".into(),
                key: Some("TK-2".into()),
                title: "Audit log".into(),
                status_name: "Done".into(),
                status_category: "done".into(),
                containers: vec!["TP".into()],
                ..Default::default()
            },
        )
        .unwrap();
    let p = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            archived: Some(true),
            ..Default::default()
        },
    );
    let t1 = task_of(&p, "TK-1");
    assert!(!t1.blocked);
    assert!(t1.blocked_by.is_empty());
}

/// Redesign step 6.3: a done task is never Blocked, whatever it waits for.
#[test]
fn a_done_task_is_not_blocked() {
    let w = world();
    let s = lock(&w.st).unwrap();
    s.add_item_dep(w.t2, w.t3, "person", "test").unwrap();
    s.upsert_tracker_item(
        w.tracker,
        &TrackerItemWrite {
            external_id: "2".into(),
            key: Some("TK-2".into()),
            title: "Audit log".into(),
            status_name: "Done".into(),
            status_category: "done".into(),
            containers: vec!["TP".into()],
            ..Default::default()
        },
    )
    .unwrap();
    drop(s);
    let p = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            archived: Some(true),
            ..Default::default()
        },
    );
    assert!(!task_of(&p, "TK-2").blocked);
}

/// Redesign step 6.3: a task's cost is the spend of its sessions, each
/// counted once, and the org's group header sums its tasks.
#[test]
fn a_tasks_cost_sums_its_sessions_and_the_group_header_sums_its_tasks() {
    let w = world();
    {
        let s = lock(&w.st).unwrap();
        for (id, micros) in [(w.s1, 1_250_000), (w.s2, 500_000)] {
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET usage_cost_micros = ?1 WHERE id = ?2",
                    rusqlite::params![micros, id],
                )
                .unwrap();
        }
    }
    link(&w, w.s1, w.t1, true);
    link(&w, w.s2, w.t1, false);
    link(&w, w.s2, w.t2, true);
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(task_of(&p, "TK-1").cost_micros, 1_750_000);
    assert_eq!(task_of(&p, "TK-2").cost_micros, 500_000);
    assert_eq!(task_of(&p, "TK-3").cost_micros, 0);
    let org_total: i64 = p
        .groups
        .iter()
        .filter(|g| g.org_id == Some(w.org_a))
        .map(|g| g.cost_micros)
        .sum();
    assert_eq!(org_total, 2_250_000);
    // A scope that cannot see the sessions sees no spend either.
    let p = page(&w, &strict(w.org_b), WorkTreeFilters::default());
    assert!(p.tasks.iter().all(|t| t.cost_micros == 0));
}

/// Step 2.8: what a rule, Jev or an LLM proposes about a task rides on it
/// in the shape a session row carries; a task nothing decided carries none.
#[test]
fn a_task_carries_what_is_proposed_about_it() {
    let w = world();
    let run = |answer: &str| crate::store::NewDecisionRun {
        at: 1_000,
        feature: "duplicate".into(),
        org_id: Some(w.org_a),
        subject_kind: crate::store::PROPOSAL_SUBJECT_WORK_ITEM.into(),
        subject_id: w.t1.to_string(),
        mode: "assist".into(),
        provider: "jev".into(),
        question_version: "duplicate.v1".into(),
        answer: Some(answer.into()),
        confidence: Some(0.9),
        ..Default::default()
    };
    let id = lock(&w.st)
        .unwrap()
        .insert_decision_run(&run("keep_both"))
        .unwrap();
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    assert_eq!(
        task_of(&p, "TK-1").proposals,
        vec![crate::store::DecisionProposal {
            feature: "duplicate".into(),
            value: "keep_both".into(),
            source: "jev".into(),
            reason: None,
            confidence_pct: Some(90),
            run_id: Some(id),
            at: Some(1_000),
            linked: None,
        }]
    );
    assert!(task_of(&p, "TK-3").proposals.is_empty());
}

// ---------------------------------------------------------------------------
// Redesign step 6.2: a named assignee, a tracker column, and group by org,
// person, mission, account or repo.

/// A tracker item with its own column and assignees.
fn item_with(w: &W, ext: &str, key: &str, column: &str, category: &str, who: &[&str]) -> i64 {
    w.st.lock()
        .unwrap()
        .upsert_tracker_item(
            w.tracker,
            &TrackerItemWrite {
                external_id: ext.into(),
                key: Some(key.into()),
                title: key.into(),
                status_name: column.into(),
                status_category: category.into(),
                containers: vec!["TP".into()],
                assignees: who.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
}

fn by(group_by: &str) -> WorkTreeFilters {
    WorkTreeFilters {
        group_by: Some(group_by.into()),
        ..Default::default()
    }
}

/// The section header of `key`'s task in a page.
fn group_of<'a>(p: &'a TreePage, key: &str) -> &'a GroupRef {
    &task_of(p, key).group
}

#[test]
fn a_tree_query_filters_by_tracker_column_and_by_a_named_assignee() {
    let w = world();
    item_with(
        &w,
        "10",
        "TK-10",
        "QA Review",
        "in_progress",
        &["Ana Novak"],
    );
    item_with(
        &w,
        "11",
        "TK-11",
        "In Progress",
        "in_progress",
        &["Ben", "ana novak"],
    );
    item_with(&w, "12", "TK-12", "qa review", "in_progress", &[]);
    let all = &OrgScope::All;

    // The column is the tracker's own name, any case; the category filter
    // could not tell QA Review from In Progress.
    let p = page(
        &w,
        all,
        WorkTreeFilters {
            status_name: Some("QA review".into()),
            ..Default::default()
        },
    );
    assert_eq!(keys(&p), vec!["TK-10", "TK-12"]);
    assert!(p.tasks.iter().all(|t| t
        .status_name
        .as_deref()
        .unwrap()
        .eq_ignore_ascii_case("qa review")));

    // A named person, any case, among several assignees.
    let p = page(
        &w,
        all,
        WorkTreeFilters {
            assignee: Some(" ANA NOVAK ".into()),
            ..Default::default()
        },
    );
    assert_eq!(keys(&p), vec!["TK-10", "TK-11"]);

    // Both at once narrow to their intersection.
    let p = page(
        &w,
        all,
        WorkTreeFilters {
            assignee: Some("ana novak".into()),
            status_name: Some("QA Review".into()),
            ..Default::default()
        },
    );
    assert_eq!(keys(&p), vec!["TK-10"]);
}

#[test]
fn a_tree_query_groups_by_org_person_and_repo() {
    let w = world();
    item_with(&w, "10", "TK-10", "QA Review", "in_progress", &["Ana"]);
    link(&w, w.s1, w.t1, true);
    let all = &OrgScope::All;

    // org: one section per org.
    let p = page(&w, all, by("org"));
    assert!(p.tasks.iter().all(|t| t.group.id == "org"));
    assert_eq!(p.groups.len(), 1);
    assert_eq!(p.groups[0].org_id, Some(w.org_a));
    assert_eq!(p.groups[0].count as usize, p.tasks.len());

    // person: the first assignee; nobody is `none`, last.
    let p = page(&w, all, by("person"));
    assert_eq!(group_of(&p, "TK-10").id, "person:ana");
    assert_eq!(group_of(&p, "TK-10").label, "Ana");
    assert_eq!(group_of(&p, "TK-1").id, "none");
    assert_eq!(p.groups.last().unwrap().group.id, "none");

    // repo: the repo its sessions run in.
    let p = page(&w, all, by("repo"));
    assert_eq!(group_of(&p, "TK-1").id, "repo:acme/api");
    assert_eq!(group_of(&p, "TK-3").id, "none");

    // A section of a grouping reads by itself with `group`.
    let p = page(
        &w,
        all,
        WorkTreeFilters {
            group: Some("person:ana".into()),
            ..by("person")
        },
    );
    assert_eq!(keys(&p), vec!["TK-10"]);
}

#[test]
fn a_tree_query_groups_by_mission_and_names_only_missions_the_caller_reads() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        let m = s
            .create_mission(
                &crate::store::NewMission {
                    org_id: Some(w.org_a),
                    owner_person_id: None,
                    root_item_id: None,
                    name: "Ship login",
                    goal: "Login works",
                    non_goals: None,
                    done_when: &[],
                    mode: None,
                    level: None,
                },
                "fleet",
            )
            .unwrap();
        s.set_mission_item(m.id, w.t1, true, "fleet").unwrap();
        let other = s
            .create_mission(
                &crate::store::NewMission {
                    org_id: Some(w.org_b),
                    owner_person_id: None,
                    root_item_id: None,
                    name: "Beta's secret",
                    goal: "Elsewhere",
                    non_goals: None,
                    done_when: &[],
                    mode: None,
                    level: None,
                },
                "fleet",
            )
            .unwrap();
        s.set_mission_item(other.id, w.t2, true, "fleet").unwrap();
    }
    let p = page(&w, &OrgScope::All, by("mission"));
    assert_eq!(group_of(&p, "TK-1").label, "Ship login");
    assert!(group_of(&p, "TK-1").id.starts_with("mission:"));
    assert_eq!(group_of(&p, "TK-3").id, "none");

    // Org A's client sees TK-2 but not Beta's mission: no name, no section.
    let p = page(&w, &strict(w.org_a), by("mission"));
    assert_eq!(group_of(&p, "TK-1").label, "Ship login");
    assert_eq!(group_of(&p, "TK-2").id, "none");
    let dump = serde_json::to_string(&p).unwrap();
    assert!(!dump.contains("Beta's secret"), "{dump}");
}

/// Orbit Fleet G7.6: a read that asks names each task's mission and its
/// wave (the Board card's chip), only for missions the caller reads; a read
/// that does not ask pays nothing and names none. `done_when` rides along.
#[test]
fn a_tree_read_with_missions_names_the_mission_and_wave_of_each_task() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        let mission = |org: i64, name: &str| {
            s.create_mission(
                &crate::store::NewMission {
                    org_id: Some(org),
                    owner_person_id: None,
                    root_item_id: None,
                    name,
                    goal: "g",
                    non_goals: None,
                    done_when: &[],
                    mode: None,
                    level: None,
                },
                "fleet",
            )
            .unwrap()
        };
        let m = mission(w.org_a, "Ship login");
        s.set_mission_item(m.id, w.t1, true, "fleet").unwrap();
        s.set_mission_item(m.id, w.t2, true, "fleet").unwrap();
        // TK-2 waits on TK-1: wave 2. TK-1 waits on TK-3, outside the
        // mission: still wave 1.
        s.add_item_dep(w.t2, w.t1, "person", "test").unwrap();
        s.add_item_dep(w.t1, w.t3, "person", "test").unwrap();
        s.set_item_done_when(w.t2, &["ci".to_string(), "review".to_string()], "test")
            .unwrap();
        let other = mission(w.org_b, "Beta's secret");
        s.set_mission_item(other.id, w.t3, true, "fleet").unwrap();
    }
    let read = |scope: &OrgScope, with_missions: bool| {
        tree(
            &w.st,
            &vs(scope),
            &TreeArgs {
                limit: Some(200),
                with_missions,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let p = read(&OrgScope::All, true);
    let m1 = task_of(&p, "TK-1").mission.clone().expect("TK-1's mission");
    assert_eq!((m1.name.as_str(), m1.wave), ("Ship login", Some(1)));
    let m2 = task_of(&p, "TK-2").mission.clone().expect("TK-2's mission");
    assert_eq!((m2.id, m2.wave), (m1.id, Some(2)));
    assert_eq!(
        task_of(&p, "TK-3")
            .mission
            .as_ref()
            .map(|m| m.name.as_str()),
        Some("Beta's secret")
    );
    assert_eq!(task_of(&p, "TK-2").done_when, vec!["ci", "review"]);
    assert!(task_of(&p, "TK-1").done_when.is_empty());

    // Not asked: no mission on any task, and none on the wire.
    let p = read(&OrgScope::All, false);
    assert!(p.tasks.iter().all(|t| t.mission.is_none()));
    assert!(!serde_json::to_string(&p).unwrap().contains("\"mission\""));

    // Org A's client sees TK-3 but not Beta's mission: no name.
    let p = read(&strict(w.org_a), true);
    assert_eq!(
        task_of(&p, "TK-1").mission.as_ref().map(|m| m.wave),
        Some(Some(1))
    );
    assert!(task_of(&p, "TK-3").mission.is_none());
    assert!(!serde_json::to_string(&p).unwrap().contains("Beta's secret"));
}

#[test]
fn a_tree_query_groups_by_the_account_its_sessions_run_on() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: "acc-1".into(),
            email: Some("dev@acme.example".into()),
            ..Default::default()
        })
        .unwrap();
        // Probed, but nothing to name it by.
        s.upsert_account(&crate::store::AccountRow {
            uuid: "acc-2".into(),
            ..Default::default()
        })
        .unwrap();
        let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
        s.upsert_session("one", "h1", Some(pid), None, 1, 1, "running", Some("acc-1"))
            .unwrap();
        s.upsert_session("two", "h1", Some(pid), None, 1, 1, "running", Some("acc-2"))
            .unwrap();
    }
    link(&w, w.s1, w.t1, true);
    link(&w, w.s2, w.t2, true);
    let p = page(&w, &OrgScope::All, by("account"));
    assert_eq!(group_of(&p, "TK-1").id, "account:acc-1");
    assert_eq!(group_of(&p, "TK-1").label, "dev@acme.example");
    // An account with no name is still a section, by its id.
    assert_eq!(group_of(&p, "TK-2").label, "Account acc-2");
    assert_eq!(group_of(&p, "TK-3").id, "none");
}

/// Sprints design 2026-09-28 §6a: a group by sprint or release, named only
/// for the buckets the caller may see.
#[test]
fn a_tree_query_groups_by_sprint_and_release() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        let bucket = |kind: &str, name: &str, org: Option<i64>| {
            s.create_bucket(&crate::store::NewBucket {
                kind,
                name,
                org_id: org,
                owner_person_id: None,
                starts_at: None,
                ends_at: None,
                goal: None,
            })
            .unwrap()
            .id
        };
        let sprint = bucket("sprint", "Sprint 24", Some(w.org_a));
        s.add_bucket_item(sprint, w.t1).unwrap();
        // An unassigned sprint: a client bound to org A without unassigned
        // work does not see it.
        let secret = bucket("sprint", "Unassigned sprint", None);
        s.add_bucket_item(secret, w.t2).unwrap();
        // Two releases: the one still planned names the section.
        let shipped = bucket("release", "0.2.0", Some(w.org_a));
        s.add_bucket_item(shipped, w.t1).unwrap();
        s.update_bucket(
            shipped,
            None,
            &crate::store::BucketPatch {
                state: Some("released".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let next = bucket("release", "0.3.0", Some(w.org_a));
        s.add_bucket_item(next, w.t1).unwrap();
    }
    let p = page(&w, &OrgScope::All, by("sprint"));
    assert_eq!(group_of(&p, "TK-1").label, "Sprint 24");
    assert!(group_of(&p, "TK-1").id.starts_with("sprint:"));
    assert_eq!(group_of(&p, "TK-1").source, "sprint");
    assert_eq!(group_of(&p, "TK-3").id, "none");
    assert_eq!(group_of(&p, "TK-3").label, "No sprint");

    let p = page(&w, &OrgScope::All, by("release"));
    assert_eq!(group_of(&p, "TK-1").label, "0.3.0");
    assert_eq!(group_of(&p, "TK-3").label, "No release");

    // A section of the grouping reads by itself.
    let id = group_of(&page(&w, &OrgScope::All, by("sprint")), "TK-1")
        .id
        .clone();
    let p = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            group: Some(id),
            ..by("sprint")
        },
    );
    assert_eq!(keys(&p), vec!["TK-1"]);

    let p = page(&w, &OrgScope::All, by("sprint"));
    assert_eq!(group_of(&p, "TK-2").label, "Unassigned sprint");
    // Org A's client sees TK-2 but not the unassigned sprint: no name, no
    // section.
    let p = page(&w, &strict(w.org_a), by("sprint"));
    assert_eq!(group_of(&p, "TK-2").id, "none");
    let dump = serde_json::to_string(&p).unwrap();
    assert!(!dump.contains("Unassigned sprint"), "{dump}");
}

/// Sprints design 2026-09-28 §3, §6a: an epic is its own section, its
/// tasks sit in it, and it carries its children's roll-up.
#[test]
fn a_tree_query_groups_by_epic_and_rolls_its_children_up() {
    let w = world();
    let (epic, done_one) = {
        let s = w.st.lock().unwrap();
        let native = |title: &str, parent: Option<i64>| {
            s.create_native_item(&crate::store::NativeItem {
                title,
                parent_id: parent,
                project_id: None,
                notes: None,
            })
            .unwrap()
        };
        let epic = native("Login revamp", None);
        s.set_local_epic(epic.id, true).unwrap();
        native("Fix login", Some(epic.id));
        let done_one = native("Write tests", Some(epic.id));
        s.set_item_status(done_one.id, "done").unwrap();
        native("Loose task", None);
        (epic, done_one)
    };
    let key = |id: i64| format!("TASK-{id}");
    let p = page(&w, &OrgScope::All, by("epic"));
    let e = task_of(&p, &key(epic.id));
    assert!(e.epic);
    assert_eq!((e.children_total, e.children_done), (2, 1));
    assert_eq!(e.group.id, format!("epic:{}", epic.id));
    assert_eq!(e.group.label, format!("{} · Login revamp", key(epic.id)));
    let d = task_of(&p, &key(done_one.id));
    assert!(!d.epic);
    assert_eq!(d.group.id, format!("epic:{}", epic.id));
    assert_eq!(d.parent_task_id, Some(format!("item:{}", epic.id)));
    assert_eq!(group_of(&p, &key(epic.id + 3)).label, "No epic");

    // A subtask of an epic's task sits in the epic's section too, two
    // levels down (owner decision 2026-10-10: three levels).
    let sub = {
        let s = w.st.lock().unwrap();
        s.create_native_item(&crate::store::NativeItem {
            title: "Edge case",
            parent_id: Some(done_one.id),
            project_id: None,
            notes: None,
        })
        .unwrap()
    };
    let p = page(&w, &OrgScope::All, by("epic"));
    let st = task_of(&p, &key(sub.id));
    assert_eq!(st.group.id, format!("epic:{}", epic.id));
    assert_eq!((st.level, task_of(&p, &key(epic.id)).level), (3, 1));
    assert_eq!(group_of(&p, "TK-1").id, "none");
}

/// Task comments: served with the task, oldest first; who wrote one is a
/// device label, withheld from a scoped caller as a placement's author is.
#[test]
fn a_task_serves_its_comments_and_withholds_their_author_from_a_scoped_caller() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        s.add_comment(w.t1, "client:phone", Some(4), "first")
            .unwrap();
        s.add_comment(w.t1, "desktop", None, "second").unwrap();
    }
    let all = task(&w.st, &vs(&OrgScope::All), &format!("item:{}", w.t1)).unwrap();
    assert_eq!(
        all.comments
            .iter()
            .map(|c| (c.author.as_str(), c.body.as_str()))
            .collect::<Vec<_>>(),
        vec![("client:phone", "first"), ("desktop", "second")]
    );
    let scoped = task(&w.st, &vs(&strict(w.org_a)), &format!("item:{}", w.t1)).unwrap();
    assert_eq!(scoped.comments.len(), 2);
    assert!(scoped
        .comments
        .iter()
        .all(|c| c.author.is_empty() && c.author_person_id.is_none()));
}

/// Owner decision 2026-10-10: one person does not learn another's device
/// names. A comment's author shows to its own person; a placement's author
/// (a device label with no person) to nobody but the hub itself and the one
/// person of a one-person hub.
#[test]
fn a_person_sees_their_own_device_names_and_no_one_elses() {
    use crate::mcp::auth::{Caller, ClientRef, TokenMode};
    use crate::service::view_scope::ViewScope;
    // A person's paired device, scoped the way a request is.
    let device = |w: &W, p: i64| -> ViewScope {
        Caller {
            api: None,
            host_alias: None,
            client: Some(ClientRef {
                id: 7,
                name: "phone".into(),
                trusted: false,
                org_id: None,
                person_id: Some(p),
            }),
            mode: TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        }
        .view_scope(&w.st.lock().unwrap())
        .unwrap()
    };
    let w = world();
    let ana = w.st.lock().unwrap().create_person("ana", None).unwrap().id;
    let id = format!("item:{}", w.t1);
    w.st.lock()
        .unwrap()
        .add_comment(w.t1, "client:ana-phone", Some(ana), "mine")
        .unwrap();
    structure::place(
        &w.st,
        &vs(&OrgScope::All),
        &id,
        Some("Now"),
        None,
        Some(0),
        "client:bo-laptop",
    )
    .unwrap();
    let authors = |v: &ViewScope| -> Vec<String> {
        task(&w.st, v, &id)
            .unwrap()
            .comments
            .into_iter()
            .map(|c| c.author)
            .collect()
    };
    let placed_by = |v: &ViewScope| task(&w.st, v, &id).unwrap().placement.unwrap().updated_by;
    // The hub itself sees every device.
    assert_eq!(
        placed_by(&vs(&OrgScope::All)).as_deref(),
        Some("client:bo-laptop")
    );
    // Each person sees their own device names and no one else's.
    let bo = w.st.lock().unwrap().create_person("bo", None).unwrap().id;
    w.st.lock()
        .unwrap()
        .add_comment(w.t1, "client:bo-laptop", Some(bo), "theirs")
        .unwrap();
    assert_eq!(authors(&device(&w, ana)), ["client:ana-phone", ""]);
    assert_eq!(authors(&device(&w, bo)), ["", "client:bo-laptop"]);
    assert_eq!(placed_by(&device(&w, ana)), None);
    assert_eq!(placed_by(&device(&w, bo)), None);
    assert_eq!(
        authors(&vs(&OrgScope::All)),
        ["client:ana-phone", "client:bo-laptop"]
    );
}

#[test]
fn an_unknown_grouping_is_refused_not_ignored() {
    let w = world();
    let err = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            filters: by("colour"),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    assert!(err.message.contains("group_by"), "{}", err.message);
    // The default grouping is the task's own group, as before.
    let p = page(&w, &OrgScope::All, by("group"));
    assert_eq!(
        p.tasks,
        page(&w, &OrgScope::All, WorkTreeFilters::default()).tasks
    );
}

/// The Work panel's chips (redesign board "Work · tasks with filters
/// open"): each task's stage, several stages and several orgs at once, and
/// how many tasks the filters hide.
#[test]
fn stages_and_orgs_filter_several_at_once_and_say_what_they_hide() {
    let w = world();
    done_item(&w, "9", "TK-9");
    let p = page(&w, &OrgScope::All, hiding());
    // In progress in the tracker; done is archived out of this view.
    assert_eq!(task_of(&p, "TK-1").stage, "in_progress");
    assert_eq!(p.hidden_by_filters, 0, "no filter hides nothing");
    let all = p.total;

    let in_review = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            stages: vec!["in_review".into(), "blocked".into()],
            ..hiding()
        },
    );
    assert_eq!(in_review.total, 0);
    // The archived TK-9 is the archived row's, not this one's.
    assert_eq!(in_review.hidden_by_filters, all);
    assert_eq!(in_review.archived_hidden, 0);

    let both = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            stages: vec!["in_progress".into(), "done".into()],
            orgs: vec![IdOrWord::Id(w.org_b), IdOrWord::Id(w.org_a)],
            ..hiding()
        },
    );
    // A Done chip shows the archived done task, as `status: done` does.
    assert_eq!(both.total, all + 1);
    assert!(keys(&both).contains(&"TK-9".to_string()));
    assert_eq!(both.hidden_by_filters, 0);

    let other_org = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            orgs: vec![IdOrWord::Id(w.org_b)],
            ..hiding()
        },
    );
    assert_eq!(other_org.total, 0);
    assert_eq!(other_org.hidden_by_filters, all);

    let done = page(
        &w,
        &OrgScope::All,
        WorkTreeFilters {
            stages: vec!["done".into()],
            archived: Some(true),
            ..Default::default()
        },
    );
    assert_eq!(keys(&done), vec!["TK-9".to_string()]);
    assert_eq!(task_of(&done, "TK-9").stage, "done");
}

#[test]
fn a_stage_or_an_org_word_it_does_not_know_is_refused() {
    let bad_stage = WorkTreeFilters {
        stages: vec!["doing".into()],
        ..Default::default()
    };
    assert!(check_filters(&bad_stage).is_err());
    let bad_org = WorkTreeFilters {
        orgs: vec![IdOrWord::Word("all".into())],
        ..Default::default()
    };
    assert!(check_filters(&bad_org).is_err());
    let ok = WorkTreeFilters {
        orgs: vec![IdOrWord::Word("none".into()), IdOrWord::Id(3)],
        stages: vec!["backlog".into()],
        ..Default::default()
    };
    assert!(check_filters(&ok).is_ok());
}

#[test]
fn a_stage_is_the_first_of_done_blocked_review_progress() {
    assert_eq!(
        stage_of(Some("done"), Some("In review"), true, true, 1),
        "done"
    );
    assert_eq!(stage_of(Some("todo"), None, true, true, 1), "blocked");
    assert_eq!(
        stage_of(Some("in_progress"), Some("QA Review"), false, false, 0),
        "in_review"
    );
    assert_eq!(stage_of(Some("todo"), None, false, true, 1), "in_review");
    assert_eq!(stage_of(Some("todo"), None, false, false, 1), "in_progress");
    assert_eq!(stage_of(None, None, false, false, 0), "backlog");
}
