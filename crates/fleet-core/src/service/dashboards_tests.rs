//! Pinned dashboards (persona design, step 6): pinned on one device, shown
//! on another, nobody else's, checked as blocks, and never pinned by the
//! agent.

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use serde_json::json;

fn store() -> Mutex<Store> {
    Mutex::new(Store::open_in_memory().unwrap())
}

/// A person's device, bound to no org, scoped the way a request is.
fn device(store: &Mutex<Store>, person: i64, name: &str) -> ViewScope {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: person * 10,
            name: name.into(),
            trusted: true,
            org_id: None,
            person_id: Some(person),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn people(store: &Mutex<Store>) -> (i64, i64) {
    let s = lock(store).unwrap();
    (
        s.create_person("ada", None).unwrap().id,
        s.create_person("bob", None).unwrap().id,
    )
}

fn block(id: &str, title: &str) -> serde_json::Value {
    json!({
        "spec": "fleet.ui/1", "kind": "dashboard", "id": id, "title": title,
        "why": "You asked how 4.2 is going.",
        "widgets": [ { "kind": "stat", "title": "Open", "query": { "status": "open" }, "value": "open" } ]
    })
}

fn pin_args(b: serde_json::Value) -> DashboardsArgs {
    DashboardsArgs {
        action: "pin".into(),
        dashboard: Some(b),
        why: Some("You asked how 4.2 is going.".into()),
        ..Default::default()
    }
}

fn id_args(action: &str, id: &str) -> DashboardsArgs {
    DashboardsArgs {
        action: action.into(),
        id: Some(id.into()),
        ..Default::default()
    }
}

#[test]
fn pinned_on_the_desktop_it_shows_on_the_phone_and_nobody_elses() {
    let st = store();
    let (ada, bob) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    let phone = device(&st, ada, "Ada's Pixel");
    let row = pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Release 4.2")),
        Some("Ada's Mac"),
    )
    .unwrap();
    assert_eq!(row.person_id, Some(ada));
    assert_eq!(row.title, "Release 4.2");
    assert_eq!(row.device.as_deref(), Some("Ada's Mac"));
    // `why` is kept beside the block, not in it.
    assert!(row.spec.get("why").is_none());
    assert_eq!(row.why.as_deref(), Some("You asked how 4.2 is going."));

    assert_eq!(list(&st, &phone).unwrap().len(), 1);
    assert!(get(&st, &phone, "dash.r42").unwrap().is_some());

    let bobs = device(&st, bob, "Bob's Mac");
    assert!(list(&st, &bobs).unwrap().is_empty());
    assert!(get(&st, &bobs, "dash.r42").unwrap().is_none());
    assert!(!unpin(&st, &bobs, "dash.r42").unwrap());
    // Bob's own pin of the same id is his own row.
    pin(&st, &bobs, &pin_args(block("dash.r42", "Bob's view")), None).unwrap();
    assert_eq!(
        get(&st, &phone, "dash.r42").unwrap().unwrap().title,
        "Release 4.2"
    );

    assert!(unpin(&st, &phone, "dash.r42").unwrap());
    assert!(list(&st, &desk).unwrap().is_empty());
    assert_eq!(list(&st, &bobs).unwrap().len(), 1);
}

#[test]
fn a_second_pin_of_an_id_replaces_it_in_place() {
    let st = store();
    let (ada, _) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    let first = pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Release 4.2")),
        None,
    )
    .unwrap();
    let again = pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Release 4.2, by person")),
        None,
    )
    .unwrap();
    assert_eq!(again.pinned_at, first.pinned_at);
    assert_eq!(list(&st, &desk).unwrap().len(), 1);
    assert_eq!(list(&st, &desk).unwrap()[0].title, "Release 4.2, by person");
}

#[test]
fn a_pin_checks_the_block_refuses_a_builtin_and_stops_at_the_limit() {
    let st = store();
    let (ada, _) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    let mut bad = block("dash.r42", "R");
    bad["widgets"][0]["query"] = json!({ "sprint": "now" });
    let e = pin(&st, &desk, &pin_args(bad), None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(
        e.message.contains("`sprint` is not a filter"),
        "{}",
        e.message
    );

    let e = pin(
        &st,
        &desk,
        &pin_args(json!({ "spec": "fleet.ui/1", "kind": "callout", "body": "x" })),
        None,
    )
    .unwrap_err();
    assert!(e.message.contains("kind \"dashboard\""), "{}", e.message);

    let e = pin(&st, &desk, &pin_args(block("dash.scrum", "Mine")), None).unwrap_err();
    assert!(e.message.contains("built-in"), "{}", e.message);

    for k in 0..MAX_PINNED {
        pin(&st, &desk, &pin_args(block(&format!("d{k}"), "D")), None).unwrap();
    }
    let e = pin(&st, &desk, &pin_args(block("one.more", "D")), None).unwrap_err();
    assert!(e.message.contains("unpin one first"), "{}", e.message);
    // Replacing one already pinned still works when full.
    pin(&st, &desk, &pin_args(block("d3", "Renamed")), None).unwrap();
}

#[test]
fn the_agent_reads_but_never_pins_or_unpins() {
    let st = store();
    let (ada, _) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    run(&st, &desk, &pin_args(block("dash.r42", "R")), None, true).unwrap();
    let listed = run(
        &st,
        &desk,
        &DashboardsArgs {
            action: "list".into(),
            ..Default::default()
        },
        None,
        false,
    )
    .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(
        run(&st, &desk, &id_args("get", "dash.r42"), None, false).unwrap()["id"],
        "dash.r42"
    );
    for args in [pin_args(block("dash.x", "X")), id_args("unpin", "dash.r42")] {
        let e = run(&st, &desk, &args, None, false).unwrap_err();
        assert_eq!(e.code, codes::E_FORBIDDEN);
    }
    assert_eq!(list(&st, &desk).unwrap().len(), 1);
}

#[test]
fn standalone_rows_are_fleets_own_and_a_bad_stored_spec_is_left_out() {
    let st = store();
    let me = ViewScope::internal();
    let row = pin(&st, &me, &pin_args(block("dash.r42", "R")), None).unwrap();
    assert_eq!(row.person_id, None);
    lock(&st)
        .unwrap()
        .pin_dashboard(&PinnedDashboardWrite {
            id: "broken",
            person_id: None,
            title: "B",
            spec: &json!({ "kind": "dashboard" }),
            why: None,
            device: None,
        })
        .unwrap();
    let ids: Vec<String> = list(&st, &me).unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, ["dash.r42"]);
    assert!(get(&st, &me, "broken").unwrap().is_none());
}

#[test]
fn the_builtin_ids_are_the_apps() {
    let ts = crate::repo_files::read("src/lib/dashboards.ts");
    for id in BUILTIN_IDS {
        assert!(
            ts.contains(&format!("id: '{id}'")),
            "{id} is not a built-in in dashboards.ts"
        );
    }
    assert!(ts.contains(&format!("MAX_PINNED = {MAX_PINNED}")));
}
