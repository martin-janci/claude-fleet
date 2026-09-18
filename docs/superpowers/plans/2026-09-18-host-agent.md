# fleet-agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a host the hub cannot reach manage itself by dialling out: a small `fleet-agent` binary opens one authenticated WebSocket to the hub and executes what the hub asks, so no inbound reachability, port forward or SSH key is needed.

**Architecture:** The existing `SshExec` trait is the whole transport contract. A new `AgentTransport` implements the same seven methods against an `AgentRegistry` of live connections; a per-host resolver picks SSH or agent from the host row. The agent binary has no dependency on `fleet-core` and speaks a small JSON frame protocol over `wss://<hub>/agent`, authenticating with the host's existing per-host token.

**Tech Stack:** Rust 2021, axum 0.8 (WebSocket upgrade on the hub), `tokio-tungstenite` (agent side), tokio, serde/serde_json, base64, rusqlite (one migration), clap 4 for the agent CLI.

**Spec:** `docs/superpowers/specs/2026-09-18-host-agent-design.md`

## Global Constraints

- Branch `feat/host-agent` off `main`. Commit per task; push only at the end, over SSH (the OAuth token lacks the `workflow` scope; `git push git@github.com:martin-janci/claude-fleet.git HEAD:refs/heads/feat/host-agent`).
- **The service layer must not change.** No file under `crates/fleet-core/src/service/` may be edited except where a host row's transport is read. If a task finds itself editing service logic, stop and report — the seam is wrong.
- The existing SSH tests must pass untouched; that is the evidence the seam held.
- `fleet-core` must never depend on any `tauri*` crate; the `hub-headless` CI job stays green. `crates/fleet-agent` must not depend on `fleet-core` (it shares only the protocol module, which lives in the agent crate and is mirrored by a small module in core, or is a third tiny crate — Task 2 decides and says which).
- The desktop app must behave exactly as today: it has no registry, every host stays SSH, and nothing in `src-tauri/` changes.
- Tokens never reach a log line, a URL or an argv. The agent's config file is 0600.
- A call for a host with no live agent fails fast with `E_AGENT_OFFLINE` — never hangs.
- Every value interpolated into a shell string still goes through `crate::shell::quote`; the agent executes an argv, not a shell line, unless the argv itself is `bash -lc <script>` as today.
- Never hold the `Store` mutex guard across an `.await`.
- On `claude-fleet-trn`, prefix every cargo invocation with `source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu RUSTUP_HOME=/usr/local/rustup PATH=$PATH:/usr/local/cargo/bin &&`. The workspace suite takes minutes: iterate with `-p <crate> <filter>`.
- `cargo deny check` must stay green; if a new dependency's licence is not already allowed, stop and report rather than widening `deny.toml`.

## File map

| Path | Responsibility |
|---|---|
| `crates/fleet-agent/` (new) | The agent binary: `run`, `install`, `status`; the executor; reconnect |
| `crates/fleet-agent/src/proto.rs` | The frame types, shared by construction with the hub side |
| `crates/fleet-core/src/agent/mod.rs` (new) | `AgentRegistry`, `AgentTransport`, the `/agent` upgrade handler |
| `crates/fleet-core/src/ssh.rs` | Unchanged trait; a resolver picks the implementation per host |
| `crates/fleet-core/migrations/033_host_transport.sql` | `hosts.transport` |
| `crates/fleet-core/src/store/hosts_accounts.rs` | Read and write `transport` |
| `crates/fleet-core/src/mcp/mod.rs` | Mount `/agent` behind the bearer check |
| `crates/fleet-hub/src/serve.rs` | Build the registry and hand it to the transport resolver |
| `docs/hub.md`, `docs/control-api.md` | Operator documentation |

---

### Task 1: `hosts.transport` and the store

**Files:** `crates/fleet-core/migrations/033_host_transport.sql`, `store/schema.rs`, `store/rows.rs`, `store/hosts_accounts.rs`, tests

**Interfaces:** `HostRow.transport: String` (`"ssh"` | `"agent"`); `Store::set_host_transport(alias, transport) -> Result<(), IpcError>` refusing anything else; `add_host` takes the transport with `"ssh"` as the default.

- [ ] **Step 1: Failing tests** — a fresh row defaults to `ssh`; setting `agent` round-trips; an unknown value is `E_INVALID`; `EXPECTED_TABLES`/schema version bump as migrations 032 did.
- [ ] **Step 2: Run, see them fail.** `cargo test -p fleet-core store::`
- [ ] **Step 3: Implement.** `ALTER TABLE hosts ADD COLUMN transport TEXT NOT NULL DEFAULT 'ssh'` with an `already_applied` probe, since `ALTER` is not idempotent (migration 031 shows the pattern).
- [ ] **Step 4: Verify and commit** — `feat(store): hosts carry their transport`

---

### Task 2: The frame protocol

**Files:** `crates/fleet-agent/Cargo.toml`, `crates/fleet-agent/src/proto.rs`, tests; workspace member registration

**Interfaces:** `enum HubFrame { Exec { id, argv, stdin, timeout_ms, cap_bytes }, Upload { id, path, mode, bytes_b64 }, Cancel { id }, Ping { id } }`, `enum AgentFrame { Hello { agent_version, host_name, os }, Result { id, exit_code, stdout_b64, stderr_b64, truncated }, Pong { id } }`, both `serde` tagged by `kind`.

Decide and state in the report how the hub sees these types: a tiny third crate, a module compiled into both, or a duplicate with a round-trip test pinning compatibility. Prefer the smallest option that keeps `fleet-agent` free of a `fleet-core` dependency.

- [ ] **Step 1: Failing tests** — every frame round-trips; an unknown `kind` deserialises to an error rather than panicking; a payload past the cap is rejected; base64 fields decode.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify and commit** — `feat(agent): the hub/agent frame protocol`

---

### Task 3: The registry and the transport

**Files:** `crates/fleet-core/src/agent/mod.rs`, `crates/fleet-core/src/lib.rs`, tests

**Interfaces:**

```rust
pub struct AgentRegistry { /* alias -> live connection */ }
impl AgentRegistry {
    pub fn new() -> Arc<Self>;
    pub fn connected(&self, alias: &str) -> bool;
    pub fn snapshot(&self) -> Vec<AgentStatus>;           // alias, since, agent_version
    pub async fn request(&self, alias: &str, frame: HubFrame, timeout: Duration) -> Result<AgentFrame, IpcError>;
}
pub struct AgentTransport { registry: Arc<AgentRegistry> }
#[async_trait] impl SshExec for AgentTransport { /* the seven methods */ }
```

- [ ] **Step 1: Failing tests** against a fake connection: each trait method sends the frame it should and maps the result; `remote_home` asks the agent rather than guessing; `upload_file` sends `Upload`; a cancellable call sends `Cancel` when its token fires; no live agent gives `E_AGENT_OFFLINE` immediately (assert it returns well inside the timeout); a timeout gives `E_TIMEOUT`.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify and commit** — `feat(core): an agent transport behind the existing SshExec seam`

---

### Task 4: Routing per host

**Files:** wherever the `Arc<SshClient>` is handed to service calls (`crates/fleet-hub/src/serve.rs`, `crates/fleet-core/src/service/sessions/reconcile.rs`'s deps construction, `mcp/tools/mod.rs`'s `FleetTools`), tests

**Interfaces:** a resolver — `struct HostRouter { ssh: Arc<SshClient>, agent: Arc<AgentTransport>, store: Arc<Mutex<Store>> }` implementing `SshExec` by reading the host's transport once per call and delegating. The desktop constructs it with an empty registry, so every host resolves to SSH.

- [ ] **Step 1: Failing tests** — a host row marked `agent` routes to the agent transport; an `ssh` row routes to `SshClient`; an unknown alias keeps today's behaviour; the store lock is taken and dropped before the delegated `.await`.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement**, changing no service function — only where the transport object is built.
- [ ] **Step 4: Verify** the whole suite, especially the untouched SSH tests. **Commit** — `feat(core): route each host to its own transport`

---

### Task 5: The `/agent` endpoint

**Files:** `crates/fleet-core/src/agent/ws.rs`, `crates/fleet-core/src/mcp/mod.rs`, tests

- [ ] **Step 1: Failing tests** over a real socket — the upgrade needs a valid per-host bearer token (401 without, 403 for a client token, since an agent is a host); a successful upgrade registers the alias; the connection drops after two missed heartbeats; a second connection for the same alias replaces the first; closing deregisters.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement** behind the same `authorize` layer as `/mcp`, rejecting a caller that is not a per-host token.
- [ ] **Step 4: Verify and commit** — `feat(hub): the /agent WebSocket endpoint`

---

### Task 6: The agent binary

**Files:** `crates/fleet-agent/src/{main.rs,cli.rs,conn.rs,exec.rs,install.rs}`, tests

- [ ] **Step 1: Failing tests** — the executor runs an argv and returns its output; the output cap truncates and flags; `Cancel` kills the child; a duplicate id is refused; the config file is written 0600; `install` renders a unit naming the right binary and user.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement** `run` (dial, hello, serve, heartbeat, reconnect with capped jittered backoff), `install`, `status`. Refuse a `ws://` hub without `--insecure`.
- [ ] **Step 4: Verify and commit** — `feat(agent): the fleet-agent binary`

---

### Task 7: End to end in one process

**Files:** `crates/fleet-core/tests/agent_e2e.rs` (or the agent crate's tests, wherever both sides can be linked)

- [ ] **Step 1:** A test that starts a hub server with a registry, runs an agent against `127.0.0.1`, and asserts: `echo hello` returns through `SshExec::run`; `upload_file` writes a temp file with the right mode; a killed connection reconnects and the next call succeeds; a call with no agent connected gives `E_AGENT_OFFLINE` fast.
- [ ] **Step 2:** Run it, watch it fail against whatever is missing, fix, and get it green. Keep it bounded — every wait has a timeout.
- [ ] **Step 3: Commit** — `test(agent): hub and agent talking over a real socket`

---

### Task 8: Host management, docs and the PR

**Files:** `mcp/tools/fleet.rs` (`add_host` gains transport; a new `agent_status` read-only tool), `docs/hub.md`, `docs/control-api.md`, `docs/control-api-reference.md` (regenerated), `scripts/hub-e2e.sh`

- [ ] **Step 1:** `add_host { …, transport? }` and `agent_status` (read-only: which agents are connected, since when, which version). Regenerate the reference in the same commit.
- [ ] **Step 2:** `docs/hub.md` gains "A host that cannot be reached": install the agent, flip the transport, what breaks if the token is rotated, and that the terminal is still SSH-only. `docs/control-api.md` documents `/agent` and the new tool.
- [ ] **Step 3:** Extend `scripts/hub-e2e.sh` with an agent leg: start an agent against the test hub, assert `agent_status` shows it, run a session command over it, stop the agent and assert `E_AGENT_OFFLINE`.
- [ ] **Step 4:** Full verification: workspace tests, clippy `-D warnings`, fmt, `cargo deny check`, `cargo build -p fleet-hub --locked`, `cargo build -p fleet-agent --locked`, and the end-to-end script. **Do not push or open a PR** — the controller does that after the whole-branch review.

---

## Self-review

**Spec coverage.** Transport seam → Tasks 3 and 4. Protocol → Task 2. Endpoint and auth → Task 5. Agent binary and install → Task 6. Registry lifecycle → Tasks 3 and 5. Host rows → Task 1. Docs, tools and end-to-end → Tasks 7 and 8.

**Placeholders.** Task 2 leaves one decision open deliberately (how the two sides share the frame types) because the answer depends on whether a third crate is worth it; the task must state what it chose and why.

**Type consistency.** `HubFrame`/`AgentFrame` in Tasks 2, 3, 5 and 6; `AgentRegistry`'s methods in Tasks 3, 4, 5 and 8; `HostRow.transport` in Tasks 1, 4 and 8.
