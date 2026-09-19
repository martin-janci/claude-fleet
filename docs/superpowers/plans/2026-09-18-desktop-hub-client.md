# Desktop as a hub client — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the Tauri desktop app point at a `fleet-hub` and become a window onto that fleet — the same sessions, hosts and conversations the phone sees, live — while keeping today's standalone behaviour exactly as it is when no hub is configured.

**Architecture:** Every Tauri command keeps its signature. Behind it, a `Backend` resolved once at startup is either `Local` (today: the service layer over the local `Store` and `SshClient`) or `Remote(HubBackend)`, which maps the command to the MCP tool that already exists and deserialises the same row types. Live updates come from the hub's `GET /events` and are re-emitted under the identical frontend event names, so no Svelte store changes.

**Tech Stack:** Rust 2021, Tauri 2, `fleet-core`, reqwest or the existing HTTP path for the hub client, SSE for events, Svelte 5 for the Settings section.

**Spec:** `docs/superpowers/specs/2026-09-18-desktop-hub-client-design.md`

## Global Constraints

- Branch `feat/desktop-hub-client` off `main`. Commit per task; push over SSH at the end (`git push git@github.com:martin-janci/claude-fleet.git HEAD:refs/heads/feat/desktop-hub-client`).
- **Standalone behaviour must not change.** With no hub configured, every code path, every tick, the embedded server and the local database behave exactly as today. The existing Rust and Vitest suites must pass untouched — that is the evidence.
- **No Svelte store changes.** Remote mode must produce the same frontend event names and payloads the local bus produces. If a store has to change, the bridge is wrong; stop and report.
- `fleet-core` gains no Tauri dependency; the `hub-headless` CI job stays green.
- The desktop pairs as an ordinary client: it stores a client token, never the master token, and never calls a tool a client is refused (`provision_hosts`, `add_host`, `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `pair_client`, `list_clients`, `revoke_client`). Those are disabled in the UI with the reason.
- The token goes to the OS keychain through Tauri's secure storage, not into `state.db` and never into a log line.
- In remote mode the app starts neither the reconcile tick nor the embedded MCP server; two hubs managing one fleet is the failure this sub-project exists to prevent.
- Never hold the `Store` mutex guard across an `.await`; shell strings still go through `crate::shell::quote`.
- On `claude-fleet-trn`, prefix every cargo invocation with `source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu RUSTUP_HOME=/usr/local/rustup PATH=$PATH:/usr/local/cargo/bin &&`. `pnpm test` and `pnpm check` run anywhere.
- The app cannot be launched here (no display); every task states what it verified and what needs the Mac.

## File map

| Path | Responsibility |
|---|---|
| `src-tauri/src/backend/mod.rs` (new) | `Backend` enum, resolution at startup |
| `src-tauri/src/backend/remote.rs` (new) | `HubBackend`: tool mapping, JSON-RPC over `POST /mcp`, error mapping |
| `src-tauri/src/backend/events.rs` (new) | Hub SSE → the same frontend events the local bus emits |
| `src-tauri/src/commands/*.rs` | Each command chooses `Local` or `Remote`; signatures unchanged |
| `src-tauri/src/lib.rs` | Resolve the backend; skip the tick and the server when remote |
| `src/lib/SettingsDialog.svelte`, `src/lib/fleet_settings.ts` | The Hub section: URL, pair, disconnect |
| `src/lib/*.svelte` | Disable client-forbidden actions with a reason; the terminal tab's remote note |
| `docs/hub.md`, `docs/concepts.md` | How to point a desktop at a hub |

---

### Task 1: The backend seam, resolved but inert

**Files:** `src-tauri/src/backend/mod.rs`, `src-tauri/src/lib.rs`, tests

**Interfaces:**

```rust
pub enum Backend { Local, Remote(RemoteConfig) }
pub struct RemoteConfig { pub base_url: String, pub token: String, pub client_name: String }
impl Backend {
    pub fn resolve(store: &Mutex<Store>, keychain: &dyn TokenStore) -> Backend; // hub.remote_url + a stored token
    pub fn is_remote(&self) -> bool;
}
```

- [ ] **Step 1: Failing tests** — an empty `hub.remote_url` resolves `Local`; a URL with a stored token resolves `Remote`; a URL with no token resolves `Local` and logs why (a half-paired app must not silently stop managing hosts); an invalid URL resolves `Local` with a warning.
- [ ] **Step 2: Run, see them fail.** `cargo test -p claude-fleet backend`
- [ ] **Step 3: Implement** the enum, the setting key (`hub.remote_url`) and a `TokenStore` trait with a keychain implementation and a test double. Wire `resolve` into `lib.rs` and, when remote, skip `spawn_reconcile_tick`, `spawn_account_usage_tick` and `maybe_start_mcp` — nothing else yet.
- [ ] **Step 4: Verify** — `cargo test --workspace`; confirm by reading the log line that a standalone start still begins the tick.
- [ ] **Step 5: Commit** — `feat(desktop): resolve a local or remote backend at startup`

---

### Task 2: `HubBackend` — the tool mapping

**Files:** `src-tauri/src/backend/remote.rs`, tests with recorded hub responses

**Interfaces:**

```rust
pub struct HubBackend { cfg: RemoteConfig, http: HttpClient }
impl HubBackend {
    pub async fn call<T: DeserializeOwned>(&self, tool: &str, args: serde_json::Value) -> Result<T, IpcError>;
    pub async fn list_sessions(&self) -> Result<Vec<SessionRow>, IpcError>;
    pub async fn list_hosts(&self) -> Result<Vec<HostRow>, IpcError>;
    pub async fn session_conversation(&self, id: i64, turns: Option<usize>) -> Result<Conversation, IpcError>;
    // …one per command with a remote path
}
```

- [ ] **Step 1: Failing tests** — a recorded SSE-framed `tools/call` response deserialises into `Vec<SessionRow>`; a result with `isError: true` becomes the same `IpcError` code the service layer would have produced; `401` becomes `E_UNAUTHORIZED` carrying "the hub revoked this client"; `403` carries the hub's body; a transport failure becomes `E_HUB_UNREACHABLE`. Record the responses as fixtures from the real tool shapes rather than inventing them.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.** The hub's tools return exactly the row types `fleet-core` defines, so deserialisation is `serde_json::from_value` into the existing structs.
- [ ] **Step 4: Verify and commit** — `feat(desktop): a hub-backed implementation of the read commands`

---

### Task 3: Commands choose their backend

**Files:** `src-tauri/src/commands/{sessions,hosts,projects,worktrees,history,files,tasks}.rs`, tests

- [ ] **Step 1: Failing tests** — for each command with a remote path, a fake remote backend receives the expected tool and arguments, and the command returns the deserialised value unchanged; with `Backend::Local` the existing service path is taken (assert the remote fake was not called).
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.** Keep every command's signature and its `IpcError` contract. Commands with no remote counterpart — the PTY, local diagnostics, catalog authoring — return `E_LOCAL_ONLY` in remote mode with a message naming what to do instead.
- [ ] **Step 4: Verify** the whole Rust suite and commit — `feat(desktop): every command honours the resolved backend`

---

### Task 4: The event bridge

**Files:** `src-tauri/src/backend/events.rs`, `src-tauri/src/lib.rs`, tests

- [ ] **Step 1: Failing tests** — a hub frame `event: session:updated` with a row payload produces exactly the frontend event a local `RowChange::SessionUpdated` produces, name and JSON both; `session:killed` likewise; an unknown event name is ignored; a dropped stream reconnects with backoff and emits one refetch signal; `lagged` triggers a refetch. Compare against `RowChange::name()`/`payload()` directly so the two can never drift.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.** In remote mode, subscribe on startup and re-emit through the same `AppHandleEventBus` the local path uses.
- [ ] **Step 4: Verify** — the Rust suite, plus `pnpm test` to prove the stores are untouched. **Commit** — `feat(desktop): the hub's event stream drives the same frontend events`

---

### Task 5: Settings, pairing and the disabled actions

**Files:** `src/lib/SettingsDialog.svelte`, `src/lib/fleet_settings.ts`, `src-tauri/src/commands/mcp.rs` (pair/disconnect commands), the affected Svelte views, frontend tests

- [ ] **Step 1: Failing frontend tests** — the Hub section shows the URL and the paired name when remote and an empty state when not; pairing with a code calls the command and moves to remote; Disconnect clears and returns to standalone without claiming to revoke anything; client-forbidden actions render disabled with the reason; the terminal tab in remote mode shows the attach hint instead of a dead pane.
- [ ] **Step 2: Run, see them fail.** `pnpm test`
- [ ] **Step 3: Implement**, including the two new Tauri commands (`hub_pair { code }`, `hub_disconnect`) that exchange at `POST /pair` and write or clear the keychain entry and the setting.
- [ ] **Step 4: Verify** — `pnpm check`, `pnpm test`, the Rust suite. **Commit** — `feat(ui): point the desktop at a hub from Settings`

---

### Task 6: Docs, verification and the PR

**Files:** `docs/hub.md`, `docs/concepts.md`, `CLAUDE.md`

- [ ] **Step 1:** `docs/hub.md` gains "Point a desktop at the hub": pair with `fleet-hub pair --name laptop`, paste the code in Settings, what changes (one brain, live updates, no local tick), what does not work remotely (the terminal, local-only commands), and how to go back. `docs/concepts.md` and `CLAUDE.md` get a sentence each.
- [ ] **Step 2:** Full verification: `cargo test --workspace`, clippy `-D warnings`, `cargo fmt --all --check`, `cargo deny check`, `pnpm check`, `pnpm test`, `cargo build -p fleet-hub --locked`. State plainly that the app itself was not launched (no display here) and that a Mac run is outstanding.
- [ ] **Step 3: Commit** — `docs: running the desktop against a hub`. Do NOT push or open a PR; the controller does that after the whole-branch review.

---

## Self-review

**Spec coverage.** Backend seam → Tasks 1–3. Events → Task 4. Pairing, Settings, disabled actions and the terminal note → Task 5. Standalone untouched → the constraint plus Task 1's tests and the untouched suites. Docs → Task 6. The PTY exclusion and "no migration" are non-goals, stated in the docs rather than implemented.

**Placeholders.** None. Task 2's fixtures are recorded from real tool shapes, which the plan says explicitly rather than leaving to invention.

**Type consistency.** `Backend`, `RemoteConfig`, `HubBackend` and `TokenStore` are used identically in Tasks 1–5; the row types are `fleet-core`'s existing ones throughout, which is what makes the mapping a deserialisation rather than a translation.
