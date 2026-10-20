//! Who reaches Control's own state through the MCP tools: the pending
//! auto-continue pick (`control_auto_continue`), the paired-client list
//! (`diagnose`), and which hosts `suggest_placement` weighs.

use super::*;
use serde_json::Value;

const PERSON: i64 = 1;

fn tools(s: Store) -> FleetTools {
    s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, "false")
        .unwrap();
    FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    )
}

fn client(id: i64, name: &str, org_id: Option<i64>, owner: bool) -> Caller {
    Caller {
        api: None,
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id,
            name: name.into(),
            trusted: false,
            org_id,
            person_id: Some(if owner { PERSON } else { PERSON + 1 }),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: owner && org_id.is_none(),
        is_fleet_admin: owner && org_id.is_none(),
    }
}

fn host(alias: &str) -> Caller {
    Caller {
        api: None,
        host_alias: Some(alias.into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    }
}

fn json(r: &CallToolResult) -> Value {
    let text: String = r
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    serde_json::from_str(&text).unwrap()
}

fn forbidden(e: McpError) {
    assert!(e.message.starts_with("E_FORBIDDEN"), "{}", e.message);
}

#[test]
fn host_tokens_are_refused_controls_auto_continue_and_diagnose() {
    for tool in ["control_auto_continue", "diagnose"] {
        assert!(guard::NOT_FOR_HOST_TOKENS.contains(&tool), "{tool}");
        assert!(enforce_admin(&host("h"), tool).is_err(), "{tool}");
        assert!(!present::visible_to(&host("h"), tool), "{tool}");
    }
}

#[tokio::test]
async fn auto_continue_is_controls_and_its_owners_only() {
    let t = tools(Store::open_in_memory().unwrap());
    let args = |a: &str| {
        Parameters(crate::service::control_continue::AutoContinueArgs {
            action: a.into(),
            turn_seq: None,
        })
    };
    // Another person's device and an org-bound one: not even status.
    for c in [
        client(8, "phone", None, false),
        client(9, "work", Some(5), true),
    ] {
        for a in ["status", "cancel", "take"] {
            forbidden(
                t.control_auto_continue(Extension(c.clone()), args(a))
                    .await
                    .unwrap_err(),
            );
        }
    }
    // Control reads it only.
    let op = client(
        3,
        crate::service::operator::OPERATOR_CLIENT_NAME,
        None,
        true,
    );
    t.control_auto_continue(Extension(op.clone()), args("status"))
        .await
        .unwrap();
    forbidden(
        t.control_auto_continue(Extension(op), args("cancel"))
            .await
            .unwrap_err(),
    );
    // The master and the owner's own device act.
    for c in [Caller::master(), client(7, "phone", None, true)] {
        t.control_auto_continue(Extension(c.clone()), args("status"))
            .await
            .unwrap();
        t.control_auto_continue(Extension(c), args("cancel"))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn diagnose_shows_a_non_owner_caller_only_its_own_client() {
    let s = Store::open_in_memory().unwrap();
    let mine = s.insert_client_token("mine", "sha-mine", "full").unwrap();
    s.insert_client_token("other", "sha-other", "full").unwrap();
    let t = tools(s);
    let names = |v: &Value| -> Vec<String> {
        let mut n: Vec<String> = v["clients"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        n.sort();
        n
    };
    let call = |c: Caller| {
        let t = t.clone();
        async move {
            json(
                &t.diagnose(Extension(c), Parameters(Default::default()))
                    .await
                    .unwrap(),
            )
        }
    };
    assert_eq!(names(&call(Caller::master()).await), ["mine", "other"]);
    assert_eq!(
        names(&call(client(mine.id, "mine", None, false)).await),
        ["mine"]
    );
}

#[tokio::test]
async fn suggest_placement_weighs_only_the_callers_orgs_hosts() {
    let s = Store::open_in_memory().unwrap();
    for h in ["h-a", "h-b"] {
        s.upsert_host(h).unwrap();
    }
    let a = s.add_org("Company A", None, false).unwrap().id;
    let b = s.add_org("Company B", None, false).unwrap().id;
    s.set_host_org("h-a", Some(a)).unwrap();
    s.set_host_org("h-b", Some(b)).unwrap();
    let t = tools(s);
    let hosts_in = |v: &Value| -> Vec<String> {
        let mut out: Vec<String> = ["candidates", "excluded"]
            .iter()
            .flat_map(|k| v[*k].as_array().cloned().unwrap_or_default())
            .map(|r| r["host_alias"].as_str().unwrap().to_string())
            .collect();
        out.sort();
        out.dedup();
        out
    };
    let call = |c: Caller, hosts: Option<Vec<String>>| {
        let t = t.clone();
        async move {
            let p = crate::service::placement::SuggestArgs {
                hosts,
                ..Default::default()
            };
            json(
                &t.suggest_placement(Extension(c), Parameters(p))
                    .await
                    .unwrap(),
            )
        }
    };
    assert_eq!(
        hosts_in(&call(Caller::master(), None).await),
        ["h-a", "h-b"]
    );
    for c in [client(9, "work", Some(a), true), host("h-a")] {
        assert_eq!(hosts_in(&call(c.clone(), None).await), ["h-a"], "{c:?}");
        // Naming another org's host does not reach it either.
        let asked = Some(vec!["h-b".to_string()]);
        assert!(hosts_in(&call(c, asked).await).is_empty());
    }
}
