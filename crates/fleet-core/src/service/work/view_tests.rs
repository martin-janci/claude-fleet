//! The Work view's read model (work graph M14.1b), one test per use case of
//! the spec (`docs/superpowers/specs/2026-09-27-work-view-design.md`) where
//! the hub owns the behaviour. Some rows the reads answer are seeded
//! (`Store::seed_*`); the writes a person makes through the hub (M14.1c)
//! are `view_write_tests.rs`. The org boundary per caller is the isolation
//! matrix's (`mcp/tools/tests_isolation.rs`).

use super::*;
use crate::service::work::structure::{self, RuleInput};
use crate::service::work::{work_link, WorkLinkArgs};
use crate::store::{RuleConditions, TrackerConfig, TrackerItemWrite, WorkTarget};

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
        scope,
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
    session_tasks(&w.st, &OrgScope::All, sid).unwrap()
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
    let d = task(&w.st, &OrgScope::All, "item:1").unwrap();
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

    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
    let it = r
        .items
        .iter()
        .find(|i| i.task.key.as_deref() == Some("TK-3"))
        .unwrap();
    assert_eq!(it.kind, "suggestion");
    assert!(!it.why.is_empty());
    assert_eq!(it.alternatives.len(), 1, "TK-2 is the other guess");

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
    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
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
    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
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
    let ra = review(&w.st, &a, None, None).unwrap();
    let sa = session_tasks(&w.st, &a, w.s1).unwrap();
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
    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
    assert!(r
        .items
        .iter()
        .any(|i| i.kind == "cross_org" && i.link_id == forced));
    // A kept conflict (M14.1c's `ack`) leaves the inbox.
    w.st.lock().unwrap().seed_review_ack(forced);
    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
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
            &OrgScope::All,
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
        &OrgScope::All,
        &TreeArgs {
            limit: Some(7),
            ..Default::default()
        },
    )
    .unwrap();
    let err = tree(
        &w.st,
        &OrgScope::All,
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
        &OrgScope::All,
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
        &OrgScope::All,
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
    assert!(task(&w.st, &OrgScope::All, "ref:ZZ-5").is_ok());
    let local =
        w.st.lock()
            .unwrap()
            .create_local_work_item(Some("ZZ-5"), "Now named")
            .unwrap();
    let d = task(&w.st, &OrgScope::All, "ref:zz-5").unwrap();
    assert!(d.task.task_id == format!("item:{}", local.id) || d.task.task_id == "ref:ZZ-5");
    assert!(parse_task_id("bogus").is_err());
}

/// The in-memory org rule is the store's (`Store::item_org`).
#[test]
fn the_graph_and_the_store_agree_on_item_orgs() {
    let w = world();
    let local = {
        let s = w.st.lock().unwrap();
        let (it, _) = s.name_session_work(w.s1, None, "Unkeyed").unwrap();
        s.seed_local_item_org(it.id, Some(w.org_b));
        it.id
    };
    let s = w.st.lock().unwrap();
    let g = Graph::load(&s).unwrap();
    for id in [w.t1, w.t2, w.t3, local] {
        assert_eq!(
            g.item_org(&g.items[&id]),
            s.item_org(id).unwrap(),
            "item {id}"
        );
    }
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
        serde_json::to_value(task(&w.st, &OrgScope::All, "item:2").unwrap()).unwrap(),
    );
    write(
        "session_tasks",
        serde_json::to_value(links_of(&w, w.s1)).unwrap(),
    );
    write(
        "review",
        serde_json::to_value(review(&w.st, &OrgScope::All, None, None).unwrap()).unwrap(),
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
                &OrgScope::All,
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
        task(&w.st, &OrgScope::All, "item:2").unwrap().rules,
        vec![rule]
    );
    let rules = structure::rules(&w.st, &OrgScope::All).unwrap();
    assert_eq!(rules.len(), 1);

    // A person's placement of TK-2 only.
    w.st.lock()
        .unwrap()
        .seed_placement("item:2", Some("Security"), None);
    let d = task(&w.st, &OrgScope::All, "item:2").unwrap();
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
        task(&w.st, &bound(w.org_a), &tid).is_ok(),
        "unassigned: visible"
    );
    let imp = structure::org_impact(&w.st, &OrgScope::All, &tid, Some(w.org_b)).unwrap();
    assert!(imp.allowed);
    assert_eq!((imp.from_org, imp.to_org), (None, Some(w.org_b)));
    assert!(imp.links[0].becomes_cross_org, "s1 is org A's");
    assert_eq!(imp.hosts_losing, vec!["h1".to_string()]);
    assert!(!imp.impact_token.is_empty());
    for scope in [bound(w.org_a), strict(w.org_a)] {
        let err = structure::org_impact(&w.st, &scope, &tid, Some(w.org_b)).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }
    let err = structure::org_impact(&w.st, &OrgScope::All, &tid, None).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID, "org_id is required (0: none)");
    let t = structure::org_impact(&w.st, &OrgScope::All, "item:1", Some(w.org_b)).unwrap();
    assert_eq!(
        (t.allowed, t.reason.as_deref()),
        (false, Some("tracker_controlled"))
    );
    // Once the item is B's (M14.1c's `assign_org`, seeded), A no longer
    // receives it, nor its link on A's own session.
    w.st.lock()
        .unwrap()
        .seed_local_item_org(local, Some(w.org_b));
    let err = task(&w.st, &bound(w.org_a), &tid).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND, "answered as unknown");
    let st = session_tasks(&w.st, &bound(w.org_a), w.s1).unwrap();
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
    let st = session_tasks(&w.st, &a, w.s1).unwrap();
    assert_eq!(st.primary_link_id, None, "the hidden primary is not named");
    assert_eq!(st.links.len(), 1);
    let text = serde_json::to_string(&st).unwrap();
    assert!(!text.contains("HID-1") && !text.contains(&format!("\"link_id\":{hidden}")));
    let r = review(&w.st, &a, None, None).unwrap();
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
    assert!(session_tasks(&w.st, &bound(w.org_a), h2_session).is_ok());
    assert!(task(&w.st, &bound(w.org_a), bare).is_ok());

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
        let err = task(&w.st, &strict(w.org_a), &tid).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND, "{what}");
    }
    let err = session_tasks(&w.st, &strict(w.org_a), h2_session).unwrap_err();
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
    let first = tree(&w.st, &OrgScope::All, &args(None)).unwrap();
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
        let p = tree(&w.st, &OrgScope::All, &args(Some(c))).unwrap();
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
    let again = tree(&w.st, &OrgScope::All, &args(first.next_cursor.clone())).unwrap();
    let again2 = tree(&w.st, &OrgScope::All, &args(first.next_cursor)).unwrap();
    assert_eq!(again.tasks, again2.tasks);
    // A cursor that is not the hub's is refused, never read as a start.
    let err = tree(&w.st, &OrgScope::All, &args(Some("bogus".into()))).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    // The review inbox pages the same way.
    let err = review(&w.st, &OrgScope::All, Some("bogus"), None).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
}

#[path = "view_write_tests.rs"]
mod writes;
