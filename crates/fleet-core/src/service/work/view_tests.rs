//! The Work view's read model and its edits (work graph M14), one test per
//! use case of the spec (`docs/superpowers/specs/2026-09-27-work-view-design.md`)
//! where the hub owns the behaviour. The org boundary per caller is the
//! isolation matrix's (`mcp/tools/tests_isolation.rs`).

use super::*;
use crate::service::work::structure::{self, LinkDecision, RuleInput};
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

/// UC8: the primary moves atomically as a compare-and-set; the other link
/// stays; a second device acting on the old primary gets a conflict.
#[test]
fn moving_the_primary_keeps_every_link_and_refuses_a_stale_device() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    link(&w, w.s1, w.t2, false);
    let before = links_of(&w, w.s1);
    let old = before.primary_link_id.unwrap();
    let new = before
        .links
        .iter()
        .find(|l| !l.link.primary)
        .unwrap()
        .link
        .link_id;
    // Device 1.
    let row = work_link(
        &WorkLinkArgs {
            link_id: Some(new),
            expected_primary: Some(old),
            ..wl(&w, "set_primary", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    assert_eq!(row.work.unwrap().link_id, new);
    // Device 2 still thinks `old` is primary.
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(old),
            expected_primary: Some(old),
            ..wl(&w, "set_primary", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(err.details.as_ref().unwrap()["primary_link_id"], new);
    let after = links_of(&w, w.s1);
    assert_eq!(after.links.len(), 2, "no link was removed or ended");
    assert_eq!(after.primary_link_id, Some(new));
    assert!(after.links.iter().all(|l| l.link.state == "active"));
    // Idempotent: setting the primary it already has changes nothing.
    work_link(
        &WorkLinkArgs {
            link_id: Some(new),
            expected_primary: Some(new),
            ..wl(&w, "set_primary", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    // A suggestion cannot be made primary.
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(999_999),
            ..wl(&w, "set_primary", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
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
/// rejection is final and does not come back; undo returns it.
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

    // Reject, with the version the person saw.
    work_link(
        &WorkLinkArgs {
            link_id: Some(it.link_id),
            expected_version: Some(it.link_version),
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
    // Undo: back to a suggestion, evidence kept.
    work_link(
        &WorkLinkArgs {
            link_id: Some(it.link_id),
            ..wl(&w, "reconsider", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
    assert!(r
        .items
        .iter()
        .any(|i| i.link_id == it.link_id && i.kind == "suggestion"));
}

/// UC11: a decision on a version someone else changed is a conflict, and
/// changes nothing; a batch answers each decision on its own.
#[test]
fn a_stale_decision_conflicts_and_a_batch_answers_each_item() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        crate::service::work::detect::on_prompt(&s, w.s1, "TK-3 and TK-2 both", false).unwrap();
    }
    let r = review(&w.st, &OrgScope::All, None, None).unwrap();
    let (a, b) = (&r.items[0], &r.items[1]);
    // Another device confirms `a` first.
    work_link(
        &WorkLinkArgs {
            link_id: Some(a.link_id),
            expected_version: Some(a.link_version),
            ..wl(&w, "confirm", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    // This device rejects `a` on the version it saw.
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(a.link_id),
            expected_version: Some(a.link_version),
            ..wl(&w, "reject", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(err.details.as_ref().unwrap()["state"], "confirmed");
    let st = links_of(&w, w.s1);
    assert!(st
        .links
        .iter()
        .any(|l| l.link.link_id == a.link_id && l.link.state == "active"));

    let res = structure::decide_batch(
        &w.st,
        &OrgScope::All,
        &[
            LinkDecision {
                session_id: w.s1,
                link_id: a.link_id,
                decision: "reject".into(),
                expected_version: Some(a.link_version),
                primary: None,
            },
            LinkDecision {
                session_id: w.s1,
                link_id: b.link_id,
                decision: "confirm".into(),
                expected_version: Some(b.link_version),
                primary: Some(false),
            },
        ],
        &|_| Ok(()),
    )
    .unwrap();
    assert_eq!(res.results[0].code.as_deref(), Some(codes::E_CONFLICT));
    assert!(
        res.results[1].ok && res.results[1].version.is_some(),
        "{:?}",
        res.results[1]
    );
    let st = links_of(&w, w.s1);
    assert_eq!(
        st.links.iter().filter(|l| l.link.state == "active").count(),
        2
    );
    assert_eq!(
        st.links.iter().filter(|l| l.link.primary).count(),
        1,
        "one primary"
    );
}

/// UC6: where a task sits and why; a person's placement beats a rule,
/// a rule beats the tracker; placements are compare-and-set; a disabled
/// rule is the way back.
#[test]
fn placement_and_rules_explain_the_group_and_can_be_undone() {
    let w = world();
    let group = |w: &W, key: &str| {
        task_of(&page(w, &OrgScope::All, WorkTreeFilters::default()), key)
            .group
            .clone()
    };
    let g = group(&w, "TK-1");
    assert_eq!((g.source.as_str(), g.label.as_str()), ("tracker", "TP"));
    assert_eq!(g.id, format!("tracker:{}:TP", w.tracker));

    // Preview first: nothing is saved by it.
    let draft = RuleInput {
        name: "Audit".into(),
        conditions: RuleConditions {
            title_contains: Some("audit".into()),
            ..Default::default()
        },
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
    let rule = structure::rule_save(&w.st, &OrgScope::All, &draft).unwrap();
    let g = group(&w, "TK-2");
    assert_eq!((g.source.as_str(), g.rule_id), ("rule", Some(rule.id)));
    assert_eq!(
        g.tracker_value.as_deref(),
        Some("TP"),
        "what the tracker says stays visible"
    );

    // A one-off correction of TK-2 only.
    let t2 = structure::place(
        &w.st,
        &OrgScope::All,
        "item:2",
        Some("Security"),
        None,
        Some(0),
        "me",
    )
    .unwrap();
    assert_eq!(
        (t2.group.source.as_str(), t2.group.label.as_str()),
        ("manual", "Security")
    );
    assert_eq!(group(&w, "TK-1").source, "tracker", "the others stay");
    // A second device placing on the version it saw (none) conflicts.
    let err = structure::place(
        &w.st,
        &OrgScope::All,
        "item:2",
        Some("Other"),
        None,
        Some(0),
        "phone",
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    // The manual placement is kept by the preview of another matching rule.
    let pv = structure::rule_preview(&w.st, &OrgScope::All, &draft).unwrap();
    assert_eq!((pv.total, pv.kept_manual), (0, 1));
    // Clearing the placement falls back to the rule; disabling the rule to
    // the tracker.
    structure::place(
        &w.st,
        &OrgScope::All,
        "item:2",
        None,
        None,
        Some(t2.placement_version),
        "me",
    )
    .unwrap();
    assert_eq!(group(&w, "TK-2").source, "rule");
    structure::rule_save(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            id: Some(rule.id),
            enabled: Some(false),
            expected_version: Some(rule.version),
            ..draft.clone()
        },
    )
    .unwrap();
    assert_eq!(group(&w, "TK-2").source, "tracker");
    // An empty rule is refused.
    let err = structure::rule_save(
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

/// UC9: a sync that renames a ticket keeps the person's placement; a
/// ticket that disappears is marked, never reassigned.
#[test]
fn a_sync_updates_the_ticket_and_keeps_local_decisions() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    structure::place(
        &w.st,
        &OrgScope::All,
        "item:1",
        Some("Mine"),
        None,
        Some(0),
        "me",
    )
    .unwrap();
    {
        let s = w.st.lock().unwrap();
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

/// UC7: a local task changes org only with a fresh impact token; the
/// impact names what changes; after the move a client bound to the old
/// org no longer receives the task, nor its link on its own session.
#[test]
fn a_local_task_moves_org_only_with_a_fresh_impact() {
    let w = world();
    let local = {
        let s = w.st.lock().unwrap();
        s.name_session_work(w.s1, Some("LOC-9"), "Refactor billing")
            .unwrap()
            .0
            .id
    };
    let tid = format!("item:{local}");
    let bound_a = OrgScope::Org { org: w.org_a };
    assert!(task(&w.st, &bound_a, &tid).is_ok(), "unassigned: visible");

    let imp = structure::org_impact(&w.st, &OrgScope::All, &tid, Some(w.org_b)).unwrap();
    assert!(imp.allowed);
    assert_eq!((imp.from_org, imp.to_org), (None, Some(w.org_b)));
    assert!(imp.links[0].becomes_cross_org, "s1 is org A's");
    assert_eq!(imp.hosts_losing, vec!["h1".to_string()]);

    let err = structure::assign_org(&w.st, &OrgScope::All, &tid, Some(w.org_b), Some("stale"))
        .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    let err = structure::assign_org(
        &w.st,
        &bound_a,
        &tid,
        Some(w.org_b),
        Some(&imp.impact_token),
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN, "a bound client moves no org");
    let moved = structure::assign_org(
        &w.st,
        &OrgScope::All,
        &tid,
        Some(w.org_b),
        Some(&imp.impact_token),
    )
    .unwrap();
    assert_eq!(
        (moved.org_id, moved.org_source.as_str()),
        (Some(w.org_b), "item")
    );
    assert!(moved.sessions[0].cross_org);

    let err = task(&w.st, &bound_a, &tid).unwrap_err();
    assert_eq!(
        err.code,
        codes::E_NOTFOUND,
        "gone for A, answered as unknown"
    );
    let st = session_tasks(&w.st, &bound_a, w.s1).unwrap();
    assert!(
        st.links.is_empty(),
        "A's own session no longer names B's task"
    );
    let row =
        w.st.lock()
            .unwrap()
            .get_session_by_id(w.s1)
            .unwrap()
            .unwrap();
    assert_eq!(
        row.work.unwrap().org_id,
        Some(w.org_b),
        "the row's work carries the new org"
    );
    // A tracker item's org is its tracker's.
    let t = structure::org_impact(&w.st, &OrgScope::All, "item:1", Some(w.org_b)).unwrap();
    assert_eq!(
        (t.allowed, t.reason.as_deref()),
        (false, Some("tracker_controlled"))
    );
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
        s.set_local_item_org(it.id, Some(w.org_b)).unwrap();
        it.id
    };
    link(&w, w.s1, w.t1, true);
    // A person forces B's task onto A's session s1 (the store takes it).
    w.st.lock()
        .unwrap()
        .link_session_work_as(w.s1, WorkTarget::Item(local_b), "manual", false)
        .unwrap();
    let a = OrgScope::Org { org: w.org_a };
    let b = OrgScope::Org { org: w.org_b };
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
    assert!(r.items.iter().any(|i| i.kind == "cross_org"));
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
        s.set_local_item_org(it.id, Some(w.org_b)).unwrap();
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
    structure::place(
        &w.st,
        &OrgScope::All,
        "item:2",
        Some("Security"),
        Some("why"),
        Some(0),
        "me",
    )
    .unwrap();
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

/// A client bound to org A whose session's primary is another org's task (a
/// forced link) saw "no primary": its compare-and-set is on what it can
/// see, so it is neither refused forever nor told the hidden link's id.
#[test]
fn a_hidden_primary_neither_blocks_nor_leaks_to_a_bound_client() {
    let w = world();
    let local_b = {
        let s = w.st.lock().unwrap();
        let (it, _) = s.name_session_work(w.s2, Some("HID-1"), "Hidden").unwrap();
        s.set_local_item_org(it.id, Some(w.org_b)).unwrap();
        it.id
    };
    // s1 (org A): B's task is its primary; A's TK-1 a secondary.
    let hidden =
        w.st.lock()
            .unwrap()
            .link_session_work_as(w.s1, WorkTarget::Item(local_b), "manual", true)
            .unwrap()
            .id;
    link(&w, w.s1, w.t1, false);
    let a = OrgScope::Org { org: w.org_a };
    let st = session_tasks(&w.st, &a, w.s1).unwrap();
    assert_eq!(st.primary_link_id, None, "the hidden primary is not named");
    let mine = st.links[0].link.link_id;
    // A stale expectation is a conflict that names nothing hidden.
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(mine),
            expected_primary: Some(mine),
            ..wl(&w, "set_primary", w.s1)
        },
        &w.st,
        &a,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert!(
        !err.message.contains(&hidden.to_string()),
        "{}",
        err.message
    );
    assert_eq!(
        err.details.as_ref().unwrap()["primary_link_id"],
        serde_json::Value::Null
    );
    // What it saw ("none") is accepted: A's task becomes primary; B's link
    // stays, as a secondary.
    work_link(
        &WorkLinkArgs {
            link_id: Some(mine),
            expected_primary: Some(0),
            ..wl(&w, "set_primary", w.s1)
        },
        &w.st,
        &a,
    )
    .unwrap();
    let all = links_of(&w, w.s1);
    assert_eq!(all.primary_link_id, Some(mine));
    assert!(all
        .links
        .iter()
        .any(|l| l.link.link_id == hidden && l.link.state == "active"));
}
