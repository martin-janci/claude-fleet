# Hub Client Access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the headless `fleet-hub` daemon everything a phone client needs: its own revocable credential obtained by scanning a QR code, a live event stream, the structured conversation over the API, and the option of serving TLS itself.

**Architecture:** A new `client_tokens` table holds named, hashed, revocable tokens; `Caller` gains a client kind that is never the master and is refused the fleet-admin tools. `fleet-hub pair` mints a single-use code, prints it as a QR, and an unauthenticated `POST /pair` exchanges it for the token. A `BroadcastEventBus` in `fleet-core` feeds a `GET /events` SSE endpoint behind the existing auth layer. `session_conversation` becomes an MCP tool over the existing service function. `serve` gains `--tls off|auto|cert`.

**Tech Stack:** Rust 2021 (cargo workspace `crates/fleet-core`, `crates/fleet-hub`, `src-tauri`), axum 0.8, rmcp 1.7, rusqlite, tokio, clap 4, `sha2`, `qrcode`, `rustls-acme` (TLS), `tokio-rustls`.

**Spec:** `docs/superpowers/specs/2026-09-17-hub-client-access-design.md`

## Global Constraints

- Branch `feat/hub-client-access`, based on `main` at `d70835e`. Commit per task; never push until the final task.
- `fleet-core` must never depend on any `tauri*` crate. The CI job `hub-headless` builds `fleet-hub` without Tauri system libraries and must stay green.
- The desktop app's behaviour must not change: it binds loopback, passes an empty Host allowlist, keeps `AppHandleEventBus`, and no desktop call site may start requiring a client token.
- Tokens never appear in a log line, a URL path, a query string or a process argv. Client tokens are stored only as lowercase-hex SHA-256; the plaintext is shown once, at pairing.
- `Caller::is_master()` must be false for a client caller. `label()` is `client:<name>`, so audit rows, rate-limit buckets and long-poll buckets stay per-client.
- Every value interpolated into a shell string goes through `crate::shell::quote`. Never hold the `Store` mutex guard across an `.await`.
- Production code logs through `tracing` only; `fleet-hub`'s user-facing output goes only through `crates/fleet-hub/src/out.rs` (the `no_eprintln` guard skips exactly that file).
- Every MCP tool parameter field carries a `///` doc comment (a test enforces it). Any `#[tool(description = …)]` change requires the regenerated `docs/control-api-reference.md` in the same commit.
- New tools join the right guard list in `crates/fleet-core/src/mcp/guard.rs`: `session_conversation` and `list_clients` in `READONLY_TOOLS`; `revoke_client` in `ADMIN_TOOLS` (master-only).
- On host `claude-fleet-trn`, prefix every cargo invocation with
  `source ~/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu RUSTUP_HOME=/usr/local/rustup PATH=$PATH:/usr/local/cargo/bin &&`.
  The workspace suite takes minutes: iterate with `-p <crate> <filter>`, run the full suite once at the end.
- `cargo deny check` must stay green. If a new dependency's licence is not already in `deny.toml`'s allowlist, stop and report rather than widening the allowlist.

---

## File map

| Path | Responsibility |
|---|---|
| `crates/fleet-core/migrations/032_client_tokens.sql` (new) | The `client_tokens` table. |
| `crates/fleet-core/src/store/schema.rs` | Register migration 32; add the table to `EXPECTED_TABLES`. |
| `crates/fleet-core/src/store/rows.rs` | `ClientTokenRow`. |
| `crates/fleet-core/src/store/clients.rs` (new) | Store methods: insert, list, resolve by hash, revoke, touch last-seen. |
| `crates/fleet-core/src/mcp/auth.rs` | `ClientRef` on `Caller`, hashing, resolution, `is_master`, `label`. |
| `crates/fleet-core/src/mcp/mod.rs` | Load client rows in `authorize`; refuse clients on `/hook`; mount `/pair` and `/events`. |
| `crates/fleet-core/src/mcp/pairing.rs` (new) | Pending-code registry, `POST /pair` handler, attempt rate limit. |
| `crates/fleet-core/src/mcp/events_route.rs` (new) | `GET /events` SSE handler, filtering, heartbeat, per-caller cap. |
| `crates/fleet-core/src/events.rs` | `BroadcastEventBus`. |
| `crates/fleet-core/src/mcp/tools/{orchestration.rs,fleet.rs}` | `session_conversation`, `list_clients`, `revoke_client`. |
| `crates/fleet-core/src/mcp/guard.rs` | Guard-list entries for the three new tools. |
| `crates/fleet-hub/src/{main.rs,pair.rs,serve.rs,config.rs}` | `pair` and `client` subcommands, QR rendering, bus wiring, `--tls`. |
| `crates/fleet-hub/src/tls.rs` (new) | `off` / `auto` / `cert` server startup. |
| `docs/hub.md`, `docs/control-api.md`, `scripts/hub-e2e.sh` | Operator docs and end-to-end coverage. |

---

### Task 1: `client_tokens` table and store methods

**Files:**
- Create: `crates/fleet-core/migrations/032_client_tokens.sql`, `crates/fleet-core/src/store/clients.rs`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS entry, `EXPECTED_TABLES`), `crates/fleet-core/src/store/mod.rs` (`mod clients;`), `crates/fleet-core/src/store/rows.rs` (`ClientTokenRow`)

**Interfaces:**
- Produces:

```rust
// store/rows.rs
pub struct ClientTokenRow {
    pub id: i64,
    pub name: String,
    pub token_sha256: String,
    pub mode: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

// store/clients.rs — all on `impl Store`
pub fn insert_client_token(&self, name: &str, token_sha256: &str, mode: &str) -> Result<ClientTokenRow, IpcError>;
pub fn list_client_tokens(&self, include_revoked: bool) -> Result<Vec<ClientTokenRow>, IpcError>;
pub fn active_client_tokens(&self) -> Result<Vec<ClientTokenRow>, IpcError>; // revoked_at IS NULL
pub fn revoke_client_token(&self, name: &str) -> Result<ClientTokenRow, IpcError>; // E_NOTFOUND when absent
pub fn touch_client_token(&self, id: i64, now: i64) -> Result<(), IpcError>;     // only when > 60s stale
```

- [ ] **Step 1: Write the failing tests** in `crates/fleet-core/src/store/clients.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open_in_memory().expect("store")
    }

    #[test]
    fn insert_list_and_resolve_by_hash() {
        let s = store();
        let row = s.insert_client_token("phone", "aa11", "full").unwrap();
        assert_eq!(row.name, "phone");
        assert_eq!(row.mode, "full");
        assert!(row.revoked_at.is_none());
        assert!(row.id > 0);
        let all = s.list_client_tokens(false).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].token_sha256, "aa11");
    }

    #[test]
    fn a_revoked_row_is_not_active_but_is_still_listed() {
        let s = store();
        s.insert_client_token("phone", "aa11", "full").unwrap();
        let revoked = s.revoke_client_token("phone").unwrap();
        assert!(revoked.revoked_at.is_some());
        assert!(s.active_client_tokens().unwrap().is_empty());
        assert_eq!(s.list_client_tokens(true).unwrap().len(), 1);
        assert!(s.list_client_tokens(false).unwrap().is_empty());
    }

    #[test]
    fn revoking_an_unknown_name_is_not_found() {
        let s = store();
        let e = s.revoke_client_token("nope").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
    }

    #[test]
    fn a_name_is_unique_and_a_revoked_name_can_be_reused() {
        let s = store();
        s.insert_client_token("phone", "aa11", "full").unwrap();
        let e = s.insert_client_token("phone", "bb22", "full").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        s.revoke_client_token("phone").unwrap();
        s.insert_client_token("phone", "bb22", "readonly").unwrap();
        let active = s.active_client_tokens().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].mode, "readonly");
    }

    #[test]
    fn touch_updates_last_seen_at_most_once_a_minute() {
        let s = store();
        let row = s.insert_client_token("phone", "aa11", "full").unwrap();
        s.touch_client_token(row.id, 1_000).unwrap();
        s.touch_client_token(row.id, 1_030).unwrap(); // inside the minute: ignored
        let seen = s.list_client_tokens(false).unwrap()[0].last_seen_at;
        assert_eq!(seen, Some(1_000));
        s.touch_client_token(row.id, 1_100).unwrap(); // past the minute: applied
        assert_eq!(s.list_client_tokens(false).unwrap()[0].last_seen_at, Some(1_100));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core store::clients`
Expected: module not found.

- [ ] **Step 3: Write the migration**

`crates/fleet-core/migrations/032_client_tokens.sql`:

```sql
-- Client tokens: one row per paired client (a phone, a laptop browser).
-- Unlike host_tokens, only the SHA-256 of the token is stored: the plaintext
-- is shown once at pairing and never needs to be displayed again.
CREATE TABLE IF NOT EXISTS client_tokens (
  id            INTEGER PRIMARY KEY,
  name          TEXT    NOT NULL,
  token_sha256  TEXT    NOT NULL,
  mode          TEXT    NOT NULL DEFAULT 'full',
  created_at    INTEGER NOT NULL,
  last_seen_at  INTEGER,
  revoked_at    INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_client_tokens_hash ON client_tokens(token_sha256);
-- A name is unique among *live* rows; a revoked row keeps its name for the
-- audit trail and does not block re-pairing under the same name.
CREATE UNIQUE INDEX IF NOT EXISTS idx_client_tokens_live_name
  ON client_tokens(name) WHERE revoked_at IS NULL;
```

Register it in `store/schema.rs` next to entry 31 (`Migration::plain(32, include_str!("../../migrations/032_client_tokens.sql"))` — plain, since every statement is `IF NOT EXISTS`), and add `"client_tokens"` to `EXPECTED_TABLES` in that file's test module.

- [ ] **Step 4: Implement the store module**

`crates/fleet-core/src/store/clients.rs` holds the five methods on `impl Store`, following the style of `store/hosts_accounts.rs` (same error mapping through `IpcError`, same `now_unix()` helper the crate already uses). A unique-index violation on insert maps to `E_INVALID` with `a client named '<name>' already exists`. `revoke_client_token` sets `revoked_at` on the live row and returns it, `E_NOTFOUND` when there is none. `touch_client_token` runs `UPDATE client_tokens SET last_seen_at = ?2 WHERE id = ?1 AND (last_seen_at IS NULL OR last_seen_at <= ?2 - 60)`. Declare `mod clients;` in `store/mod.rs`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p fleet-core store::clients schema`
Expected: the five new tests pass and the schema table test still passes.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/migrations crates/fleet-core/src/store
git commit -m "feat(store): client_tokens table with hashed, revocable rows"
```

---

### Task 2: A client caller in the auth layer

**Files:**
- Modify: `crates/fleet-core/src/mcp/auth.rs`, `crates/fleet-core/src/mcp/mod.rs` (`authorize`, `AuthState`), `crates/fleet-core/src/mcp/hooks.rs` (refuse clients), `crates/fleet-core/src/mcp/tools/support.rs` + `messaging.rs` if `is_master` semantics need a comment

**Interfaces:**
- Consumes: `Store::active_client_tokens`, `Store::touch_client_token` (Task 1).
- Produces:

```rust
// mcp/auth.rs
pub struct ClientRef { pub id: i64, pub name: String }
pub struct Caller {
    pub host_alias: Option<String>,
    pub client: Option<ClientRef>,
    pub mode: TokenMode,
}
impl Caller {
    pub fn master() -> Self;                 // host_alias None, client None, Full
    pub fn is_master(&self) -> bool;         // host_alias.is_none() && client.is_none()
    pub fn is_client(&self) -> bool;
    pub fn label(&self) -> String;           // "master" | "host:<alias>" | "client:<name>"
}
pub fn sha256_hex(s: &str) -> String;
pub fn resolve_token(
    presented: &str,
    master: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
) -> Option<Caller>;
pub fn check_request(
    headers: &HeaderMap,
    master_token: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
    allowed: &[String],
) -> Result<Caller, StatusCode>;
```

- [ ] **Step 1: Write the failing tests** (append to `auth.rs`'s test module)

```rust
    fn client_row(id: i64, name: &str, token: &str, mode: &str) -> ClientTokenRow {
        ClientTokenRow {
            id,
            name: name.into(),
            token_sha256: sha256_hex(token),
            mode: mode.into(),
            created_at: 0,
            last_seen_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn a_client_token_resolves_to_a_client_caller_that_is_not_master() {
        let rows = vec![client_row(7, "phone", "tok-phone", "full")];
        let c = resolve_token("tok-phone", "s3cret", &[], &rows).unwrap();
        assert!(!c.is_master(), "a client must never count as the master");
        assert!(c.is_client());
        assert_eq!(c.label(), "client:phone");
        assert_eq!(c.mode, TokenMode::Full);
        assert_eq!(c.client.as_ref().unwrap().id, 7);
        assert!(c.host_alias.is_none());
    }

    #[test]
    fn a_readonly_client_keeps_its_mode_and_an_unknown_token_resolves_to_nothing() {
        let rows = vec![client_row(1, "tablet", "tok-t", "readonly")];
        assert_eq!(resolve_token("tok-t", "s3cret", &[], &rows).unwrap().mode, TokenMode::Readonly);
        assert!(resolve_token("nope", "s3cret", &[], &rows).is_none());
    }

    #[test]
    fn the_master_and_host_tokens_still_resolve_with_clients_present() {
        let clients = vec![client_row(1, "phone", "tok-phone", "full")];
        let hosts = vec![host_row("mefistos", "tok-mef", "full")];
        assert_eq!(resolve_token("s3cret", "s3cret", &hosts, &clients).unwrap(), Caller::master());
        let h = resolve_token("tok-mef", "s3cret", &hosts, &clients).unwrap();
        assert_eq!(h.host_alias.as_deref(), Some("mefistos"));
        assert!(!h.is_client());
    }

    #[test]
    fn sha256_hex_is_lowercase_hex_of_the_token() {
        // Known vector: SHA-256 of "abc".
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
```

Update every existing `resolve_token` / `check_request` call in the test module with the new `&[]` client-rows argument.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core mcp::auth`
Expected: compile errors — `ClientRef`, `sha256_hex`, `is_client`, arity.

- [ ] **Step 3: Implement**

In `auth.rs`: add `ClientRef` (derive `Clone, Debug, PartialEq, Eq`), the `client` field on `Caller`, `sha256_hex` (via the `sha2` crate the workspace already uses), `is_client`, the `label` arm, and the client scan in `resolve_token` — hash the presented token once, then compare each row's `token_sha256` with `constant_time_eq`, keeping the existing no-short-circuit shape. `Caller::master()` sets `client: None`; `is_master()` becomes `self.host_alias.is_none() && self.client.is_none()`.

In `mcp/mod.rs`: `AuthState` keeps only the store handle it already has, so `authorize` loads client rows next to host rows in the same brief lock (`s.active_client_tokens().unwrap_or_default()`), passes them to `check_request`, and — when the caller is a client — spawns nothing but calls `s.touch_client_token(id, now)` inside that same lock (best-effort, errors logged at debug).

In `mcp/hooks.rs` `handle_hook`: a client caller returns `403` before any body handling, because `service::hooks::caller_host` would otherwise attribute a phone's events to `local`. One test in that file.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p fleet-core mcp:: && cargo clippy --workspace --all-targets -- -D warnings`
Expected: green, including the existing routing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/mcp
git commit -m "feat(mcp): client tokens resolve to a non-master client caller"
```

---

### Task 3: Tool gating for clients

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/support.rs` (tests), `crates/fleet-core/src/mcp/guard.rs` (doc comment only), `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Consumes: `Caller` with `client` (Task 2). Produces no new API — this task proves the existing gates behave for the new caller kind, which is the security claim the spec makes.

- [ ] **Step 1: Write the failing tests** in `crates/fleet-core/src/mcp/tools/tests.rs`

```rust
    fn client_caller(mode: TokenMode) -> Caller {
        Caller {
            host_alias: None,
            client: Some(crate::mcp::auth::ClientRef { id: 1, name: "phone".into() }),
            mode,
        }
    }

    #[test]
    fn a_client_is_refused_every_fleet_admin_tool() {
        for tool in guard::ADMIN_TOOLS {
            assert!(
                enforce_admin(&client_caller(TokenMode::Full), tool).is_err(),
                "{tool} must be master-only"
            );
        }
        // …and the master still reaches them.
        for tool in guard::ADMIN_TOOLS {
            assert!(enforce_admin(&Caller::master(), tool).is_ok());
        }
    }

    #[test]
    fn a_readonly_client_is_refused_mutating_tools_but_allowed_reads() {
        let c = client_caller(TokenMode::Readonly);
        assert!(enforce_mode(&c, "send_prompt").is_err());
        assert!(enforce_mode(&c, "list_sessions").is_ok());
        let full = client_caller(TokenMode::Full);
        assert!(enforce_mode(&full, "send_prompt").is_ok());
    }

    #[test]
    fn a_client_may_drive_sessions_on_any_host() {
        // require_host only constrains a per-host caller.
        assert!(require_host(&client_caller(TokenMode::Full), "mefistos", "the session").is_ok());
    }

    #[test]
    fn a_client_cannot_skip_the_untrusted_marker() {
        // `raw: true` is master-only; a client's prompt keeps the marker.
        let c = client_caller(TokenMode::Full);
        let out = apply_marker("hello", "phone", &c, true);
        assert!(out.contains("claude-fleet"), "marker missing: {out}");
    }
```

Match the helper names and signatures to what `tools/tests.rs` already imports; if `apply_marker`'s signature differs, adapt the call and keep the assertion.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core mcp::tools::tests`
Expected: compile error on the `client` field until Task 2 is in (it is), then a real failure only if a gate is wrong.

- [ ] **Step 3: Fix whatever the tests expose**

Expected outcome: nothing to change in production code — the gates key off `is_master()` and `mode`, both correct after Task 2. If a gate turns out to admit a client (most likely `marker_origin`/`apply_marker`, which asks `is_master`), fix it there and say so in the report.

- [ ] **Step 4: Update the guard doc comment**

`guard.rs` `ADMIN_TOOLS`: extend the existing comment to say a client token is refused these too, whatever its mode.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p fleet-core mcp::`

```bash
git add crates/fleet-core/src/mcp
git commit -m "test(mcp): a client token is refused admin tools and honours readonly"
```

---

### Task 4: Pairing — codes, `POST /pair`, and the QR CLI

**Files:**
- Create: `crates/fleet-core/src/mcp/pairing.rs`, `crates/fleet-hub/src/pair.rs`
- Modify: `crates/fleet-core/src/mcp/mod.rs` (`mod pairing;`, mount `/pair`, carry the registry), `crates/fleet-hub/src/{main.rs,serve.rs}`, `crates/fleet-hub/Cargo.toml` (`qrcode`)

**Interfaces:**
- Produces:

```rust
// mcp/pairing.rs
pub struct PendingPairings { /* Mutex<HashMap<String, Pending>> */ }
pub struct PairingRequest { pub code: String, pub name: String, pub mode: String, pub expires_at: Instant }
impl PendingPairings {
    pub fn new() -> Self;
    pub fn mint(&self, name: &str, mode: &str, ttl: Duration) -> PairingRequest; // 8 Crockford base32 chars
    pub fn consume(&self, code: &str) -> Option<PairingRequest>;                  // single use, expiry checked
    pub fn sweep(&self, now: Instant);
}
pub async fn handle_pair(State(state): State<PairState>, Json(body): Json<PairBody>) -> Response;
pub fn pair_url(base: &str, code: &str) -> String; // "<base>/pair#<code>"
```

- [ ] **Step 1: Write the failing tests** in `pairing.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_code_is_eight_crockford_chars_and_unique() {
        let p = PendingPairings::new();
        let a = p.mint("phone", "full", Duration::from_secs(600));
        let b = p.mint("tablet", "full", Duration::from_secs(600));
        assert_eq!(a.code.len(), 8);
        assert!(a.code.chars().all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)), "{}", a.code);
        assert_ne!(a.code, b.code);
    }

    #[test]
    fn a_code_works_once() {
        let p = PendingPairings::new();
        let req = p.mint("phone", "readonly", Duration::from_secs(600));
        let got = p.consume(&req.code).expect("first use");
        assert_eq!(got.name, "phone");
        assert_eq!(got.mode, "readonly");
        assert!(p.consume(&req.code).is_none(), "second use must fail");
    }

    #[test]
    fn an_expired_code_is_refused_and_swept() {
        let p = PendingPairings::new();
        let req = p.mint("phone", "full", Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(p.consume(&req.code).is_none());
    }

    #[test]
    fn an_unknown_code_is_refused() {
        let p = PendingPairings::new();
        assert!(p.consume("ZZZZZZZZ").is_none());
    }

    #[test]
    fn pair_url_puts_the_code_in_the_fragment() {
        assert_eq!(pair_url("https://fleet.example.com", "ABCD1234"), "https://fleet.example.com/pair#ABCD1234");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core mcp::pairing`
Expected: module not found.

- [ ] **Step 3: Implement the registry and the endpoint**

`PendingPairings` stores `{ name, mode, expires_at }` by code in a `Mutex<HashMap<…>>`; `mint` draws 5 bytes from `rand::rng()` and renders 8 Crockford base32 characters; `consume` removes the entry, returning `None` when absent or expired; `sweep` drops expired entries and is called from `mint`.

`POST /pair` takes `{ "code": "..." }`, consumes it, generates a token with the existing `mcp::generate_token()`, inserts `insert_client_token(name, sha256_hex(&token), mode)`, and answers `{ token, name, mode, hub }` where `hub` is the configured public URL (or the loopback base). Any failure — unknown, used, expired code — answers `404` with body `{"error":"invalid code"}` and logs nothing token-shaped. A per-remote-address limiter (reuse `guard::RateLimiter` keyed by the peer IP, one attempt per 6 s → 10/min) returns `429` with `Retry-After`.

Mount it in `build_app` **outside** the `authorize` layer, the same way `/healthz` is merged, and add a doc comment saying why: pairing is how a client gets its first credential, so it cannot require one. The route takes its own state (store + registry + base URL).

- [ ] **Step 4: Write the routing tests** in `mcp/mod.rs`'s test module

Assert, over a real socket: `POST /pair` with a minted code returns 200 and a 64-hex token; the same code again returns 404; an unknown code returns 404; `/pair` needs no `Authorization` header and passes with a foreign `Host`; and the token it returned then authenticates a `/mcp` call. Show RED for the last assertion before the route exists.

- [ ] **Step 5: Implement the `pair` CLI**

`crates/fleet-hub/src/pair.rs`: `fleet-hub pair --name <name> [--mode full|readonly] [--ttl <secs>]` needs a **running hub** (it must mint into the server's registry), so it talks to the local server: `POST /pair/mint` — no. Simpler and stated in the spec: the CLI mints through the running server's admin surface. Implement it as a master-token call to a new MCP tool `pair_client { name, mode, ttl_s }` (added in Task 5) rather than a second write path, and render the QR from the returned URL. `fleet-hub pair` therefore: reads the master token and base URL from the data dir, calls the hub's `/mcp` `tools/call pair_client`, and prints the QR plus the URL and the expiry. If the hub is not running, it exits 1 with `start fleet-hub serve first`.

QR rendering: the `qrcode` crate's `render::unicode::Dense1x2` output (no image dependency; disable the crate's default features so `image`/`svg` are not pulled in — check `cargo deny` stays green).

- [ ] **Step 6: Verify**

Run: `cargo test -p fleet-core mcp::pairing mcp::mod && cargo test -p fleet-hub && cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/mcp crates/fleet-hub
git commit -m "feat(hub): pairing codes, POST /pair, and a QR in the terminal"
```

---

### Task 5: Client management tools

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/fleet.rs` (three tools), `crates/fleet-core/src/mcp/tools/params.rs`, `crates/fleet-core/src/mcp/guard.rs` (lists), `docs/control-api-reference.md` (regenerated), `crates/fleet-hub/src/main.rs` (`client list|revoke`)

**Interfaces:**
- Produces MCP tools `pair_client { name, mode?, ttl_s? }` (master-only, returns `{ url, code, expires_in_s }`), `list_clients { include_revoked? }` (read-only), `revoke_client { name }` (master-only). CLI `fleet-hub client list` and `fleet-hub client revoke <name>`.

- [ ] **Step 1: Write the failing tests**

In `mcp/tools/tests.rs`: `pair_client` and `revoke_client` are in `ADMIN_TOOLS`; `list_clients` is in `READONLY_TOOLS`; a client caller is refused `pair_client` and `revoke_client`; every new parameter field has a doc comment (the existing `every_tool_parameter_is_documented` test covers this once the tools are registered).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core mcp::tools`

- [ ] **Step 3: Implement the tools**

Follow the `wait_for_session` template exactly: `audit(...)` first, `Parameters<…>` struct in `params.rs` with `///` on every field, `ok_json` out, `to_mcp_err` for errors. `pair_client` mints through the same `PendingPairings` the `/pair` route consumes (the registry lives in `FleetTools` alongside the guards). `list_clients` returns rows without `token_sha256`. `revoke_client` returns the revoked row.

Add the names to `guard.rs`: `list_clients` → `READONLY_TOOLS`; `pair_client`, `revoke_client` → `ADMIN_TOOLS`.

- [ ] **Step 4: Regenerate the reference**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, and commit `docs/control-api-reference.md` with the code.

- [ ] **Step 5: Implement the CLI**

`fleet-hub client list` prints a table (name, mode, created, last seen, revoked) and `fleet-hub client revoke <name>` prints a confirmation, both by calling the hub's `/mcp` with the master token, like `pair`. A dead hub exits 1 with the same message.

- [ ] **Step 6: Verify and commit**

Run: `cargo test -p fleet-core mcp:: && cargo test -p fleet-hub && cargo test -p fleet-core doc_gen`

```bash
git add crates/fleet-core crates/fleet-hub docs/control-api-reference.md
git commit -m "feat(mcp): pair_client, list_clients and revoke_client"
```

---

### Task 6: Broadcast event bus and `GET /events`

**Files:**
- Create: `crates/fleet-core/src/mcp/events_route.rs`
- Modify: `crates/fleet-core/src/events.rs` (`BroadcastEventBus`), `crates/fleet-core/src/mcp/mod.rs` (mount `/events`, carry a receiver factory), `crates/fleet-hub/src/serve.rs` (wire the bus), `crates/fleet-core/src/mcp/guard.rs` (reuse `LongPollLimiter` for stream slots)

**Interfaces:**
- Produces:

```rust
// events.rs
pub struct BroadcastEventBus { tx: tokio::sync::broadcast::Sender<EventMessage> }
#[derive(Clone)] pub struct EventMessage { pub name: &'static str, pub payload: serde_json::Value }
impl BroadcastEventBus {
    pub fn new(capacity: usize) -> Self;           // capacity 256
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventMessage>;
}
impl EventBus for BroadcastEventBus { fn emit(&self, e: &RowChange); }
```

- [ ] **Step 1: Write the failing tests** in `events.rs`

```rust
    #[tokio::test]
    async fn a_subscriber_receives_emitted_row_changes() {
        let bus = BroadcastEventBus::new(16);
        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(42));
        let msg = rx.recv().await.expect("one message");
        assert_eq!(msg.name, "session:killed");
        assert_eq!(msg.payload["id"], 42);
    }

    #[tokio::test]
    async fn emitting_without_subscribers_is_not_an_error() {
        let bus = BroadcastEventBus::new(4);
        bus.emit(&RowChange::SessionKilled(1)); // must not panic
    }

    #[tokio::test]
    async fn a_lagging_subscriber_reports_lag_rather_than_stalling_the_bus() {
        let bus = BroadcastEventBus::new(2);
        let mut rx = bus.subscribe();
        for i in 0..5 { bus.emit(&RowChange::SessionKilled(i)); }
        let err = rx.recv().await.unwrap_err();
        assert!(matches!(err, tokio::sync::broadcast::error::RecvError::Lagged(_)));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-core events::`

- [ ] **Step 3: Implement the bus and the route**

`BroadcastEventBus::emit` builds `EventMessage { name: e.name(), payload: e.payload() }` and `send`s it, ignoring `Err` (no subscribers). `GET /events` lives behind the `authorize` layer, takes an optional `?kinds=session,host` filter matched against the part of the name before `:`, and returns an axum SSE stream:

- first message `event: ready`, data `{ "version": <app version>, "now": <unix secs> }`;
- one message per matching `EventMessage`, `event:` its name, `data:` its payload;
- `Sse::keep_alive` every 15 s;
- on `RecvError::Lagged(n)`, one `event: lagged` with `{ "skipped": n }` and then close;
- a stream slot from the existing per-caller `LongPollLimiter` (8 per caller); refuse the ninth with `429` and `Retry-After: 1`.

`fleet-hub serve` builds the store with `BroadcastEventBus` and passes a subscribe handle into `start_with_handle`; every other `fleet-hub` store-open site keeps `NoopEventBus`. The desktop's call keeps passing nothing (no stream), so `/events` on the desktop answers `503` with `events are not enabled on this server` — one test.

- [ ] **Step 4: Write the route tests** in `mcp/mod.rs`'s test module

Over a real socket, with the master token: `/events` returns 200 with `content-type: text/event-stream`, the first frame is `event: ready`, a `RowChange` emitted after connecting arrives as its own frame, `?kinds=host` drops a session event, and a request without a token returns 401. Show RED first.

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p fleet-core events:: mcp:: && cargo test -p fleet-hub && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add crates/fleet-core crates/fleet-hub
git commit -m "feat(hub): broadcast event bus and a GET /events SSE stream"
```

---

### Task 7: `session_conversation` as an MCP tool

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/orchestration.rs`, `crates/fleet-core/src/mcp/tools/params.rs`, `crates/fleet-core/src/mcp/guard.rs` (`READONLY_TOOLS`, lifecycle deadline class in `support.rs`), `docs/control-api-reference.md`

**Interfaces:**
- Produces MCP tool `session_conversation { session_id, turns? }` returning the `Conversation` JSON.

- [ ] **Step 1: Write the failing test**

In `mcp/tools/tests.rs`: `session_conversation` is in `READONLY_TOOLS` and in the lifecycle deadline class (it reads over SSH, like `session_transcript`); its parameters are documented. In `service/transcript.rs`, if no test already covers `conv_limits`, add one asserting `conv_limits(None)` gives `(10, 64_000)` and that a large `turns` clamps to `CONV_MAX_TURNS` and the character ceiling.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleet-core mcp::tools transcript::`

- [ ] **Step 3: Implement**

```rust
    #[tool(description = "Read a session's recent conversation as structured turns: \
        each turn carries the human prompt, its timestamp, the turn's end \
        timestamp, and items that are either assistant text or a one-line tool \
        summary (flagged when that tool call failed). turns defaults to 10 and \
        is capped at 100; the character budget scales with it. Prefer this over \
        session_transcript when you want the shape of the exchange rather than \
        one flat blob. Read-only. Errors: E_INVALID_STATE (no claude_session_id \
        yet), E_NO_TRANSCRIPT (nothing written yet).")]
    pub(super) async fn session_conversation(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionConversationParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_conversation", &format!("session_id={} turns={:?}", p.session_id, p.turns));
        let row = self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;
        let (turns, max_chars) = transcript::conv_limits(p.turns);
        let args = transcript::resolve_args(&self.store, &row, turns, max_chars).map_err(to_mcp_err)?;
        let conv = transcript::fetch_conversation(args, &self.ssh).await.map_err(to_mcp_err)?;
        ok_json(&conv)
    }
```

with `SessionConversationParams { /// Fleet session id … session_id: i64, /// How many turns … #[serde(default)] turns: Option<usize> }`. Add the name to `READONLY_TOOLS` and to the lifecycle list in `support.rs` beside `session_transcript`.

- [ ] **Step 4: Regenerate the reference, verify, commit**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current && cargo test -p fleet-core mcp::`

```bash
git add crates/fleet-core docs/control-api-reference.md
git commit -m "feat(mcp): session_conversation returns structured turns"
```

---

### Task 8: Built-in TLS

**Files:**
- Create: `crates/fleet-hub/src/tls.rs`
- Modify: `crates/fleet-hub/src/{config.rs,serve.rs,main.rs}`, `crates/fleet-hub/Cargo.toml`, `crates/fleet-core/src/mcp/mod.rs` (accept a prepared listener/acceptor), `deploy/hub/*`, `docs/hub.md`

**Interfaces:**
- Produces `--tls off|auto|cert`, `--acme-email`, `--acme-staging`, `--tls-cert`, `--tls-key`, each with its `FLEET_HUB_*` env and `hub.tls*` setting, resolved by the existing precedence.

- [ ] **Step 1: Write the failing config tests**

`--tls auto` without `--acme-email` is an error naming the flag; `--tls cert` without both files is an error; `--tls auto` with a public URL whose host is a bare IP is an error (ACME cannot issue for it); `--tls off` keeps today's behaviour; a non-loopback bind with `--tls auto` or `cert` no longer needs `--allow-plaintext` (TLS is the protection the plaintext rule was asking for).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleet-hub config`

- [ ] **Step 3: Implement**

`mcp::start_with_handle` gains a variant that accepts an already-prepared `axum::serve`-compatible acceptor so `fleet-hub` can hand it a TLS one without `fleet-core` taking a TLS dependency: the smallest shape is a new `start_with_listener(…, listener: TcpListener, tls: Option<TlsAcceptor>)`, with the existing entry points delegating. Keep the desktop path byte-identical.

`tls.rs`: `off` returns `None`; `cert` loads the PEM pair with `tokio-rustls`; `auto` uses `rustls-acme` with the cache under `<data-dir>/acme`, the domain from the public URL, the ACME directory from `--acme-staging`. Startup failure (no email, unreachable domain, bad PEM) exits 1 with the reason; a renewal failure while running is logged and the old certificate keeps serving.

Dependency gate: after adding `rustls-acme`/`tokio-rustls`, run `cargo deny check`. If a licence is not already allowed, stop, report, and ship `cert` mode only.

- [ ] **Step 4: Test**

A test serving `cert` mode with a self-signed pair generated in the test (`rcgen` if licence-clean, otherwise a fixture pair committed under `crates/fleet-hub/tests/data/`), asserting an HTTPS request to `/healthz` returns the body. `auto` gets an `#[ignore]` integration test documenting the staging-directory run.

- [ ] **Step 5: Docs and commit**

`docs/hub.md` gains a "single binary with its own certificate" section; the compose file keeps Caddy as the default with a note that `--tls auto` is the alternative.

```bash
git add crates/fleet-hub crates/fleet-core deploy docs
git commit -m "feat(hub): serve TLS directly, with ACME or supplied certificates"
```

---

### Task 9: Docs, end-to-end coverage, and the PR

**Files:**
- Modify: `docs/hub.md`, `docs/control-api.md`, `scripts/hub-e2e.sh`, `CLAUDE.md` (one line about client tokens)

- [ ] **Step 1: Extend the end-to-end script**

Add, keeping the existing 36 checks green: mint a pairing code through `pair_client` with the master token; exchange it at `/pair` for a client token; use that token for a `list_sessions` call; confirm the same code fails the second time; confirm a readonly client is refused `send_prompt` and allowed `list_sessions`; confirm a client is refused `provision_hosts`; subscribe to `/events`, cause a row change, and see the frame; call `session_conversation` on a session with no transcript and expect the documented error code; revoke the client and confirm its token now gets 401.

- [ ] **Step 2: Run it**

Run: `cargo build -p fleet-hub --release --locked && BIN=$PWD/target/release/fleet-hub bash scripts/hub-e2e.sh`
Expected: every check passes.

- [ ] **Step 3: Docs**

`docs/hub.md`: a "Pair a phone" section (run `fleet-hub pair --name phone`, scan, done), a "Clients" section (list, revoke, what a readonly client can do), an "Events" paragraph, and the TLS section from Task 8. `docs/control-api.md`: the three new tools in the index and one paragraph on `/events` and `/pair` next to the existing endpoint documentation. `CLAUDE.md`: one line in the architecture list.

- [ ] **Step 4: Full verification**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check && cargo deny check && cargo test -p fleet-core doc_gen && cargo build -p fleet-hub --locked`

- [ ] **Step 5: Push and open the PR**

Push over SSH (the OAuth token lacks the `workflow` scope; the host's key authenticates as `martin-janci`):

```bash
git push git@github.com:martin-janci/claude-fleet.git HEAD:refs/heads/feat/hub-client-access
GH_TOKEN="$(gh auth token --user martin-janci)" gh pr create --base main --title "feat: client access for the hub" --body-file <(…)
```

The PR body lists what shipped, the verification, and the manual acceptance that still needs a real deployment: pairing a real phone, an events stream over the public URL, and `--tls auto` against Let's Encrypt staging.

---

## Self-review

**Spec coverage.** Client tokens → Tasks 1–3. Pairing and QR → Task 4. Management tools and CLI → Task 5. Events stream and the bus → Task 6. Conversation tool → Task 7. TLS → Task 8. Docs and end-to-end → Task 9. The spec's decided open questions (master-only management, full payloads on the stream) are reflected in Tasks 5 and 6.

**Placeholder scan.** One place needed rework and got it: Task 4 originally had the CLI minting codes directly in a second write path; it now goes through the `pair_client` tool so there is one registry and one code path, which is also why Task 5 owns that tool. The PR body in Task 9 is described rather than quoted, deliberately, since it summarises results that do not exist yet.

**Type consistency.** `ClientTokenRow` fields are identical in Tasks 1, 2 and 5. `Caller { host_alias, client, mode }` and `ClientRef { id, name }` match across Tasks 2, 3 and the tests. `resolve_token`/`check_request` arities match Task 2's interfaces and the Task 6 route. `conv_limits(turns) -> (usize, usize)` in Task 7 matches the existing helper the desktop command uses.
