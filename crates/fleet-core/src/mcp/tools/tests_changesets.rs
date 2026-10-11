//! `changesets` driven the way `call_tool` drives it — spec, Testing
//! (authorization): "an ungranted client and a per-host token cannot apply,
//! undo or admit in a catalog they have no grant for". A per-host token
//! never reaches the tool at all (R25 amended: it is `NOT_FOR_HOST_TOKENS`;
//! a host's "can list" is `list_assets` and the inventory). `list` is the
//! master's or an unbound full client's, as M3's `list_catalogs` (PF15).

use super::tests_catalog_admin::{
    client, code_of, host, json_of, message_of, tools, tools_notifying, two_catalog_store,
};
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
            change: None,
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
    }
    // desk holds a grant (personal), so it is told which one it lacks;
    // plain holds none and is refused before any card is read.
    let r = call(
        &t,
        &client(desk, TokenMode::Full, None),
        "reject_item",
        Some(card),
        Some(vec![0]),
    )
    .await;
    assert!(message_of(r).contains("--catalog acme"), "names the remedy");
    let r = call(
        &t,
        &client(plain, TokenMode::Full, None),
        "reject_item",
        Some(card),
        Some(vec![0]),
    )
    .await;
    assert!(
        !message_of(r).contains("acme"),
        "a grantless client learns no catalog"
    );
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

/// Fix round 1 (Critical): `writes_hosts` and the catalogs checked come from
/// exactly the items `apply` runs. A drift card whose take_host (acme's
/// alone) was rejected applies its restore when no positions are named — a
/// host write — so it needs personal and the confirm gate, whose summary
/// names the card and the restore, and an approved retry gets through.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_drift_restore_applied_without_positions_needs_personal_and_confirmation() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, ops, _plain, acme, _card) = seeded();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true")
        .unwrap();
    let drift = s
        .insert_changeset(
            "drift",
            "skill/w differs on oci",
            &[
                item(
                    "drift",
                    Some(acme),
                    "skill",
                    "take_host",
                    r#"{"host":"oci","hash":"h2"}"#,
                ),
                item(
                    "drift",
                    Some(acme),
                    "skill",
                    "restore",
                    r#"{"host":"oci","hash":"h2"}"#,
                ),
            ],
        )
        .unwrap();
    let asked_for: Arc<Mutex<Vec<String>>> = Arc::default();
    let rec = asked_for.clone();
    let t = tools_notifying(
        s,
        Arc::new(move |r: &guard::ConfirmRequest| rec.lock().unwrap().push(r.summary.clone())),
    );
    let ops = client(ops, TokenMode::Full, None);
    let r = call(&t, &ops, "reject_item", Some(drift.id), Some(vec![0])).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(
        state_of(&t, drift.id),
        "proposed",
        "the restore is still pending"
    );

    let r = call(&t, &ops, "apply", Some(drift.id), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "the lone restore writes a host");
    assert!(message_of(r).contains("catalog personal"));

    let m = Caller::master();
    let asked = call(&t, &m, "apply", Some(drift.id), None)
        .await
        .unwrap_err();
    assert!(
        asked.message.starts_with(codes::E_CONFIRM_REQUIRED),
        "{}",
        asked.message
    );
    let nonce = asked.data.as_ref().unwrap()["details"]["confirm_nonce"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        *asked_for.lock().unwrap(),
        vec![format!(
            "changeset {} (drift): restore #1 skill/w on oci [h2]",
            drift.id
        )],
        "the approval names what runs"
    );
    assert!(t.guards.confirms.resolve(&nonce, true));
    let after = t
        .changesets(
            Extension(m.clone()),
            Parameters(ChangesetsParams {
                action: "apply".into(),
                id: Some(drift.id),
                positions: None,
                confirm_nonce: Some(nonce),
                change: None,
            }),
        )
        .await
        .unwrap_err();
    assert!(
        !after.message.starts_with(codes::E_CONFIRM_REQUIRED)
            && !after.message.starts_with("E_FORBIDDEN"),
        "past the gate, into the apply: {}",
        after.message
    );
}

/// Fix round 1: an org-bound client is refused before any card is read —
/// the same refusal, word for word, for an id that exists and one that
/// does not, on every action.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn an_org_bound_client_learns_nothing_about_cards() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, _ops, _plain, acme, card) = seeded();
    let org_id = s.get_catalog(acme).unwrap().unwrap().org_id;
    // Granted acme while unbound, then bound: the grant row stays.
    let bound = s.insert_client_token("bound", "ee55", "full").unwrap();
    s.set_client_catalog_grant("bound", acme, true).unwrap();
    s.set_client_org("bound", org_id).unwrap();
    let t = tools(s);
    let c = client(bound.id, TokenMode::Full, org_id);
    for action in ["apply", "dismiss", "undo", "list"] {
        let known = message_of(call(&t, &c, action, Some(card), None).await);
        let unknown = message_of(call(&t, &c, action, Some(9_999), None).await);
        assert!(known.starts_with("E_FORBIDDEN"), "{action}: {known}");
        assert_eq!(known, unknown, "{action}");
        assert!(!known.contains("acme"), "{action}: {known}");
    }
    assert_eq!(state_of(&t, card), "proposed");
}

/// Fix round 1, R22: for an unbound client the code is the same for a card
/// that does not exist and one it holds no grant on. What still differs,
/// by design: a client holding some grant is told which catalog an
/// existing card needs (`--catalog acme`), and a client holding none is
/// told nothing for either.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn an_unbound_clients_refusals_share_a_code_for_unknown_and_ungranted_cards() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, plain, _acme, card) = seeded();
    let t = tools(s);
    let desk = client(desk, TokenMode::Full, None);
    let known = call(&t, &desk, "apply", Some(card), None).await;
    let unknown = call(&t, &desk, "apply", Some(9_999), None).await;
    assert_eq!(code_of(&known), "E_FORBIDDEN");
    assert_eq!(code_of(&unknown), "E_FORBIDDEN");
    assert!(message_of(known).contains("--catalog acme"));
    assert!(!message_of(unknown).contains("9999"));

    let plain = client(plain, TokenMode::Full, None);
    let known = message_of(call(&t, &plain, "apply", Some(card), None).await);
    let unknown = message_of(call(&t, &plain, "apply", Some(9_999), None).await);
    assert!(known.starts_with("E_FORBIDDEN"), "{known}");
    assert_eq!(
        known, unknown,
        "a grantless client is refused before the card"
    );
}

/// One `changesets { propose_layer }` call through the gates `call_tool`
/// runs first.
async fn propose_layer(
    t: &FleetTools,
    caller: &Caller,
    change: Option<crate::service::catalog::changesets::LayerChange>,
) -> Result<CallToolResult, McpError> {
    enforce_mode(caller, "changesets")?;
    enforce_admin(caller, "changesets")?;
    t.changesets(
        Extension(caller.clone()),
        Parameters(ChangesetsParams {
            action: "propose_layer".into(),
            id: None,
            positions: None,
            confirm_nonce: None,
            change,
        }),
    )
    .await
}

/// A create of layer `servers` in `catalog` (`None`: personal).
fn create_servers(catalog: Option<&str>) -> crate::service::catalog::changesets::LayerChange {
    crate::service::catalog::changesets::LayerChange::Create {
        catalog: catalog.map(String::from),
        layer: "servers".into(),
        axis: None,
        description: None,
        members: vec![],
        orgs: vec![],
    }
}

/// [`two_catalog_store`] with personal loaded (no layers), so a layer
/// change can be validated against it.
fn with_personal_loaded(s: &Store) {
    let p = s.personal_catalog().unwrap().unwrap().id;
    crate::service::catalog::registry::install_personal(crate::service::catalog::repo::Catalog {
        id: p,
        name: "personal".into(),
        ..Default::default()
    })
    .unwrap();
}

fn layer_cards(t: &FleetTools) -> usize {
    t.store
        .lock()
        .unwrap()
        .list_changesets()
        .unwrap()
        .iter()
        .filter(|c| c.kind == "layer")
        .count()
}

/// Assets M6 (R5): the master, and a client granted the catalog, propose a
/// layer card.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn propose_layer_makes_a_card_for_a_granted_caller() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, _plain, _acme) = two_catalog_store();
    with_personal_loaded(&s);
    let t = tools(s);
    let r = propose_layer(&t, &Caller::master(), Some(create_servers(None))).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    let card = json_of(r);
    assert_eq!(card["kind"], "layer");
    assert_eq!(card["summary"], "New layer servers in personal");
    assert_eq!(card["items"][0]["decider"], "person");
    // desk holds the personal grant. The first card is only proposed, so
    // the catalog has no `servers` yet and the same change is still valid.
    let r = propose_layer(
        &t,
        &client(desk, TokenMode::Full, None),
        Some(create_servers(Some("personal"))),
    )
    .await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(layer_cards(&t), 2);
}

/// R25: a layer change needs a grant on the catalog it names — the grant on
/// another catalog says nothing about it — and nothing is recorded.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn propose_layer_in_a_catalog_the_client_has_no_grant_for_is_forbidden() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, plain, _acme) = two_catalog_store();
    with_personal_loaded(&s);
    let t = tools(s);
    let r = propose_layer(
        &t,
        &client(desk, TokenMode::Full, None),
        Some(create_servers(Some("acme"))),
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("--catalog acme"), "names the remedy");
    let r = propose_layer(
        &t,
        &client(ops, TokenMode::Full, None),
        Some(create_servers(None)),
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("catalog personal"));
    let r = propose_layer(
        &t,
        &client(plain, TokenMode::Full, None),
        Some(create_servers(Some("nosuch"))),
    )
    .await;
    assert_eq!(
        code_of(&r),
        "E_FORBIDDEN",
        "an unknown catalog reads as ungranted"
    );
    let r = propose_layer(&t, &host("h1"), Some(create_servers(None))).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "never a per-host token's");
    assert_eq!(layer_cards(&t), 0, "no card was inserted");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn propose_layer_without_a_change_is_invalid() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, _ops, _plain, _acme) = two_catalog_store();
    let t = tools(s);
    let r = propose_layer(&t, &Caller::master(), None).await;
    assert_eq!(code_of(&r), "E_INVALID");
    assert!(message_of(r).contains("propose_layer needs a change"));
    assert_eq!(layer_cards(&t), 0);
}
