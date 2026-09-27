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
use crate::backend::connection::{
    ConnectionReporter, ConnectionView, HubConnection, HubConnectionStatus,
};
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

impl Fake {
    fn answer(&self, url: &str, bearer: &str, body: String) -> Result<HubResponse, String> {
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

#[async_trait::async_trait]
impl HubTransport for Fake {
    async fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: String,
    ) -> Result<HubResponse, String> {
        self.answer(url, bearer, body)
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

/// `restore_host_sessions` and `discover_lost_sessions` run a batch on the
/// hub that outlasts an ordinary exchange: the desktop must wait for the
/// hub's own 300 s cap rather than report a failure while the hub is still
/// restoring — which invited a retry that raced the first call. They earn
/// that bound by being `Deadline::Lifecycle` tools, so the rule is checked
/// where the bound is computed rather than through a transport hook of their
/// own (see `the_client_timeout_dominates_the_hub_deadline_for_every_routed_tool`).
#[test]
fn the_restore_and_discover_calls_get_the_hubs_full_batch_deadline() {
    for tool in ["restore_host_sessions", "discover_lost_sessions"] {
        assert!(
            call_timeout(tool) >= std::time::Duration::from_secs(300),
            "{tool}: client bound {:?} gives up before the hub's 300 s cap",
            call_timeout(tool)
        );
        assert!(
            call_timeout(tool) > call_timeout("list_hosts"),
            "{tool} must outlast an ordinary read"
        );
    }
}

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

/// The reads that still have a method here: the ones whose arguments are
/// more than their command's own struct, or that the event bridge's resync
/// shares with the command. Everything else routes straight from its
/// argument struct and is driven through its command in `tests_routing.rs`,
/// which is the stronger place to assert it — that path also proves the
/// command reaches the hub arm at all.
#[test]
fn each_typed_read_names_its_tool_and_arguments() {
    // (what we call, the tool it must reach, the arguments it must carry)
    let fake = Fake::answering(Ok(ok("[]")));
    let _ = block_on(backend(&fake).list_hosts());
    assert_eq!(fake.only_call().0, "list_hosts");

    // Full rows and no cap: the desktop draws the whole tree, where the
    // tool's own defaults (slim, one page) are shaped for an agent.
    let fake = Fake::answering(Ok(ok(r#"{"total":0,"worktrees":[]}"#)));
    let worktrees = block_on(backend(&fake).list_worktrees(Some(7))).expect("worktrees");
    assert!(worktrees.is_empty());
    assert_eq!(
        fake.only_call(),
        (
            "list_worktrees".into(),
            json!({"project_id": 7, "summary": false, "limit": 0})
        )
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

/// The same guard for `session_conversations`: its rows carry five optional
/// columns (an unfinished conversation has neither `ended_at` nor
/// `end_reason`), so a hub answering with `ok_json_compact` sends them absent.
#[test]
fn a_null_stripped_conversation_row_survives_the_round_trip() {
    let original = fleet_core::store::ConversationRow {
        id: 3,
        session_id: 12,
        claude_session_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into(),
        transcript_path: None,
        started_at: 1_725_000_000,
        ended_at: None,
        start_source: "clear".into(),
        end_reason: None,
        model: None,
        first_prompt: None,
        turns: 0,
        compactions: 0,
        current: true,
    };
    let mut v = serde_json::to_value(&original).expect("serialise");
    strip_nulls(&mut v);
    assert_eq!(
        v.get("end_reason"),
        None,
        "the fixture must really be stripped"
    );

    let payload = serde_json::to_string(&json!([v])).unwrap();
    let fake = Fake::answering(Ok(ok(&payload)));
    let rows = block_on(backend(&fake).session_conversations(12, 50)).expect("rows");
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
        row_version: 0,
        prompt_submit_seq: 0,
        usage: fleet_core::store::SessionUsage {
            usage_input_tokens: 1200,
            usage_output_tokens: 800,
            usage_cache_write_tokens: 0,
            usage_cache_read_tokens: 0,
            usage_cost_micros: 31500,
            usage_model: Some("claude-opus-5".into()),
            usage_updated_at: Some(1_757_003_500),
        },
        context: fleet_core::store::SessionContext::default(),
        pending_input: None,
        work: None,
        work_rejected: vec![],
        work_suggested: None,
        org_id: None,
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

/// A hub that serves no such tool is the shape an OLDER hub takes: every
/// tool this app learns to call is a tool some hub out there has not got
/// yet. It gets its own code so a caller can tell "your hub is behind" from
/// an internal error and degrade instead of showing a failure.
#[test]
fn a_jsonrpc_protocol_error_is_reported_as_one() {
    let fake = Fake::answering(Ok(sse(json!({
        "jsonrpc": "2.0", "id": 1,
        "error": { "code": -32601, "message": "tool not found: list_sessions" },
    }))));
    let err = block_on(backend(&fake).list_sessions(false)).expect_err("an error");
    assert_eq!(err.code, codes::E_HUB_PROTOCOL);
    assert!(err.message.contains("tool not found"), "{}", err.message);
}

// --- the wire-contract gate --------------------------------------------------
//
// The event bridge refuses to APPLY a row from a hub whose wire contract is
// outside this build's range (`super::contract`), but a call made from a
// command walks straight past that: its answer is deserialised into the same
// row types, with the same `#[serde(default)]` on forty-six fields, so a
// renamed column arrives as a silent default — the exact failure the check
// exists to prevent. These pin the other half: while the last thing this
// window learned about the hub is a skew, no call is made at all.
//
// The state is the REAL [`HubConnectionStatus`], reported into exactly as the
// bridge reports into it. One value plays both parts on purpose: a second
// copy of "where the connection stands" is a copy that can drift.
//
// What the gate reads is the last CONTRACT VERDICT, not the current state —
// see [`a_dropped_socket_after_a_skew_does_not_reopen_the_gate`] for why the
// difference is the whole point.

/// A sink that throws the event away — these are about what the status
/// REMEMBERS, which is what the gate reads.
struct Silent;

impl crate::backend::events::RemoteEventSink for Silent {
    fn emit_remote(&self, _name: &'static str, _payload: Value) {}
}

fn link(state: HubConnection) -> Arc<HubConnectionStatus> {
    let status = Arc::new(HubConnectionStatus::remote(Arc::new(Silent), &cfg().token));
    status.report(state);
    status
}

fn watched(fake: &Arc<Fake>, status: &Arc<HubConnectionStatus>) -> HubBackend {
    HubBackend::with_transport(cfg(), fake.clone())
        .watching(Arc::clone(status) as Arc<dyn ConnectionView>)
}

/// Nothing was sent. The point is not that the call failed — it is that
/// nothing came back to deserialise.
fn nothing_was_sent(fake: &Arc<Fake>) {
    let seen = fake.seen.lock().unwrap();
    assert!(
        seen.is_empty(),
        "a hub this build cannot read was called anyway: {seen:?}"
    );
}

#[test]
fn a_hub_too_old_is_refused_before_the_transport_is_touched() {
    let fake = Fake::answering(Ok(ok("[]")));
    let status = link(HubConnection::HubTooOld {
        hub_contract: 1,
        min_contract: 3,
    });
    let err = block_on(watched(&fake, &status).list_sessions(false))
        .expect_err("a hub whose rows this build cannot read must not be read");

    assert_eq!(err.code, codes::E_HUB_CONTRACT);
    // The banner's own sentence for this state (`src/lib/hub_connection.ts`),
    // so the toast and the banner say one thing rather than two.
    assert!(
        err.message
            .contains("wire contract is revision 1, older than the 3 this app requires"),
        "{}",
        err.message
    );
    assert!(err.message.contains("Update the hub."), "{}", err.message);
    assert_eq!(
        err.details,
        Some(json!({ "hub_contract": 1, "min_contract": 3 })),
        "the numbers ride along structurally too"
    );
    nothing_was_sent(&fake);
}

#[test]
fn a_hub_too_new_is_refused_and_names_the_other_side() {
    let fake = Fake::answering(Ok(ok("[]")));
    let status = link(HubConnection::HubTooNew {
        hub_contract: 9,
        max_contract: 1,
    });
    let err = block_on(watched(&fake, &status).list_hosts()).expect_err("no read from here");

    assert_eq!(err.code, codes::E_HUB_CONTRACT);
    assert!(
        err.message
            .contains("wire contract is revision 9, newer than the 1 this app understands"),
        "{}",
        err.message
    );
    assert!(err.message.contains("Update this app."), "{}", err.message);
    assert_eq!(
        err.details,
        Some(json!({ "hub_contract": 9, "max_contract": 1 }))
    );
    nothing_was_sent(&fake);
}

/// A mutation is gated for the same reason a read is: its return value is
/// deserialised into a row and optimistically merged into the stores.
#[test]
fn a_mutation_is_gated_too() {
    let fake = Fake::answering(Ok(ok("{}")));
    let status = link(HubConnection::HubTooOld {
        hub_contract: 0,
        min_contract: 2,
    });
    let err = block_on(watched(&fake, &status).repair_session(7)).expect_err("no mutation either");
    assert_eq!(err.code, codes::E_HUB_CONTRACT);
    nothing_was_sent(&fake);
}

/// A window that has never been told a hub's revision calls in every state.
///
/// `connecting` is the case that matters: no handshake has completed on this
/// launch, so nothing is known, and gating it would make every startup list
/// wait on `GET /events`. The two down states say the connection is gone, not
/// that the hub's rows are unreadable, and they keep the behaviour they had
/// before this gate existed.
#[test]
fn a_hub_that_has_never_been_judged_is_called_in_every_state() {
    for state in [
        HubConnection::Connecting,
        HubConnection::Connected,
        HubConnection::Reconnecting {
            attempt: 2,
            retry_in_secs: 4,
            reason: "the hub closed the event stream".into(),
        },
        HubConnection::Offline {
            attempt: 1,
            retry_in_secs: 1,
            reason: "connection refused".into(),
        },
    ] {
        let fake = Fake::answering(Ok(ok("[]")));
        let status = link(state.clone());
        let rows: Vec<SessionRow> = block_on(watched(&fake, &status).list_sessions(false))
            .unwrap_or_else(|e| panic!("{state:?} must still reach the hub: {e:?}"));
        assert!(rows.is_empty());
        assert_eq!(fake.seen.lock().unwrap().len(), 1, "{state:?}");
    }
}

/// The verdict outlives the connection state that carried it.
///
/// `/events` and `POST /mcp` are separate sockets. After a skew the bridge
/// ends that connection and loops, and its next iteration reports `Offline`
/// (the open failed) or `Reconnecting` (a stream that ended before its `ready`
/// frame) — neither of which has re-judged anything. A gate that read the
/// CURRENT state would open there, and reads would come back from a hub still
/// known to be incompatible: exactly the case this gate exists to prevent, and
/// a likely one, since a hub whose event stream is down or behind a flapping
/// proxy can still answer `/mcp`.
#[test]
fn a_dropped_socket_after_a_skew_does_not_reopen_the_gate() {
    for skew in [
        HubConnection::HubTooOld {
            hub_contract: 1,
            min_contract: 3,
        },
        HubConnection::HubTooNew {
            hub_contract: 9,
            max_contract: 1,
        },
    ] {
        for after in [
            HubConnection::Offline {
                attempt: 1,
                retry_in_secs: 1,
                reason: "connection refused".into(),
            },
            HubConnection::Reconnecting {
                attempt: 2,
                retry_in_secs: 4,
                reason: "the hub closed the event stream".into(),
            },
        ] {
            let fake = Fake::answering(Ok(ok("[]")));
            let status = link(skew.clone());
            status.report(after.clone());
            let err = block_on(watched(&fake, &status).list_sessions(false)).expect_err(
                "a dropped socket re-judges nothing, so the hub is still the one \
                 this build cannot read",
            );
            assert_eq!(err.code, codes::E_HUB_CONTRACT, "{skew:?} then {after:?}");
            // And the numbers are still the skew's, not the reconnect's.
            assert!(
                err.message.contains("wire contract is revision"),
                "{skew:?} then {after:?}: {}",
                err.message
            );
            nothing_was_sent(&fake);
        }
    }
}

/// The hub was upgraded (or this app was) while the desktop ran: the bridge's
/// next `ready` frame classifies it in range and reports `Connected`, and the
/// gate opens again without a restart.
#[test]
fn a_later_compatible_hello_opens_the_gate_again() {
    let fake = Fake::answering(Ok(ok("[]")));
    let status = link(HubConnection::HubTooNew {
        hub_contract: 9,
        max_contract: 1,
    });
    assert_eq!(
        block_on(watched(&fake, &status).list_sessions(false))
            .expect_err("gated while skewed")
            .code,
        codes::E_HUB_CONTRACT
    );
    nothing_was_sent(&fake);

    status.report(HubConnection::Connected);
    let rows: Vec<SessionRow> =
        block_on(watched(&fake, &status).list_sessions(false)).expect("rows");
    assert!(rows.is_empty());
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

/// The gate is only as real as its wiring: a `HubBackend` nobody handed the
/// status to never refuses anything, and every test above would still pass
/// with the two production call sites left unwatched.
#[test]
fn the_production_hub_backends_are_watched() {
    for (what, src) in [
        ("lib.rs", include_str!("../lib.rs")),
        ("bootstrap/tasks.rs", include_str!("../bootstrap/tasks.rs")),
    ] {
        assert!(
            src.contains(".watching("),
            "{what} builds a hub client that consults no connection state, so a hub \
             whose wire contract this build cannot read is called anyway"
        );
    }
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
        // These two were real leaks: every other external-text path here was
        // scrubbed and these were not, so the claim that "every scrap of
        // text that originates outside this process is now scrubbed" was
        // false. A JSON-RPC protocol error's message…
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

// --- how long a call may take ------------------------------------------------

/// The client must never give up before the hub does: a lifecycle tool the
/// hub bounds at 300 s answered "did not answer" at 30 s while the hub kept
/// creating the session, and the user's retry made two. Every routed tool's
/// client bound is the hub's own deadline plus a margin.
#[test]
fn the_client_timeout_dominates_the_hub_deadline_for_every_routed_tool() {
    let margin = std::time::Duration::from_secs(10);
    for (cmd, verdict) in crate::backend::verdicts::VERDICTS {
        let Some(tool) = verdict.tool() else { continue };
        let hub = fleet_core::mcp::tool_deadline(tool);
        assert!(
            call_timeout(tool) >= hub + margin,
            "{cmd} -> {tool}: client {:?} must be >= hub {:?} + {margin:?}",
            call_timeout(tool),
            hub
        );
    }
    assert!(call_timeout("move_session") >= std::time::Duration::from_secs(15 * 60));
}

/// A timed-out exchange is NOT "unreachable": the hub may well have taken the
/// request. The code and the words say the outcome is unknown.
#[tokio::test(start_paused = true)]
async fn a_timed_out_call_says_the_outcome_is_unknown() {
    struct Silent;
    #[async_trait::async_trait]
    impl HubTransport for Silent {
        async fn post_json(&self, _: &str, _: &str, _: String) -> Result<HubResponse, String> {
            std::future::pending().await
        }
    }
    let b = HubBackend::with_transport(cfg(), Arc::new(Silent));
    let e = b.list_sessions(false).await.expect_err("never answers");
    assert_eq!(e.code, codes::E_HUB_TIMEOUT);
    assert!(e.message.contains("may still complete"), "{}", e.message);
    assert!(!e.message.contains("cl_s3cret-token"), "{}", e.message);
}

/// While the event bridge has already failed twice to even CONNECT, a call is
/// refused at once instead of hanging its full bound: the bridge's reconnect
/// is the probe, and the user's click is not.
#[test]
fn a_call_while_the_link_is_known_offline_is_refused_before_the_transport_is_touched() {
    let fake = Fake::answering(Ok(ok("[]")));
    let status = link(HubConnection::Offline {
        attempt: 2,
        retry_in_secs: 7,
        reason: "connect 127.0.0.1:4180: connection refused".into(),
    });
    let err = block_on(watched(&fake, &status).list_sessions(false)).expect_err("refused");
    assert_eq!(err.code, codes::E_HUB_UNREACHABLE);
    assert!(err.message.contains("retrying in 7"), "{}", err.message);
    nothing_was_sent(&fake);
    // The first failed attempt is not yet a verdict: the call still goes out.
    let status = link(HubConnection::Offline {
        attempt: 1,
        retry_in_secs: 1,
        reason: "connect 127.0.0.1:4180: connection refused".into(),
    });
    let fake = Fake::answering(Ok(ok("[]")));
    block_on(watched(&fake, &status).list_sessions(false)).expect("still tried");
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

/// The breaker is about a hub that cannot be REACHED. A hub that answered —
/// with a 504, a 503 on `/events`, or a close after the head — is reachable,
/// and its `/mcp` socket is a different one from the event stream's: refusing
/// calls there turns one unhappy stream into a window that cannot do anything
/// at all, and the refusal (`E_HUB_UNREACHABLE`) is not even true.
#[test]
fn an_answered_but_unhappy_event_stream_never_refuses_a_call() {
    for reason in [
        "the hub answered 504 Gateway Timeout to GET /events",
        "the hub answered 503 to GET /events: events are not enabled on this server",
        "the hub closed the connection before answering",
        "read from fleet.example.com:443: connection reset by peer",
    ] {
        let fake = Fake::answering(Ok(ok("[]")));
        let status = link(HubConnection::Offline {
            attempt: 3,
            retry_in_secs: 7,
            reason: reason.into(),
        });
        block_on(watched(&fake, &status).list_sessions(false))
            .unwrap_or_else(|e| panic!("{reason:?} must still reach the hub: {e:?}"));
        assert_eq!(fake.seen.lock().unwrap().len(), 1, "{reason:?}");
    }
}

/// `Reconnecting` is a stream that opened and then ended: the hub answered,
/// so it never arms the breaker however many attempts have gone by.
#[test]
fn a_reconnecting_link_never_refuses_a_call() {
    for attempt in [1u32, 2, 9] {
        let fake = Fake::answering(Ok(ok("[]")));
        let status = link(HubConnection::Reconnecting {
            attempt,
            retry_in_secs: 4,
            reason: "connect 127.0.0.1:4180: connection refused".into(),
        });
        block_on(watched(&fake, &status).list_sessions(false))
            .unwrap_or_else(|e| panic!("attempt {attempt} must still reach the hub: {e:?}"));
        assert_eq!(fake.seen.lock().unwrap().len(), 1, "attempt {attempt}");
    }
}

/// A timed-out call says whether the hub may have CHANGED anything. Only a
/// mutation leaves an unknown outcome; a read that never answered changed
/// nothing, and the frontend's fleet-wide re-fetch (`fleet:outcome-unknown`)
/// is itself made of reads, so broadcasting for one amplifies.
#[tokio::test(start_paused = true)]
async fn a_timed_out_read_is_not_outcome_unknown_but_a_timed_out_mutation_is() {
    struct Silent;
    #[async_trait::async_trait]
    impl HubTransport for Silent {
        async fn post_json(&self, _: &str, _: &str, _: String) -> Result<HubResponse, String> {
            std::future::pending().await
        }
    }
    let b = HubBackend::with_transport(cfg(), Arc::new(Silent));
    let unknown = |e: &fleet_core::ipc_error::IpcError| {
        e.details
            .as_ref()
            .and_then(|d| d.get("outcome_unknown"))
            .cloned()
    };
    let e = b
        .call_text("list_sessions", json!({}))
        .await
        .expect_err("never answers");
    assert_eq!(e.code, codes::E_HUB_TIMEOUT);
    assert_eq!(unknown(&e), Some(json!(false)), "{:?}", e.details);
    let e = b
        .call_text("kill_session", json!({ "session_id": 1 }))
        .await
        .expect_err("never answers");
    assert_eq!(e.code, codes::E_HUB_TIMEOUT);
    assert_eq!(unknown(&e), Some(json!(true)), "{:?}", e.details);
}

/// The table above says which duration each tool gets; this says the bound is
/// real. A hub that accepts the request and then says nothing must still end
/// the call — the timeout moved out of the transport into
/// [`HubBackend::call_text`], and a timeout nobody applies is worse than
/// none, because the window would wait forever.
///
/// `start_paused` (the `test-util` feature already in this crate's
/// dev-dependencies) auto-advances the clock whenever every task is idle, so
/// this reaches the deadline without waiting for it.
#[tokio::test(start_paused = true)]
async fn a_hub_that_accepts_the_request_and_then_says_nothing_still_ends_the_call() {
    struct Silent;
    #[async_trait::async_trait]
    impl HubTransport for Silent {
        async fn post_json(&self, _: &str, _: &str, _: String) -> Result<HubResponse, String> {
            std::future::pending().await
        }
    }
    let b = HubBackend::with_transport(cfg(), Arc::new(Silent));
    let e = b
        .list_sessions(false)
        .await
        .expect_err("a hub that never answers cannot produce rows");
    assert_eq!(e.code, codes::E_HUB_TIMEOUT);
    assert!(e.message.contains("no answer within"), "{}", e.message);
    assert!(!e.message.contains("cl_s3cret-token"), "{}", e.message);
}
