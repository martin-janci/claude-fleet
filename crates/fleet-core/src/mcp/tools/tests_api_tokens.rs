//! Named Control API tokens (M15 step G2.8): what each scope reaches, the
//! host limit, expiry, and the `api_tokens` tool's own gates.

use super::*;
use crate::mcp::auth::{resolve_token_at, ApiTokenRef, ClientRef};
use crate::service::view_scope::Visibility;
use crate::store::{ApiScope, ControlTokenRow};

const OWNER: Option<i64> = Some(1);

fn row(name: &str, token: &str, scope: ApiScope, hosts: Option<&[&str]>) -> ControlTokenRow {
    ControlTokenRow {
        id: 3,
        name: name.into(),
        token_sha256: crate::mcp::auth::sha256_hex(token),
        scope,
        hosts: hosts.map(|h| h.iter().map(|s| s.to_string()).collect()),
        expires_at: None,
        created_at: 0,
        last_used_at: None,
        revoked_at: None,
    }
}

fn resolve(token: &str, rows: &[ControlTokenRow], now: i64) -> Option<Caller> {
    resolve_token_at(token, "master-tok", &[], &[], rows, OWNER, now)
}

fn api_caller(scope: ApiScope, hosts: Option<&[&str]>) -> Caller {
    resolve("flt_live_x", &[row("ci", "flt_live_x", scope, hosts)], 0).expect("resolves")
}

#[test]
fn each_scope_resolves_to_its_reach() {
    let read = api_caller(ApiScope::Read, None);
    assert_eq!(read.mode, TokenMode::Readonly);
    assert!(!read.is_master() && read.speaks_for_owner() && read.is_personal_owner);
    assert_eq!(read.label(), "token:ci");
    let act = api_caller(ApiScope::Act, None);
    assert_eq!(act.mode, TokenMode::Full);
    assert!(!act.is_master());
    let admin = api_caller(ApiScope::Admin, None);
    assert!(admin.is_master(), "admin is what the master token is");

    // Read: observes, never mutates.
    assert!(enforce_mode(&read, "list_sessions").is_ok());
    assert!(enforce_mode(&read, "send_prompt").is_err());
    // Act: drives sessions, never fleet admin, settings or token minting.
    for t in ["send_prompt", "kill_session", "new_session"] {
        assert!(
            enforce_mode(&act, t).is_ok() && enforce_admin(&act, t).is_ok(),
            "{t}"
        );
    }
    for t in [
        "provision_hosts",
        "remove_host",
        "set_secret",
        "pair_client",
        "api_tokens",
        "set_setting",
    ] {
        let e = enforce_admin(&act, t).expect_err(t);
        assert!(e.message.starts_with("E_FORBIDDEN"), "{t}: {}", e.message);
    }
    // Admin: everything the master reaches.
    for t in ["provision_hosts", "set_secret", "api_tokens"] {
        assert!(enforce_admin(&admin, t).is_ok(), "{t}");
    }
}

#[test]
fn an_expired_or_unknown_token_is_nobody() {
    let mut r = row("ci", "flt_live_x", ApiScope::Act, None);
    r.expires_at = Some(100);
    assert!(resolve("flt_live_x", std::slice::from_ref(&r), 99).is_some());
    assert!(
        resolve("flt_live_x", std::slice::from_ref(&r), 100).is_none(),
        "expired"
    );
    assert!(resolve("flt_live_y", std::slice::from_ref(&r), 0).is_none());
    r.expires_at = None;
    r.revoked_at = Some(5);
    assert!(resolve("flt_live_x", &[r], 0).is_none(), "revoked");
    // The master still resolves beside the named rows.
    assert!(resolve("master-tok", &[], 0).unwrap().api.is_none());
}

#[test]
fn an_admin_row_never_carries_a_host_limit_into_the_caller() {
    let c = resolve(
        "flt_live_x",
        &[row("root", "flt_live_x", ApiScope::Admin, Some(&["a"]))],
        0,
    )
    .unwrap();
    assert!(c.is_master() && c.api_hosts().is_none());
}

#[test]
fn the_host_limit_holds_for_host_addressed_calls() {
    let limited = api_caller(ApiScope::Act, Some(&["mercury"]));
    assert!(require_host(&limited, "mercury", "x").is_ok());
    let e = require_host(&limited, "venus", "the session").unwrap_err();
    assert!(
        e.message.starts_with("E_FORBIDDEN") && e.message.contains("mercury"),
        "{}",
        e.message
    );
    assert!(require_host(&api_caller(ApiScope::Act, None), "venus", "x").is_ok());
}

#[test]
fn a_named_token_starts_sessions_as_the_owner() {
    let s = Store::open_in_memory().unwrap();
    let owner = s.personal_owner_id().unwrap();
    assert!(owner.is_some());
    assert_eq!(
        super::fleet::owner_for(&api_caller(ApiScope::Act, None), &s),
        owner
    );
}

#[test]
fn the_host_limit_hides_another_hosts_sessions() {
    let s = Store::open_in_memory().unwrap();
    let owner = s.personal_owner_id().unwrap();
    s.upsert_host("mercury").unwrap();
    s.upsert_host("venus").unwrap();
    let on = |h: &str| {
        let id = s
            .upsert_session(&format!("s-{h}"), h, None, None, 0, 0, "running", None)
            .unwrap();
        s.claim_if_unclaimed(id, owner).unwrap();
        s.get_session_by_id(id).unwrap().unwrap()
    };
    let (m, v) = (on("mercury"), on("venus"));
    let everywhere = api_caller(ApiScope::Act, None).view_scope(&s).unwrap();
    assert_eq!(everywhere.sees_session_row(&m), Visibility::RowAndContent);
    assert_eq!(everywhere.sees_session_row(&v), Visibility::RowAndContent);
    let limited = api_caller(ApiScope::Act, Some(&["mercury"]))
        .view_scope(&s)
        .unwrap();
    assert_eq!(limited.sees_session_row(&m), Visibility::RowAndContent);
    assert_eq!(limited.sees_session_row(&v), Visibility::None);
}

fn owners_device(trusted: bool, mode: TokenMode) -> Caller {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "laptop".into(),
            trusted,
            org_id: None,
            person_id: OWNER,
        }),
        mode,
        pane: None,
        is_personal_owner: true,
    }
}

async fn tokens_call(
    t: &FleetTools,
    c: &Caller,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    enforce_mode(c, "api_tokens")
        .and_then(|()| enforce_admin(c, "api_tokens"))
        .map_err(|e| e.message.to_string())?;
    t.api_tokens(
        Extension(c.clone()),
        Parameters(serde_json::from_value(args).unwrap()),
    )
    .await
    .map(|r| super::tests::result_json(&r))
    .map_err(|e| e.message.to_string())
}

#[tokio::test]
async fn the_master_creates_a_token_that_then_authenticates() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("mercury").unwrap();
    let t = super::tests::test_tools(s);
    let v = tokens_call(
        &t,
        &Caller::master(),
        serde_json::json!({"action": "create", "name": "grafana", "scope": "read",
                           "expires_in_days": 90, "hosts": ["mercury"]}),
    )
    .await
    .unwrap();
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("flt_live_"));
    assert_eq!(v["env_line"], format!("FLEET_MCP_TOKEN={token}"));
    assert_eq!(v["scope"], "read");
    assert!(v.get("token_sha256").is_none());

    let rows = t.store.lock().unwrap().auth_control_tokens().unwrap();
    let c = resolve_token_at(
        &token,
        "m",
        &[],
        &[],
        &rows,
        OWNER,
        crate::store::now_unix(),
    )
    .unwrap();
    assert_eq!(
        c.api,
        Some(ApiTokenRef {
            id: rows[0].id,
            name: "grafana".into(),
            scope: ApiScope::Read,
            hosts: Some(vec!["mercury".into()]),
        })
    );

    let listed = tokens_call(&t, &Caller::master(), serde_json::json!({"action": "list"}))
        .await
        .unwrap();
    assert!(
        !listed.to_string().contains(&token),
        "the list never carries the token"
    );
    assert_eq!(listed[0]["name"], "grafana");

    tokens_call(
        &t,
        &Caller::master(),
        serde_json::json!({"action": "revoke", "name": "grafana"}),
    )
    .await
    .unwrap();
    let rows = t.store.lock().unwrap().auth_control_tokens().unwrap();
    assert!(resolve_token_at(&token, "m", &[], &[], &rows, OWNER, 0).is_none());
}

#[tokio::test]
async fn a_device_mints_read_or_act_only_and_only_when_trusted_and_full() {
    let t = super::tests::test_tools(Store::open_in_memory().unwrap());
    let create = |scope: &str, name: &str| serde_json::json!({"action": "create", "name": name, "scope": scope});
    let untrusted = owners_device(false, TokenMode::Full);
    assert!(
        tokens_call(&t, &untrusted, serde_json::json!({"action": "list"}))
            .await
            .is_ok()
    );
    let e = tokens_call(&t, &untrusted, create("act", "a"))
        .await
        .unwrap_err();
    assert!(e.starts_with("E_FORBIDDEN"), "{e}");
    let readonly = owners_device(true, TokenMode::Readonly);
    assert!(tokens_call(&t, &readonly, create("read", "b"))
        .await
        .is_err());
    let trusted = owners_device(true, TokenMode::Full);
    assert!(tokens_call(&t, &trusted, create("act", "c")).await.is_ok());
    let e = tokens_call(&t, &trusted, create("admin", "d"))
        .await
        .unwrap_err();
    assert!(e.starts_with("E_FORBIDDEN"), "{e}");
    // A named act token cannot mint at all: the tool is not served to it.
    let act = api_caller(ApiScope::Act, None);
    assert!(tokens_call(&t, &act, create("read", "e")).await.is_err());
    // A host token never reaches the tool.
    let host = Caller {
        api: None,
        host_alias: Some("mercury".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    };
    assert!(
        tokens_call(&t, &host, serde_json::json!({"action": "list"}))
            .await
            .is_err()
    );
}

#[test]
fn a_host_limited_token_reaches_only_the_session_gated_tools() {
    let limited = api_caller(ApiScope::Act, Some(&["mercury"]));
    for t in crate::mcp::guard::HOST_LIMITED_TOOLS {
        assert!(enforce_admin(&limited, t).is_ok(), "{t}");
    }
    for t in [
        "routines",
        "set_clipboard",
        "get_clipboard",
        "fleet_health",
        "probe_host",
        "delete_worktree",
    ] {
        assert!(
            enforce_admin(&limited, t).is_err(),
            "{t} names a host outside the session gate"
        );
    }
    // Every listed tool is a real one.
    for t in crate::mcp::guard::HOST_LIMITED_TOOLS {
        assert!(crate::mcp::guard::policy(t).is_some(), "{t}");
    }
    // Unlimited, the same token reaches them.
    let act = api_caller(ApiScope::Act, None);
    assert!(enforce_admin(&act, "routines").is_ok());
}
