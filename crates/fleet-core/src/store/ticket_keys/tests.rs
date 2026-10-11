use super::*;
use crate::store::{NativeItem, TrackerItemWrite};

fn write(ext: &str, key: &str) -> TrackerItemWrite {
    TrackerItemWrite {
        external_id: ext.into(),
        key: Some(key.into()),
        title: format!("{key} title"),
        status_name: "Open".into(),
        status_category: "todo".into(),
        ..Default::default()
    }
}

fn task(s: &Store, title: &str, project_id: Option<i64>, parent_id: Option<i64>) -> WorkItemRow {
    s.create_native_item(&NativeItem {
        title,
        parent_id,
        project_id,
        notes: None,
    })
    .unwrap()
}

#[test]
fn a_prefix_is_derived_from_the_name_like_jira_would() {
    assert_eq!(derive_ticket_prefix("claude-fleet"), "CF");
    assert_eq!(derive_ticket_prefix("fleet_mobile"), "FM");
    assert_eq!(derive_ticket_prefix("fleetMobile"), "FM");
    assert_eq!(derive_ticket_prefix("payments"), "PAY");
    assert_eq!(derive_ticket_prefix("web"), "WEB");
    assert_eq!(derive_ticket_prefix("Acme Corp Asana Board Five"), "ACAB");
    assert_eq!(derive_ticket_prefix("2fa-service"), "FS");
    assert_eq!(derive_ticket_prefix("x"), "TKT");
    assert_eq!(derive_ticket_prefix("—"), "TKT");
    for p in ["CF", "PAY", "TKT", "ACAB"] {
        assert!(valid_ticket_prefix(p), "{p}");
    }
    for p in ["C", "cf", "1AB", "ABCDEFGHIJK", "A-B"] {
        assert!(!valid_ticket_prefix(p), "{p}");
    }
}

#[test]
fn a_task_in_a_project_draws_the_projects_next_key_and_a_child_its_parents() {
    let s = Store::open_in_memory().unwrap();
    let p = s.upsert_project("acme", "claude-fleet", "/p/cf").unwrap();
    let a = task(&s, "First", Some(p), None);
    let b = task(&s, "Second", Some(p), None);
    assert_eq!(a.key.as_deref(), Some("CF-1"));
    assert_eq!(a.display_key.as_deref(), Some("CF-1"));
    assert_eq!(b.key.as_deref(), Some("CF-2"));
    let child = task(&s, "Child", None, Some(a.id));
    assert_eq!(child.key.as_deref(), Some("CF-3"));
    // Outside any project: the fleet-wide TASK-<id>, no ticket key.
    let loose = task(&s, "Loose", None, None);
    assert_eq!(loose.key, Some(format!("TASK-{}", loose.id)));
    assert_eq!(loose.display_key, None);
}

#[test]
fn two_projects_whose_names_suggest_one_prefix_get_distinct_ones() {
    let s = Store::open_in_memory().unwrap();
    let a = s.upsert_project("acme", "claude-fleet", "/p/a").unwrap();
    let b = s.upsert_project("other", "core-flow", "/p/b").unwrap();
    assert_eq!(task(&s, "a", Some(a), None).key.as_deref(), Some("CF-1"));
    assert_eq!(task(&s, "b", Some(b), None).key.as_deref(), Some("CF2-1"));
}

#[test]
fn a_prefix_a_key_already_uses_is_not_taken_and_a_hand_named_key_is_skipped() {
    let s = Store::open_in_memory().unwrap();
    s.create_local_work_item(Some("PAY-1"), "named by hand")
        .unwrap();
    let p = s.upsert_project("acme", "payments", "/p/pay").unwrap();
    assert_eq!(task(&s, "t", Some(p), None).key.as_deref(), Some("PAY2-1"));

    // A number some item already carries is skipped, not reused.
    let q = s.upsert_project("acme", "web", "/p/web").unwrap();
    assert_eq!(s.project_ticket_space(q).unwrap().prefix, "WEB");
    s.create_local_work_item(Some("WEB-1"), "hand").unwrap();
    assert_eq!(task(&s, "t", Some(q), None).key.as_deref(), Some("WEB-2"));
}

#[test]
fn an_asana_task_draws_a_ticket_key_once_and_answers_to_it() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("asana", "Company B", "https://app.asana.com")
        .unwrap();
    let out = s
        .upsert_tracker_item(t.id, &write("1209876543210987", "asana:1209876543210987"))
        .unwrap();
    let item = s.get_work_item(out.id).unwrap().unwrap();
    assert_eq!(item.key.as_deref(), Some("asana:1209876543210987"));
    assert_eq!(item.display_key.as_deref(), Some("CB-1"));
    // A second sync keeps it.
    s.upsert_tracker_item(t.id, &write("1209876543210987", "asana:1209876543210987"))
        .unwrap();
    let again = s.get_work_item(out.id).unwrap().unwrap();
    assert_eq!(again.display_key.as_deref(), Some("CB-1"));
    let other = s
        .upsert_tracker_item(t.id, &write("1209876543210999", "asana:1209876543210999"))
        .unwrap();
    assert_eq!(
        s.get_work_item(other.id)
            .unwrap()
            .unwrap()
            .display_key
            .as_deref(),
        Some("CB-2")
    );
    // Every key lookup answers to it, in any case.
    assert_eq!(
        s.tracker_item_for_key("cb-1").unwrap().map(|i| i.id),
        Some(out.id)
    );
    assert_eq!(
        s.item_by_ticket_key("CB-1").unwrap().map(|i| i.id),
        Some(out.id)
    );
}

#[test]
fn a_github_issue_shares_the_numbering_of_the_project_whose_repository_it_is() {
    let s = Store::open_in_memory().unwrap();
    let p = s.upsert_project("acme", "claude-fleet", "/p/cf").unwrap();
    let t = s
        .add_tracker("github", "GitHub", "https://github.com/acme")
        .unwrap();
    let native = task(&s, "native", Some(p), None);
    let out = s
        .upsert_tracker_item(t.id, &write("I_1", "acme/claude-fleet#42"))
        .unwrap();
    let issue = s.get_work_item(out.id).unwrap().unwrap();
    assert_eq!(native.key.as_deref(), Some("CF-1"));
    assert_eq!(issue.display_key.as_deref(), Some("CF-2"));
    // A repository no project claims numbers on its own.
    let lone = s
        .upsert_tracker_item(t.id, &write("I_2", "acme/web#7"))
        .unwrap();
    assert_eq!(
        s.get_work_item(lone.id)
            .unwrap()
            .unwrap()
            .display_key
            .as_deref(),
        Some("WEB-1")
    );
}

#[test]
fn a_jira_key_draws_nothing() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    let out = s
        .upsert_tracker_item(t.id, &write("10001", "ABC-12"))
        .unwrap();
    assert_eq!(s.get_work_item(out.id).unwrap().unwrap().display_key, None);
    assert!(s.list_ticket_spaces().unwrap().is_empty());
}

#[test]
fn a_prefix_a_tracker_probed_is_never_derived() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &crate::store::TrackerConfig {
            key_prefixes: vec!["CF".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let p = s.upsert_project("acme", "claude-fleet", "/p/cf").unwrap();
    assert_eq!(s.project_ticket_space(p).unwrap().prefix, "CF2");
}

#[test]
fn renaming_a_prefix_moves_every_key_and_the_old_one_still_resolves() {
    let s = Store::open_in_memory().unwrap();
    let p = s.upsert_project("acme", "claude-fleet", "/p/cf").unwrap();
    let gh = s
        .add_tracker("github", "GitHub", "https://github.com/acme")
        .unwrap();
    let a = task(&s, "a", Some(p), None);
    let issue = s
        .upsert_tracker_item(gh.id, &write("I_1", "acme/claude-fleet#3"))
        .unwrap()
        .id;
    let space = s.project_ticket_space(p).unwrap();

    assert_eq!(
        s.rename_ticket_prefix(space.id, "bad prefix")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        s.rename_ticket_prefix(space.id, "TASK").unwrap_err().code,
        codes::E_EXISTS
    );
    let renamed = s.rename_ticket_prefix(space.id, "flt").unwrap();
    assert_eq!(renamed.prefix, "FLT");
    assert_eq!(renamed.former_prefixes, vec!["CF".to_string()]);

    let a2 = s.get_work_item(a.id).unwrap().unwrap();
    assert_eq!(a2.key.as_deref(), Some("FLT-1"));
    let i2 = s.get_work_item(issue).unwrap().unwrap();
    assert_eq!(i2.key.as_deref(), Some("acme/claude-fleet#3"));
    assert_eq!(i2.display_key.as_deref(), Some("FLT-2"));

    // The old keys resolve to the same items.
    assert_eq!(
        s.local_work_item_by_key("CF-1").unwrap().map(|i| i.id),
        Some(a.id)
    );
    assert_eq!(
        s.tracker_item_for_key("CF-2").unwrap().map(|i| i.id),
        Some(issue)
    );
    // Nothing else may take the old prefix; the space itself may take it back.
    let q = s.upsert_project("acme", "core-front", "/p/x").unwrap();
    assert_ne!(s.project_ticket_space(q).unwrap().prefix, "CF");
    assert_eq!(s.rename_ticket_prefix(space.id, "CF").unwrap().prefix, "CF");
    // New work keeps counting where the space left off.
    assert_eq!(task(&s, "b", Some(p), None).key.as_deref(), Some("CF-3"));
}
