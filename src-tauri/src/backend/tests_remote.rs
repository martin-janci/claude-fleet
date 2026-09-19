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
        lost_reason: None,
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
        // The Task 2 review found these two, and both were real leaks: every
        // other external-text path here was scrubbed and these were not, so
        // the claim that "every scrap of text that originates outside this
        // process is now scrubbed" was false. A JSON-RPC protocol error's
        // message…
        Ok(sse(json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32602, "message": "bad request: Bearer cl_s3cret-token" },
        }))),
        // …and a tool error's structured `details`, which is the worse of the
        // two: `IpcError` derives `Serialize`, so `details` crosses the IPC
        // boundary into the frontend rather than merely appearing in a Debug
        // line. Nested, inside an array, and once as an object *key*, so a
        // shallow scrub of the top level would not be enough.
        Ok(tool_error(
            codes::E_INVALID,
            "bad request",
            json!({
                "echoed": "cl_s3cret-token",
                "candidates": ["cl_s3cret-token", { "cl_s3cret-token": "as a key" }],
            }),
        )),
    ];
    for answer in answers {
        let fake = Fake::answering(answer);
        let b = backend(&fake);
        let err = block_on(b.list_sessions(false)).expect_err("an error");
        // `details` is rendered separately: it is the field that actually
        // reaches the frontend, so relying on `{err:?}` alone would miss it
        // the day the Debug impl stops printing it.
        let details = err
            .details
            .as_ref()
            .map(|d| d.to_string())
            .unwrap_or_default();
        let shown = format!("{err:?} {} {} {details}", err.code, err.message);
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

/// A hub address is http or https and nothing else. Anything else must be
/// refused by name rather than producing a confusing connect error.
/// (`normalise_base_url` already rejects these when the setting is read; this
/// is the transport's own guard.)
#[test]
fn the_tcp_transport_refuses_a_scheme_that_is_not_a_hub_address() {
    for url in ["ftp://fleet.example.com/mcp", "file:///etc/passwd"] {
        let e = match block_on(TcpTransport.post_json(url, "t", "{}".into())) {
            Err(e) => e,
            Ok(r) => panic!("{url} should have been refused, got {r:?}"),
        };
        assert!(e.contains("not a hub address"), "for {url}: {e}");
    }
}

/// The platform trust store must actually yield roots on this machine —
/// otherwise every `https://` hub fails with an opaque certificate error and
/// nobody knows why. `TcpTransport` fails closed and says so; this asserts
/// that the happy path really is happy here.
#[test]
fn the_platform_trust_store_yields_roots() {
    let found = rustls_native_certs::load_native_certs();
    assert!(
        !found.certs.is_empty(),
        "no roots loaded; errors: {:?}",
        found.errors
    );
}

/// Proof that the TLS path actually completes a handshake against a real
/// server and reads a real response — the closed-port test above only shows
/// that it fails correctly. Ignored by default because it needs the network;
/// run with `cargo test -p claude-fleet --lib -- --ignored tls_really`.
///
/// It is deliberately NOT pointed at a hub: any https server proves the
/// handshake, the platform roots and the read loop. The status will be a 4xx
/// (the host is not an MCP endpoint), which is exactly what we assert — a
/// parsed status means the whole path worked.
#[test]
#[ignore = "needs network"]
fn tls_really_completes_a_handshake_against_a_real_server() {
    let r = block_on(TcpTransport.post_json("https://crates.io/", "t", "{}".into()))
        .expect("a TLS exchange");
    assert!(
        r.status >= 200,
        "expected a parsed HTTP status, got {}",
        r.status
    );
}

/// An https hub that is not listening must come back as an ordinary transport
/// error — which `HubBackend` maps to `E_HUB_UNREACHABLE` — not a panic and
/// not a hang. Port 1 on loopback is closed everywhere.
#[test]
fn an_https_hub_that_is_not_listening_fails_as_a_transport_error() {
    let e = match block_on(TcpTransport.post_json("https://127.0.0.1:1/mcp", "t", "{}".into())) {
        Err(e) => e,
        Ok(r) => panic!("a closed port should have failed, got {r:?}"),
    };
    assert!(e.contains("connect 127.0.0.1:1"), "{e}");
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

// --- a peer that half-closes -------------------------------------------------
//
// The Task 2 review's finding 6. rustls 0.23 reports a TCP close with no
// `close_notify` as `UnexpectedEof`, and `speak` used to `?` that — discarding
// a body that had already arrived. Nothing exercised it, because the one live
// test points at crates.io, which does send `close_notify`.

/// Hands back `body` (in whatever chunks the reader's buffer allows), then
/// fails with `kind` instead of reporting a clean end of stream — which is
/// what a peer that drops the connection without `close_notify` looks like.
struct HalfClosing {
    body: Vec<u8>,
    kind: std::io::ErrorKind,
}

impl tokio::io::AsyncRead for HalfClosing {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if !self.body.is_empty() {
            // Never more than the buffer has room for; `put_slice` panics
            // rather than truncating, and `read_to_end` grows its buffer
            // between calls.
            let n = self.body.len().min(buf.remaining());
            let rest = self.body.split_off(n);
            buf.put_slice(&self.body);
            self.body = rest;
            return std::task::Poll::Ready(Ok(()));
        }
        // Deliberately an error rather than `Ok(())` with nothing written:
        // zero bytes IS a clean EOF, which is the case this test is not about.
        std::task::Poll::Ready(Err(std::io::Error::new(self.kind, "peer went away")))
    }
}

impl tokio::io::AsyncWrite for HalfClosing {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

fn half_closing(body: &str, kind: std::io::ErrorKind) -> HalfClosing {
    HalfClosing {
        body: body.as_bytes().to_vec(),
        kind,
    }
}

const ONE_RESPONSE: &str = "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n\
                            event: message\ndata: {\"a\":1}\n\n";

/// A hub or proxy that closes the TCP connection without `close_notify` has
/// still answered. Throwing the answer away turns a working hub into
/// `E_HUB_UNREACHABLE` for no reason, and the plain-HTTP path has no
/// equivalent failure — so this was a regression the TLS work introduced.
#[test]
fn a_peer_that_half_closes_still_yields_the_response_it_already_sent() {
    let raw = block_on(speak(
        half_closing(ONE_RESPONSE, std::io::ErrorKind::UnexpectedEof),
        "hub.example.com",
        443,
        "GET / HTTP/1.1\r\n\r\n",
    ))
    .expect("a complete response must survive a missing close_notify");
    assert_eq!(raw, ONE_RESPONSE.as_bytes());
    // And it still parses, which is the thing the caller actually needs.
    assert_eq!(split_response(&raw).expect("a response").status, 200);
}

/// The other half: an `UnexpectedEof` with nothing read is a real failure and
/// must stay one. Otherwise a hub that never answered would surface as an
/// empty body and a confusing parse error instead of "it did not answer".
#[test]
fn an_unexpected_eof_with_nothing_read_is_still_a_failure() {
    let e = block_on(speak(
        half_closing("", std::io::ErrorKind::UnexpectedEof),
        "hub.example.com",
        443,
        "GET / HTTP/1.1\r\n\r\n",
    ))
    .expect_err("nothing arrived, so this is not a response");
    assert!(e.contains("read from hub.example.com:443"), "{e}");
}

/// And only `UnexpectedEof` is forgiven. A connection reset mid-body means
/// the bytes in hand are a *truncated* response, which must not be parsed as
/// if it were whole.
#[test]
fn a_real_read_error_is_not_swallowed_even_with_bytes_in_hand() {
    let e = block_on(speak(
        half_closing(ONE_RESPONSE, std::io::ErrorKind::ConnectionReset),
        "hub.example.com",
        443,
        "GET / HTTP/1.1\r\n\r\n",
    ))
    .expect_err("a reset is not a clean end of response");
    assert!(e.contains("read from hub.example.com:443"), "{e}");
}

// --- the endpoint ------------------------------------------------------------

/// One parse for every request this app makes, so a fix lands once.
#[test]
fn an_endpoint_splits_a_hub_url_into_what_a_hand_written_request_needs() {
    let at = Endpoint::parse("https://fleet.example.com/mcp").expect("a hub URL");
    assert_eq!(at.host, "fleet.example.com");
    assert_eq!(at.port, 443, "https defaults to 443");
    assert!(at.tls);
    assert_eq!(
        at.authority, "fleet.example.com",
        "no port in the Host header when it is the scheme's default — the \
         hub's allowlist is matched against exactly this string"
    );
    assert_eq!(at.target, "/mcp");

    let at = Endpoint::parse("http://hub.example.com:4180/fleet/events").expect("a hub URL");
    assert_eq!(at.port, 4180);
    assert!(!at.tls);
    assert_eq!(
        at.authority, "hub.example.com:4180",
        "a non-default port is"
    );
    assert_eq!(at.target, "/fleet/events");

    assert!(Endpoint::parse("ftp://hub.example.com").is_err());
    assert!(Endpoint::parse("not a url").is_err());
}

/// An IPv6-literal hub was simply unreachable: `host_str()` keeps the URL's
/// brackets, and `[::1]` neither resolves nor parses as a certificate name,
/// so `https://[::1]:8787` failed before a single byte went out. The `Host`
/// header is the one place the brackets belong.
#[test]
fn an_ipv6_literal_hub_connects_and_still_sends_a_bracketed_host_header() {
    let at = Endpoint::parse("https://[2001:db8::1]:8787/mcp").expect("an IPv6 hub URL");
    assert_eq!(
        at.host, "2001:db8::1",
        "the connect and SNI name must be unbracketed"
    );
    assert_eq!(at.port, 8787);
    assert_eq!(
        at.authority, "[2001:db8::1]:8787",
        "the Host header keeps the brackets"
    );
    // And the unbracketed form really is what rustls accepts as a name.
    assert!(
        tokio_rustls::rustls::pki_types::ServerName::try_from(at.host.clone()).is_ok(),
        "an IP literal is matched against an IP SAN"
    );
    assert!(
        tokio_rustls::rustls::pki_types::ServerName::try_from("[2001:db8::1]".to_string()).is_err(),
        "the bracketed form is what used to be passed, and it is rejected"
    );
    // Loopback too, since that is the tunnelled setup docs/hub.md describes.
    let at = Endpoint::parse("http://[::1]:8787/events").expect("a loopback IPv6 hub URL");
    assert_eq!(at.host, "::1");
    assert_eq!(at.authority, "[::1]:8787");
}

// --- chunked framing ---------------------------------------------------------

#[test]
fn a_chunked_body_is_rejoined() {
    let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
               5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    assert_eq!(split_response(raw).expect("a response").body, "hello world");
}

/// The shortcut this replaces worked only because a chunk-size line is not a
/// `data:` line and rmcp happened to write one frame per body frame. Split a
/// frame across two chunks and the size line lands INSIDE a `data:` line.
#[test]
fn a_chunk_boundary_inside_a_data_line_no_longer_corrupts_the_payload() {
    let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"[]"}]}}"#;
    let frame = format!("event: message\ndata: {payload}\n\n");
    let cut = frame.len() / 2;
    let (a, b) = frame.split_at(cut);
    let raw = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
         {:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
        a.len(),
        b.len()
    );
    let body = split_response(&raw).expect("a response").body;
    assert_eq!(body, frame, "the two chunks are rejoined byte for byte");
    assert_eq!(
        fleet_core::mcp::wire::last_event_payload(&body),
        payload,
        "and the envelope reads back whole"
    );
}

#[test]
fn a_body_that_did_not_declare_chunked_is_left_alone() {
    let raw = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n5\r\nx";
    assert_eq!(
        split_response(raw).expect("a response").body,
        "5\r\nx",
        "what looks like a chunk header is body when nothing declared chunking"
    );
}

#[test]
fn the_transfer_encoding_header_is_matched_case_insensitively_and_in_a_list() {
    for head in [
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked",
    ] {
        assert!(head_is_chunked(head), "{head:?}");
    }
    for head in [
        "HTTP/1.1 200 OK\r\nContent-Length: 3",
        // The status line is skipped, so a reason phrase cannot match.
        "HTTP/1.1 200 Transfer-Encoding: chunked",
        "HTTP/1.1 200 OK\r\nX-Note: Transfer-Encoding: chunked",
    ] {
        assert!(!head_is_chunked(head), "{head:?}");
    }
}

/// SF-6. The whole-body path decoded UTF-8 BEFORE de-chunking: `speak` ran
/// `from_utf8_lossy` over the raw bytes, chunk framing and all, so a
/// character split by a chunk boundary became two replacement characters,
/// the chunk's byte count stopped matching, and the call died blaming the hub
/// ("the body is not the UTF-8 it claimed to be") for a client-side ordering
/// bug. Driven through `speak`, the real caller, because the old test fed
/// `dechunk` a hand-built `&str` that `speak` could never produce.
#[test]
fn a_character_split_across_two_chunks_survives_the_whole_body_path() {
    let text = "event: message\ndata: {\"friendly_name\":\"Zürich\"}\n\n";
    let bytes = text.as_bytes();
    // Cut between the two bytes of "ü".
    let cut = text.find('ü').expect("the fixture has one") + 1;
    let (a, b) = bytes.split_at(cut);
    let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for chunk in [a, b] {
        raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        raw.extend_from_slice(chunk);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");

    let got = block_on(speak(
        HalfClosing {
            body: raw,
            kind: std::io::ErrorKind::UnexpectedEof,
        },
        "hub.example.com",
        443,
        "POST /mcp HTTP/1.1\r\n\r\n",
    ))
    .expect("the bytes arrived");
    let r = split_response(&got).expect("a split character is not a malformed body");
    assert_eq!(r.body, text, "de-chunk the bytes, THEN decode them");
}

/// A peer that dies mid-chunk leaves what arrived, matching `speak`'s own
/// tolerance for a half-close. Refusing here would undo that.
#[test]
fn a_truncated_chunked_body_yields_what_arrived() {
    assert_eq!(dechunk(b"5\r\nhel").expect("partial"), b"hel");
    assert_eq!(
        dechunk(b"5\r\nhello\r\n6\r\n wor").expect("partial"),
        b"hello wor"
    );
}

#[test]
fn an_unreadable_chunk_size_is_an_error_not_a_guess() {
    let e = dechunk(b"zz\r\nxx").expect_err("zz is not hex");
    assert!(e.contains("chunk size"), "{e}");
}

/// A chunk size is a BYTE count, so a chunk may end inside a character —
/// that is legal framing, not a malformed body. (This used to assert an
/// error, which is the SF-6 bug stated as a requirement.) Bytes that are not
/// UTF-8 at all still cannot panic: they decode lossily at the end.
#[test]
fn a_chunk_may_end_inside_a_character_and_invalid_bytes_never_panic() {
    // "ä" is 0xC3 0xA4: one byte per chunk.
    let joined = dechunk(b"1\r\n\xC3\r\n1\r\n\xA4\r\n0\r\n\r\n").expect("legal framing");
    assert_eq!(joined, "ä".as_bytes());
    let r = split_response(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n\xFF\r\n0\r\n\r\n",
    )
    .expect("a response");
    assert_eq!(r.body, "\u{FFFD}");
}

// --- the trust store, loaded off the runtime and never cached as a failure ---

/// `load_native_certs` is blocking file I/O — and on macOS a keychain query,
/// which can be slow or prompt. Calling it straight from an `async fn` parks a
/// runtime worker, and there are only as many workers as cores.
///
/// A source assertion rather than a behavioural one because the defect is a
/// *thread*, and nothing in-process can observe which thread a blocking read
/// happened on. It is the same shape as
/// `startup::tests::lib_rs_cannot_start_a_background_task_behind_this_modules_back`,
/// and it catches the regression that matters: someone calling the builder
/// directly again.
#[test]
fn the_trust_store_is_never_loaded_on_a_runtime_worker() {
    let src = include_str!("remote.rs");
    let calls: Vec<&str> = src
        .lines()
        .filter(|l| l.contains("build_tls_connector") && !l.trim_start().starts_with("//"))
        .collect();
    assert_eq!(
        calls.len(),
        2,
        "expected exactly the definition and the spawn_blocking call, got: {calls:#?}"
    );
    assert!(
        calls
            .iter()
            .any(|l| l.contains("spawn_blocking(build_tls_connector)")),
        "the only call must go through spawn_blocking: {calls:#?}"
    );
    assert!(
        !src.contains("load_native_certs()") || src.matches("load_native_certs()").count() == 1,
        "the platform trust store is read in one place only"
    );
}

/// The `OnceLock` holds a `TlsConnector`, not a `Result<TlsConnector, _>`.
///
/// That distinction is the whole of the second half of the finding: a trust
/// store that was momentarily unreadable — a keychain still locked just after
/// login, a profile not yet mounted — used to poison every https call for the
/// life of the process, so the app had to be restarted to recover from a
/// condition that had already cleared. A cell that cannot hold an error cannot
/// cache one; the type is the guarantee, and this test is what stops the type
/// quietly widening again.
#[test]
fn a_failed_trust_store_read_is_not_remembered() {
    let src = include_str!("remote.rs");
    assert!(
        src.contains("static CONNECTOR: std::sync::OnceLock<tokio_rustls::TlsConnector>"),
        "CONNECTOR must not be a OnceLock<Result<..>>: a cached failure \
         survives the condition that caused it, and only a restart clears it"
    );
}
