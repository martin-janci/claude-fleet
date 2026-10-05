use super::*;
use crate::store::{WorkSummary, WorkTarget};

/// The hub's own reader with one org half substituted: these tests are about
/// the ORG boundary, and the person half (multi-user M1) is tested where it
/// is implemented.
fn vs(scope: &OrgScope) -> crate::service::view_scope::ViewScope {
    crate::service::view_scope::ViewScope::internal().with_org(scope.clone())
}

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

/// The ORG half of a session read, and only that half (multi-user M1): the
/// person half is `service::view_scope`'s, and its own table test is beside
/// it.
///
/// The host arm no longer has three unconditional wins. It used to read
/// `row_host == alias || row_org.is_none() || row_org == *org` with a
/// permissive fall-through under it — which let a host token in org X read
/// every session of org X on every host in the fleet. It is now the rule a
/// machine's token is supposed to have: its own host's rows, and nothing
/// else. D7's `isolate_sessions` is subsumed rather than dropped — it only
/// ever hid OTHER hosts' rows from a host token, and those are now hidden
/// whatever any org says.
#[test]
fn a_host_token_sees_its_own_hosts_sessions_and_nothing_else() {
    // (scope, row host, row org, visible)
    let table = [
        (OrgScope::All, "h-b", Some(2), true),
        // Its own host's rows, whatever org they are placed in …
        (host(Some(1), &[]), "h-a", Some(1), true),
        (host(Some(1), &[]), "h-a", Some(2), true),
        (host(Some(1), &[]), "h-a", None, true),
        (host(None, &[]), "h-a", Some(1), true),
        // … and no other host's, however the orgs line up. Each of the
        // three rows below was a WIN before M1: the same org, no org, and
        // (with D7 off) simply "nobody isolates".
        (host(Some(1), &[]), "h-b", Some(1), false),
        (host(Some(1), &[]), "h-b", None, false),
        (host(Some(1), &[]), "h-b", Some(2), false),
        (host(None, &[]), "h-b", None, false),
        // D7 on changes none of these answers any more.
        (host(Some(1), &[2]), "h-b", Some(2), false),
        (host(Some(1), &[1, 2]), "h-a", Some(1), true),
    ];
    for (scope, h, org, want) in table {
        assert_eq!(
            scope.sees_session_org_only(h, org),
            want,
            "{scope:?} {h} {org:?}"
        );
    }
}

/// A bound client (M14) is unchanged: strictly its own org's sessions, plus
/// unassigned ones while D31 is on. It has no host fence — a phone is not a
/// host.
#[test]
fn a_bound_client_sees_its_own_orgs_sessions_on_any_host() {
    let bound = |org: i64, unassigned: bool| OrgScope::Org {
        org,
        sees_unassigned: unassigned,
    };
    let table = [
        (bound(1, true), "h-b", Some(1), true),
        (bound(1, true), "h-b", Some(2), false),
        (bound(1, true), "h-b", None, true),
        (bound(1, false), "h-b", None, false),
    ];
    for (scope, h, org, want) in table {
        assert_eq!(
            scope.sees_session_org_only(h, org),
            want,
            "{scope:?} {h} {org:?}"
        );
    }
}

fn summary(org: Option<i64>) -> WorkSummary {
    WorkSummary {
        link_id: 1,
        item_id: None,
        key: Some("ABC-1".into()),
        title: "Secret title".into(),
        source: "manual".into(),
        kind: String::new(),
        status_category: None,
        effective_status: None,
        status_name: None,
        url: None,
        unavailable: false,
        state: "confirmed".into(),
        strength: None,
        rule: None,
        preselected: false,
        suggestions: 0,
        org_id: org,
        archived_at: None,
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
    r.work_rev = 42;
    r
}

#[test]
fn a_row_loses_the_work_its_reader_may_not_see() {
    let a = host(Some(1), &[]);
    // A session of org 2: all of its work goes.
    let mut r = row(Some(2), Some(2));
    a.redact_row_org_only(&mut r);
    assert!(r.work.is_none() && r.work_suggested.is_none() && r.work_rejected.is_empty());
    // A session of org 1 with a link to org 2's ticket: that link goes.
    let mut r = row(Some(1), Some(2));
    a.redact_row_org_only(&mut r);
    assert!(r.work.is_none());
    assert!(r.work_rejected.is_empty(), "bare keys never reach a host");
    assert_eq!(
        r.work_rev, 0,
        "a digest over every link never reaches a host"
    );
    // Its own org's work stays; the master keeps everything.
    let mut r = row(Some(1), Some(1));
    a.redact_row_org_only(&mut r);
    assert!(r.work.is_some());
    assert_eq!(r.work_rev, 0);
    let mut r = row(Some(2), Some(2));
    OrgScope::All.redact_row_org_only(&mut r);
    assert!(r.work.is_some());
    assert_eq!(r.work_rev, 42);
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
    assert!(
        v["rows"][1].get("work_rev").is_none(),
        "cleared even in its own org"
    );
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
    let got = scopes(&st, &vs(&OrgScope::All)).unwrap();
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
    let got = scopes(&st, &vs(&OrgScope::All)).unwrap();
    assert_eq!(got[0].id, Some(a.id));
    assert_eq!(got[0].session_count, 1);
    assert_eq!(got[1].owner.as_deref(), Some("beta"));
    // A host outside Company A sees no Company A scope.
    let outsider = host(None, &[]);
    assert!(scopes(&st, &vs(&outsider))
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
    let got = org_suggestions(&st, &vs(&OrgScope::All)).unwrap();
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
    let got = org_suggestions(&st, &vs(&OrgScope::All)).unwrap();
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
    let got = org_suggestions(&st, &vs(&OrgScope::All)).unwrap();
    assert!(got.iter().all(|g| g.owner.as_deref() != Some("beta")));
    // A per-host token gets nothing to act on.
    assert!(org_suggestions(&st, &vs(&host(None, &[])))
        .unwrap()
        .is_empty());
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
    assert!(org_suggestions(&st, &vs(&OrgScope::All))
        .unwrap()
        .is_empty());
}

#[test]
fn cross_org_links_need_force_and_unassigned_never_conflicts() {
    assert!(check_cross_org(Some(1), Some(1), "X-1", false).is_ok());
    assert!(check_cross_org(None, Some(1), "X-1", false).is_ok());
    assert!(check_cross_org(Some(2), None, "X-1", false).is_ok());
    let e = check_cross_org(Some(2), Some(1), "X-1", false).unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    assert!(e.message.contains("force_cross_org"), "{}", e.message);
    assert!(check_cross_org(Some(2), Some(1), "X-1", true).is_ok());
}

/// **An ended link with NOTHING recorded is withheld, not shared** (multi-user
/// M1, T9c — arm 3 of [`link_person_visible`]).
///
/// `link_person_visible_memo` used to end in a `for` loop over the link's
/// conversation ids and then `Ok(true)`. With no live participant AND no
/// conversation id the body never ran, so the function answered "visible" for
/// the one shape it has nothing at all to judge — fail-OPEN. The shape is
/// reachable on a fleet running M1: `work_links.claude_session_id` is NULL for
/// a session that never had a conversation (`new_shell_session`, or a Claude
/// session reaped before its first SessionStart hook), and migration 046's
/// retire trigger fills `snap_claude_ids` only `HAVING COUNT(*) > 0`. What
/// leaked was the snapshot — `snap_host`, `snap_tmux`, `snap_name`,
/// `snap_branch`, `snap_pr_url` — through `work { links }`, `today`, `tree`
/// and `resume_plan`.
///
/// Rule 7 is kept exactly where it is about rule 7: the single-person hub
/// still sees it (`ViewScope::is_sole_person`), and a per-host token still
/// does (§4.4 gives it a HOST's reach, with no person dimension). On a hub
/// with two people it is nobody's — not even the OWNER's, because there is no
/// record left that says it was hers. That is the fail-closed direction the
/// `None => false` arm above already takes for a participant naming a row
/// that is gone.
#[test]
fn an_ended_link_with_no_conversation_recorded_is_withheld() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let ada = s.personal_owner_id().unwrap().expect("096 mints one");
    let bob = s.create_person("bob", None).unwrap().id;
    assert!(s.sole_enabled_person().unwrap().is_none(), "two people");
    let pid = s.upsert_project("o", "r", "/p").unwrap();
    // A shell session: no Claude conversation, ever.
    let sid = s
        .upsert_session("dev-ada-shell", "h", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    assert!(s.claim_if_unclaimed(sid, Some(ada)).unwrap());
    let item = s
        .name_session_work(sid, Some("LOC-9"), "Ada's")
        .unwrap()
        .0
        .id;
    s.delete_session(sid).unwrap();
    // Belt and braces: the retire trigger wrote no ids, and neither did the
    // insert. This is the shape under test.
    s.conn_ref()
        .execute(
            "UPDATE work_links SET claude_session_id = NULL, snap_claude_ids = NULL, \
             ended_at = ?2 WHERE item_id = ?1",
            rusqlite::params![item, crate::store::now_unix()],
        )
        .unwrap();
    let link = s
        .ended_work_links_for_key("LOC-9")
        .unwrap()
        .into_iter()
        .next()
        .expect("the link survives its session");
    assert!(
        link.claude_session_id.is_none() && link.snap_claude_ids.is_none(),
        "nothing recorded: {link:?}"
    );

    for (who, person) in [("Bob", bob), ("Ada", ada)] {
        let view = crate::mcp::auth::device_view(&s, person);
        assert!(
            !link_person_visible(&s, &view, &link).unwrap(),
            "{who} was handed an ended link nothing can attribute"
        );
    }
    // The hub's own readers keep it (the GC has to see every link), and so
    // does a per-host token, whose fence is the host's.
    assert!(link_person_visible(
        &s,
        &crate::service::view_scope::ViewScope::internal(),
        &link
    )
    .unwrap());
    assert!(link_person_visible(
        &s,
        &crate::service::view_scope::org_only_view(&OrgScope::Host {
            alias: "h".into(),
            org: None,
            isolated: Default::default(),
        }),
        &link
    )
    .unwrap());
}

/// The other half of the rule above: on a hub with ONE person, nothing
/// narrows (rule 7). The same link the test above withholds from everybody is
/// still that person's.
#[test]
fn a_single_person_hub_still_sees_an_ended_link_with_nothing_recorded() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let ada = s.personal_owner_id().unwrap().expect("096 mints one");
    assert_eq!(
        s.sole_enabled_person().unwrap(),
        Some(ada),
        "one person, so the carve-out applies"
    );
    let pid = s.upsert_project("o", "r", "/p").unwrap();
    let sid = s
        .upsert_session("dev-shell", "h", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    assert!(s.claim_if_unclaimed(sid, Some(ada)).unwrap());
    let item = s
        .name_session_work(sid, Some("LOC-9"), "work")
        .unwrap()
        .0
        .id;
    s.delete_session(sid).unwrap();
    s.conn_ref()
        .execute(
            "UPDATE work_links SET claude_session_id = NULL, snap_claude_ids = NULL, \
             ended_at = ?2 WHERE item_id = ?1",
            rusqlite::params![item, crate::store::now_unix()],
        )
        .unwrap();
    let link = s
        .ended_work_links_for_key("LOC-9")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let view = crate::mcp::auth::device_view(&s, ada);
    assert!(
        link_person_visible(&s, &view, &link).unwrap(),
        "the hub's only person could see it yesterday"
    );
}
