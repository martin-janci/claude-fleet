//! Pinned dashboards (persona design, step 6): pinned on one device, shown
//! on another, nobody else's, checked as blocks, changed by the agent
//! when asked and taken back with undo.

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::service::operator::OPERATOR_CLIENT_NAME;
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
fn the_agent_pins_a_change_but_never_unpins_or_undoes() {
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
    // Owner report 2026-11-06: a change asked of Control now lands.
    let changed = run(
        &st,
        &desk,
        &pin_args(block("dash.r42", "R, by person")),
        Some(OPERATOR_CLIENT_NAME),
        false,
    )
    .unwrap();
    assert_eq!(changed["title"], "R, by person");
    assert_eq!(changed["device"], OPERATOR_CLIENT_NAME);
    assert_eq!(changed["can_undo"], true);
    for args in [id_args("unpin", "dash.r42"), id_args("undo", "dash.r42")] {
        let e = run(&st, &desk, &args, Some(OPERATOR_CLIENT_NAME), false).unwrap_err();
        assert_eq!(e.code, codes::E_FORBIDDEN);
    }
    assert_eq!(list(&st, &desk).unwrap().len(), 1);
}

#[test]
fn undo_puts_back_the_version_before_controls_change_once() {
    let st = store();
    let (ada, _) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Mine")),
        Some("Ada's Mac"),
    )
    .unwrap();
    // A pin of the same block keeps nothing to undo.
    let same = pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Mine")),
        Some("Ada's Mac"),
    )
    .unwrap();
    assert!(!same.can_undo);
    let e = undo(&st, &desk, "dash.r42").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);

    pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Control's")),
        Some(OPERATOR_CLIENT_NAME),
    )
    .unwrap();
    let back = run(&st, &desk, &id_args("undo", "dash.r42"), None, true).unwrap();
    assert_eq!(back["undone"], true);
    let row = get(&st, &desk, "dash.r42").unwrap().unwrap();
    assert_eq!(row.title, "Mine");
    assert_eq!(row.spec["title"], "Mine");
    assert_eq!(row.device.as_deref(), Some("Ada's Mac"));
    assert!(!row.can_undo, "one step only");
    assert!(undo(&st, &desk, "dash.r42").is_err());

    // A dashboard Control pinned new is taken back by unpinning it.
    pin(
        &st,
        &desk,
        &pin_args(block("dash.scrum.mine", "Copy")),
        Some(OPERATOR_CLIENT_NAME),
    )
    .unwrap();
    let gone = undo(&st, &desk, "dash.scrum.mine").unwrap();
    assert_eq!(gone["removed"], true);
    assert!(get(&st, &desk, "dash.scrum.mine").unwrap().is_none());
    assert_eq!(
        undo(&st, &desk, "nope").unwrap_err().code,
        codes::E_NOTFOUND
    );
}

/// Review of the dashboards work, 2: Control's second change in a row
/// keeps the version before its first, so Undo never takes away the
/// person's own dashboard; and the row says what Undo does.
#[test]
fn undo_after_two_control_changes_puts_back_the_persons_version() {
    let st = store();
    let (ada, _) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    let mine = pin(
        &st,
        &desk,
        &pin_args(block("dash.r42", "Mine")),
        Some("Ada's Mac"),
    )
    .unwrap();
    assert_eq!(mine.undo, None);
    for title in ["Control's", "Control's, fixed"] {
        let row = pin(
            &st,
            &desk,
            &pin_args(block("dash.r42", title)),
            Some(OPERATOR_CLIENT_NAME),
        )
        .unwrap();
        assert_eq!(row.undo.as_deref(), Some("restore"), "{title}");
    }
    let back = undo(&st, &desk, "dash.r42").unwrap();
    assert_eq!(
        (back["undone"].clone(), back["removed"].clone()),
        (json!(true), json!(false))
    );
    let row = get(&st, &desk, "dash.r42").unwrap().unwrap();
    assert_eq!(row.title, "Mine");
    assert_eq!(row.device.as_deref(), Some("Ada's Mac"));
    assert_eq!(row.undo, None);

    // Pinned new by Control, changed by it again: Undo still removes it.
    for title in ["Copy", "Copy, fixed"] {
        let row = pin(
            &st,
            &desk,
            &pin_args(block("dash.new", title)),
            Some(OPERATOR_CLIENT_NAME),
        )
        .unwrap();
        assert_eq!(row.undo.as_deref(), Some("remove"), "{title}");
    }
    assert_eq!(
        serde_json::to_value(get(&st, &desk, "dash.new").unwrap().unwrap()).unwrap()["undo"],
        "remove"
    );
    assert_eq!(undo(&st, &desk, "dash.new").unwrap()["removed"], true);
}

/// Review of the dashboards work, 1: on the standalone desktop (the hub's
/// own caller) a dashboard Control pinned as the hub's owner is the
/// desktop's: it undoes, unpins and is replaced in place, never doubled.
#[test]
fn the_standalone_desktop_owns_what_control_pinned_as_the_hubs_owner() {
    let st = store();
    let owner = lock(&st).unwrap().personal_owner_id().unwrap().unwrap();
    let control = device(&st, owner, OPERATOR_CLIENT_NAME);
    let desk = ViewScope::internal();
    pin(
        &st,
        &control,
        &pin_args(block("dash.developer.mine", "Mine")),
        Some(OPERATOR_CLIENT_NAME),
    )
    .unwrap();
    assert_eq!(list(&st, &desk).unwrap().len(), 1);
    let replaced = pin(
        &st,
        &desk,
        &pin_args(block("dash.developer.mine", "Mine, again")),
        None,
    )
    .unwrap();
    assert_eq!(replaced.person_id, Some(owner));
    assert_eq!(list(&st, &desk).unwrap().len(), 1, "replaced in place");
    assert_eq!(
        undo(&st, &desk, "dash.developer.mine").unwrap()["undone"],
        true
    );
    assert!(unpin(&st, &desk, "dash.developer.mine").unwrap());
    assert!(list(&st, &desk).unwrap().is_empty());

    // A feed Control writes is the desktop's too.
    feed(&st, &control, &feed_args("deploys", "3"), false).unwrap();
    feed(&st, &desk, &feed_args("deploys", "4"), true).unwrap();
    let rows = feeds(&st, &desk).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].value.as_str(), rows[0].person_id),
        ("4", Some(owner))
    );
}

#[test]
fn standalone_rows_are_the_hub_owners_and_a_bad_stored_spec_is_left_out() {
    let st = store();
    let me = ViewScope::internal();
    let row = pin(&st, &me, &pin_args(block("dash.r42", "R")), None).unwrap();
    assert_eq!(
        row.person_id,
        lock(&st).unwrap().personal_owner_id().unwrap()
    );
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

#[test]
fn a_pin_and_an_unpin_tell_the_persons_devices_with_an_empty_frame() {
    let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
    let st = Mutex::new(Store::open_with_bus_in_memory(bus.clone()).unwrap());
    let me = ViewScope::internal();
    let count = || {
        bus.names()
            .iter()
            .filter(|n| **n == "dashboards:changed")
            .count()
    };
    pin(&st, &me, &pin_args(block("dash.r42", "R")), None).unwrap();
    assert_eq!(count(), 1);
    // Unpinning what is not pinned changes nothing and says nothing.
    assert!(!unpin(&st, &me, "dash.other").unwrap());
    assert_eq!(count(), 1);
    assert!(unpin(&st, &me, "dash.r42").unwrap());
    assert_eq!(count(), 2);
    assert_eq!(
        crate::events::RowChange::DashboardsChanged.payload(),
        json!({})
    );
}

fn feed_args(key: &str, value: &str) -> DashboardsArgs {
    DashboardsArgs {
        action: "feed".into(),
        key: Some(key.into()),
        value: Some(value.into()),
        ..Default::default()
    }
}

#[test]
fn a_feed_is_written_by_control_or_the_person_and_read_by_them_only() {
    let st = store();
    let (ada, bob) = people(&st);
    let desk = device(&st, ada, "Ada's Mac");
    let theirs = device(&st, bob, "Bob's Mac");
    let w = run(
        &st,
        &desk,
        &feed_args("deploys", "  3 today  "),
        Some(OPERATOR_CLIENT_NAME),
        false,
    )
    .unwrap();
    assert_eq!(w["value"], "3 today");
    assert_eq!(w["by"], "agent");
    run(&st, &desk, &feed_args("deploys", "4 today"), None, true).unwrap();
    let mine = feeds(&st, &desk).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(
        (mine[0].value.as_str(), mine[0].by.as_str()),
        ("4 today", "person")
    );
    assert!(feeds(&st, &theirs).unwrap().is_empty());
    // An empty value removes it.
    assert_eq!(
        run(&st, &desk, &feed_args("deploys", " "), None, true).unwrap(),
        json!(null)
    );
    assert!(feeds(&st, &desk).unwrap().is_empty());
    for bad in ["", "has space", "quote\"d"] {
        let e = run(&st, &desk, &feed_args(bad, "x"), None, true).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{bad:?}");
    }
}

#[test]
fn a_feed_value_is_cut_and_the_feeds_stop_at_the_limit() {
    let st = store();
    let me = ViewScope::internal();
    let long = "x".repeat(FEED_VALUE_MAX_CHARS + 10);
    let w = feed(&st, &me, &feed_args("long", &long), true)
        .unwrap()
        .unwrap();
    assert_eq!(w.value.chars().count(), FEED_VALUE_MAX_CHARS);
    for i in 1..MAX_FEEDS {
        feed(&st, &me, &feed_args(&format!("k{i}"), "1"), true).unwrap();
    }
    let e = feed(&st, &me, &feed_args("one.more", "1"), true).unwrap_err();
    assert!(e.message.contains("feeds are kept"), "{}", e.message);
    // Rewriting one already kept still works when full.
    feed(&st, &me, &feed_args("k1", "2"), true).unwrap();
}

#[test]
fn a_routine_reply_names_its_feeds_in_blocks() {
    let reply = "Done.\n<dashboard-feed key=\"deploys\">\n3 today\n</dashboard-feed>\n\
                 The tag is `<dashboard-feed key=\"x\">no</dashboard-feed>`.\n\
                 <dashboard-feed>no key</dashboard-feed>\
                 <dashboard-feed key=\"bad key\">no</dashboard-feed>\
                 <dashboard-feed key=\"ci:main\">green</dashboard-feed>";
    assert_eq!(
        feed_blocks(reply),
        vec![
            ("deploys".to_string(), "3 today".to_string()),
            ("ci:main".to_string(), "green".to_string()),
        ]
    );
    assert!(feed_blocks("<dashboard-feed key=\"open\">never closed").is_empty());
    // Review of the dashboards work, 4: a quoted tag does not swallow the
    // real block after it, and a mention restarts at the real open.
    let one = vec![("deploys".to_string(), "3".to_string())];
    assert_eq!(
        feed_blocks(
            "Updated `<dashboard-feed key=\"deploys\">` as asked:\n\
             <dashboard-feed key=\"deploys\">3</dashboard-feed>"
        ),
        one
    );
    assert_eq!(
        feed_blocks(
            "I end with <dashboard-feed key=\"deploys\"> as told.\n\n\
             <dashboard-feed key=\"deploys\">3</dashboard-feed>"
        ),
        one
    );
}

#[test]
fn a_feed_write_tells_the_persons_devices() {
    let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
    let st = Mutex::new(Store::open_with_bus_in_memory(bus.clone()).unwrap());
    let me = ViewScope::internal();
    feed(&st, &me, &feed_args("k", "1"), false).unwrap();
    feed(&st, &me, &feed_args("k", ""), true).unwrap();
    // Removing what is not there says nothing.
    feed(&st, &me, &feed_args("k", ""), true).unwrap();
    let n = bus
        .names()
        .iter()
        .filter(|n| **n == "dashboards:changed")
        .count();
    assert_eq!(n, 2);
}
