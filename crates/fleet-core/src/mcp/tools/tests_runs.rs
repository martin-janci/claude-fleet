//! The `runs` tool (Orbit Fleet 8.3): who is served it, and that a caller
//! is answered only the runs its view scope reaches.

use super::*;
use serde_json::{json, Value};

const PERSON: i64 = 1;

struct Fx {
    t: FleetTools,
    s_a: i64,
    s_b: i64,
}

/// Two orgs, one host each, one session on each OWNED by the hub's one
/// person (so `private`), a task in each, and a Jev run about org b's
/// tracker (a run that belongs to no session).
fn fixture() -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, "false")
        .unwrap();
    for h in ["h-a", "h-b"] {
        s.upsert_host(h).unwrap();
    }
    let a = s.add_org("Company A", None, false).unwrap().id;
    let b = s.add_org("Company B", None, false).unwrap().id;
    s.set_host_org("h-a", Some(a)).unwrap();
    s.set_host_org("h-b", Some(b)).unwrap();
    assert_eq!(s.personal_owner_id().unwrap(), Some(PERSON));
    let sess = |name: &str, host: &str| {
        let id = s
            .upsert_session(name, host, None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(id, Some(PERSON)).unwrap();
        id
    };
    let s_a = sess("s-a", "h-a");
    let s_b = sess("s-b", "h-b");
    s.insert_task(None, Some(s_a), "SECRET-A work", "n1")
        .unwrap();
    s.insert_task(None, Some(s_b), "SECRET-B work", "n2")
        .unwrap();
    s.conn_for_test()
        .execute(
            "INSERT INTO decision_runs (at, feature, org_id, subject_kind, subject_id, mode, \
               provider, question_version, called) \
             VALUES (20, 'status_map', ?1, 'tracker_section', '1:x', 'shadow', 'jev', 'q', 1)",
            [b],
        )
        .unwrap();
    let t = FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    );
    Fx { t, s_a, s_b }
}

fn client(mode: TokenMode, org_id: Option<i64>) -> Caller {
    Caller {
        api: None,
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id: 7,
            name: "phone".into(),
            trusted: false,
            org_id,
            person_id: Some(PERSON),
        }),
        mode,
        pane: None,
        is_personal_owner: org_id.is_none(),
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
    }
}

/// The call as `call_tool` makes it: the mode, admin and host-token gates,
/// then the tool.
async fn call(fx: &Fx, c: &Caller, args: Value) -> Result<Value, String> {
    enforce_mode(c, "runs")
        .and_then(|()| enforce_admin(c, "runs"))
        .map_err(|e| e.message.to_string())?;
    let r =
        fx.t.runs(
            Extension(c.clone()),
            Parameters(serde_json::from_value(args).unwrap()),
        )
        .await
        .map_err(|e| e.message.to_string())?;
    let text: String = r
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    if r.is_error == Some(true) {
        return Err(text);
    }
    Ok(serde_json::from_str(&text).unwrap())
}

fn sources(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["id"]
                .as_str()
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    out.sort();
    out
}

#[test]
fn runs_is_a_readonly_client_tool_no_host_token_is_served() {
    assert!(guard::is_client_tool("runs"));
    assert!(guard::is_readonly_tool("runs"));
    assert!(!guard::needs_confirmation("runs"));
    assert!(guard::NOT_FOR_HOST_TOKENS.contains(&"runs"));
    assert!(present::visible_to(
        &client(TokenMode::Readonly, None),
        "runs"
    ));
    assert!(!present::visible_to(&host("h-a"), "runs"));
}

#[tokio::test]
async fn each_caller_is_answered_only_the_runs_it_may_see() {
    let fx = fixture();
    let list = json!({ "action": "list" });

    // The master (the hub's one person) and that person's own unbound
    // device: every run, the Jev run included (whole-fleet spend).
    for c in [Caller::master(), client(TokenMode::Readonly, None)] {
        let v = call(&fx, &c, list.clone()).await.unwrap();
        assert_eq!(sources(&v), ["jev", "task", "task"], "{c:?}");
        assert_eq!(v["total"], 3);
    }

    // A device bound to org A: org A's task only. Nothing of B — not its
    // task, not the Jev run about its tracker.
    let bound_a = client(TokenMode::Full, Some(1));
    let v = call(&fx, &bound_a, list.clone()).await.unwrap();
    assert_eq!(sources(&v), ["task"]);
    assert_eq!(v["runs"][0]["session_ids"], json!([fx.s_a]));
    assert!(!v.to_string().contains("SECRET-B"), "{v}");
    // Naming B's session finds nothing, rather than refusing.
    let v = call(
        &fx,
        &bound_a,
        json!({ "action": "list", "session_id": fx.s_b }),
    )
    .await
    .unwrap();
    assert_eq!(v["total"], 0);

    // A colleague's device sees none of the owner's private sessions.
    let mut other = client(TokenMode::Full, None);
    if let Some(cl) = other.client.as_mut() {
        cl.person_id = Some(PERSON + 1);
    }
    other.is_personal_owner = false;
    let v = call(&fx, &other, list.clone()).await.unwrap();
    assert_eq!(v["total"], 0, "{v}");

    // A per-host token is refused the tool outright.
    let e = call(&fx, &host("h-a"), list.clone()).await.unwrap_err();
    assert!(e.starts_with("E_"), "{e}");
}

#[tokio::test]
async fn runs_refuses_an_unknown_action_or_filter() {
    let fx = fixture();
    let master = Caller::master();
    let e = call(&fx, &master, json!({ "action": "delete" }))
        .await
        .unwrap_err();
    assert!(e.starts_with("E_INVALID"), "{e}");
    let e = call(&fx, &master, json!({ "action": "list", "kind": "cron" }))
        .await
        .unwrap_err();
    assert!(e.starts_with("E_INVALID"), "{e}");
}
