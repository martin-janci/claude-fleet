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
    let all = local_items(&fx.store, &OrgScope::All).unwrap();
    assert_eq!(all.len(), 3);
    let count =
        |rows: &[LocalWorkItem], id: i64| rows.iter().find(|r| r.id == id).map(|r| r.live_sessions);
    assert_eq!(count(&all, on_b), Some(2));
    assert_eq!(count(&all, on_a), Some(1));

    let a = local_items(&fx.store, &host(&fx, "h-a")).unwrap();
    assert_eq!(
        a.iter().map(|r| r.id).collect::<BTreeSet<_>>(),
        [on_a, on_b].into()
    );
    assert_eq!(
        count(&a, on_b),
        Some(1),
        "host A counts only its own sessions"
    );
    let b = local_items(&fx.store, &host(&fx, "h-b")).unwrap();
    assert_eq!(b.iter().map(|r| r.id).collect::<Vec<_>>(), vec![on_b]);
    // Ended work still lists for the host its session ran on.
    fx.store.lock().unwrap().delete_session(fx.s_b).unwrap();
    let b = local_items(&fx.store, &host(&fx, "h-b")).unwrap();
    assert_eq!(
        b.iter()
            .map(|r| (r.id, r.live_sessions))
            .collect::<Vec<_>>(),
        vec![(on_b, 0)]
    );
}
