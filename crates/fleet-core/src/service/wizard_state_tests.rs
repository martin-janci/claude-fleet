//! Wizards that resume on another device (gap plan G7.2): a person's wizard
//! saved from one device reads back on another, nobody else reads it, and
//! a secret never reaches the store.

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

fn save_args(kind: &str, step: i64, answers: serde_json::Value) -> WizardStateArgs {
    WizardStateArgs {
        action: "save".into(),
        kind: Some(kind.into()),
        step: Some(step),
        answers: Some(answers),
        label: Some("acme/api".into()),
        ..Default::default()
    }
}

#[test]
fn a_wizard_saved_on_the_phone_resumes_on_the_desktop() {
    let st = store();
    let (ada, _) = people(&st);
    let phone = device(&st, ada, "Ada's Pixel");
    let desk = device(&st, ada, "Ada's Mac");
    let saved = save(
        &st,
        &phone,
        &save_args(
            "add_project",
            2,
            json!({ "url": "git@github.com:acme/api.git" }),
        ),
        Some("Ada's Pixel"),
    )
    .unwrap();
    assert_eq!(saved.person_id, Some(ada));
    assert_eq!(saved.device.as_deref(), Some("Ada's Pixel"));
    let back = get(&st, &desk, "add_project", "")
        .unwrap()
        .expect("resumes");
    assert_eq!(back.step, 2);
    assert_eq!(back.answers["url"], "git@github.com:acme/api.git");
    assert_eq!(back.label.as_deref(), Some("acme/api"));
    // The desktop carries on: the same row, now from the desktop.
    let on = save(
        &st,
        &desk,
        &save_args("add_project", 3, json!({ "url": "x" })),
        Some("Ada's Mac"),
    )
    .unwrap();
    assert_eq!((on.step, on.created_at), (3, saved.created_at));
    assert_eq!(list(&st, &phone, None).unwrap().len(), 1);
    assert!(clear(&st, &phone, "add_project", "").unwrap());
    assert!(get(&st, &desk, "add_project", "").unwrap().is_none());
    assert!(!clear(&st, &phone, "add_project", "").unwrap());
}

#[test]
fn another_persons_wizard_reads_as_absent() {
    let st = store();
    let (ada, bob) = people(&st);
    let a = device(&st, ada, "a");
    let b = device(&st, bob, "b");
    save(&st, &a, &save_args("add_project", 1, json!({})), None).unwrap();
    assert!(get(&st, &b, "add_project", "").unwrap().is_none());
    assert!(list(&st, &b, None).unwrap().is_empty());
    assert!(!clear(&st, &b, "add_project", "").unwrap());
    // Bob's own save is his row, beside Ada's.
    save(&st, &b, &save_args("add_project", 4, json!({})), None).unwrap();
    assert_eq!(get(&st, &a, "add_project", "").unwrap().unwrap().step, 1);
    assert_eq!(get(&st, &b, "add_project", "").unwrap().unwrap().step, 4);
}

#[test]
fn a_secret_never_reaches_the_store_and_bad_input_is_refused() {
    let st = store();
    let me = ViewScope::internal();
    for answers in [
        json!({ "api_key": "sk-1" }),
        json!({ "account": { "Token": "x" } }),
        json!({ "list": [{ "password": "p" }] }),
    ] {
        let e = save(&st, &me, &save_args("add_account", 1, answers), None).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains("never hold a secret"), "{}", e.message);
    }
    let big = json!({ "x": "a".repeat(ANSWERS_MAX_BYTES) });
    assert_eq!(
        save(&st, &me, &save_args("form", 1, big), None)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        save(&st, &me, &save_args("nope", 1, json!({})), None)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        save(&st, &me, &save_args("form", 0, json!({})), None)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        save(&st, &me, &save_args("form", 1, json!([1])), None)
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert!(lock(&st).unwrap().wizard_states(None).unwrap().is_empty());
}

#[test]
fn the_add_host_drafts_are_wizard_rows() {
    let st = store();
    {
        let s = lock(&st).unwrap();
        s.save_host_setup("mercury", "merc", 3, &[], &json!({ "install_agent": true }))
            .unwrap();
    }
    let rows = list(&st, &ViewScope::internal(), Some("add_host")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].key.as_str(), rows[0].label.as_deref(), rows[0].step),
        ("mercury", Some("merc"), 3)
    );
    let s = lock(&st).unwrap();
    assert_eq!(s.host_setups().unwrap()[0].alias, "merc");
}

#[test]
fn run_answers_each_action_and_purge_forgets_old_wizards() {
    let st = store();
    let me = ViewScope::internal();
    let saved = run(
        &st,
        &me,
        &save_args("new_session", 2, json!({ "host": "mac" })),
        None,
    )
    .unwrap();
    assert_eq!(saved["step"], 2);
    let got = run(
        &st,
        &me,
        &WizardStateArgs {
            action: "get".into(),
            kind: Some("new_session".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(got["answers"]["host"], "mac");
    let e = run(
        &st,
        &me,
        &WizardStateArgs {
            action: "get".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let now = crate::store::now_unix();
    assert_eq!(purge(&st, now).unwrap(), 0);
    assert_eq!(purge(&st, now + KEEP_SECS + 1).unwrap(), 1);
    let gone = run(
        &st,
        &me,
        &WizardStateArgs {
            action: "list".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(gone, json!([]));
}
