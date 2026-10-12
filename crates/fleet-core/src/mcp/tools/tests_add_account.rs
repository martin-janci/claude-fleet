//! Add account (M15 step G2.9): who reaches the `add_account` tool. What
//! it does is tested in `service::add_account`'s own tests.

use super::*;
use crate::mcp::auth::{resolve_token_at, ClientRef};
use crate::store::{ApiScope, ControlTokenRow};

fn device(trusted: bool) -> Caller {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "laptop".into(),
            trusted,
            org_id: None,
            person_id: Some(1),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: true,
    }
}

fn named(scope: ApiScope, hosts: Option<Vec<String>>) -> Caller {
    let row = ControlTokenRow {
        id: 1,
        name: "ci".into(),
        token_sha256: crate::mcp::auth::sha256_hex("flt_live_x"),
        scope,
        hosts,
        expires_at: None,
        created_at: 0,
        last_used_at: None,
        revoked_at: None,
    };
    resolve_token_at("flt_live_x", "master", &[], &[], &[row], Some(1), 0).unwrap()
}

#[test]
fn only_the_owner_and_an_admin_token_reach_add_account() {
    assert!(enforce_admin(&device(false), "add_account").is_ok());
    assert!(enforce_admin(&named(ApiScope::Admin, None), "add_account").is_ok());
    for c in [
        named(ApiScope::Act, None),
        named(ApiScope::Read, None),
        named(ApiScope::Act, Some(vec!["mercury".into()])),
    ] {
        let refused =
            enforce_mode(&c, "add_account").and_then(|()| enforce_admin(&c, "add_account"));
        assert!(refused.is_err(), "{}", c.label());
    }
}

#[tokio::test]
async fn an_untrusted_device_cannot_write_an_account() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("mercury").unwrap();
    let t = super::tests::test_tools(s);
    let e = t
        .add_account(
            Extension(device(false)),
            Parameters(
                serde_json::from_value(serde_json::json!({
                    "action": "api_key",
                    "host_alias": "mercury",
                    "profile": "api",
                    "api_key": "sk-ant-api03-aaaaaaaaaaaaaaaaaaaa",
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap_err();
    assert!(e.message.starts_with("E_FORBIDDEN"), "{}", e.message);
    assert!(!e.message.contains("sk-ant"), "the key never comes back");
}
