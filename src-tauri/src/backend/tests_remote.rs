//! Tests for [`super`] (`backend::remote`), against recorded hub responses and a fake
//! transport. No network.
//!
//! Every fixture here is built from the hub's *real* encoding, not from
//! imagination:
//!
//! - the SSE framing and the `Accept` pair come from
//!   `crates/fleet-core/src/mcp/mod.rs` (see its `post_mcp_to` test helper)
//!   and are the same ones `crates/fleet-hub/src/pair.rs` speaks;
//! - a successful result is `CallToolResult::success` with the tool's JSON as
//!   `content[0].text` — a *string* (`mcp::tools::support::ok_json`);
//! - a list tool's payload is null-stripped (`ok_json_compact` →
//!   `strip_nulls`), which is why rows arrive with keys missing;
//! - a failed tool is `isError: true` with
//!   `structured_content = {code, message, details}` and a `"CODE: message"`
//!   text block (`mcp::tools::support::tool_error_result` / `mcp_err`);
//! - `401` and `403` are bare `axum::http::StatusCode` replies from
//!   `mcp::authorize`, so an empty body is the normal case.
//!
//! `a_null_stripped_session_row_survives_the_round_trip` is the guard against
//! the fixtures rotting: it serialises a real `SessionRow`, strips nulls the
//! way the hub does, and reads it back.

use super::*;
use crate::backend::RemoteConfig;
use fleet_core::ipc_error::codes;
use fleet_core::store::SessionRow;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// --- fixtures ----------------------------------------------------------------

/// One `tools/call` answer as the hub frames it: SSE, so the JSON-RPC
/// envelope is on a `data:` line rather than being the body.
fn sse(envelope: Value) -> HubResponse {
    HubResponse {
        status: 200,
        body: format!("event: message\ndata: {envelope}\n\n"),
    }
}

/// A successful tool result carrying `payload` (the tool's JSON, which the
/// envelope holds as a *string*).
fn ok(payload: &str) -> HubResponse {
    sse(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "content": [{ "type": "text", "text": payload }] },
    }))
}

/// A failed tool result, exactly as `tool_error_result` builds one.
fn tool_error(code: &str, message: &str, details: Value) -> HubResponse {
    let text = match &details {
        Value::Null => format!("{code}: {message}"),
        d => format!("{code}: {message} {d}"),
    };
    sse(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "isError": true,
            "content": [{ "type": "text", "text": text }],
            "structuredContent": { "code": code, "message": message, "details": details },
        },
    }))
}

/// Two rows of `list_sessions` with `summary: false`, null-stripped the way
/// `ok_json_compact` leaves them: every `None` column is simply absent, the
/// `usage_*` fields are flattened onto the row, and `is_controller` rides
/// alongside (`SessionWithController`).
const LIST_SESSIONS_PAYLOAD: &str = r#"[{"is_controller":true,"id":12,"tmux_name":"feat-hub-client","host_alias":"trn","project_id":3,"created_at":1757000000,"last_activity_at":1757003600,"status":"running","kind":"tmux","worktree_key":"trn:/home/dev/p/.worktrees/hub","claude_session_id":"0f3a9c1e-1111-4222-8333-444455556666","claude_status":"working","context_pct":42.5,"turn_seq":7,"tags":["review"],"usage_input_tokens":1200,"usage_output_tokens":800,"usage_cache_write_tokens":0,"usage_cache_read_tokens":0,"usage_cost_micros":31500,"usage_model":"claude-opus-5","usage_updated_at":1757003500},{"is_controller":false,"id":13,"tmux_name":"bg:abc123","host_alias":"hetzner","created_at":1757001000,"last_activity_at":1757001500,"status":"ghost","kind":"bg","lost_at":1757002000,"claude_status":"failed","stuck_kind":"oom","turn_seq":0,"tags":[],"usage_input_tokens":0,"usage_output_tokens":0,"usage_cache_write_tokens":0,"usage_cache_read_tokens":0,"usage_cost_micros":0}]"#;

// --- the fake transport ------------------------------------------------------

/// Records what it was asked and answers from a script.
struct Fake {
    answers: Mutex<Vec<Result<HubResponse, String>>>,
    seen: Mutex<Vec<(String, String, String)>>,
}

impl Fake {
    fn answering(r: Result<HubResponse, String>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(vec![r]),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// The one request it was given: `(url, bearer, body)`.
    fn only_request(&self) -> (String, String, String) {
        let seen = self.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "expected exactly one request: {seen:?}");
        seen[0].clone()
    }

    /// The `params` of the one `tools/call` it was given.
    fn only_call(&self) -> (String, Value) {
        let (_, _, body) = self.only_request();
        let v: Value = serde_json::from_str(&body).expect("a JSON-RPC body");
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["method"], "tools/call");
        (
            v["params"]["name"]
                .as_str()
                .expect("a tool name")
                .to_string(),
            v["params"]["arguments"].clone(),
        )
    }
}

#[async_trait::async_trait]
impl HubTransport for Fake {
    async fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: String,
    ) -> Result<HubResponse, String> {
        self.seen
            .lock()
            .unwrap()
            .push((url.to_string(), bearer.to_string(), body));
        self.answers
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| panic!("the fake transport ran out of answers"))
    }
}

fn cfg() -> RemoteConfig {
    RemoteConfig {
        base_url: "http://hub.example.com:4180".into(),
        token: "cl_s3cret-token".into(),
        client_name: "laptop".into(),
    }
}

fn backend(fake: &Arc<Fake>) -> HubBackend {
    HubBackend::with_transport(cfg(), fake.clone())
}

/// `tokio::test` needs a runtime; these calls never actually await I/O.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

// --- the request -------------------------------------------------------------

#[test]
fn a_call_is_a_jsonrpc_tools_call_to_the_hubs_mcp_endpoint() {
    let fake = Fake::answering(Ok(ok("[]")));
    let rows: Vec<SessionRow> = block_on(backend(&fake).list_sessions(false)).expect("rows");
    assert!(rows.is_empty());

    let (url, bearer, _) = fake.only_request();
    assert_eq!(url, "http://hub.example.com:4180/mcp");
    assert_eq!(bearer, "cl_s3cret-token", "the client token authenticates");

    let (tool, args) = fake.only_call();
    assert_eq!(tool, "list_sessions");
    // The desktop renders full rows; the tool's slim default exists for an
    // MCP caller's token cap, which an IPC caller does not have.
    assert_eq!(args["summary"], false);
    assert_eq!(args["force"], false);
    // The sidebar shows ghosts until they are dismissed, so lost rows must
    // not be filtered out on the way in.
    assert_eq!(args["include_lost"], true);
}

#[test]
fn the_refresh_button_asks_the_hub_to_reconcile_first() {
    let fake = Fake::answering(Ok(ok("[]")));
    let _: Vec<SessionRow> = block_on(backend(&fake).list_sessions(true)).expect("rows");
    assert_eq!(fake.only_call().1["force"], true);
}

#[test]
fn each_typed_read_names_its_tool_and_arguments() {
    // (what we call, the tool it must reach, the arguments it must carry)
    let fake = Fake::answering(Ok(ok("[]")));
    let _ = block_on(backend(&fake).related_sessions(42));
    assert_eq!(
        fake.only_call(),
        ("related_sessions".into(), json!({"session_id": 42}))
    );

    let fake = Fake::answering(Ok(ok("[]")));
    let _ = block_on(backend(&fake).list_hosts());
    assert_eq!(fake.only_call().0, "list_hosts");

    let fake = Fake::answering(Ok(ok("[]")));
    let _ = block_on(backend(&fake).list_worktrees(Some(7)));
    assert_eq!(
        fake.only_call(),
        ("list_worktrees".into(), json!({"project_id": 7}))
    );

    let fake = Fake::answering(Ok(ok("[]")));
    let _ = block_on(backend(&fake).session_history(5, Some(20)));
    assert_eq!(
        fake.only_call(),
        (
            "session_history".into(),
            json!({"session_id": 5, "limit": 20})
        )
    );

    let fake = Fake::answering(Ok(ok("[]")));
    let _ = block_on(backend(&fake).list_tasks(None, Some("queued".into()), Some(50)));
    assert_eq!(
        fake.only_call(),
        (
            "list_tasks".into(),
            json!({"requester_session_id": null, "state": "queued", "limit": 50})
        )
    );
}

// --- the successful answer ---------------------------------------------------

#[test]
fn an_sse_framed_tool_result_deserialises_into_session_rows() {
    let fake = Fake::answering(Ok(ok(LIST_SESSIONS_PAYLOAD)));
    let rows: Vec<SessionRow> = block_on(backend(&fake).list_sessions(false)).expect("rows");

    assert_eq!(rows.len(), 2);

    let live = &rows[0];
    assert_eq!(live.id, 12);
    assert_eq!(live.tmux_name, "feat-hub-client");
    assert_eq!(live.host_alias, "trn");
    assert_eq!(live.project_id, Some(3));
    assert_eq!(live.status, "running");
    assert_eq!(live.kind, "tmux");
    assert_eq!(live.claude_status.as_deref(), Some("working"));
    assert_eq!(live.context_pct, Some(42.5));
    assert_eq!(live.turn_seq, 7);
    assert_eq!(live.tags, vec!["review".to_string()]);
    // The columns the hub stripped because they were null must read as None,
    // not as a deserialisation failure — that is how `ok_json_compact`
    // encodes every absent value.
    assert_eq!(live.worktree_id, None);
    assert_eq!(live.notes, None);
    assert_eq!(live.pr_url, None);
    assert_eq!(live.stuck_kind, None);
    // `usage_*` is flattened onto the row on the wire.
    assert_eq!(live.usage.usage_input_tokens, 1200);
    assert_eq!(live.usage.usage_cost_micros, 31500);
    assert_eq!(live.usage.usage_model.as_deref(), Some("claude-opus-5"));

    let ghost = &rows[1];
    assert_eq!(ghost.id, 13);
    assert_eq!(ghost.status, "ghost");
    assert_eq!(ghost.lost_at, Some(1757002000));
    assert_eq!(ghost.stuck_kind.as_deref(), Some("oom"));
    assert_eq!(ghost.project_id, None);
    assert!(ghost.tags.is_empty());
    assert_eq!(ghost.usage.usage_model, None);
}

/// The guard on the fixture above: whatever the hub's row type grows next,
/// a real `SessionRow` serialised and null-stripped the way `ok_json_compact`
/// leaves it must still read back. A new non-`Option` column without a
/// default fails here first.
#[test]
fn a_null_stripped_session_row_survives_the_round_trip() {
    let original = sample_session_row();
    let mut v = serde_json::to_value(&original).expect("serialise");
    strip_nulls(&mut v);
    // Nothing that was null may still be present — otherwise this test would
    // not be exercising the stripped shape at all.
    assert_eq!(v.get("notes"), None, "the fixture must really be stripped");
    // `SessionWithController` adds this alongside the flattened row.
    v.as_object_mut()
        .unwrap()
        .insert("is_controller".into(), json!(false));

    let payload = serde_json::to_string(&json!([v])).unwrap();
    let fake = Fake::answering(Ok(ok(&payload)));
    let rows: Vec<SessionRow> = block_on(backend(&fake).list_sessions(false)).expect("rows");
    assert_eq!(rows, vec![original]);
}

/// Mirrors `fleet_core::mcp::tools::support::strip_nulls`, which is private
/// to that module. If the two ever disagree, the round-trip above stops
/// testing the real encoding — which is why this is spelled out rather than
/// approximated.
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                strip_nulls(val);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

fn sample_session_row() -> SessionRow {
    SessionRow {
        id: 12,
        tmux_name: "feat-hub-client".into(),
        host_alias: "trn".into(),
        project_id: Some(3),
        worktree_id: None,
        created_at: 1_757_000_000,
        last_activity_at: 1_757_003_600,
        status: "running".into(),
        notes: None,
        account_uuid: None,
        kind: "tmux".into(),
        reviews_session_id: None,
        worktree_key: Some("trn:/home/dev/p/.worktrees/hub".into()),
        lost_at: None,
        claude_session_id: Some("0f3a9c1e-1111-4222-8333-444455556666".into()),
        claude_status: Some("working".into()),
        effort_level: None,
        pr_url: None,
        current_activity: None,
        context_pct: Some(42.5),
        stuck_kind: None,
        friendly_name: None,
        safe_kill_state: None,
        safe_kill_nonce: None,
        safe_kill_detail: None,
        safe_kill_requested_at: None,
        idle_since: None,
        stuck_since: None,
        last_playbook_at: None,
        last_prompt: None,
        started_at: None,
        last_turn_at: None,
        ci_status: None,
        turn_seq: 7,
        last_stop_at: None,
        parent_session_id: None,
        tags: vec!["review".into()],
        usage: fleet_core::store::SessionUsage {
            usage_input_tokens: 1200,
            usage_output_tokens: 800,
            usage_cache_write_tokens: 0,
            usage_cache_read_tokens: 0,
            usage_cost_micros: 31500,
            usage_model: Some("claude-opus-5".into()),
            usage_updated_at: Some(1_757_003_500),
        },
    }
}

/// A plain JSON body (rmcp's `json_response` mode) is read too — the hub does
/// not use it today, but a hub that switched it on must not break the app.
#[test]
fn a_plain_json_body_is_read_as_well_as_an_sse_frame() {
    let envelope = json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "content": [{ "type": "text", "text": "[]" }] },
    });
    let fake = Fake::answering(Ok(HubResponse {
        status: 200,
        body: envelope.to_string(),
    }));
    let rows: Vec<SessionRow> = block_on(backend(&fake).list_sessions(false)).expect("rows");
    assert!(rows.is_empty());
}

// --- the failed answer -------------------------------------------------------

#[test]
fn a_tool_error_becomes_the_same_ipc_error_the_service_layer_raised() {
    let fake = Fake::answering(Ok(tool_error(
        codes::E_NOTFOUND,
        "session 99 not found",
        Value::Null,
    )));
    let err = block_on(backend(&fake).session_history(99, None)).expect_err("an error");
    assert_eq!(err.code, codes::E_NOTFOUND);
    assert_eq!(err.message, "session 99 not found");
    assert_eq!(err.details, None);
}

/// `E_AMBIGUOUS` and `E_LINT` put the part a caller acts on in `details`;
/// losing it would turn an actionable error into prose.
#[test]
fn an_errors_structured_details_survive_the_round_trip() {
    let details = json!({ "candidates": [{ "host_alias": "trn", "tmux_name": "api" }] });
    let fake = Fake::answering(Ok(tool_error(
        codes::E_AMBIGUOUS,
        "several sessions named 'api'",
        details.clone(),
    )));
    let err = block_on(backend(&fake).session_history(1, None)).expect_err("an error");
    assert_eq!(err.code, codes::E_AMBIGUOUS);
    assert_eq!(err.message, "several sessions named 'api'");
    assert_eq!(err.details, Some(details));
}

/// A hub that sends no `structured_content` still yields the right code: the
/// text block is `"CODE: message"` (`mcp_err`).
#[test]
fn a_tool_error_without_structured_content_is_read_from_its_text() {
    let fake = Fake::answering(Ok(sse(json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {
            "isError": true,
            "content": [{ "type": "text", "text": "E_FORBIDDEN: readonly token" }],
        },
    }))));
    let err = block_on(backend(&fake).list_hosts()).expect_err("an error");
    assert_eq!(err.code, codes::E_FORBIDDEN);
    assert_eq!(err.message, "readonly token");
}

/// Prose that merely contains a colon must not be shredded into a made-up
/// code — only an `E_`-shaped token counts.
#[test]
fn an_uncoded_tool_error_is_not_mistaken_for_a_code() {
    let fake = Fake::answering(Ok(sse(json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {
            "isError": true,
            "content": [{ "type": "text", "text": "something broke: badly" }],
        },
    }))));
    let err = block_on(backend(&fake).list_hosts()).expect_err("an error");
    assert_eq!(err.code, codes::E_INTERNAL);
    assert!(
        err.message.contains("something broke: badly"),
        "{}",
        err.message
    );
}

#[test]
fn a_jsonrpc_protocol_error_is_reported_as_one() {
    let fake = Fake::answering(Ok(sse(json!({
        "jsonrpc": "2.0", "id": 1,
        "error": { "code": -32601, "message": "tool not found: list_sessions" },
    }))));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_INTERNAL);
    assert!(err.message.contains("tool not found"), "{}", err.message);
}

// --- the HTTP layer ----------------------------------------------------------

#[test]
fn a_401_is_unauthorized_and_says_the_hub_revoked_this_client() {
    // `mcp::authorize` returns a bare StatusCode, so the body really is empty.
    let fake = Fake::answering(Ok(HubResponse {
        status: 401,
        body: String::new(),
    }));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_UNAUTHORIZED);
    assert!(
        err.message.contains("the hub revoked this client"),
        "{}",
        err.message
    );
    // The user has to be told where to go; retrying cannot help.
    assert!(err.message.contains("Settings"), "{}", err.message);
}

#[test]
fn a_403_carries_the_hubs_own_body() {
    let fake = Fake::answering(Ok(HubResponse {
        status: 403,
        body: "Invalid Host header: fleet.example.com".into(),
    }));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_FORBIDDEN);
    assert!(
        err.message
            .contains("Invalid Host header: fleet.example.com"),
        "the hub's own words must reach the user: {}",
        err.message
    );
}

/// A 403 straight from `authorize` has no body at all; the message must still
/// say what went wrong.
#[test]
fn a_403_with_no_body_still_explains_itself() {
    let fake = Fake::answering(Ok(HubResponse {
        status: 403,
        body: String::new(),
    }));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_FORBIDDEN);
    assert!(err.message.contains("allowlist"), "{}", err.message);
}

#[test]
fn a_transport_failure_is_e_hub_unreachable() {
    let fake = Fake::answering(Err(
        "connect hub.example.com:4180: Connection refused".into()
    ));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_HUB_UNREACHABLE);
    assert!(
        err.message.contains("Connection refused"),
        "{}",
        err.message
    );
    assert!(err.message.contains("hub.example.com"), "{}", err.message);
}

/// A proxy's 502 reached something, but not a working hub — same code, and
/// the status stays in the message so nothing is hidden behind it.
#[test]
fn any_other_status_is_e_hub_unreachable_and_names_it() {
    let fake = Fake::answering(Ok(HubResponse {
        status: 502,
        body: "<html><title>502 Bad Gateway</title>".into(),
    }));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_HUB_UNREACHABLE);
    assert!(err.message.contains("502"), "{}", err.message);
}

/// A proxy can answer with a whole HTML page; the user gets its first line,
/// not all of it.
#[test]
fn an_enormous_error_body_is_trimmed() {
    let fake = Fake::answering(Ok(HubResponse {
        status: 500,
        body: format!("{}\nand more lines\n", "x".repeat(5000)),
    }));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert!(err.message.len() < 400, "{} chars", err.message.len());
    assert!(!err.message.contains("and more lines"), "{}", err.message);
}

// --- the token ---------------------------------------------------------------

/// The one invariant that matters most: nothing the user or a log can see may
/// carry the bearer token.
#[test]
fn no_error_and_no_debug_output_ever_carries_the_token() {
    let answers: Vec<Result<HubResponse, String>> = vec![
        Err("connect failed for Bearer cl_s3cret-token".into()),
        Ok(HubResponse {
            status: 401,
            body: "cl_s3cret-token".into(),
        }),
        Ok(HubResponse {
            status: 403,
            body: "cl_s3cret-token".into(),
        }),
        Ok(HubResponse {
            status: 500,
            body: "cl_s3cret-token".into(),
        }),
        Ok(HubResponse {
            status: 200,
            body: "not json".into(),
        }),
        // A tool error's own text and its structured message. The hub builds
        // those from service-layer prose, never from a request header, so the
        // token cannot reach them — which is exactly why it is worth
        // asserting rather than assuming.
        Ok(tool_error(
            codes::E_INVALID,
            "bad token cl_s3cret-token",
            Value::Null,
        )),
        Ok(sse(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "isError": true,
                "content": [{ "type": "text", "text": "leaked cl_s3cret-token" }],
            },
        }))),
    ];
    for answer in answers {
        let fake = Fake::answering(answer);
        let b = backend(&fake);
        let err = block_on(b.list_sessions(false)).expect_err("an error");
        let shown = format!("{err:?} {} {}", err.code, err.message);
        assert!(
            !shown.contains("cl_s3cret-token"),
            "an error leaked the token: {shown}"
        );
        assert!(
            !format!("{b:?}").contains("cl_s3cret-token"),
            "Debug leaked the token"
        );
    }
}

// --- the raw HTTP transport --------------------------------------------------

/// The desktop's hub is normally `https://` (`docs/hub.md`), and this build
/// cannot speak it. That must be a clear refusal, not a hang or a silent
/// fallback — see `TcpTransport`'s doc comment for the dependency decision
/// this is waiting on.
#[test]
fn the_tcp_transport_refuses_https_with_a_reason() {
    let e = block_on(TcpTransport.post_json("https://fleet.example.com/mcp", "t", "{}".into()))
        .expect_err("https must be refused");
    assert!(e.contains("https"), "{e}");
    assert!(e.to_lowercase().contains("tls"), "{e}");
}

#[test]
fn a_raw_http_response_is_split_into_its_status_and_body() {
    let raw = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
               event: message\ndata: {\"a\":1}\n\n";
    let r = split_response(raw).expect("a response");
    assert_eq!(r.status, 200);
    assert!(r.body.starts_with("event: message"));

    // The status token, not a substring: a reason phrase carrying " 200"
    // must not read as success.
    let r = split_response("HTTP/1.1 500 Internal Error 200 OK\r\n\r\nboom").unwrap();
    assert_eq!(r.status, 500);

    assert!(split_response("").is_err());
}
