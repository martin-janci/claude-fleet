//! `changesets` driven the way `call_tool` drives it — spec, Testing
//! (authorization): "an ungranted client and a per-host token cannot apply,
//! undo or admit in a catalog they have no grant for". A per-host token
//! never reaches the tool at all (R25 amended: it is `NOT_FOR_HOST_TOKENS`;
//! a host's "can list" is `list_assets` and the inventory). `list` is the
//! master's or an unbound full client's, as M3's `list_catalogs` (PF15).

use super::tests_catalog_admin::{client, code_of, host, message_of, tools, two_catalog_store};
use super::*;
use crate::store::NewChangesetItem;

/// One `changesets` call through the gates `call_tool` runs first.
async fn call(
    t: &FleetTools,
    caller: &Caller,
    action: &str,
    id: Option<i64>,
    positions: Option<Vec<i64>>,
) -> Result<CallToolResult, McpError> {
    enforce_mode(caller, "changesets")?;
    enforce_admin(caller, "changesets")?;
    t.changesets(
        Extension(caller.clone()),
        Parameters(ChangesetsParams {
            action: action.into(),
            id,
            positions,
            confirm_nonce: None,
        }),
    )
    .await
}

fn item(
    grp: &str,
    catalog_id: Option<i64>,
    kind: &str,
    action: &str,
    params: &str,
) -> NewChangesetItem {
    NewChangesetItem {
        grp: grp.into(),
        catalog_id,
        kind: kind.into(),
        name: if kind == "host" {
            "h".into()
        } else {
            "w".into()
        },
        action: action.into(),
        params: Some(params.into()),
        decider: "rule".into(),
    }
}

/// [`two_catalog_store`] plus one open New card importing into acme.
/// Returns `(store, desk, ops, plain, acme_id, card_id)`.
fn seeded() -> (Store, i64, i64, i64, i64, i64) {
    let (s, desk, ops, plain, acme) = two_catalog_store();
    let card = s
        .insert_changeset(
            "new",
            "New on oci: skill/w → core",
            &[item(
                "core",
                Some(acme),
                "skill",
                "import",
                r#"{"from_host":"oci","layer":"core","member":"skill/w","hash":"h"}"#,
            )],
        )
        .unwrap();
    (s, desk, ops, plain, acme, card.id)
}

fn state_of(t: &FleetTools, id: i64) -> String {
    t.store
        .lock()
        .unwrap()
        .get_changeset(id)
        .unwrap()
        .unwrap()
        .state
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn list_is_open_to_unbound_clients_but_only_a_grant_on_the_cards_catalog_may_act() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, plain, acme, card) = seeded();
    let org_id = s.get_catalog(acme).unwrap().unwrap().org_id;
    let bound = s.insert_client_token("bound", "ee55", "full").unwrap();
    s.set_client_org("bound", org_id).unwrap();
    let t = tools(s);
    for caller in [
        Caller::master(),
        client(desk, TokenMode::Full, None),
        client(plain, TokenMode::Full, None),
    ] {
        assert_eq!(code_of(&call(&t, &caller, "list", None, None).await), "OK");
        assert_eq!(
            code_of(&call(&t, &caller, "list", Some(card), None).await),
            "OK"
        );
    }
    // PF15: an org-bound client sees no cards, as `list_catalogs`.
    let r = call(
        &t,
        &client(bound.id, TokenMode::Full, org_id),
        "list",
        None,
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("bound to no org"));
    // R25 amended: a per-host token never reaches the tool, nor sees it.
    for action in ["list", "dismiss"] {
        let r = call(&t, &host("h1"), action, Some(card), None).await;
        assert_eq!(code_of(&r), "E_FORBIDDEN", "{action}");
        assert!(message_of(r).contains("never a per-host token's"));
    }
    assert!(!present::visible_to(&host("h1"), "changesets"));
    assert!(present::visible_to(&Caller::master(), "changesets"));

    for caller in [
        client(desk, TokenMode::Full, None),
        client(plain, TokenMode::Full, None),
    ] {
        for action in ["apply", "undo", "dismiss"] {
            let r = call(&t, &caller, action, Some(card), None).await;
            assert_eq!(code_of(&r), "E_FORBIDDEN", "{action}: {:?}", r.err());
        }
        let r = call(&t, &caller, "reject_item", Some(card), Some(vec![0])).await;
        assert_eq!(code_of(&r), "E_FORBIDDEN");
        assert!(message_of(r).contains("--catalog acme"), "names the remedy");
    }
    assert_eq!(state_of(&t, card), "proposed", "nothing changed");
    let r = call(
        &t,
        &client(ops, TokenMode::Full, None),
        "dismiss",
        Some(card),
        None,
    )
    .await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(state_of(&t, card), "dismissed");
}

/// R25: propose is fleet-wide (the personal grant); a readonly client is
/// refused by the mode gate, list included.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn propose_needs_the_personal_grant_and_a_readonly_client_is_refused() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, ops, _plain, _acme, _card) = seeded();
    let kiosk = s.insert_client_token("kiosk", "dd44", "readonly").unwrap();
    let t = tools(s);
    let r = call(
        &t,
        &client(ops, TokenMode::Full, None),
        "propose",
        None,
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("catalog personal"));
    let r = call(
        &t,
        &client(kiosk.id, TokenMode::Readonly, None),
        "list",
        None,
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    let r = call(&t, &Caller::master(), "bogus", None, None).await;
    assert_eq!(code_of(&r), "E_INVALID");
}

/// R25: applying a rollout writes hosts — the personal grant too, and the
/// `apply_sync` confirm gate.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn applying_a_rollout_needs_personal_too_and_passes_the_confirm_gate() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, _plain, acme, _card) = seeded();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true")
        .unwrap();
    let rollout = s
        .insert_changeset(
            "rollout",
            "Roll out ops to h",
            &[item(
                "ops",
                Some(acme),
                "host",
                "sync",
                r#"{"layer":"ops","assets":["skill/w"]}"#,
            )],
        )
        .unwrap();
    let t = tools(s);
    let r = call(
        &t,
        &client(ops, TokenMode::Full, None),
        "apply",
        Some(rollout.id),
        None,
    )
    .await;
    assert_eq!(
        code_of(&r),
        "E_FORBIDDEN",
        "a host-writing apply also needs personal"
    );
    assert!(message_of(r).contains("catalog personal"));
    let r = call(
        &t,
        &client(desk, TokenMode::Full, None),
        "apply",
        Some(rollout.id),
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "personal alone is not acme");
    assert!(message_of(r).contains("--catalog acme"));
    let asked = call(&t, &Caller::master(), "apply", Some(rollout.id), None)
        .await
        .unwrap_err();
    assert!(
        asked.message.starts_with(codes::E_CONFIRM_REQUIRED),
        "{}",
        asked.message
    );
    assert_eq!(state_of(&t, rollout.id), "proposed");
}

/// R22: an unknown card is `E_NOTFOUND` for the master and the same
/// `E_FORBIDDEN` as an ungranted one for a client, granted or not, so a
/// refusal tells a client nothing. A card that names no catalog (hide only)
/// needs the personal grant. A missing id or positions is `E_INVALID`.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn unknown_cards_hide_cards_and_missing_arguments() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, _plain, _acme, card) = seeded();
    let hide = s
        .insert_changeset(
            "new",
            "Hide skill/x on h",
            &[item("hide", None, "skill", "hide", r#"{"hash":"x"}"#)],
        )
        .unwrap();
    let t = tools(s);
    let r = call(&t, &Caller::master(), "dismiss", Some(9_999), None).await;
    assert_eq!(code_of(&r), "E_NOTFOUND");
    let r = call(&t, &Caller::master(), "list", Some(9_999), None).await;
    assert_eq!(code_of(&r), "E_NOTFOUND");
    for c in [ops, desk] {
        let r = call(
            &t,
            &client(c, TokenMode::Full, None),
            "undo",
            Some(9_999),
            None,
        )
        .await;
        assert_eq!(code_of(&r), "E_FORBIDDEN", "{:?}", r.err());
    }

    let r = call(
        &t,
        &client(ops, TokenMode::Full, None),
        "dismiss",
        Some(hide.id),
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("catalog personal"));
    let r = call(
        &t,
        &client(desk, TokenMode::Full, None),
        "dismiss",
        Some(hide.id),
        None,
    )
    .await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(state_of(&t, hide.id), "dismissed");

    let ops = client(ops, TokenMode::Full, None);
    assert_eq!(
        code_of(&call(&t, &ops, "apply", None, None).await),
        "E_INVALID"
    );
    assert_eq!(
        code_of(&call(&t, &ops, "reject_item", Some(card), None).await),
        "E_INVALID"
    );
    assert_eq!(state_of(&t, card), "proposed");
}
