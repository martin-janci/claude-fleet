//! `catalog_admin` driven the way `call_tool` drives it: the mode and admin
//! gates, then the tool. The store predicate it reads
//! (`Store::client_is_assets_admin`) has its own tests in `store/clients.rs`;
//! these pin who the TOOL answers, the nested `apply_sync` confirm gate, and
//! that the grant is read live.

use super::*;
use crate::service::catalog::admin::AdminCall;
use serde_json::{json, Value};

fn tools(s: Store) -> FleetTools {
    FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    )
}

fn client(id: i64, mode: TokenMode, org_id: Option<i64>) -> Caller {
    Caller {
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id,
            name: format!("client-{id}"),
            trusted: false,
            org_id,
        }),
        mode,
    }
}

fn host(alias: &str) -> Caller {
    Caller {
        host_alias: Some(alias.into()),
        client: None,
        mode: TokenMode::Full,
    }
}

/// One `catalog_admin` call through the gates `call_tool` runs first. `Ok`
/// is the result; `Err` is the error message, which starts with its code.
async fn call(
    t: &FleetTools,
    caller: &Caller,
    action: &str,
    args: Option<Value>,
    confirm_nonce: Option<String>,
) -> Result<CallToolResult, McpError> {
    enforce_mode(caller, "catalog_admin")?;
    enforce_admin(caller, "catalog_admin")?;
    t.catalog_admin(
        Extension(caller.clone()),
        Parameters(CatalogAdminParams {
            action: action.into(),
            args,
            confirm_nonce,
        }),
    )
    .await
}

fn code_of(r: &Result<CallToolResult, McpError>) -> String {
    match r {
        Ok(_) => "OK".into(),
        Err(e) => e.message.split(':').next().unwrap_or_default().to_string(),
    }
}

#[tokio::test]
async fn catalog_admin_answers_the_master_and_a_granted_full_unbound_client_only() {
    let s = Store::open_in_memory().unwrap();
    let desk = s.insert_client_token("desk", "aa11", "full").unwrap();
    s.set_client_assets_admin("desk", true).unwrap();
    let plain = s.insert_client_token("plain", "bb22", "full").unwrap();
    let kiosk = s.insert_client_token("kiosk", "cc33", "readonly").unwrap();
    // Granted while unbound, then bound to an org: the column stays set,
    // and the live check still says no.
    let org = s.add_org("A", None, false).unwrap();
    let contractor = s.insert_client_token("contractor", "dd44", "full").unwrap();
    s.set_client_assets_admin("contractor", true).unwrap();
    s.set_client_org("contractor", Some(org.id)).unwrap();
    let t = tools(s);

    let cases: Vec<(&str, Caller, &str)> = vec![
        ("master", Caller::master(), "OK"),
        (
            "granted full client",
            client(desk.id, TokenMode::Full, None),
            "OK",
        ),
        (
            "un-granted client",
            client(plain.id, TokenMode::Full, None),
            "E_FORBIDDEN",
        ),
        (
            "readonly client",
            client(kiosk.id, TokenMode::Readonly, None),
            "E_FORBIDDEN",
        ),
        (
            "org-bound client",
            client(contractor.id, TokenMode::Full, Some(org.id)),
            "E_FORBIDDEN",
        ),
        ("per-host token", host("h1"), "E_FORBIDDEN"),
    ];
    for (who, caller, want) in &cases {
        let r = call(&t, caller, "config", None, None).await;
        assert_eq!(code_of(&r), *want, "{who}: {:?}", r.err());
    }

    // The readonly client is refused by the tool itself too, not only by
    // the mode gate in front of it: its row is not `full`.
    let direct = t
        .catalog_admin(
            Extension(client(kiosk.id, TokenMode::Readonly, None)),
            Parameters(CatalogAdminParams {
                action: "config".into(),
                args: None,
                confirm_nonce: None,
            }),
        )
        .await;
    assert_eq!(code_of(&direct), "E_FORBIDDEN");

    // The grant is read on every call: taken back, the next call is refused.
    let granted = client(desk.id, TokenMode::Full, None);
    assert_eq!(
        code_of(&call(&t, &granted, "config", None, None).await),
        "OK"
    );
    t.store
        .lock()
        .unwrap()
        .set_client_assets_admin("desk", false)
        .unwrap();
    let after = call(&t, &granted, "config", None, None).await;
    assert_eq!(code_of(&after), "E_FORBIDDEN");
    assert!(
        after
            .unwrap_err()
            .message
            .contains("fleet-hub client grant <name> assets"),
        "the refusal names the operator's remedy"
    );
}

/// Security fix round 1, CRITICAL 1: `import_assets` is `Access::Client`
/// (a per-host token or any paired client can reach it), and with Task 6's
/// remote `host_alias` it makes the hub SSH into another host and write into
/// the catalog. It must answer the master and a granted full unbound client
/// only, exactly like `catalog_admin` — checked both at the central gate
/// (`NOT_FOR_HOST_TOKENS`, for a per-host token) and inside the tool body
/// (`may_admin_catalog`, for an ungranted client, which the central gate
/// cannot see).
#[tokio::test]
async fn import_assets_answers_the_master_and_a_granted_full_unbound_client_only() {
    let s = Store::open_in_memory().unwrap();
    let desk = s.insert_client_token("desk", "aa11", "full").unwrap();
    s.set_client_assets_admin("desk", true).unwrap();
    let plain = s.insert_client_token("plain", "bb22", "full").unwrap();
    let t = tools(s);

    async fn import(t: &FleetTools, caller: &Caller) -> Result<CallToolResult, McpError> {
        enforce_mode(caller, "import_assets")?;
        enforce_admin(caller, "import_assets")?;
        t.import_assets(
            Extension(caller.clone()),
            Parameters(catalog::ImportArgs {
                host_alias: "local".into(),
                dry_run: true,
                only: vec![],
            }),
        )
        .await
    }

    let cases: Vec<(&str, Caller, &str)> = vec![
        // Not `E_FORBIDDEN`: the catalog isn't configured in this fresh
        // store, so a caller the gate lets through hits that instead — the
        // point here is that it is never the grant that stops them.
        ("master", Caller::master(), codes::E_CATALOG_NOT_CONFIGURED),
        (
            "granted full client",
            client(desk.id, TokenMode::Full, None),
            codes::E_CATALOG_NOT_CONFIGURED,
        ),
        (
            "un-granted client",
            client(plain.id, TokenMode::Full, None),
            "E_FORBIDDEN",
        ),
        ("per-host token", host("h1"), "E_FORBIDDEN"),
    ];
    for (who, caller, want) in &cases {
        let r = import(&t, caller).await;
        assert_eq!(code_of(&r), *want, "{who}: {:?}", r.err());
    }
    // The per-host token is refused at the central gate before the tool
    // body ever runs — `import_assets` is not even in its tool list.
    assert!(
        !present::visible_to(&host("h1"), "import_assets"),
        "a per-host token must never see import_assets in its tool list"
    );
    // The un-granted client IS refused by the tool itself, not only by a
    // gate in front of it: calling it directly (skipping `enforce_admin`,
    // which lets any full client through) still comes back E_FORBIDDEN.
    let direct = t
        .import_assets(
            Extension(client(plain.id, TokenMode::Full, None)),
            Parameters(catalog::ImportArgs {
                host_alias: "local".into(),
                dry_run: true,
                only: vec![],
            }),
        )
        .await;
    assert_eq!(code_of(&direct), "E_FORBIDDEN");
    assert!(
        direct
            .unwrap_err()
            .message
            .contains("fleet-hub client grant <name> assets"),
        "the refusal names the operator's remedy"
    );

    // The grant is read on every call: taken back, the next call is refused.
    let granted = client(desk.id, TokenMode::Full, None);
    assert_eq!(
        code_of(&import(&t, &granted).await),
        codes::E_CATALOG_NOT_CONFIGURED
    );
    t.store
        .lock()
        .unwrap()
        .set_client_assets_admin("desk", false)
        .unwrap();
    assert_eq!(code_of(&import(&t, &granted).await), "E_FORBIDDEN");
}

#[tokio::test]
async fn catalog_admin_refuses_an_unknown_action_or_malformed_args() {
    let t = tools(Store::open_in_memory().unwrap());
    let m = Caller::master();
    let r = call(&t, &m, "rm_rf", None, None).await;
    assert_eq!(code_of(&r), "E_INVALID");
    let r = call(&t, &m, "get_asset", Some(json!({ "kind": "skill" })), None).await;
    assert_eq!(code_of(&r), "E_INVALID");
    // A hostile name comes back as E_INVALID through the tool too.
    let r = call(
        &t,
        &m,
        "delete_asset",
        Some(json!({ "kind": "skill", "name": "../x" })),
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_INVALID");
}

/// `apply_sync` through `catalog_admin` passes the same confirm gate as the
/// `apply_sync` tool: no nonce → `E_CONFIRM_REQUIRED`; the approved nonce
/// lets the call through to the sync itself (which then finds no such plan).
#[tokio::test]
async fn catalog_admin_apply_sync_is_confirm_gated() {
    let s = Store::open_in_memory().unwrap();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true")
        .unwrap();
    let t = tools(s);
    let m = Caller::master();
    let args = json!({ "plan_id": "no-such-plan", "force_partial": false, "call_id": 42 });

    let asked = call(&t, &m, "apply_sync", Some(args.clone()), None)
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
    assert!(t.guards.confirms.resolve(&nonce, true));
    let after = call(&t, &m, "apply_sync", Some(args), Some(nonce))
        .await
        .unwrap_err();
    assert!(
        after.message.starts_with("E_SYNC_PLAN_STALE"),
        "past the gate, into the sync: {}",
        after.message
    );
}

/// The caller's `call_id` is dropped before the sync runs: it names a
/// cancellation slot in the caller's process, not this one. Checked on the
/// step between parsing and running, since the id leaves no trace in the
/// answer.
#[test]
fn catalog_admin_apply_sync_drops_the_callers_call_id() {
    let t = tools(Store::open_in_memory().unwrap());
    let mut call: AdminCall = serde_json::from_value(json!({
        "action": "apply_sync",
        "args": { "plan_id": "p", "force_partial": true, "call_id": 42 },
    }))
    .unwrap();
    // Confirmations are off here, so the gate lets the master through.
    t.prepare_admin_call(&mut call, None, &Caller::master())
        .unwrap();
    match call {
        AdminCall::ApplySync(a) => {
            assert_eq!(a.call_id, None);
            assert_eq!(a.plan_id, "p");
            assert!(a.force_partial);
        }
        _ => unreachable!(),
    }
}

/// Which calls `prepare_admin_call` refreshes the catalog for. With a
/// configured catalog that cannot load (no checkout, no remote), a call that
/// runs `ensure_fresh` fails there, and one that does not passes: config /
/// configure / load, and `apply_sync`, which applies a plan already computed
/// exactly as the `apply_sync` tool does.
#[test]
fn catalog_admin_refreshes_the_catalog_for_every_call_but_config_load_and_apply_sync() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let s = Store::open_in_memory().unwrap();
    s.set_catalog_config("/nonexistent/claude-fleet-catalog-test", None)
        .unwrap();
    let t = tools(s);
    let prepare = |wire: Value| {
        let mut call: AdminCall = serde_json::from_value(wire).unwrap();
        t.prepare_admin_call(&mut call, None, &Caller::master())
    };

    for wire in [
        json!({ "action": "config" }),
        json!({ "action": "load", "args": {} }),
        json!({
            "action": "apply_sync",
            "args": { "plan_id": "p", "force_partial": false },
        }),
    ] {
        assert!(prepare(wire.clone()).is_ok(), "no refresh for {wire}");
    }
    for wire in [
        json!({ "action": "inventory" }),
        json!({ "action": "list_layers" }),
    ] {
        let err = prepare(wire.clone()).unwrap_err();
        assert!(
            err.message.contains("not a git repository"),
            "{wire} refreshes: {}",
            err.message
        );
    }
}

/// `CatalogAdminParams::action`'s description lists every action the tool
/// takes, so the generated reference names none that is missing.
#[test]
fn the_action_param_names_every_admin_call() {
    let params = include_str!("params.rs");
    let start = params
        .find("pub struct CatalogAdminParams")
        .expect("CatalogAdminParams");
    let doc = &params[start..start + params[start..].find("pub action").unwrap()];
    for action in AdminCall::ACTIONS {
        let listed = doc
            .split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
            .any(|w| w == *action);
        assert!(
            listed,
            "{action} is missing from CatalogAdminParams::action"
        );
    }
}

/// Multi-harness F3a: the tool sets, normalises and clears a host's harness
/// choice; `catalog_admin` reaches the same function (the way a granted
/// desktop does); a per-host token is refused.
#[tokio::test]
async fn set_host_harnesses_sets_normalises_and_clears() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let t = tools(s);
    let harnesses_of = |t: &FleetTools| {
        t.store
            .lock()
            .unwrap()
            .get_host_row("local")
            .unwrap()
            .unwrap()
            .harnesses
    };

    t.set_host_harnesses(Parameters(SetHostHarnessesParams {
        host_alias: "local".into(),
        harnesses: Some(vec!["codex".into(), "claude".into()]),
    }))
    .await
    .unwrap();
    assert_eq!(
        harnesses_of(&t),
        Some(vec!["claude".to_string(), "codex".to_string()])
    );

    let err = t
        .set_host_harnesses(Parameters(SetHostHarnessesParams {
            host_alias: "local".into(),
            harnesses: Some(vec!["codex".into()]),
        }))
        .await
        .unwrap_err();
    assert!(err.message.starts_with("E_INVALID"), "{}", err.message);

    t.set_host_harnesses(Parameters(SetHostHarnessesParams {
        host_alias: "local".into(),
        harnesses: None,
    }))
    .await
    .unwrap();
    assert_eq!(harnesses_of(&t), None);

    call(
        &t,
        &Caller::master(),
        "set_host_harnesses",
        Some(json!({ "host_alias": "local", "harnesses": ["claude"] })),
        None,
    )
    .await
    .unwrap();
    assert_eq!(harnesses_of(&t), Some(vec!["claude".to_string()]));

    let err = enforce_admin(&host("local"), "set_host_harnesses").unwrap_err();
    assert!(err.message.starts_with("E_FORBIDDEN"), "{}", err.message);
}
