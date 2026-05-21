# claude-fleet — Control API (embedded MCP server) design

**Status:** Draft
**Author:** Martin via Claude
**Date:** 2026-05-21
**Sibling specs:** `2026-05-21-iter4b-reviews-design.md`, `2026-05-21-hardening-review.md`

## Goal

Let an AI assistant drive claude-fleet programmatically — list the fleet, spawn
and steer sessions, send prompts, manage hosts — by embedding a **Model Context
Protocol (MCP) server directly in the Rust backend**. The AI connects straight
to the running app over a localhost HTTP transport; there is no intermediate
process. claude-fleet becomes its own MCP server.

## Decisions (locked during brainstorming)

- **Transport:** the app speaks MCP itself, via the streamable-HTTP server
  transport, bound to `127.0.0.1` only. No separate Node/TS wrapper.
- **Surface:** the API exposes the full command set — read/query, prompt
  sending, session lifecycle, and host/project management.
- **Security:** localhost bind **plus** a bearer token. The token is generated
  on first run and shown in Settings. Every MCP request must carry
  `Authorization: Bearer <token>`.
- **Shared state:** MCP tool handlers reuse the *exact same* `Store`, `SshClient`,
  and `CancellationRegistry` the Tauri commands use — one process, one source of
  truth. This requires extracting the command logic into a transport-agnostic
  service layer.

## Non-goals (explicitly out of scope for v1)

- Streaming events to MCP clients (session created/updated pushes). The AI polls
  `list_sessions` instead. The event bus stays frontend-only in v1.
- Exposing the PTY / terminal byte stream over MCP. `capture_pane`-style read of
  a session's screen is deferred to v2 (it's the natural next tool, but needs a
  size/encoding decision).
- Remote (non-localhost) access, TLS, multi-user tokens, OAuth.
- An MCP *client* inside claude-fleet (the app consuming other MCP servers).
- Packaging a standalone MCP binary. The server is in-process only.

## Architecture & data flow

```
  AI client (Claude Desktop / Claude Code / SDK)
        │  MCP over streamable-HTTP
        │  POST 127.0.0.1:<port>/mcp   Authorization: Bearer <token>
        ▼
  ┌─────────────────────────────────────────────┐
  │  claude-fleet (single process)               │
  │                                              │
  │  axum Router ── /mcp ──► rmcp StreamableHttp  │
  │       │  bearer-auth middleware               │
  │       ▼                                       │
  │  FleetTools (MCP tool router)                 │
  │       │  calls                                │
  │       ▼                                       │
  │  service layer  ◄────────────  Tauri commands │
  │   (transport-agnostic fns)         (frontend) │
  │       │                                       │
  │       ▼                                       │
  │  Arc<Mutex<Store>> · Arc<SshClient> ·         │
  │  Arc<CancellationRegistry>                    │
  └─────────────────────────────────────────────┘
```

Both the Tauri IPC commands and the MCP tools become thin adapters over a shared
**service layer**. Neither path is privileged; they call the same functions.

## Crate dependencies

Added to `src-tauri/Cargo.toml` (pin to the current published versions at
implementation time):

| Crate | Purpose |
|---|---|
| `rmcp` (features: `server`, `transport-streamable-http-server`, `macros`) | Official Rust MCP SDK — tool router, schema generation, HTTP transport. |
| `axum` | HTTP router that hosts the MCP service and the bearer-auth middleware. |
| `tower` / `tower-http` | Middleware plumbing (already a transitive dep of axum). |
| `rand` | Token generation (CSPRNG). |
| `schemars` | JSON-schema derive for tool argument structs (rmcp requires it). |

`tokio` is already `features = ["full"]`, so no runtime change. All new crates
are pure Rust — no extra system libraries, so they don't widen the headless
build gap noted in CLAUDE.md.

## Service-layer refactor (the spine of this feature)

Today every command is a `#[tauri::command]` whose body mixes argument
validation, `tauri::State` extraction, and business logic. The MCP handlers
cannot call those — they have no `State`. So:

1. **Managed-state change.** The store is currently managed as `Mutex<Store>`.
   Change to `Arc<Mutex<Store>>` so the MCP server task can hold a clone.
   `SshClient` and `CancellationRegistry` are already `Arc`-managed.
2. **Transport-agnostic signatures.** Each command's logic moves into (or its
   existing `*_inner` helper is re-typed to) a function in a new
   `src-tauri/src/service/` module that takes plain references:
   `&Mutex<Store>`, `&SshClient`, `&CancellationRegistry` — not `tauri::State`.
   Because `State<Arc<Mutex<Store>>>` derefs to `Arc<Mutex<Store>>` and
   `&Arc<Mutex<Store>>` coerces to `&Mutex<Store>`, both callers pass `&store`
   unchanged.
3. **Thin adapters.** Each `#[tauri::command]` becomes a one-liner that unwraps
   `State` and calls the service function. Each MCP tool does the same with its
   `Arc` clones. **No behaviour change** — existing Rust + Vitest tests must
   stay green after this refactor alone.

Service modules mirror the command modules: `service/sessions.rs`,
`service/hosts.rs`, `service/projects.rs`, `service/health.rs`. Errors stay
`IpcError`; the MCP layer maps `IpcError` → `rmcp::ErrorData` (code + message).

## MCP module (`src-tauri/src/mcp/`)

- `mcp/mod.rs` — `spawn_server(...)`: builds the axum router, binds the
  listener, spawns the serve loop on a tokio task, returns a handle for
  shutdown. Called from `lib.rs::run()`'s `.setup()` once the store is managed.
- `mcp/auth.rs` — axum middleware: constant-time compare of the
  `Authorization: Bearer` header against the configured token; `401` on
  mismatch or absence.
- `mcp/tools.rs` — `FleetTools` struct holding `Arc<Mutex<Store>>`,
  `Arc<SshClient>`, `Arc<CancellationRegistry>`. `#[tool_router]` impl with one
  `#[tool]` method per command below. Each method deserializes a `schemars`
  argument struct, calls the service layer, serializes the result to JSON.

### Tool surface (maps 1:1 to the service layer)

| MCP tool | Service fn | Notes |
|---|---|---|
| `fleet_health` | health | version + schema_version + db_ready |
| `list_hosts` | hosts | all registered hosts |
| `discover_hosts` | hosts | scan `~/.ssh/config` |
| `add_host` | hosts | mutating — strict probe |
| `remove_host` | hosts | mutating |
| `probe_host` | hosts | re-probe reachability |
| `hide_host` | hosts | toggle hidden flag |
| `list_accounts` | hosts | cached Claude accounts |
| `list_projects` | projects | projects + worktrees |
| `refresh_projects` | projects | rescan filesystem |
| `list_sessions` | sessions | full reconcile |
| `related_sessions` | sessions | siblings by worktree |
| `new_session` | sessions | mutating — spawns tmux |
| `kill_session` | sessions | mutating |
| `rename_session` | sessions | mutating |
| `restart_session` | sessions | mutating |
| `send_prompt` | sessions | deliver text to a session |
| `spawn_review` | sessions | spawn a review session |

`probe_ssh_alias` and `cancel_command` are intentionally **not** exposed —
they're UI-interaction affordances (preview probe, abort an in-flight dialog
call) with no meaning to a polling AI client. MCP tool calls run to completion;
cancellation is internal.

Long-running tools (`new_session`, `spawn_review`) call the service layer with
`reg.register_anonymous()` — no client-supplied `call_id`, since MCP has no
abort channel in v1.

## Configuration & secret storage

Stored in the existing `settings` table (key/value, present since migration
001 — **no new migration needed**):

| Key | Default | Meaning |
|---|---|---|
| `mcp.enabled` | `false` | Server starts on launch only when `true`. Opt-in. |
| `mcp.port` | `4180` | localhost port. If the bind fails (port taken), the app logs and the Settings panel shows the error. |
| `mcp.token` | *(generated)* | 32-byte CSPRNG value, hex-encoded. Generated the first time the API is enabled; regenerable from Settings. |

The server is **off by default**. Enabling it from Settings generates the token
(if absent) and starts the listener without an app restart; disabling stops it.

## Backend changes summary

- `Cargo.toml` — new deps above.
- `lib.rs` — managed state `Mutex<Store>` → `Arc<Mutex<Store>>`; in `.setup()`,
  read `mcp.enabled`/`mcp.port`/`mcp.token` and conditionally `mcp::spawn_server`;
  on `WindowEvent::Destroyed`, shut the server down alongside SSH + PTY.
- `store.rs` — small helpers: `get_setting(key) -> Option<String>`,
  `set_setting(key, value)`. (Generic — also useful beyond MCP.)
- `commands/*.rs` — every `#[tauri::command]` re-typed to
  `State<Arc<Mutex<Store>>>` and reduced to a thin call into `service/`.
- `service/` — new module: transport-agnostic command logic.
- `mcp/` — new module: server, auth middleware, tool router.
- Two new Tauri commands for the Settings UI: `mcp_status` (enabled, port,
  running, bind error, token) and `mcp_configure` (set enabled/port, regenerate
  token) — these start/stop the server live.

## Frontend changes

- `src/lib/mcp.ts` — store + wrappers for `mcp_status` / `mcp_configure`.
- `SettingsDialog.svelte` — new **"Control API"** section:
  - Enable/disable toggle.
  - Port input.
  - Running indicator / bind-error message.
  - The connection URL `http://127.0.0.1:<port>/mcp`.
  - The bearer token, masked with a reveal + copy button, and a "Regenerate"
    action (warns it invalidates existing clients).
  - A ready-to-paste MCP client config block (JSON for the streamable-HTTP
    transport, with the URL + `Authorization` header filled in).

## Security

- **Bind:** `127.0.0.1` only — never `0.0.0.0`. Asserted in code, not config.
- **Token:** 256-bit CSPRNG; compared in constant time; absence or mismatch →
  `401`. The token guards against other local processes and against browser
  `fetch` from a malicious page (which cannot read the token).
- **Off by default:** no listener exists until the user opts in.
- **No path/host trust change:** MCP tools call the same service functions as
  the UI, which already run `validate::host_alias` / `validate::tmux_name` and
  shell-quote every interpolated value. The MCP surface introduces **no new**
  SSH command construction — it reuses existing, hardened paths. (Per CLAUDE.md,
  no change to `ssh.rs` / `tmux.rs` quoting is in scope here.)
- **Audit:** every MCP tool call is logged (tool name, host/session args, outcome)
  to stderr — a mutating remote-control surface should be traceable.

## Error handling

| Scenario | Behaviour |
|---|---|
| Missing/bad bearer token | `401 Unauthorized`, no tool dispatch. |
| Port already in use at startup | Server doesn't start; `mcp_status` reports the bind error; app + UI otherwise unaffected. |
| Tool args fail validation | Service layer returns `IpcError` (`E_INVALID` etc.) → mapped to an MCP tool error with the code + message. |
| Store mutex poisoned | `E_LOCK` surfaced as a tool error, consistent with the Tauri path. |
| Server enabled but token absent | Enabling generates the token before the listener binds — never a tokenless listener. |

## Testing strategy

### Rust
- **Refactor safety:** the full existing suite (104+ tests) stays green after the
  service-layer extraction with zero behaviour change.
- `service/` functions are now directly unit-testable without a Tauri runtime —
  add focused tests using the existing mock `TmuxExec` / in-memory `Store`.
- `mcp/auth.rs` — constant-time compare: accepts the right token, rejects wrong
  token, rejects absent header.
- Settings round-trip: `get_setting`/`set_setting`; token generation is 64 hex
  chars; `mcp.enabled` parses to bool.
- Tool-router smoke test: `FleetTools` lists exactly the 17 tools above; a
  `fleet_health` call returns the expected shape.

### Vitest
- `mcp.ts` wrappers parse `mcp_status` / `mcp_configure` results.
- `SettingsDialog`: Control API section renders; toggle calls `mcp_configure`;
  token is masked until revealed; copy button copies URL + token.

### Live verify
- Enable the API in Settings; connect Claude Code via the generated config
  block; call `list_sessions`, `new_session`, `send_prompt`, `kill_session`
  end-to-end against a local host.
- Wrong token → `401`. Disable the API → connection refused.

## Milestones / slices

**M1 — Service-layer refactor.** `Arc<Mutex<Store>>`; extract `service/`;
re-type all commands to thin adapters. No behaviour change; all tests green.
Independently committable and shippable on its own. (~1 day)

**M2 — MCP server skeleton.** Crate deps; `settings` get/set helpers; `mcp/`
module with the streamable-HTTP server, bearer auth, lifecycle wiring in
`lib.rs`; one tool (`fleet_health`) proving the transport end-to-end. (~1 day)

**M3 — Full tool surface.** The remaining 16 tools wired to the service layer;
`IpcError` → MCP error mapping; per-call audit logging. (~1 day)

**M4 — Settings UI.** `mcp_status` / `mcp_configure` commands; `mcp.ts`;
Control API section in `SettingsDialog` with toggle, port, URL, masked token,
regenerate, and the paste-able config block. (~half day)

**M5 — Docs + live verify.** A `docs/` how-to for connecting an MCP client;
end-to-end verification with a real AI client; push. (interactive)

Estimated ~4 days. M1 lands cleanly on its own (it's a pure refactor that also
makes the command logic unit-testable); M2–M4 build the API on top.

## Open risks

1. **rmcp API churn.** The Rust MCP SDK is young; the `#[tool_router]` / tool
   macro surface may differ from this sketch. M2 pins a concrete version and
   adapts; the architecture (axum + a tool struct over the service layer) holds
   regardless of the exact macro spelling.
2. **Refactor blast radius.** Re-typing ~20 command signatures is mechanical but
   wide. Mitigated by M1 being a standalone, behaviour-preserving commit gated
   on the existing test suite — if anything drifts, it's caught before any MCP
   code exists.
3. **Headless build gap.** Per CLAUDE.md, `cargo build`/`test` need Tauri system
   libs and fail on a headless box. The new crates are pure Rust and don't widen
   that gap, but full backend verification requires a desktop dev environment —
   call this out at M2/M5.
4. **Concurrent mutation.** An AI client and the human user can now both mutate
   the fleet at once. The `Store` mutex serialises DB writes, and tmux/SSH
   operations are idempotent-ish (reconcile re-derives truth), so this is safe
   but can surprise the user. The event bus already repaints the UI live when
   the backend state changes, so an MCP-driven mutation shows up in the UI
   without extra work.
5. **Two long-running tools, no abort.** `new_session`/`spawn_review` can take
   seconds; MCP v1 has no cancellation. Acceptable — they're bounded by the
   existing SSH timeouts. v2 can map MCP's progress/cancellation if needed.

## Self-review

- **Placeholder scan:** no TBD/TODO; deps, settings keys, tool list, and
  milestones are concrete. Crate *versions* are deliberately pinned at
  implementation time (M2) — flagged, not left ambiguous.
- **Internal consistency:** the service layer is the single hinge — defined once,
  referenced by both the Tauri and MCP adapters throughout.
- **Scope check:** one feature, one spec, 5 milestones; event streaming, PTY
  exposure, and remote access explicitly fenced as non-goals.
- **Security check:** localhost-only bind asserted in code, 256-bit token,
  off-by-default, no new SSH-construction surface — consistent with the
  hardening review's constraints.
