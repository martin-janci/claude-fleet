# MCP transport and tool-contract hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the embedded MCP control API stateless (no session id to lose), advertise the current protocol revision, return tool failures as `is_error` tool results with the `E_*` code preserved, and bound every tool call with a per-class wall clock.

**Architecture:** All changes sit in `src-tauri/src/mcp/`. `mod.rs` builds the rmcp `StreamableHttpService` (transport config); `tools/mod.rs` owns the hand-written `ServerHandler::call_tool` gate where error translation and the timeout wrap go; `tools/support.rs` holds the pure helpers (`mcp_err`, the new `tool_error_result`, `tool_deadline`, `timeout_result`, `bounded`) so they are unit-testable without a transport. The end-to-end path is covered by an in-process axum test in `mod.rs` that speaks real JSON-RPC over HTTP.

**Tech Stack:** Rust, rmcp 1.7 (`transport-streamable-http-server`), axum 0.8, tokio. Tests: `cargo test` (needs the Tauri sysroot on this host — see Global Constraints).

**Spec:** `docs/superpowers/specs/2026-09-14-mcp-transport-and-contract-design.md`

## Global Constraints

- Backend build on `claude-fleet-trn`: before any `cargo` command run
  `source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu`, from `src-tauri/`.
- No `#[tool(...)]` description changes → `docs/control-api-reference.md` must stay byte-identical (`reference_is_current` test).
- No `eprintln!` (repo lint `no_eprintln_tests.rs`); use `tracing`.
- Never hold the `Mutex<Store>` guard across an `.await`.
- Keep `json_response` at its default `false` (SSE framing carries the 15 s keep-alive that long polls need).
- Commit messages: Conventional Commits, ending with the line
  `Claude-Session: https://claude.ai/code/session_01LnFcQ2QC6QszexSkn2embn`.
- Branch: `feat/mcp-hooks-modernize` (already checked out in this worktree, based on `origin/main`).

---

### Task 1: Stateless transport + protocol `2025-11-25`

**Files:**
- Modify: `src-tauri/src/mcp/mod.rs` (imports near line 19; `start()` around lines 226-241; tests module at the end)
- Modify: `src-tauri/src/mcp/tools/mod.rs:167` (`get_info`)

**Interfaces:**
- Produces: `pub(crate) fn streamable_service(tools: FleetTools, cancel: CancellationToken) -> StreamableHttpService<FleetTools, NeverSessionManager>` in `mcp/mod.rs`, used by `start()` and by tests.

- [ ] **Step 1: Write the failing integration test** — append to the `tests` module at the bottom of `src-tauri/src/mcp/mod.rs`:

```rust
    /// The control API is stateless streamable HTTP: no `Mcp-Session-Id` is
    /// issued or required, every POST stands alone, `GET /mcp` is not served,
    /// and the server advertises MCP 2025-11-25.
    #[tokio::test]
    async fn mcp_is_stateless_and_advertises_latest_protocol() {
        use std::net::Ipv4Addr;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
        };
        let tools = FleetTools::new(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
        );
        let service = streamable_service(tools, CancellationToken::new());
        let app = build_app(axum::routing::any_service(service), hook_state, auth_state);

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        async fn round_trip(addr: std::net::SocketAddr, req: &str) -> String {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(req.as_bytes()).await.unwrap();
            let mut buf = Vec::new();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(800), async {
                let mut tmp = [0u8; 8192];
                loop {
                    match s.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        Err(_) => break,
                    }
                }
            })
            .await;
            String::from_utf8_lossy(&buf).into_owned()
        }
        let post = |body: &str| {
            format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json, \
                 text/event-stream\r\nContent-Type: application/json\r\n\
                 Authorization: Bearer s3cret\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            )
        };

        // initialize without any session header → 200, latest protocol, and
        // no Mcp-Session-Id handed back.
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let r = round_trip(addr, &post(init)).await;
        assert!(r.contains("200 OK"), "initialize:\n{r}");
        assert!(
            r.contains(r#""protocolVersion":"2025-11-25""#),
            "must advertise 2025-11-25:\n{r}"
        );
        assert!(
            !r.to_ascii_lowercase().contains("mcp-session-id"),
            "stateless: no session id must be issued:\n{r}"
        );

        // A second, unrelated POST (no session header) is served on its own.
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let r = round_trip(addr, &post(list)).await;
        assert!(r.contains("200 OK"), "tools/list:\n{r}");
        assert!(r.contains(r#""name":"list_sessions""#), "tools listed:\n{r}");

        // GET /mcp (the stateful SSE channel) is not part of the contract.
        let get = "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\
                   Authorization: Bearer s3cret\r\nConnection: close\r\n\r\n";
        let r = round_trip(addr, get).await;
        assert!(r.contains("405"), "GET must be 405 in stateless mode:\n{r}");
    }
```

- [ ] **Step 2: Run it to confirm it fails to compile** (no `streamable_service` yet):

```bash
cd src-tauri && source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu
cargo test mcp_is_stateless 2>&1 | tail -20
```
Expected: `error[E0425]: cannot find function \`streamable_service\``.

- [ ] **Step 3: Implement.** In `src-tauri/src/mcp/mod.rs` change the rmcp import (line ~19) from `session::local::LocalSessionManager` to `session::never::NeverSessionManager`, then add the factory above `start()` and use it inside `start()`:

```rust
/// The rmcp streamable-HTTP service in **stateless** mode: every POST is a
/// self-contained JSON-RPC exchange served by a fresh `FleetTools` clone, no
/// `Mcp-Session-Id` is issued or required, and `GET`/`DELETE` are refused
/// (405). The server never sends server-initiated messages (tools only), so a
/// session bought nothing and cost a reconnect after every app restart, port
/// or token change, or tunnel bounce. Responses keep SSE framing
/// (`json_response` default `false`) so the 15 s keep-alive still flows on
/// long polls (`wait_for_session`, `run_prompt`) through the reverse tunnel.
pub(crate) fn streamable_service(
    tools: FleetTools,
    cancel: CancellationToken,
) -> StreamableHttpService<FleetTools, NeverSessionManager> {
    StreamableHttpService::new(
        move || Ok(tools.clone()),
        NeverSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_cancellation_token(cancel),
    )
}
```

and in `start()` replace

```rust
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default()
                .with_cancellation_token(serve_shutdown.child_token()),
        );
```

with

```rust
        let service = streamable_service(tools, serve_shutdown.child_token());
```

In `src-tauri/src/mcp/tools/mod.rs` `get_info()` replace
`.with_protocol_version(ProtocolVersion::V_2024_11_05)` with
`.with_protocol_version(ProtocolVersion::LATEST)` and add a one-line comment:
`// 2025-11-25; rmcp negotiates down for a client that asks for an older known revision.`

- [ ] **Step 4: Run the test and the whole mcp module:**

```bash
cargo test mcp:: 2>&1 | grep -E "^test result|FAILED|panicked"
```
Expected: all pass (the existing `mcp_and_hook_routes_serve_behind_shared_auth` still uses the fake `MCP_OK` service and is unaffected).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/mcp/mod.rs src-tauri/src/mcp/tools/mod.rs
git commit -m "feat(mcp): stateless streamable HTTP and protocol 2025-11-25

No Mcp-Session-Id is issued or required: an app restart, port/token
change or tunnel bounce no longer leaves clients holding a dead session
(404 until reconnect). SSE framing stays for the keep-alive on long polls.

Claude-Session: https://claude.ai/code/session_01LnFcQ2QC6QszexSkn2embn"
```

---

### Task 2: Tool failures as `is_error` tool results

**Files:**
- Modify: `src-tauri/src/mcp/tools/support.rs:20-80` (`to_mcp_err`, `mcp_err`; add `tool_error_result`)
- Modify: `src-tauri/src/mcp/tools/mod.rs:125-145` (`call_tool`)
- Test: `src-tauri/src/mcp/tools/tests.rs` (unit), `src-tauri/src/mcp/mod.rs` tests (HTTP)

**Interfaces:**
- Produces: `pub(super) fn tool_error_result(e: McpError) -> Result<CallToolResult, McpError>` — `Ok(is_error result)` when `e.data` carries a `code` string (our `mcp_err` shape), `Err(e)` otherwise (rmcp protocol errors).
- `mcp_err(code, message, data)` now yields `McpError { message: "CODE: message", data: Some({"code": CODE, "details": data|null}) }`.

- [ ] **Step 1: Write the failing unit tests** — append to `src-tauri/src/mcp/tools/tests.rs`:

```rust
// ---- tool errors become is_error results (spec §3) ----

#[test]
fn mcp_err_carries_the_code_in_data() {
    let e = mcp_err("E_NOTFOUND", "no such session", None);
    assert_eq!(e.message, "E_NOTFOUND: no such session");
    assert_eq!(e.data.as_ref().unwrap()["code"], "E_NOTFOUND");
    assert!(e.data.as_ref().unwrap()["details"].is_null());

    let d = serde_json::json!({ "candidates": [1, 2] });
    let e = to_mcp_err(IpcError::new("E_AMBIGUOUS", "two match").with_details(d.clone()));
    assert_eq!(e.data.as_ref().unwrap()["code"], "E_AMBIGUOUS");
    assert_eq!(e.data.as_ref().unwrap()["details"], d);
}

#[test]
fn tool_error_result_turns_coded_errors_into_is_error_results() {
    let e = mcp_err("E_FORBIDDEN", "readonly token", None);
    let r = tool_error_result(e).expect("coded error is a tool result");
    assert_eq!(r.is_error, Some(true));
    assert_eq!(text_of(&r.content[0]), "E_FORBIDDEN: readonly token");
    let sc = r.structured_content.unwrap();
    assert_eq!(sc["code"], "E_FORBIDDEN");
    assert_eq!(sc["message"], "readonly token");
    assert!(sc["details"].is_null());
}

#[test]
fn tool_error_result_keeps_protocol_errors_as_errors() {
    // rmcp's own "tool not found" / bad-arguments errors carry no code and
    // must stay JSON-RPC errors.
    let e = McpError::invalid_params("tool not found", None);
    let err = tool_error_result(e).expect_err("protocol error passes through");
    assert_eq!(err.message, "tool not found");
}
```

Check how `IpcError` attaches details: `grep -n "fn with_details\|pub details" src-tauri/src/ipc_error.rs`. If there is no `with_details`, construct it as `IpcError { code: "E_AMBIGUOUS".into(), message: "two match".into(), details: Some(d.clone()) }` (adjust to the struct's field names).

- [ ] **Step 2: Run to confirm failure:**

```bash
cargo test tool_error_result 2>&1 | tail -15
```
Expected: `cannot find function \`tool_error_result\``.

- [ ] **Step 3: Implement in `src-tauri/src/mcp/tools/support.rs`.** Replace `to_mcp_err` and `mcp_err` with:

```rust
/// Key under which [`mcp_err`] stores the `E_*` code in `McpError::data`.
/// `ServerHandler::call_tool` reads it back to tell a tool-execution error
/// (→ `CallToolResult { is_error: true }`) from an rmcp protocol error.
const ERR_CODE_KEY: &str = "code";

/// Map a backend `IpcError` to an MCP tool error, preserving the `E_*` code.
/// Structured `details` (e.g. `E_AMBIGUOUS` candidates) ride along as the
/// error's data so a caller can act on them without parsing prose.
pub(super) fn to_mcp_err(e: IpcError) -> McpError {
    mcp_err(&e.code, e.message, e.details)
}

/// Build a coded tool error: message `CODE: message`, data
/// `{ "code": CODE, "details": <data or null> }`.
pub(super) fn mcp_err(
    code: &str,
    message: impl std::fmt::Display,
    data: Option<serde_json::Value>,
) -> McpError {
    let details = data.unwrap_or(serde_json::Value::Null);
    let msg = match &details {
        serde_json::Value::Null => format!("{code}: {message}"),
        d => format!("{code}: {message} {d}"),
    };
    McpError::internal_error(
        msg,
        Some(serde_json::json!({ ERR_CODE_KEY: code, "details": details })),
    )
}

/// Translate a router outcome for the wire (MCP spec: tool *execution*
/// failures are a `CallToolResult` with `is_error: true`, which the client
/// shows the model as the tool's output so it can correct the call; JSON-RPC
/// errors are for *protocol* failures such as an unknown tool or malformed
/// arguments). A coded error (built by [`mcp_err`] / [`to_mcp_err`], or by
/// the readonly / admin / no-caller gates) becomes the result; anything else
/// — rmcp's own `invalid_params` — passes through unchanged.
pub(super) fn tool_error_result(e: McpError) -> Result<CallToolResult, McpError> {
    let Some(code) = e
        .data
        .as_ref()
        .and_then(|d| d.get(ERR_CODE_KEY))
        .and_then(|c| c.as_str())
        .map(str::to_string)
    else {
        return Err(e);
    };
    let details = e
        .data
        .as_ref()
        .and_then(|d| d.get("details"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    // `message` is "CODE: message[ details]"; keep the human line as-is for
    // the text block and the bare message for the structured field.
    let bare = e
        .message
        .strip_prefix(&format!("{code}: "))
        .unwrap_or(&e.message)
        .to_string();
    let bare = match &details {
        serde_json::Value::Null => bare,
        d => bare.strip_suffix(&format!(" {d}")).unwrap_or(&bare).to_string(),
    };
    let mut r = CallToolResult::error(vec![Content::text(e.message.to_string())]);
    r.structured_content = Some(serde_json::json!({
        "code": code,
        "message": bare,
        "details": details,
    }));
    Ok(r)
}
```

Then in `src-tauri/src/mcp/tools/mod.rs` `call_tool`, replace the last line
`self.tool_router.call(tcc).await` with:

```rust
        match self.tool_router.call(tcc).await {
            Ok(result) => Ok(result),
            Err(e) => tool_error_result(e),
        }
```

and wrap the three pre-router gates so their coded errors take the same path — change

```rust
        let caller = caller_from_context(&context)
            .ok_or_else(|| mcp_err("E_FORBIDDEN", "request carries no caller identity", None))?;
        let tool = request.name.to_string();
        // Audit first so refused calls are on the timeline too.
        persist_audit(&self.store, &tool, request.arguments.as_ref(), &caller);
        enforce_mode(&caller, &tool)?;
        enforce_admin(&caller, &tool)?;
```

to

```rust
        let caller = match caller_from_context(&context) {
            Some(c) => c,
            None => {
                return tool_error_result(mcp_err(
                    "E_FORBIDDEN",
                    "request carries no caller identity",
                    None,
                ))
            }
        };
        let tool = request.name.to_string();
        // Audit first so refused calls are on the timeline too.
        persist_audit(&self.store, &tool, request.arguments.as_ref(), &caller);
        if let Err(e) = enforce_mode(&caller, &tool).and_then(|()| enforce_admin(&caller, &tool)) {
            return tool_error_result(e);
        }
```

Existing tests that assert `enforce_mode(...)` returns `Err` with message starting `E_FORBIDDEN` (`forbidden(e)` helper) keep passing: the gates still return `McpError`; only `call_tool` translates.

- [ ] **Step 4: Add the HTTP-level assertion** — in `src-tauri/src/mcp/mod.rs` append a test next to the one from Task 1 (copy its setup; the helpers are local fns so duplicate the `round_trip` / `post` closures verbatim):

```rust
    /// A tool that fails in the service layer answers with a tool RESULT
    /// carrying `isError: true` and the `E_*` code, not a JSON-RPC error.
    #[tokio::test]
    async fn tools_call_failure_is_an_is_error_result() {
        // …same server setup as mcp_is_stateless_and_advertises_latest_protocol…
        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"set_friendly_name","arguments":{"session_id":999999,"friendly_name":"x"}}}"#;
        let r = round_trip(addr, &post(call)).await;
        assert!(r.contains("200 OK"), "tools/call:\n{r}");
        assert!(r.contains(r#""isError":true"#), "must be a tool result:\n{r}");
        assert!(r.contains("E_NOTFOUND"), "code preserved:\n{r}");
        assert!(
            !r.contains(r#""error":{"#),
            "must not be a JSON-RPC error:\n{r}"
        );
    }
```

To avoid duplicating ~40 lines of setup, first lift the setup of Task 1's test into a test helper in the same module:

```rust
    /// Real FleetTools behind the real stateless service, bound on an
    /// ephemeral loopback port. Returns the address; requests use the
    /// master token `s3cret`.
    async fn serve_real_tools() -> std::net::SocketAddr { /* the setup block from Task 1, ending in `addr` */ }
    async fn round_trip(addr: std::net::SocketAddr, req: &str) -> String { /* as in Task 1 */ }
    fn post_mcp(body: &str) -> String { /* the `post` closure from Task 1 */ }
```

and make both tests call them.

- [ ] **Step 5: Run the tests:**

```bash
cargo test mcp:: 2>&1 | grep -E "^test result|FAILED|panicked"
```
Expected: all pass. Also `cargo test reference_is_current` still passes (no description changed).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/mcp/tools/support.rs src-tauri/src/mcp/tools/mod.rs src-tauri/src/mcp/tools/tests.rs src-tauri/src/mcp/mod.rs
git commit -m "feat(mcp): return tool failures as is_error results with the E_* code

Per the MCP spec, tool-execution failures are CallToolResult
{ is_error: true } (shown to the model as the tool's output), while
JSON-RPC errors stay reserved for unknown tools and malformed arguments.
The text block keeps the documented 'E_CODE: message' line; structured
content carries { code, message, details }.

Claude-Session: https://claude.ai/code/session_01LnFcQ2QC6QszexSkn2embn"
```

---

### Task 3: Per-class wall clock on every tool call

**Files:**
- Modify: `src-tauri/src/mcp/tools/support.rs` (add `tool_deadline`, `timeout_result`, `bounded`)
- Modify: `src-tauri/src/mcp/tools/mod.rs` (`call_tool`)
- Test: `src-tauri/src/mcp/tools/tests.rs`

**Interfaces:**
- Produces: `pub(super) fn tool_deadline(tool: &str) -> std::time::Duration`;
  `pub(super) fn timeout_result(tool: &str, limit: std::time::Duration) -> CallToolResult`;
  `pub(super) async fn bounded<F>(tool: &str, limit: Duration, fut: F) -> Result<CallToolResult, McpError> where F: Future<Output = Result<CallToolResult, McpError>>`.

- [ ] **Step 1: Write the failing tests** — append to `src-tauri/src/mcp/tools/tests.rs`:

```rust
// ---- per-tool wall clock (spec §4) ----

#[test]
fn every_router_tool_is_explicitly_classified() {
    // A new tool must be placed in a class on purpose; the 60 s default is
    // for the wire, not a way to skip the decision.
    let listed: Vec<String> = FleetTools::tool_router_for_doc()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(!listed.is_empty());
    for name in &listed {
        assert!(
            LONG_POLL_TOOLS.contains(&name.as_str())
                || LIFECYCLE_TOOLS.contains(&name.as_str())
                || QUICK_TOOLS.contains(&name.as_str()),
            "tool {name} is not classified in support.rs"
        );
    }
    for name in LONG_POLL_TOOLS.iter().chain(LIFECYCLE_TOOLS).chain(QUICK_TOOLS) {
        assert!(listed.iter().any(|l| l == name), "{name} is classified but not served");
    }
}

#[test]
fn tool_deadline_uses_the_documented_caps() {
    use std::time::Duration;
    assert_eq!(tool_deadline("wait_for_session"), Duration::from_secs(660));
    assert_eq!(tool_deadline("run_prompt"), Duration::from_secs(660));
    assert_eq!(tool_deadline("new_session"), Duration::from_secs(300));
    assert_eq!(tool_deadline("provision_hosts"), Duration::from_secs(300));
    assert_eq!(tool_deadline("list_sessions"), Duration::from_secs(60));
    assert_eq!(tool_deadline("not_a_tool"), Duration::from_secs(60));
}

#[test]
fn timeout_result_is_a_coded_is_error_result() {
    let r = timeout_result("new_session", std::time::Duration::from_secs(300));
    assert_eq!(r.is_error, Some(true));
    assert_eq!(
        text_of(&r.content[0]),
        "E_TIMEOUT: new_session exceeded its 300 s limit; the call may have partially completed"
    );
    let sc = r.structured_content.unwrap();
    assert_eq!(sc["code"], "E_TIMEOUT");
    assert_eq!(sc["tool"], "new_session");
    assert_eq!(sc["limit_secs"], 300);
}

#[tokio::test]
async fn bounded_turns_a_hung_call_into_the_timeout_result() {
    let hung = std::future::pending::<Result<CallToolResult, McpError>>();
    let r = bounded("list_hosts", std::time::Duration::from_millis(10), hung)
        .await
        .expect("timeout is a result, not an error");
    assert_eq!(r.is_error, Some(true));
    assert!(text_of(&r.content[0]).starts_with("E_TIMEOUT: list_hosts"));

    let quick = async { Ok(CallToolResult::success(vec![Content::text("ok")])) };
    let r = bounded("list_hosts", std::time::Duration::from_secs(5), quick)
        .await
        .unwrap();
    assert_eq!(r.is_error, None);
}
```

- [ ] **Step 2: Run to confirm failure:**

```bash
cargo test tool_deadline 2>&1 | tail -10
```
Expected: `cannot find function \`tool_deadline\`` (and the constants).

- [ ] **Step 3: Implement in `src-tauri/src/mcp/tools/support.rs`** (append):

```rust
use std::time::Duration;

/// Tools that are themselves bounded long-polls (`timeout_s` ≤ 600):
/// the wire cap sits above their own maximum.
pub(super) const LONG_POLL_TOOLS: &[&str] = &["wait_for_session", "wait_for_task", "run_prompt"];
pub(super) const LONG_POLL_CAP: Duration = Duration::from_secs(660);

/// Tools that compose several SSH round trips or spawn processes on a host
/// (session lifecycle, provisioning, host probes, repo/transcript reads that
/// may page through large files).
pub(super) const LIFECYCLE_TOOLS: &[&str] = &[
    "add_host",
    "probe_host",
    "provision_hosts",
    "new_session",
    "new_bg_session",
    "new_shell_session",
    "recreate_session",
    "restart_session",
    "repair_session",
    "move_session",
    "spawn_review",
    "dispatch_task",
    "safe_kill_session",
    "kill_session",
    "delete_worktree",
    "import_assets",
    "scan_assets",
    "refresh_projects",
    "session_transcript",
    "usage_report",
    "broadcast_prompt",
];
pub(super) const LIFECYCLE_CAP: Duration = Duration::from_secs(300);

/// Everything else: store reads and single SSH round trips.
pub(super) const QUICK_TOOLS: &[&str] = &[
    "cancel_task",
    "capture_session",
    "discover_hosts",
    "dismiss_ghost_session",
    "fleet_health",
    "get_clipboard",
    "hide_host",
    "inbox",
    "list_accounts",
    "list_assets",
    "list_hosts",
    "list_projects",
    "list_sessions",
    "list_tasks",
    "list_worktrees",
    "peek_session",
    "peer_status",
    "register_self",
    "related_sessions",
    "remove_host",
    "rename_session",
    "repo_branches",
    "repo_changes",
    "repo_commit",
    "repo_commit_diff",
    "repo_diff",
    "repo_file",
    "repo_log",
    "repo_tree",
    "send_message",
    "send_prompt",
    "session_history",
    "set_clipboard",
    "set_friendly_name",
    "set_session_tags",
    "whoami",
];
pub(super) const QUICK_CAP: Duration = Duration::from_secs(60);

/// Wall-clock cap for one tool call. An unknown name gets the quick cap;
/// the classification test guarantees every served tool is listed.
pub(super) fn tool_deadline(tool: &str) -> Duration {
    if LONG_POLL_TOOLS.contains(&tool) {
        LONG_POLL_CAP
    } else if LIFECYCLE_TOOLS.contains(&tool) {
        LIFECYCLE_CAP
    } else {
        QUICK_CAP
    }
}

/// The `E_TIMEOUT` tool result for a call that outran [`tool_deadline`].
pub(super) fn timeout_result(tool: &str, limit: Duration) -> CallToolResult {
    let secs = limit.as_secs();
    let mut r = CallToolResult::error(vec![Content::text(format!(
        "E_TIMEOUT: {tool} exceeded its {secs} s limit; the call may have partially completed"
    ))]);
    r.structured_content = Some(serde_json::json!({
        "code": "E_TIMEOUT",
        "tool": tool,
        "limit_secs": secs,
    }));
    r
}

/// Run one tool call under its wall clock. On elapse the future is dropped
/// — safe because no code path holds the store guard across an `.await`,
/// SSH children are reaped by `SshClient::run_child`'s own clock, and the
/// long-poll permit releases in `Drop` — and the caller gets
/// [`timeout_result`].
pub(super) async fn bounded<F>(
    tool: &str,
    limit: Duration,
    fut: F,
) -> Result<CallToolResult, McpError>
where
    F: std::future::Future<Output = Result<CallToolResult, McpError>>,
{
    match tokio::time::timeout(limit, fut).await {
        Ok(outcome) => outcome,
        Err(_elapsed) => {
            tracing::warn!(tool, limit_secs = limit.as_secs(), "[mcp] tool call timed out");
            Ok(timeout_result(tool, limit))
        }
    }
}
```

Adjust the three lists so that the union is exactly the 57 names the router
serves (`grep -o "async fn [a-z_]*"` over the `#[tool(...)]` blocks); the
classification test tells you any name that is missing or extra.

Then in `src-tauri/src/mcp/tools/mod.rs` `call_tool` replace

```rust
        match self.tool_router.call(tcc).await {
            Ok(result) => Ok(result),
            Err(e) => tool_error_result(e),
        }
```

with

```rust
        match bounded(&tool, tool_deadline(&tool), self.tool_router.call(tcc)).await {
            Ok(result) => Ok(result),
            Err(e) => tool_error_result(e),
        }
```

- [ ] **Step 4: Run fmt, clippy and the tests:**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo test mcp:: 2>&1 | grep -E "^test result|FAILED|panicked"
```
Expected: clippy clean, all pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/mcp/tools/support.rs src-tauri/src/mcp/tools/mod.rs src-tauri/src/mcp/tools/tests.rs
git commit -m "feat(mcp): bound every tool call with a per-class wall clock

60 s for reads and single round trips, 300 s for session lifecycle /
provisioning / host probes, 660 s for the self-bounded long polls. On
elapse the caller gets an E_TIMEOUT tool result instead of hanging until
its own MCP_TOOL_TIMEOUT.

Claude-Session: https://claude.ai/code/session_01LnFcQ2QC6QszexSkn2embn"
```

---

### Task 4: Docs, local CI mirror, PR

**Files:**
- Modify: `docs/control-api.md` (sections *Connecting a client* ~line 66, *Tools* ~line 94, *Troubleshooting*)

- [ ] **Step 1: Docs.** In `docs/control-api.md`, after the paragraph ending `tools.` (line ~92, "Then, inside a Claude Code session, `/mcp` lists…") insert:

```markdown
The server speaks MCP **2025-11-25** over **stateless streamable HTTP**: every
`POST /mcp` is a self-contained JSON-RPC exchange, no `Mcp-Session-Id` is
issued or required, and `GET /mcp` is not served (`405`). An app restart, a
port or token change, or a reverse-tunnel bounce therefore needs no reconnect
on the client side — the next call simply works. Responses are SSE-framed so a
long poll keeps receiving a keep-alive every 15 s.
```

In the *Tools* section, after the "Index by area" list, add a subsection:

```markdown
### Errors and limits

A tool that fails in fleet's service layer answers with a **tool result**
carrying `isError: true` (what the MCP spec prescribes for tool-execution
failures, so the model sees it as the tool's output and can correct the call),
not a JSON-RPC error. The text block is the documented `E_CODE: message` line;
`structuredContent` carries `{ code, message, details }` (for example the
candidate rows of `E_AMBIGUOUS` or the `confirm_nonce` of
`E_CONFIRM_REQUIRED`). JSON-RPC errors are reserved for protocol failures:
an unknown tool name or arguments that do not match the schema.

Every call runs under a wall clock: 60 s for reads and single round trips,
300 s for session lifecycle, provisioning and host probes, 660 s for the
self-bounded long polls (`wait_for_session`, `wait_for_task`, `run_prompt`,
whose own `timeout_s` maxes at 600). On elapse the result is
`E_TIMEOUT: <tool> exceeded its <n> s limit; the call may have partially
completed` with `structuredContent { code: "E_TIMEOUT", tool, limit_secs }`.
```

In *Troubleshooting*, add a bullet:

```markdown
- **`E_TIMEOUT` from a lifecycle tool** — the host answered too slowly for
  the call's wall clock (see *Errors and limits*). Check the host with
  `probe_host`, then `list_sessions { force: true }`: a `new_session` that
  timed out may still have created the tmux session.
```

- [ ] **Step 2: Run the full local CI mirror:**

```bash
cd src-tauri && source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | grep -E "^test result|FAILED|panicked"
cd .. && pnpm install --frozen-lockfile && pnpm run check && pnpm run test 2>&1 | grep -E "Test Files|Tests " && pnpm run build 2>&1 | tail -1
```
Expected: rust all green (≈1280 tests), frontend 76 files green, build ok.
(`cargo deny` is part of `scripts/ci-local.sh`; run it if `cargo-deny` is installed, otherwise note it in the PR.)

- [ ] **Step 3: Commit docs, push, PR, merge**

```bash
git add docs/control-api.md
git commit -m "docs(control-api): stateless transport, is_error tool results, wall clocks

Claude-Session: https://claude.ai/code/session_01LnFcQ2QC6QszexSkn2embn"
git push -u origin HEAD
gh pr create --base main --head feat/mcp-hooks-modernize --title "feat(mcp): stateless transport, is_error tool results, per-call wall clock" --body "<summary of the three commits, the local CI results, and the session link on the last line>"
gh pr merge <num> --merge --admin
git push origin --delete feat/mcp-hooks-modernize
```
