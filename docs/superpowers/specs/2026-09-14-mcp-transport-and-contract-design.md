# MCP transport and tool-contract hardening

**Date:** 2026-09-14
**Scope:** `src-tauri/src/mcp/` (server construction, `ServerHandler::call_tool`,
error mapping), `docs/control-api.md`. No tool descriptions change, so
`docs/control-api-reference.md` is untouched. No frontend change. No
migration.

## Problem

The embedded control API (rmcp 1.7, streamable HTTP on `127.0.0.1:<port>/mcp`)
works, but four behaviours make it less reliable than it needs to be for
clients that reach it through a reverse SSH tunnel and for an AI caller that
has to recover from its own mistakes:

1. **Stateful sessions in memory.** `StreamableHttpServerConfig::default()`
   is `stateful_mode: true` with `LocalSessionManager`. Every client holds an
   `Mcp-Session-Id`; an app restart, a port/token change or a tunnel that
   re-establishes leaves the client with an id the server no longer knows
   (`404 Not Found: Session not found`) until the client notices and
   re-initializes. The server never sends server-initiated messages (tools
   only, no resources/prompts, no `list_changed`), so the session buys
   nothing.
2. **Protocol version `2024-11-05`.** rmcp 1.7 knows up to `2025-11-25`
   (`ProtocolVersion::LATEST`); Claude Code's v2 runtime negotiates. Advertising
   the oldest revision hides nothing but also signals nothing.
3. **Every tool error is a JSON-RPC `internal_error`.** `to_mcp_err` /
   `mcp_err` fold the `E_*` code into the message and return
   `McpError::internal_error`. The MCP spec distinguishes *protocol* errors
   (unknown tool, malformed arguments → JSON-RPC error) from *tool execution*
   errors (→ a `CallToolResult` with `is_error: true`), and clients treat
   them differently: a tool-result error is shown to the model as the tool's
   output, so it can correct arguments or pick another session; a JSON-RPC
   error reads as "the server failed".
4. **No wall clock on a tool call.** `call_tool` awaits the router with no
   bound. The service layer bounds SSH children, but a tool that composes
   several SSH round trips (provisioning, `new_session`, `move_session`,
   `spawn_review`) has no end-to-end cap, so a wedged host can hold a
   client's request until the client's own `MCP_TOOL_TIMEOUT` fires.

## Design

### 1. Stateless streamable HTTP

`StreamableHttpService::new(factory, NeverSessionManager::default().into(),
StreamableHttpServerConfig::default().with_stateful_mode(false)
.with_cancellation_token(…))`.

- Each POST is self-contained: rmcp builds a service from the factory
  (`FleetTools::clone`), dispatches the one request through a
  `OneshotTransport`, streams the single response back. No `Mcp-Session-Id`
  is issued or required; `GET` / `DELETE` on `/mcp` answer
  `405 Method Not Allowed` with `Allow: POST` (rmcp behaviour).
- **SSE framing for responses is kept** (`json_response` stays `false`).
  In JSON-direct mode a long-poll tool (`wait_for_session`, `run_prompt`,
  up to 600 s) would sit on a silent connection; the SSE stream carries a
  keep-alive every 15 s (rmcp default `sse_keep_alive`) so tunnels and the
  client's idle-timeout see traffic.
- `MCP-Protocol-Version` header: rmcp validates it when present against its
  known list and accepts its absence. Unchanged.
- The `Caller` the auth middleware inserts into the request extensions still
  reaches `call_tool`: rmcp injects the HTTP `Parts` into the request in the
  stateless branch exactly as in the stateful one, and
  `caller_from_context` reads it from there.
- `FleetTools` is already `Clone` and shares every backend handle via `Arc`
  (rate limiter, pending confirms, long-poll limiter are per-process, not
  per-session), so per-request construction changes nothing.

Effect: an app restart or tunnel bounce is invisible to the client; the next
POST just works.

### 2. Protocol version

`get_info()` advertises `ProtocolVersion::LATEST` (`2025-11-25`). rmcp's
initialize handling answers a client that asked for a known older revision
with that revision (negotiation down), so older clients keep working.

### 3. Tool execution errors as tool results

`support::mcp_err(code, message, data)` keeps building an `McpError` (so every
tool body and every existing direct-call test stays as is) but now carries the
code in structured `data`: `{ "code": "E_…", "details": <data or null> }`.
`to_mcp_err(IpcError)` is expressed through it.

`ServerHandler::call_tool` becomes the single translation point:

```text
match self.tool_router.call(tcc).await {
    Ok(result)                         => Ok(result),
    Err(e) if e.data carries "code"    => Ok(CallToolResult::error(vec![Content::text(format!("{code}: {message}"))])
                                              .with structured_content { code, message, details }),
    Err(e)                             => Err(e),   // rmcp protocol errors: unknown tool, invalid params
}
```

- The text content is the same `E_CODE: message[ details]` string the client
  sees today, so the control skill's documented error codes keep matching.
- `structured_content` lets a caller act on `E_AMBIGUOUS` candidates or
  `E_CONFIRM_REQUIRED` nonces without parsing prose.
- Policy refusals (`E_FORBIDDEN` readonly token, `E_RATE_LIMITED`,
  `E_CONFIRM_REQUIRED`) are tool-execution outcomes too and go the same way.
- `enforce_mode` / `enforce_admin` / the "no caller" fail-closed path happen
  before the router and produce `mcp_err` values, so they are translated by
  the same arm.
- Every `is_error` result is also recorded on the timeline via the existing
  `persist_audit` (unchanged; it already runs before dispatch).

### 4. Wall clock per tool call

`support::tool_deadline(name) -> Duration`, a pure classifier:

| Class | Tools | Cap |
|---|---|---|
| long-poll (self-bounded to ≤ 600 s) | `wait_for_session`, `wait_for_task`, `run_prompt` | 660 s |
| host / session lifecycle and provisioning | `add_host`, `probe_host`, `provision_hosts`, `new_session`, `new_bg_session`, `new_shell_session`, `recreate_session`, `restart_session`, `repair_session`, `move_session`, `spawn_review`, `dispatch_task`, `safe_kill_session`, `kill_session`, `delete_worktree`, `import_assets`, `refresh_projects`, `session_transcript`, `session_conversation`, `usage_report` | 300 s |
| everything else (store reads, single SSH round trips, messaging) | all remaining tools | 60 s |

`call_tool` wraps the router call in `tokio::time::timeout(tool_deadline(&tool), …)`.
On elapse it returns `CallToolResult::error("E_TIMEOUT: <tool> exceeded <n>s")`
with `structured_content { code: "E_TIMEOUT", tool, limit_secs }`.

Safety of dropping the future:
- No code path holds the `Mutex<Store>` guard across an `.await` (repo
  invariant), so a dropped future never leaks a lock.
- SSH children are bounded and reaped by `SshClient::run_child`'s own wall
  clock, independently of the awaiting future.
- The long-poll permit (`LongPollPermit`) releases in `Drop`.
- Confirm nonces and rate-limit slots are consumed before the awaited work
  starts; a timed-out mutating call may have partially executed (same as a
  client-side timeout today) — the error text says so.

Unknown tool names (a future tool not yet classified) get the 60 s default;
the classifier test asserts every tool the router lists is explicitly
classified so a new tool cannot silently inherit it.

## Docs

`docs/control-api.md`:
- *Connecting a client*: note the server is stateless streamable HTTP (no
  session id; an app restart needs no reconnect), advertises MCP
  `2025-11-25`, and that `GET /mcp` is not served.
- *Tools*: a short **Errors and limits** paragraph: tool failures come back as
  a tool result with `is_error: true`, text `E_CODE: message`, and
  `structured_content { code, message, details }`; the per-class wall clock
  and `E_TIMEOUT`.
- *Troubleshooting*: replace any advice that assumes a session id.

## Testing

- `mcp/mod.rs`: in-process axum test — POST `initialize` without any session
  header → 200 with an SSE body carrying `protocolVersion: "2025-11-25"` and
  no `Mcp-Session-Id` response header; POST `tools/list` (still no session
  header) → 200 listing tools; `GET /mcp` with a valid token → 405.
- `mcp/tools/tests.rs`: through `ServerHandler::call_tool` with a
  `RequestContext` carrying a caller: (a) a tool that fails with `E_NOTFOUND`
  returns `Ok(result)` with `is_error == Some(true)`, text starting
  `E_NOTFOUND:` and `structured_content.code == "E_NOTFOUND"`; (b) a readonly
  caller on a mutating tool → `is_error` result with `E_FORBIDDEN`; (c) an
  unknown tool name → `Err` (JSON-RPC), unchanged; (d) `tool_deadline`
  classifies every router tool and returns the documented caps; (e) the
  timeout wrapper turns a never-resolving future into the `E_TIMEOUT` result
  (exercised with a tiny cap through a test seam, not a real tool).
- Existing tests continue to pass; the `reference_is_current` doc test is
  unaffected (no description edits).

## Out of scope

Hook events (`SessionEnd`, `StopFailure`, `Notification`) and the hook
contract docs are a separate, already-designed follow-up. Tool cancellation
via `call_id` stays out (MCP calls run to completion or time out).
