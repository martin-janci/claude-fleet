//! Who sees which run: the reach `service::runs` hands the store.

use super::*;
use crate::ipc_error::codes;
use crate::service::orgs::OrgScope;
use crate::store::{NewAuxUsage, NewMission};

struct Fx {
    s: Store,
    org_a: i64,
    org_b: i64,
    on_a: i64,
    on_b: i64,
    mission_b: i64,
}

/// A task in a session on each org's host, a mission of org b with its
/// brake and planner run, and a Jev run about a tracker (no session).
fn fixture() -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("a", Some("a")).unwrap();
    s.insert_host("b", Some("b")).unwrap();
    let org_a = s.add_org("Acme", None, false).unwrap().id;
    let org_b = s.add_org("Beta", None, false).unwrap().id;
    s.set_host_org("a", Some(org_a)).unwrap();
    s.set_host_org("b", Some(org_b)).unwrap();
    let on_a = s
        .upsert_session("on-a", "a", None, None, 1, 1, "running", None)
        .unwrap();
    let on_b = s
        .upsert_session("on-b", "b", None, None, 1, 1, "running", None)
        .unwrap();
    s.insert_task(None, Some(on_a), "work for a", "n1").unwrap();
    s.insert_task(None, Some(on_b), "work for b", "n2").unwrap();
    let m = s
        .create_mission(
            &NewMission {
                name: "Beta mission",
                goal: "g",
                org_id: Some(org_b),
                ..Default::default()
            },
            "person:1",
        )
        .unwrap();
    s.record_mission_event(
        m.id,
        &crate::store::NewMissionEvent {
            kind: "budget",
            actor: "loop",
            payload: Some(serde_json::json!({ "why": "spent" })),
            ..Default::default()
        },
    )
    .unwrap();
    s.insert_aux_usage(&NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_PLANNER,
        host_alias: "b".into(),
        model: "sonnet".into(),
        mission_id: Some(m.id),
        org_id: Some(org_b),
        cost_micros: 5,
        at: 10,
        ..Default::default()
    })
    .unwrap();
    s.conn_for_test()
        .execute(
            "INSERT INTO decision_runs (at, feature, org_id, subject_kind, subject_id, mode, \
               provider, question_version, called) \
             VALUES (20, 'status_map', ?1, 'tracker_section', '1:x', 'shadow', 'jev', 'q', 1)",
            [org_b],
        )
        .unwrap();
    Fx {
        s,
        org_a,
        org_b,
        on_a,
        on_b,
        mission_b: m.id,
    }
}

fn kinds(page: &RunsPage) -> Vec<String> {
    let mut v: Vec<String> = page
        .runs
        .iter()
        .map(|r| format!("{}:{}", r.source, r.kind))
        .collect();
    v.sort();
    v
}

fn bound(org: i64) -> ViewScope {
    ViewScope::internal().with_org(OrgScope::Org {
        org,
        sees_unassigned: false,
    })
}

#[test]
fn the_hubs_own_reader_sees_every_run() {
    let fx = fixture();
    assert_eq!(
        reach(&fx.s, &ViewScope::internal()).unwrap(),
        RunsReach::All
    );
    let page = list_in(&fx.s, &ViewScope::internal(), &RunsArgs::default()).unwrap();
    assert_eq!(page.total, 5);
    assert_eq!(
        kinds(&page),
        [
            "aux:planner",
            "jev:jev",
            "orchestration:mission",
            "task:operator",
            "task:operator"
        ]
    );
}

#[test]
fn a_reader_bound_to_one_org_sees_none_of_anothers_runs() {
    let fx = fixture();
    let a = bound(fx.org_a);
    match reach(&fx.s, &a).unwrap() {
        RunsReach::Scoped {
            sessions,
            missions,
            routines,
            spend,
        } => {
            assert_eq!(sessions, [fx.on_a]);
            assert!(missions.is_empty());
            assert!(routines.is_empty());
            // An org fence never reads whole-fleet spend.
            assert!(!spend);
        }
        RunsReach::All => panic!("a bound reader is not unrestricted"),
    }
    let page = list_in(&fx.s, &a, &RunsArgs::default()).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.runs[0].session_ids, [fx.on_a]);
    // Asking for org b's session or mission by id finds nothing, rather than
    // refusing: no existence oracle.
    for args in [
        RunsArgs {
            session_id: Some(fx.on_b),
            ..Default::default()
        },
        RunsArgs {
            mission_id: Some(fx.mission_b),
            ..Default::default()
        },
        RunsArgs {
            org_id: Some(fx.org_b),
            ..Default::default()
        },
    ] {
        let page = list_in(&fx.s, &a, &args).unwrap();
        assert_eq!((page.runs.len(), page.total), (0, 0), "{args:?}");
    }

    // Org b's reader sees its task and its mission's rows, still not the
    // Jev run about a tracker (that is whole-fleet spend).
    let page = list_in(&fx.s, &bound(fx.org_b), &RunsArgs::default()).unwrap();
    assert_eq!(
        kinds(&page),
        ["aux:planner", "orchestration:mission", "task:operator"]
    );
}

#[test]
fn a_bad_filter_is_refused() {
    let fx = fixture();
    let e = list_in(
        &fx.s,
        &ViewScope::internal(),
        &RunsArgs {
            outcome: Some("great".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    // An empty filter string is no filter.
    let page = list_in(
        &fx.s,
        &ViewScope::internal(),
        &RunsArgs {
            kind: Some(String::new()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.total, 5);
}
