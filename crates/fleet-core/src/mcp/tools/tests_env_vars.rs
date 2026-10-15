//! `env_vars` through the tool: the `person` scope is every person's own
//! (their git identity, their `GH_TOKEN`), and the shared scopes stay the
//! admin's.

use super::*;
use crate::mcp::auth::ClientRef;

/// A colleague's paired device: a person, not this hub's owner, not a
/// fleet admin, and not trusted by the operator.
fn device_of(person: i64, mode: TokenMode) -> Caller {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 100 + person,
            name: format!("phone-{person}"),
            trusted: false,
            org_id: None,
            person_id: Some(person),
        }),
        mode,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    }
}

fn host_token() -> Caller {
    Caller {
        api: None,
        host_alias: Some("mercury".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    }
}

async fn call(
    t: &FleetTools,
    c: &Caller,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    enforce_mode(c, "env_vars")
        .and_then(|()| enforce_admin(c, "env_vars"))
        .map_err(|e| e.message.to_string())?;
    t.env_vars(
        Extension(c.clone()),
        Parameters(serde_json::from_value(args).unwrap()),
    )
    .await
    .map(|r| super::tests::result_json(&r))
    .map_err(|e| e.message.to_string())
}

fn people(t: &FleetTools) -> (i64, i64) {
    let s = t.store.lock().unwrap();
    let add = |name: &str| -> i64 {
        s.conn_for_test()
            .query_row(
                "INSERT INTO people (name, is_personal_owner, created_at) VALUES (?1, 0, 1) \
                 RETURNING id",
                [name],
                |r| r.get(0),
            )
            .unwrap()
    };
    (add("ada"), add("bob"))
}

fn names(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{}:{}:{}",
                r["scope"].as_str().unwrap(),
                r["scope_key"].as_str().unwrap(),
                r["name"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn any_persons_device_reaches_the_tool_and_a_host_token_does_not() {
    assert!(enforce_admin(&device_of(9, TokenMode::Full), "env_vars").is_ok());
    assert!(enforce_admin(&Caller::master(), "env_vars").is_ok());
    let e = enforce_admin(&host_token(), "env_vars").unwrap_err();
    assert!(e.message.starts_with("E_FORBIDDEN"), "{}", e.message);
    assert!(enforce_mode(&device_of(9, TokenMode::Readonly), "env_vars").is_err());
}

#[tokio::test]
async fn a_person_keeps_their_identity_to_themselves() {
    let t = super::tests::test_tools(Store::open_in_memory().unwrap());
    let (ada, bob) = people(&t);
    let (ada_dev, bob_dev) = (
        device_of(ada, TokenMode::Full),
        device_of(bob, TokenMode::Full),
    );

    // Ada sets her own identity, defaulting to herself ('' and 'me').
    let out = call(
        &t,
        &ada_dev,
        serde_json::json!({"action": "set", "scope": "person", "name": "GH_TOKEN",
                           "value": "ghp_ada_only_1234", "secret": true}),
    )
    .await
    .unwrap();
    assert_eq!(out["scope_key"], ada.to_string());
    assert!(!out.to_string().contains("ghp_ada_only_1234"), "{out}");
    call(
        &t,
        &ada_dev,
        serde_json::json!({"action": "set", "scope": "person", "scope_key": "me",
                           "name": "GIT_AUTHOR_EMAIL", "value": "ada@example.com"}),
    )
    .await
    .unwrap();

    // Bob can neither read nor change Ada's rows.
    let bob_list = call(&t, &bob_dev, serde_json::json!({"action": "list"}))
        .await
        .unwrap();
    assert!(names(&bob_list).is_empty(), "{bob_list}");
    for action in ["set", "unset"] {
        let e = call(
            &t,
            &bob_dev,
            serde_json::json!({"action": action, "scope": "person", "scope_key": ada.to_string(),
                               "name": "GH_TOKEN", "value": "ghp_bob_hijack"}),
        )
        .await
        .unwrap_err();
        assert!(e.starts_with("E_FORBIDDEN"), "{action}: {e}");
    }
    // Nor can the master reach into a person's scope.
    let e = call(
        &t,
        &Caller::master(),
        serde_json::json!({"action": "set", "scope": "person", "scope_key": ada.to_string(),
                           "name": "GH_TOKEN", "value": "x"}),
    )
    .await
    .unwrap_err();
    assert!(e.starts_with("E_FORBIDDEN"), "{e}");

    // Ada's listing has her two rows, the secret's value never.
    let ada_list = call(&t, &ada_dev, serde_json::json!({"action": "list"}))
        .await
        .unwrap();
    assert_eq!(
        names(&ada_list),
        [
            format!("person:{ada}:GH_TOKEN"),
            format!("person:{ada}:GIT_AUTHOR_EMAIL")
        ]
    );
    let raw = ada_list.to_string();
    assert!(
        raw.contains("ada@example.com") && !raw.contains("ghp_ada_only_1234"),
        "{raw}"
    );

    // A colleague stays out of the shared scopes, and the master's listing
    // shows them but no one's person scope.
    let e = call(
        &t,
        &ada_dev,
        serde_json::json!({"action": "set", "scope": "global", "name": "X", "value": "v"}),
    )
    .await
    .unwrap_err();
    assert!(e.starts_with("E_FORBIDDEN"), "{e}");
    call(
        &t,
        &Caller::master(),
        serde_json::json!({"action": "set", "scope": "host", "scope_key": "mercury",
                           "name": "GH_TOKEN", "value": "ghp_fallback", "secret": true}),
    )
    .await
    .unwrap();
    let master_list = call(&t, &Caller::master(), serde_json::json!({"action": "list"}))
        .await
        .unwrap();
    assert_eq!(names(&master_list), ["host:mercury:GH_TOKEN"]);
    assert!(names(
        &call(&t, &ada_dev, serde_json::json!({"action": "list"}))
            .await
            .unwrap()
    )
    .iter()
    .all(|n| n.starts_with("person:")));

    // At launch, Ada's session gets her token over the host fallback; an
    // unclaimed launch gets the fallback.
    let s = t.store.lock().unwrap();
    let get = |person: Option<i64>| -> String {
        s.env_for_launch("mercury", None, person, None)
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "GH_TOKEN")
            .map(|(_, v)| match v {
                crate::store::EnvSource::Value(v) => v.expose().to_string(),
                crate::store::EnvSource::Ref(r) => r,
            })
            .unwrap()
    };
    assert_eq!(get(Some(ada)), "ghp_ada_only_1234");
    assert_eq!(get(Some(bob)), "ghp_fallback");
    assert_eq!(get(None), "ghp_fallback");
}
