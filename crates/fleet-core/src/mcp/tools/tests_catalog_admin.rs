//! `catalog_admin` driven the way `call_tool` drives it: the mode and admin
//! gates, then the tool. The store predicate it reads
//! (`Store::client_is_assets_admin`) has its own tests in `store/clients.rs`;
//! these pin who the TOOL answers, the nested `apply_sync` confirm gate, and
//! that the grant is read live.

use super::*;
use crate::service::catalog::admin::AdminCall;
use serde_json::{json, Value};

pub(super) fn tools(s: Store) -> FleetTools {
    tools_notifying(s, Arc::new(|_: &guard::ConfirmRequest| {}))
}

/// [`tools`] whose confirm requests go to `notify` (a recording closure).
pub(super) fn tools_notifying(s: Store, notify: guard::ConfirmNotify) -> FleetTools {
    FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(notify),
    )
}

pub(super) fn client(id: i64, mode: TokenMode, org_id: Option<i64>) -> Caller {
    Caller {
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id,
            name: format!("client-{id}"),
            trusted: false,
            org_id,
            person_id: None,
        }),
        mode,
        pane: None,
        is_personal_owner: false,
    }
}

pub(super) fn host(alias: &str) -> Caller {
    Caller {
        host_alias: Some(alias.into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
}

/// One `catalog_admin` call through the gates `call_tool` runs first. `Ok`
/// is the result; `Err` is the error message, which starts with its code.
async fn call_with(
    t: &FleetTools,
    caller: &Caller,
    p: CatalogAdminParams,
) -> Result<CallToolResult, McpError> {
    enforce_mode(caller, "catalog_admin")?;
    enforce_admin(caller, "catalog_admin")?;
    t.catalog_admin(Extension(caller.clone()), Parameters(p))
        .await
}

/// [`call_with`] on the default (personal) catalog.
async fn call(
    t: &FleetTools,
    caller: &Caller,
    action: &str,
    args: Option<Value>,
    confirm_nonce: Option<String>,
) -> Result<CallToolResult, McpError> {
    let p = CatalogAdminParams {
        action: action.into(),
        args,
        confirm_nonce,
        catalog: None,
    };
    call_with(t, caller, p).await
}

/// [`call_with`] with the tool's `catalog` parameter.
async fn call_on(
    t: &FleetTools,
    caller: &Caller,
    action: &str,
    args: Option<Value>,
    catalog: Option<&str>,
) -> Result<CallToolResult, McpError> {
    let p = CatalogAdminParams {
        action: action.into(),
        args,
        confirm_nonce: None,
        catalog: catalog.map(String::from),
    };
    call_with(t, caller, p).await
}

pub(super) fn code_of(r: &Result<CallToolResult, McpError>) -> String {
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
                catalog: None,
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
#[allow(clippy::await_holding_lock)]
async fn catalog_admin_refuses_an_unknown_action_or_malformed_args() {
    // PF5: this reaches `ensure_fresh`, which now touches the process-global
    // registry (an eviction pass) even with nothing configured.
    let _g = crate::service::catalog::lock_registry_for_test();
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
/// configure / load, the catalog-set calls (Assets M3), and `apply_sync`,
/// which applies a plan already computed exactly as the `apply_sync` tool
/// does.
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
        json!({ "action": "list_catalogs" }),
        json!({ "action": "add_catalog", "args": { "name": "a", "repo_path": "/a" } }),
        json!({ "action": "remove_catalog", "args": { "name": "a" } }),
        json!({ "action": "admit_catalog", "args": { "host_alias": "h", "catalog": "a" } }),
        json!({ "action": "unadmit_catalog", "args": { "host_alias": "h", "catalog": "a" } }),
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
#[allow(clippy::await_holding_lock)]
async fn set_host_harnesses_sets_normalises_and_clears() {
    // PF5: the `catalog_admin` call below reaches `ensure_fresh`, which now
    // touches the process-global registry (an eviction pass) even with
    // nothing configured.
    let _g = crate::service::catalog::lock_registry_for_test();
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

/// `personal` and an `acme` org catalog (neither loadable), host `h` with no
/// org; `desk` holds personal, `ops` holds acme, `plain` holds nothing.
/// Returns `(store, desk_id, ops_id, plain_id, acme_id)`.
pub(super) fn two_catalog_store() -> (Store, i64, i64, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.set_catalog_config("/nonexistent/m3-personal", None)
        .unwrap();
    let org = s.add_org("acme", None, false).unwrap();
    let acme = s
        .upsert_catalog("acme", "/nonexistent/m3-acme", None, Some(org.id))
        .unwrap();
    let desk = s.insert_client_token("desk", "aa11", "full").unwrap();
    s.set_client_assets_admin("desk", true).unwrap();
    let ops = s.insert_client_token("ops", "bb22", "full").unwrap();
    s.set_client_catalog_grant("ops", acme.id, true).unwrap();
    let plain = s.insert_client_token("plain", "cc33", "full").unwrap();
    (s, desk.id, ops.id, plain.id, acme.id)
}

/// The message of an `Err`, for asserting what a refusal says.
pub(super) fn message_of(r: Result<CallToolResult, McpError>) -> String {
    r.err().map(|e| e.message.to_string()).unwrap_or_default()
}

/// Spec, Testing (authorization): a client without a grant on the catalog
/// an action touches cannot admit, unadmit or read it (nor remove it, which
/// even a grant does not allow: final review M-c); the grant on
/// one catalog says nothing about another; a personal-only or fleet-wide
/// action refuses another catalog instead of running on personal.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn each_action_needs_a_grant_on_the_catalog_it_touches() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, ops, _plain, _acme) = two_catalog_store();
    let t = tools(s);
    let desk = client(desk, TokenMode::Full, None);
    let ops = client(ops, TokenMode::Full, None);
    let admit = json!({ "host_alias": "h", "catalog": "acme" });

    let r = call(&t, &desk, "admit_catalog", Some(admit.clone()), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("--catalog acme"), "names the remedy");
    let r = call(&t, &ops, "admit_catalog", Some(admit.clone()), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(
        t.store.lock().unwrap().host_admissions("h").unwrap().len(),
        1
    );
    let r = call(&t, &Caller::master(), "unadmit_catalog", Some(admit), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert!(t
        .store
        .lock()
        .unwrap()
        .host_admissions("h")
        .unwrap()
        .is_empty());

    assert_eq!(
        code_of(&call_on(&t, &ops, "config", None, Some("acme")).await),
        "OK"
    );
    assert_eq!(
        code_of(&call_on(&t, &ops, "config", None, None).await),
        "E_FORBIDDEN"
    );
    assert_eq!(
        code_of(&call_on(&t, &desk, "config", None, Some("acme")).await),
        "E_FORBIDDEN"
    );
    assert_eq!(
        code_of(&call_on(&t, &desk, "config", None, None).await),
        "OK",
        "parity"
    );

    // R21: authoring is per catalog — the grant decides, not the action
    // (a granted client reaching acme's checkout:
    // `a_granted_client_authors_in_the_named_catalogs_checkout`).
    let r = call_on(&t, &desk, "repo_status", None, Some("acme")).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("--catalog acme"), "names the remedy");

    // PF16: the refusal is the catalog parameter's, not a parse failure.
    let m = Caller::master();
    let r = call_on(&t, &m, "plan_sync", Some(json!({})), Some("acme")).await;
    assert_eq!(code_of(&r), "E_INVALID");
    assert!(message_of(r).contains("not per catalog"), "fleet-wide");

    // Final review M-c: `remove_catalog` is the master's alone, like
    // `add_catalog` — it cascades every other client's grant on the
    // catalog, and only the master could add it back. A grant on acme is
    // not enough.
    let rm = json!({ "name": "acme" });
    assert_eq!(
        code_of(&call(&t, &desk, "remove_catalog", Some(rm.clone()), None).await),
        "E_FORBIDDEN"
    );
    let r = call(&t, &ops, "remove_catalog", Some(rm.clone()), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "{:?}", r.err());
    assert!(message_of(r).contains("remove_catalog needs the master token"));
    assert!(t
        .store
        .lock()
        .unwrap()
        .get_catalog_by_name("acme")
        .unwrap()
        .is_some());
    let r = call(&t, &m, "remove_catalog", Some(rm), None).await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
}

/// R22 (M-d): the master naming a catalog that does not exist is told so at
/// the gate — the call never falls through to personal.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_master_naming_an_unknown_catalog_gets_not_found() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, ..) = two_catalog_store();
    let t = tools(s);
    let r = call_on(&t, &Caller::master(), "repo_status", None, Some("ghost")).await;
    assert_eq!(code_of(&r), "E_NOTFOUND");
    assert!(message_of(r).contains("no catalog named ghost"));
}

/// A git checkout holding `catalog.yaml` and a file naming `tag`, so two of
/// them never share a HEAD.
fn git_catalog(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("fleet-mcp-catalog-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
    std::fs::write(root.join("TAG"), tag).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["add", "."],
        &["commit", "-q", "-m", "init"],
    ] {
        let o = crate::proc::std_command("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    root
}

fn git_head(root: &std::path::Path) -> String {
    let o = crate::proc::std_command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn json_of(r: Result<CallToolResult, McpError>) -> Value {
    let r = r.unwrap_or_else(|e| panic!("{}", e.message));
    let text = r.content[0].as_text().expect("text content").text.clone();
    serde_json::from_str(&text).expect("tool result is JSON")
}

/// R21 (carry 1): a client granted only acme authors in acme's checkout —
/// what it reads and writes is acme's, personal's HEAD never moves — and
/// the same client still cannot touch personal.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_granted_client_authors_in_the_named_catalogs_checkout() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let personal = git_catalog("m4-personal");
    let acme_root = git_catalog("m4-acme");
    let s = Store::open_in_memory().unwrap();
    s.set_catalog_config(&personal.to_string_lossy(), None)
        .unwrap();
    let org = s.add_org("acme", None, false).unwrap();
    let acme = s
        .upsert_catalog("acme", &acme_root.to_string_lossy(), None, Some(org.id))
        .unwrap();
    let ops = s.insert_client_token("ops", "bb22", "full").unwrap();
    s.set_client_catalog_grant("ops", acme.id, true).unwrap();
    let t = tools(s);
    let ops = client(ops.id, TokenMode::Full, None);
    let personal_head = git_head(&personal);
    let acme_head = git_head(&acme_root);
    assert_ne!(personal_head, acme_head);

    let status = json_of(call_on(&t, &ops, "repo_status", None, Some("acme")).await);
    assert_eq!(
        status["head"],
        acme_head.as_str(),
        "acme's checkout answered"
    );

    let create = json!({ "kind": "skill", "name": "ops" });
    let made = json_of(call_on(&t, &ops, "create_asset", Some(create), Some("acme")).await);
    assert_eq!(made["commit"], git_head(&acme_root).as_str());
    assert!(acme_root.join("skills/ops/asset.yaml").is_file());
    assert!(!personal.join("skills/ops").exists());
    assert_eq!(git_head(&personal), personal_head, "personal untouched");
    let got = json_of(
        call_on(
            &t,
            &ops,
            "get_asset",
            Some(json!({ "kind": "skill", "name": "ops" })),
            Some("acme"),
        )
        .await,
    );
    assert_eq!(got["asset"]["name"], "ops", "{got}");

    let r = call_on(&t, &ops, "repo_status", None, None).await;
    assert_eq!(
        code_of(&r),
        "E_FORBIDDEN",
        "the acme grant is not personal's"
    );
}

/// R11: `list_catalogs` needs no grant (but a per-host token never reaches
/// `catalog_admin`, PF6); `add_catalog` is the master's alone.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn list_catalogs_needs_no_grant_and_add_catalog_needs_the_master() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, plain, _acme) = two_catalog_store();
    let t = tools(s);
    let r = call(
        &t,
        &client(plain, TokenMode::Full, None),
        "list_catalogs",
        None,
        None,
    )
    .await;
    assert_eq!(code_of(&r), "OK", "{:?}", r.err());
    assert_eq!(
        code_of(&call(&t, &host("h1"), "list_catalogs", None, None).await),
        "E_FORBIDDEN"
    );

    let add = json!({ "name": "beta", "repo_path": "/nonexistent/m3-beta", "org": "acme" });
    let r = call(
        &t,
        &client(desk, TokenMode::Full, None),
        "add_catalog",
        Some(add.clone()),
        None,
    )
    .await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("master"));
    let r = call(&t, &Caller::master(), "add_catalog", Some(add), None).await;
    assert_eq!(
        code_of(&r),
        codes::E_CATALOG_GIT,
        "past the gate: {:?}",
        r.err()
    );
}

fn host_plan_writing(catalog: &str) -> crate::service::catalog::sync::plan::HostPlan {
    use crate::service::catalog::sync::plan::{Action, ActionOp, HostPlan};
    HostPlan {
        host_alias: "h".into(),
        harness: "claude".into(),
        status: "planned".into(),
        detail: None,
        actions: vec![Action {
            kind: "skill".into(),
            name: "c".into(),
            op: ActionOp::Create,
            catalog: Some(catalog.into()),
            reason: None,
            files: Vec::new(),
            merges: Vec::new(),
            backup: false,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: None,
            expected: Default::default(),
            secret_files: Default::default(),
            remove_entry: None,
            plugin: None,
        }],
        snapshot: Default::default(),
        manifest: Default::default(),
    }
}

/// Final review M-f: a plan that writes from a catalog removed since is
/// refused as stale — "re-plan" — not with a grant no one could make.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn apply_sync_from_a_catalog_removed_since_the_plan_says_re_plan() {
    use crate::service::catalog::sync::plan;
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, _plain, _acme) = two_catalog_store();
    s.remove_catalog("acme").unwrap();
    let t = tools(s);
    let id = plan::registry_put(plan::SyncPlan::new(vec![host_plan_writing("acme")]));
    let args = json!({ "plan_id": id, "force_partial": false });
    let r = call(
        &t,
        &client(desk, TokenMode::Full, None),
        "apply_sync",
        Some(args),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        r.message.starts_with(codes::E_SYNC_PLAN_STALE),
        "{}",
        r.message
    );
    assert!(
        r.message.contains("catalog acme no longer exists"),
        "{}",
        r.message
    );
    assert!(!r.message.contains("client grant"), "{}", r.message);
    assert!(plan::registry_take(&id).is_some(), "the gate only peeks");
}

/// R11: `apply_sync` needs a grant on every catalog its plan writes from.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn apply_sync_needs_a_grant_on_every_catalog_its_plan_writes() {
    use crate::service::catalog::sync::plan;
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, desk, _ops, _plain, acme) = two_catalog_store();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true")
        .unwrap();
    let t = tools(s);
    let id = plan::registry_put(plan::SyncPlan::new(vec![host_plan_writing("acme")]));
    let desk_c = client(desk, TokenMode::Full, None);
    let args = json!({ "plan_id": id, "force_partial": false });

    let r = call(&t, &desk_c, "apply_sync", Some(args.clone()), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN");
    assert!(message_of(r).contains("acme"));
    t.store
        .lock()
        .unwrap()
        .set_client_catalog_grant("desk", acme, true)
        .unwrap();
    let r = call(&t, &desk_c, "apply_sync", Some(args), None)
        .await
        .unwrap_err();
    assert!(
        r.message.starts_with(codes::E_CONFIRM_REQUIRED),
        "past the grant: {}",
        r.message
    );
    assert!(plan::registry_take(&id).is_some(), "the gate only peeks");
}

/// Fix round 1: an org-bound client is refused everything here, its grants
/// notwithstanding (R3) — `list_catalogs` included, which shows every org's
/// paths, remotes and grantees.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn an_org_bound_client_is_refused_every_catalog_action() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, _ops, _plain, acme) = two_catalog_store();
    let bound = s.insert_client_token("bound", "dd44", "full").unwrap();
    s.set_client_assets_admin("bound", true).unwrap();
    s.set_client_catalog_grant("bound", acme, true).unwrap();
    let org = s.get_catalog_by_name("acme").unwrap().unwrap().org_id;
    s.set_client_org("bound", org).unwrap();
    let t = tools(s);
    let bound = client(bound.id, TokenMode::Full, org);

    let admit = json!({ "host_alias": "h", "catalog": "acme" });
    let rm = json!({ "name": "acme" });
    for (action, args, catalog) in [
        ("admit_catalog", Some(admit), None),
        ("remove_catalog", Some(rm), None),
        ("config", None, None),
        ("config", None, Some("acme")),
        ("list_catalogs", None, None),
    ] {
        let r = call_on(&t, &bound, action, args, catalog).await;
        assert_eq!(
            code_of(&r),
            "E_FORBIDDEN",
            "{action} {catalog:?}: {:?}",
            r.err()
        );
    }
    assert!(t
        .store
        .lock()
        .unwrap()
        .host_admissions("h")
        .unwrap()
        .is_empty());
}

/// Fix round 1: the fleet-wide actions need the personal grant; a client
/// granted only `acme` can neither plan nor apply, even a plan that writes
/// acme alone.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn an_acme_only_client_cannot_plan_or_apply() {
    use crate::service::catalog::sync::plan;
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, ops, _plain, _acme) = two_catalog_store();
    let t = tools(s);
    let ops = client(ops, TokenMode::Full, None);
    let r = call(&t, &ops, "plan_sync", Some(json!({})), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "{:?}", r.err());
    let id = plan::registry_put(plan::SyncPlan::new(vec![host_plan_writing("acme")]));
    let args = json!({ "plan_id": id, "force_partial": false });
    let r = call(&t, &ops, "apply_sync", Some(args), None).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "{:?}", r.err());
    assert!(plan::registry_take(&id).is_some(), "refused, still parked");
}

/// Fix round 1: a client cannot tell an unknown catalog from one it is not
/// granted — the refusal is the same but for the name it was given. R22:
/// the authoring actions too, now that they take a catalog (M4).
#[tokio::test]
async fn an_unknown_catalog_reads_like_an_ungranted_one() {
    let (s, desk, _ops, _plain, _acme) = two_catalog_store();
    let t = tools(s);
    let desk = client(desk, TokenMode::Full, None);
    for action in ["config", "repo_status"] {
        let known = call_on(&t, &desk, action, None, Some("acme")).await;
        let unknown = call_on(&t, &desk, action, None, Some("nope")).await;
        assert_eq!(code_of(&known), "E_FORBIDDEN", "{action}");
        assert_eq!(code_of(&unknown), "E_FORBIDDEN", "{action}");
        assert_eq!(
            message_of(unknown),
            message_of(known).replace("acme", "nope"),
            "{action}"
        );
    }
}

/// Fix round 1: the catalog-set actions refuse a `catalog` parameter they
/// cannot honour, for the master too.
#[tokio::test]
async fn catalog_set_actions_refuse_a_parameter_they_cannot_honour() {
    let (s, _desk, _ops, _plain, _acme) = two_catalog_store();
    let t = tools(s);
    let m = Caller::master();
    let r = call_on(&t, &m, "list_catalogs", None, Some("acme")).await;
    assert_eq!(code_of(&r), "E_INVALID");
    assert!(message_of(r).contains("takes no catalog parameter"));
    let rm = json!({ "name": "acme" });
    let r = call_on(&t, &m, "remove_catalog", Some(rm), Some("personal")).await;
    assert_eq!(code_of(&r), "E_INVALID");
    assert!(message_of(r).contains("drop the catalog parameter"));
    assert!(t
        .store
        .lock()
        .unwrap()
        .get_catalog_by_name("acme")
        .unwrap()
        .is_some());
}

/// Fix round 1: the `apply_sync` gate fails closed for a client — a plan it
/// cannot see (unknown, expired, or taken by an apply in flight) is stale,
/// answered before the confirm gate and before anything runs.
#[tokio::test]
async fn apply_sync_with_an_unknown_plan_is_refused_before_run_for_a_client() {
    let (s, desk, _ops, _plain, _acme) = two_catalog_store();
    s.set_setting(guard::SETTING_CONFIRM_DESTRUCTIVE, "true")
        .unwrap();
    let t = tools(s);
    let args = json!({ "plan_id": "no-such-plan", "force_partial": false });
    let r = call(
        &t,
        &client(desk, TokenMode::Full, None),
        "apply_sync",
        Some(args.clone()),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        r.message.starts_with(codes::E_SYNC_PLAN_STALE),
        "{}",
        r.message
    );
    // The master keeps today's path: the confirm gate, then the sync.
    let r = call(&t, &Caller::master(), "apply_sync", Some(args), None)
        .await
        .unwrap_err();
    assert!(
        r.message.starts_with(codes::E_CONFIRM_REQUIRED),
        "{}",
        r.message
    );
}

/// Final review M2: `import_host` into an org catalog reads a host's whole
/// Claude config. A client granted only that catalog may import from a
/// host bound to its org or admitted to the catalog; any other source host
/// also needs the personal grant (the master is never refused).
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn importing_an_outside_host_into_an_org_catalog_needs_the_personal_grant_too() {
    let _g = crate::service::catalog::lock_registry_for_test();
    let (s, _desk, ops, _plain, acme) = two_catalog_store();
    let both = s.insert_client_token("both", "dd44", "full").unwrap();
    s.set_client_assets_admin("both", true).unwrap();
    s.set_client_catalog_grant("both", acme, true).unwrap();
    let t = tools(s);
    let args = json!({ "host_alias": "h", "dry_run": true });
    let ops = client(ops, TokenMode::Full, None);

    let r = call_on(&t, &ops, "import_host", Some(args.clone()), Some("acme")).await;
    assert_eq!(code_of(&r), "E_FORBIDDEN", "h is outside acme's org");
    assert!(message_of(r).contains("catalog personal"));
    for caller in [client(both.id, TokenMode::Full, None), Caller::master()] {
        let r = call_on(&t, &caller, "import_host", Some(args.clone()), Some("acme")).await;
        assert_ne!(code_of(&r), "E_FORBIDDEN", "{:?}", r.err());
    }

    t.store
        .lock()
        .unwrap()
        .admit_host_catalog("h", acme)
        .unwrap();
    let r = call_on(&t, &ops, "import_host", Some(args), Some("acme")).await;
    assert_ne!(
        code_of(&r),
        "E_FORBIDDEN",
        "an admitted host: {:?}",
        r.err()
    );
}
