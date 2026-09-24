use super::*;
use crate::store::{WorkSummary, WorkTarget};

fn host(org: Option<i64>, isolated: &[i64]) -> OrgScope {
    OrgScope::Host {
        alias: "h-a".into(),
        org,
        isolated: isolated.iter().copied().collect(),
    }
}

#[test]
fn work_data_is_visible_in_the_hosts_org_and_unassigned_only() {
    // (scope, data org, visible)
    let table = [
        (OrgScope::All, Some(2), true),
        (OrgScope::All, None, true),
        (host(Some(1), &[]), Some(1), true),
        (host(Some(1), &[]), None, true),
        (host(Some(1), &[]), Some(2), false),
        // A host nobody placed sees only unassigned data.
        (host(None, &[]), None, true),
        (host(None, &[]), Some(1), false),
    ];
    for (scope, org, want) in table {
        assert_eq!(scope.sees_org(org), want, "{scope:?} {org:?}");
    }
}

#[test]
fn sessions_are_fenced_only_between_isolating_orgs() {
    // (scope, row host, row org, visible)
    let table = [
        (OrgScope::All, "h-b", Some(2), true),
        // Off everywhere: sessions are not fenced (D7 default).
        (host(Some(1), &[]), "h-b", Some(2), true),
        // B isolates: A's host cannot see B's sessions …
        (host(Some(1), &[2]), "h-b", Some(2), false),
        // … nor can an unplaced host.
        (host(None, &[2]), "h-b", Some(2), false),
        // A isolates: its hosts see only A and unassigned.
        (host(Some(1), &[1]), "h-b", Some(2), false),
        (host(Some(1), &[1]), "h-b", None, true),
        (host(Some(1), &[1, 2]), "h-a2", Some(1), true),
        // Its own host's sessions, always.
        (host(Some(1), &[2]), "h-a", Some(2), true),
    ];
    for (scope, h, org, want) in table {
        assert_eq!(scope.sees_session(h, org), want, "{scope:?} {h} {org:?}");
    }
}

fn summary(org: Option<i64>) -> WorkSummary {
    WorkSummary {
        link_id: 1,
        item_id: None,
        key: Some("ABC-1".into()),
        title: "Secret title".into(),
        source: "manual".into(),
        status_category: None,
        status_name: None,
        url: None,
        unavailable: false,
        state: "confirmed".into(),
        strength: None,
        rule: None,
        preselected: false,
        suggestions: 0,
        org_id: org,
    }
}

fn row(org: Option<i64>, work: Option<i64>) -> SessionRow {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h-b").unwrap();
    let id = s
        .upsert_session("dev", "h-b", None, None, 1, 1, "running", None)
        .unwrap();
    let mut r = s.get_session_by_id(id).unwrap().unwrap();
    r.org_id = org;
    r.work = work.map(|o| summary(Some(o)));
    r.work_suggested = Some(summary(work));
    r.work_rejected = vec!["X-1".into()];
    r
}

#[test]
fn a_row_loses_the_work_its_reader_may_not_see() {
    let a = host(Some(1), &[]);
    // A session of org 2: all of its work goes.
    let mut r = row(Some(2), Some(2));
    a.redact_row(&mut r);
    assert!(r.work.is_none() && r.work_suggested.is_none() && r.work_rejected.is_empty());
    // A session of org 1 with a link to org 2's ticket: that link goes.
    let mut r = row(Some(1), Some(2));
    a.redact_row(&mut r);
    assert!(r.work.is_none());
    assert_eq!(r.work_rejected, vec!["X-1".to_string()]);
    // Its own org's work stays; the master keeps everything.
    let mut r = row(Some(1), Some(1));
    a.redact_row(&mut r);
    assert!(r.work.is_some());
    let mut r = row(Some(2), Some(2));
    OrgScope::All.redact_row(&mut r);
    assert!(r.work.is_some());
}

#[test]
fn serialised_rows_are_redacted_wherever_they_nest() {
    let a = host(Some(1), &[]);
    let mut v = serde_json::json!({
        "task": { "worker": serde_json::to_value(row(Some(2), Some(2))).unwrap() },
        "rows": [
            serde_json::to_value(row(Some(1), Some(2))).unwrap(),
            serde_json::to_value(row(Some(1), Some(1))).unwrap(),
        ],
        // Not a session row: left alone.
        "work": { "key": "ZZ-1" },
    });
    let by_field = |m: &serde_json::Map<String, serde_json::Value>| {
        m.get("org_id").and_then(serde_json::Value::as_i64)
    };
    a.redact_json(&mut v, &by_field);
    let w = &v["task"]["worker"];
    assert!(w.get("work").is_none() && w.get("work_suggested").is_none());
    assert!(w.get("work_rejected").is_none());
    assert!(v["rows"][0].get("work").is_none());
    assert!(v["rows"][1].get("work").is_some());
    assert_eq!(v["work"]["key"], "ZZ-1");
    // The lookup, not the object, decides: a projection that dropped
    // `org_id` cannot make a row look unassigned.
    let mut v = serde_json::to_value(row(Some(2), Some(2))).unwrap();
    v.as_object_mut().unwrap().remove("org_id");
    a.redact_json(&mut v, &|_| Some(2));
    assert!(v.get("work").is_none());
}

#[test]
fn scopes_list_orgs_then_uncovered_owners_then_the_rest() {
    let st = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = st.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_host("h2").unwrap();
        let acme = s.upsert_project("acme", "api", "/src/api").unwrap();
        let beta = s.upsert_project("beta", "web", "/src/web").unwrap();
        let notes = s.upsert_adopted_project("local", "notes", "/n").unwrap();
        s.upsert_session("a1", "h", Some(acme), None, 1, 1, "running", None)
            .unwrap();
        let b = s
            .upsert_session("b1", "h", Some(beta), None, 1, 1, "running", None)
            .unwrap();
        s.set_session_claude_status_for_test(b, "blocked");
        s.upsert_session("n1", "h", Some(notes), None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("x", "h2", None, None, 1, 1, "running", None)
            .unwrap();
    }
    // Zero config: one pseudo-scope per owner, then the unassigned rest.
    let got = scopes(&st, &OrgScope::All).unwrap();
    let labels: Vec<(&str, usize, usize)> = got
        .iter()
        .map(|e| (e.label.as_str(), e.session_count, e.needs_you))
        .collect();
    assert_eq!(
        labels,
        vec![("acme", 1, 0), ("beta", 1, 1), ("Unassigned", 2, 0)]
    );
    // Name an org for acme: it replaces the owner scope.
    let a = {
        let s = st.lock().unwrap();
        let a = s.add_org("Company A", Some("#f00"), false).unwrap();
        s.add_org_rule(OrgRuleRow {
            org_id: a.id,
            owner: Some("acme".into()),
            ..Default::default()
        })
        .unwrap();
        a
    };
    let got = scopes(&st, &OrgScope::All).unwrap();
    assert_eq!(got[0].id, Some(a.id));
    assert_eq!(got[0].session_count, 1);
    assert_eq!(got[1].owner.as_deref(), Some("beta"));
    // A host outside Company A sees no Company A scope.
    let outsider = host(None, &[]);
    assert!(scopes(&st, &outsider)
        .unwrap()
        .iter()
        .all(|e| e.id != Some(a.id)));
    // Details carry the rules; out-of-scope orgs are not listed.
    let d = org_details(&st, &OrgScope::All).unwrap();
    assert_eq!(d[0].rules.len(), 1);
    assert!(org_details(&st, &outsider).unwrap().is_empty());
    let _ = WorkTarget::Key("unused");
}

fn admin_args(action: &str) -> crate::service::trackers::admin::WorkAdminArgs {
    crate::service::trackers::admin::WorkAdminArgs {
        action: action.into(),
        ..Default::default()
    }
}

fn run(
    st: &Mutex<Store>,
    a: crate::service::trackers::admin::WorkAdminArgs,
) -> Result<serde_json::Value, IpcError> {
    crate::service::trackers::admin::admin_sync(&a, st)
}

#[test]
fn org_admin_crud_refusals_and_row_announcements() {
    let (bus, st) = {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        (bus, Mutex::new(s))
    };
    let sid = {
        let s = st.lock().unwrap();
        s.upsert_host("h").unwrap();
        let p = s.upsert_project("acme", "api", "/src/api").unwrap();
        s.upsert_session("dev", "h", Some(p), None, 1, 1, "running", None)
            .unwrap()
    };
    let org = run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            name: Some("Company A".into()),
            isolate_sessions: Some(true),
            ..admin_args("add_org")
        },
    )
    .unwrap();
    let oid = org["id"].as_i64().unwrap();
    assert_eq!(org["isolate_sessions"], true);
    // Missing fields name themselves.
    for (action, field) in [
        ("add_org", "name"),
        ("update_org", "org_id"),
        ("remove_org", "org_id"),
        ("add_rule", "org_id"),
        ("remove_rule", "rule_id"),
        ("assign_host", "host_alias"),
        ("unassign_host", "host_alias"),
        ("assign_tracker", "tracker_id"),
    ] {
        let e = run(&st, admin_args(action)).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{action}");
        assert!(e.message.contains(field), "{action}: {}", e.message);
    }
    // A rule that moves the session announces it, once.
    bus.take();
    let before = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
    let rule = run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            org_id: Some(oid),
            owner: Some("acme".into()),
            ..admin_args("add_rule")
        },
    )
    .unwrap();
    assert_eq!(bus.names(), vec!["session:updated"]);
    let after = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(after.org_id, Some(oid));
    assert!(
        after.row_version > before.row_version,
        "the merge guard sees it"
    );
    // A change that moves nobody announces nothing.
    bus.take();
    run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            org_id: Some(oid),
            color: Some("#123456".into()),
            ..admin_args("update_org")
        },
    )
    .unwrap();
    assert!(bus.names().is_empty());
    // Hosts and trackers.
    run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            host_alias: Some("h".into()),
            org_id: Some(oid),
            ..admin_args("assign_host")
        },
    )
    .unwrap();
    let tid = st
        .lock()
        .unwrap()
        .add_tracker("jira", "acme", "https://acme.atlassian.net")
        .unwrap()
        .id;
    let t = run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            tracker_id: Some(tid),
            org_id: Some(oid),
            ..admin_args("assign_tracker")
        },
    )
    .unwrap();
    assert_eq!(t["org_id"], oid);
    // An org that owns a tracker is not removed, and says which.
    let e = run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            org_id: Some(oid),
            ..admin_args("remove_org")
        },
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    assert!(e.message.contains("acme (tracker"), "{}", e.message);
    // Unassign the tracker, remove the rule and the org.
    run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            tracker_id: Some(tid),
            ..admin_args("assign_tracker")
        },
    )
    .unwrap();
    run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            rule_id: rule["id"].as_i64(),
            ..admin_args("remove_rule")
        },
    )
    .unwrap();
    assert_eq!(
        run(
            &st,
            crate::service::trackers::admin::WorkAdminArgs {
                rule_id: rule["id"].as_i64(),
                ..admin_args("remove_rule")
            },
        )
        .unwrap_err()
        .code,
        codes::E_NOTFOUND
    );
    run(
        &st,
        crate::service::trackers::admin::WorkAdminArgs {
            org_id: Some(oid),
            ..admin_args("remove_org")
        },
    )
    .unwrap();
    assert!(run(&st, admin_args("list_orgs"))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(st.lock().unwrap().host_org("h").unwrap(), None);
    // Removals pass the confirmation gate; the rest do not.
    use crate::service::trackers::admin::AdminAction;
    for name in AdminAction::NAMES {
        let a = AdminAction::parse(name).unwrap();
        assert_eq!(
            a.is_removal(),
            matches!(*name, "remove" | "remove_org" | "remove_rule"),
            "{name}"
        );
    }
}

#[test]
fn suggestions_come_from_uncovered_owners_and_tracker_sites() {
    let st = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = st.lock().unwrap();
        s.upsert_host("h").unwrap();
        let acme = s.upsert_project("acme", "api", "/src/api").unwrap();
        let acme2 = s.upsert_project("acme", "web", "/src/web").unwrap();
        let beta = s.upsert_project("beta", "site", "/src/site").unwrap();
        let notes = s.upsert_adopted_project("local", "notes", "/n").unwrap();
        for (n, p) in [("a1", acme), ("a2", acme2), ("b1", beta), ("n1", notes)] {
            s.upsert_session(n, "h", Some(p), None, 1, 1, "running", None)
                .unwrap();
        }
    }
    let got = org_suggestions(&st, &OrgScope::All).unwrap();
    let names: Vec<(&str, Option<&str>, usize)> = got
        .iter()
        .map(|g| (g.name.as_str(), g.owner.as_deref(), g.sessions))
        .collect();
    assert_eq!(
        names,
        vec![("acme", Some("acme"), 2), ("beta", Some("beta"), 1)],
        "`local` is never proposed"
    );
    // A tracker on acme.atlassian.net pairs with the owner of that name.
    let tid = st
        .lock()
        .unwrap()
        .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
        .unwrap()
        .id;
    let got = org_suggestions(&st, &OrgScope::All).unwrap();
    assert_eq!(got[0].tracker_id, Some(tid));
    assert_eq!(got[0].owner.as_deref(), Some("acme"));
    assert!(got[0].reason.contains("share a name"), "{}", got[0].reason);
    assert_eq!(got.len(), 2, "acme is not proposed twice");
    // Once an org covers an owner it is no longer proposed.
    {
        let s = st.lock().unwrap();
        let b = s.add_org("Beta Inc", None, false).unwrap();
        s.add_org_rule(OrgRuleRow {
            org_id: b.id,
            owner: Some("beta".into()),
            ..Default::default()
        })
        .unwrap();
    }
    let got = org_suggestions(&st, &OrgScope::All).unwrap();
    assert!(got.iter().all(|g| g.owner.as_deref() != Some("beta")));
    // A per-host token gets nothing to act on.
    assert!(org_suggestions(&st, &host(None, &[])).unwrap().is_empty());
}

#[test]
fn a_single_owner_fleet_gets_no_suggestions() {
    let st = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = st.lock().unwrap();
        s.upsert_host("h").unwrap();
        let p = s.upsert_project("me", "api", "/src/api").unwrap();
        s.upsert_session("a", "h", Some(p), None, 1, 1, "running", None)
            .unwrap();
    }
    assert!(org_suggestions(&st, &OrgScope::All).unwrap().is_empty());
}
