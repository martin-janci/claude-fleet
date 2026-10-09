//! "Name this work…" through the service: the fences, the key rules, the
//! listing.

use super::*;
use crate::store::{TrackerItemWrite, WorkTarget};

struct Fx {
    store: Mutex<Store>,
    s_a: i64,
    s_b: i64,
    ticket_a: i64,
    ticket_b: i64,
}

fn fixture() -> Fx {
    let s = Store::open_in_memory().unwrap();
    for h in ["h-a", "h-b"] {
        s.upsert_host(h).unwrap();
    }
    let a = s.add_org("A", None, false).unwrap();
    let b = s.add_org("B", None, false).unwrap();
    s.set_host_org("h-a", Some(a.id)).unwrap();
    s.set_host_org("h-b", Some(b.id)).unwrap();
    let ticket = |org: i64, site: &str, key: &str| {
        let t = s.add_tracker("jira", site, site).unwrap();
        s.set_tracker_org(t.id, Some(org)).unwrap();
        s.upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: Some(key.into()),
                title: format!("{key} ticket"),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    };
    let ticket_a = ticket(a.id, "https://a.atlassian.net", "AA-1");
    let ticket_b = ticket(b.id, "https://b.atlassian.net", "BB-1");
    let s_a = s
        .upsert_session("s-a", "h-a", None, None, 1, 1, "running", None)
        .unwrap();
    let s_b = s
        .upsert_session("s-b", "h-b", None, None, 1, 1, "running", None)
        .unwrap();
    Fx {
        store: Mutex::new(s),
        s_a,
        s_b,
        ticket_a,
        ticket_b,
    }
}

fn host(fx: &Fx, alias: &str) -> OrgScope {
    OrgScope::for_host(&fx.store.lock().unwrap(), alias).unwrap()
}

fn name(sid: i64, title: &str, key: Option<&str>) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "name".into(),
        session_id: Some(sid),
        title: Some(title.into()),
        key: key.map(str::to_string),
        ..Default::default()
    }
}

fn rename(item: i64, title: &str) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "name".into(),
        item_id: Some(item),
        title: Some(title.into()),
        ..Default::default()
    }
}

/// The whole scope for `local_items` (multi-user M1, T8d): every case here is
/// about the ORG half, so the person half is the hub's own unrestricted reader
/// with the org under test put back on it.
fn lv(scope: &OrgScope) -> crate::service::view_scope::ViewScope {
    crate::service::view_scope::ViewScope::internal().with_org(scope.clone())
}

#[test]
fn naming_returns_the_row_with_its_new_primary_work() {
    let fx = fixture();
    let row = name_session_work(
        &name(fx.s_a, "Billing", Some("bill-7")),
        &fx.store,
        &OrgScope::All,
    )
    .unwrap();
    let w = row.work.expect("named work is the row's work");
    assert_eq!(w.title, "Billing");
    // The key goes through the one canonical spelling.
    assert_eq!(w.key.as_deref(), Some("BILL-7"));
    assert_eq!(w.state, "confirmed");
    assert_eq!(w.source, "manual");
}

/// D34: an agent naming its work (a per-host token, the operator) records
/// `agent`, never a person's `manual`; a person's naming is unchanged.
#[test]
fn an_agent_naming_work_is_recorded_as_the_agents() {
    let fx = fixture();
    for (decider, sid, title, want) in [
        (Decider::Agent, fx.s_a, "Agent's", "agent"),
        (Decider::Person, fx.s_b, "Person's", "manual"),
    ] {
        let row = name_session_work_as(&name(sid, title, None), &fx.store, &OrgScope::All, decider)
            .unwrap();
        let w = row.work.expect("named work is the row's work");
        assert_eq!((w.title.as_str(), w.source.as_str()), (title, want));
    }
}

#[test]
fn a_title_is_required_and_validated() {
    let fx = fixture();
    let mut no_title = name(fx.s_a, "x", None);
    no_title.title = None;
    for args in [
        no_title,
        name(fx.s_a, " ", None),
        name(fx.s_a, &"t".repeat(121), None),
        name(fx.s_a, "tab\there", None),
    ] {
        assert_eq!(
            name_session_work(&args, &fx.store, &OrgScope::All)
                .unwrap_err()
                .code,
            codes::E_INVALID,
            "{:?}",
            args.title
        );
    }
    assert_eq!(
        name_session_work(
            &name(fx.s_a, "ok", Some("bad\u{1}key")),
            &fx.store,
            &OrgScope::All
        )
        .unwrap_err()
        .code,
        codes::E_INVALID
    );
}

#[test]
fn a_host_names_work_only_on_its_own_sessions_and_another_reads_as_unknown() {
    let fx = fixture();
    let a = host(&fx, "h-a");
    assert!(name_session_work(&name(fx.s_a, "Mine", None), &fx.store, &a).is_ok());
    let other = name_session_work(&name(fx.s_b, "Theirs", None), &fx.store, &a).unwrap_err();
    let unknown = name_session_work(&name(9_999, "Theirs", None), &fx.store, &a).unwrap_err();
    assert_eq!(other.code, codes::E_NOTFOUND);
    assert_eq!(
        other.message.replace(&fx.s_b.to_string(), "<X>"),
        unknown.message.replace("9999", "<X>"),
        "no existence oracle"
    );
    assert!(fx
        .store
        .lock()
        .unwrap()
        .get_session_by_id(fx.s_b)
        .unwrap()
        .unwrap()
        .work
        .is_none());
}

#[test]
fn a_visible_tickets_key_is_refused_an_invisible_one_is_just_a_key() {
    let fx = fixture();
    // The master sees B's ticket: that work has a ticket, link it instead.
    let e = name_session_work(
        &name(fx.s_a, "Dup", Some("bb-1")),
        &fx.store,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
    assert_eq!(e.details.as_ref().unwrap()["item_id"], fx.ticket_b);
    // Host A sees its own ticket …
    let a = host(&fx, "h-a");
    assert_eq!(
        name_session_work(&name(fx.s_a, "Dup", Some("AA-1")), &fx.store, &a)
            .unwrap_err()
            .code,
        codes::E_EXISTS
    );
    // … and not B's: BB-1 answers as a key nothing carries.
    let row = name_session_work(&name(fx.s_a, "Mine", Some("BB-1")), &fx.store, &a).unwrap();
    let w = row.work.unwrap();
    assert_eq!(w.title, "Mine");
    assert_ne!(w.item_id, Some(fx.ticket_b));
    let _ = fx.ticket_a;
}

#[test]
fn a_taken_local_key_is_refused_with_the_item_only_for_who_sees_it() {
    let fx = fixture();
    name_session_work(&name(fx.s_a, "Ops", Some("OPS")), &fx.store, &OrgScope::All).unwrap();
    let master = name_session_work(
        &name(fx.s_b, "Ops2", Some("OPS")),
        &fx.store,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(master.code, codes::E_EXISTS);
    assert!(master.details.is_some());
    let b = host(&fx, "h-b");
    let host_b = name_session_work(&name(fx.s_b, "Ops2", Some("OPS")), &fx.store, &b).unwrap_err();
    assert_eq!(host_b.code, codes::E_EXISTS);
    assert!(
        host_b.details.is_none(),
        "host B does not see host A's item"
    );
}

#[test]
fn rename_is_for_local_items_the_caller_sees() {
    let fx = fixture();
    let row = name_session_work(&name(fx.s_a, "Old", None), &fx.store, &OrgScope::All).unwrap();
    let item = row.work.unwrap().item_id.unwrap();
    let a = host(&fx, "h-a");
    let b = host(&fx, "h-b");
    assert_eq!(
        rename_local_item(&rename(item, "New"), &fx.store, &a)
            .unwrap()
            .title,
        "New"
    );
    // Host B: the item reads as one that does not exist.
    let hidden = rename_local_item(&rename(item, "B's"), &fx.store, &b).unwrap_err();
    let unknown = rename_local_item(&rename(9_999, "B's"), &fx.store, &b).unwrap_err();
    assert_eq!(hidden.code, codes::E_NOTFOUND);
    assert_eq!(
        hidden.message.replace(&item.to_string(), "<X>"),
        unknown.message.replace("9999", "<X>")
    );
    // A ticket: refused for who sees it, unknown for who does not.
    assert_eq!(
        rename_local_item(&rename(fx.ticket_b, "x"), &fx.store, &OrgScope::All)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        rename_local_item(&rename(fx.ticket_b, "x"), &fx.store, &b)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        rename_local_item(&rename(fx.ticket_b, "x"), &fx.store, &a)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        rename_local_item(&rename(item, ""), &fx.store, &OrgScope::All)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
}

#[test]
fn local_items_list_what_the_scope_sees_with_live_counts() {
    let fx = fixture();
    let named = |sid: i64, t: &str| {
        name_session_work(&name(sid, t, None), &fx.store, &OrgScope::All)
            .unwrap()
            .work
            .unwrap()
            .item_id
            .unwrap()
    };
    let on_a = named(fx.s_a, "On A");
    let on_b = named(fx.s_b, "On B");
    {
        let s = fx.store.lock().unwrap();
        // A second session on B's item, and an orphan item nobody links.
        s.link_session_work(fx.s_a, WorkTarget::Item(on_b), "manual")
            .unwrap();
        s.create_local_work_item(None, "Orphan").unwrap();
    }
    let all = local_items(&fx.store, &lv(&OrgScope::All)).unwrap();
    assert_eq!(all.len(), 3);
    let count =
        |rows: &[LocalWorkItem], id: i64| rows.iter().find(|r| r.id == id).map(|r| r.live_sessions);
    assert_eq!(count(&all, on_b), Some(2));
    assert_eq!(count(&all, on_a), Some(1));

    let a = local_items(&fx.store, &lv(&host(&fx, "h-a"))).unwrap();
    assert_eq!(
        a.iter().map(|r| r.id).collect::<BTreeSet<_>>(),
        [on_a, on_b].into()
    );
    assert_eq!(
        count(&a, on_b),
        Some(1),
        "host A counts only its own sessions"
    );
    let b = local_items(&fx.store, &lv(&host(&fx, "h-b"))).unwrap();
    assert_eq!(b.iter().map(|r| r.id).collect::<Vec<_>>(), vec![on_b]);
    // Ended work still lists for the host its session ran on.
    fx.store.lock().unwrap().delete_session(fx.s_b).unwrap();
    let b = local_items(&fx.store, &lv(&host(&fx, "h-b"))).unwrap();
    assert_eq!(
        b.iter()
            .map(|r| (r.id, r.live_sessions))
            .collect::<Vec<_>>(),
        vec![(on_b, 0)]
    );
}

// ── Shared work context (design 2026-09-29): create, propose, decide ──

fn args(action: &str) -> WorkLinkArgs {
    WorkLinkArgs {
        action: action.into(),
        ..Default::default()
    }
}

#[test]
fn an_unscoped_caller_creates_a_task_and_a_subtask() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let all = OrgScope::All;
    let t = create_task(
        &WorkLinkArgs {
            title: Some("Release notes".into()),
            ..args("create")
        },
        &store,
        &all,
    )
    .unwrap();
    let sub = create_task(
        &WorkLinkArgs {
            title: Some("Changelog".into()),
            parent: Some(format!("item:{}", t.id)),
            ..args("create")
        },
        &store,
        &all,
    )
    .unwrap();
    assert_eq!(sub.parent_id, Some(t.id));
}

#[test]
fn a_parent_is_item_colon_id() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let e = create_task(
        &WorkLinkArgs {
            title: Some("x".into()),
            parent: Some("TASK-1".into()),
            ..args("create")
        },
        &store,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[test]
fn a_host_token_may_not_create_a_standalone_task() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let scope = OrgScope::for_host(&s, "h").unwrap();
    let store = Mutex::new(s);
    let e = create_task(
        &WorkLinkArgs {
            title: Some("x".into()),
            ..args("create")
        },
        &store,
        &scope,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
}

#[test]
fn a_host_token_adds_a_subtask_only_under_work_it_sees() {
    let fx = fixture();
    let (mine, _) = lock(&fx.store)
        .unwrap()
        .name_session_work(fx.s_a, Some("MINE-1"), "mine")
        .unwrap();
    let (theirs, _) = lock(&fx.store)
        .unwrap()
        .name_session_work(fx.s_b, Some("THEIRS-1"), "theirs")
        .unwrap();
    let a = host(&fx, "h-a");
    let sub = |parent: i64| WorkLinkArgs {
        title: Some("step".into()),
        parent: Some(format!("item:{parent}")),
        ..args("create")
    };
    let ok = create_task(&sub(mine.id), &fx.store, &a).unwrap();
    assert_eq!(ok.parent_id, Some(mine.id));
    let e = create_task(&sub(theirs.id), &fx.store, &a).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let unknown = create_task(&sub(999_999), &fx.store, &a).unwrap_err();
    assert_eq!(
        e.message.replace(&theirs.id.to_string(), "<X>"),
        unknown.message.replace("999999", "<X>"),
        "another host's parent reads as an unknown one"
    );
}

#[test]
fn a_host_token_proposes_under_work_its_own_session_does_but_cannot_decide() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("w", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let (item, _) = s.name_session_work(sid, Some("OPS-1"), "ops").unwrap();
    let scope = OrgScope::for_host(&s, "h").unwrap();
    let store = Mutex::new(s);
    let p = propose(
        &WorkLinkArgs {
            title: Some("Add a test".into()),
            parent: Some(format!("item:{}", item.id)),
            why: Some("no coverage".into()),
            ..args("propose")
        },
        &store,
        &scope,
        "w · h",
    )
    .unwrap();
    assert_eq!(p.proposal_state.as_deref(), Some("proposed"));
    let e = decide(
        &WorkLinkArgs {
            item_id: Some(p.id),
            ..args("accept")
        },
        &store,
        &scope,
        true,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    let ok = decide(
        &WorkLinkArgs {
            item_id: Some(p.id),
            ..args("accept")
        },
        &store,
        &OrgScope::All,
        true,
    )
    .unwrap();
    assert_eq!(ok.proposal_state.as_deref(), Some("accepted"));
}

#[test]
fn propose_needs_a_parent() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let e = propose(
        &WorkLinkArgs {
            title: Some("x".into()),
            ..args("propose")
        },
        &store,
        &OrgScope::All,
        "me",
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

/// Redesign 6.9: a reject naming `task_id: item:<id>` is Merge — the
/// proposal's session link moves onto that task and the proposal closes;
/// a scoped caller (an agent) can merge nothing, and a malformed target is
/// refused before anything changes.
#[test]
fn merge_moves_the_proposal_onto_the_task_it_duplicates() {
    use crate::service::orgs::OrgScope;
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let ticket = s.create_local_work_item(Some("OM-1"), "Parent").unwrap();
    let existing = s
        .create_native_item(&crate::store::NativeItem {
            title: "Receipt totals",
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap();
    let p = s
        .propose_subtask(&crate::store::Proposal {
            parent_id: ticket.id,
            title: "Fix receipt totals",
            notes: None,
            why: None,
            proposed_by: "agent",
        })
        .unwrap();
    let sid = s
        .upsert_session("a", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Item(p.id), "manual")
        .unwrap();
    let store = Mutex::new(s);
    let merge = |task: &str| WorkLinkArgs {
        action: "reject".into(),
        item_id: Some(p.id),
        task_id: Some(task.into()),
        ..Default::default()
    };
    assert_eq!(
        decide(&merge("ref:X"), &store, &OrgScope::All, false)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    let scoped = OrgScope::Org {
        org: 1,
        sees_unassigned: true,
    };
    assert!(decide(
        &merge(&format!("item:{}", existing.id)),
        &store,
        &scoped,
        false
    )
    .is_err());
    let row = decide(
        &merge(&format!("item:{}", existing.id)),
        &store,
        &OrgScope::All,
        false,
    )
    .unwrap();
    assert_eq!(row.proposal_state.as_deref(), Some("rejected"));
    let s = store.lock().unwrap();
    let live: Vec<_> = s
        .session_work_links(sid)
        .unwrap()
        .into_iter()
        .filter(|l| l.ended_at.is_none())
        .map(|l| l.item_id)
        .collect();
    assert_eq!(live, vec![Some(existing.id)]);
}
