# Client access: pairing, live events, conversation and built-in TLS

**Date:** 2026-09-17
**Status:** Approved design, awaiting implementation plan
**Scope:** `crates/fleet-core` (client tokens in the store and the auth layer, an
events stream, a broadcast event bus, a conversation MCP tool), `crates/fleet-hub`
(pairing CLI with a QR code, client management, built-in TLS), `docs/hub.md` and
`docs/control-api.md`. One new migration. No desktop UI change.

## Why

Sub-project 1 put the fleet behind one always-on `fleet-hub` reachable over
HTTPS. A phone app (sub-project 3) needs four things that hub does not have:

1. **Its own credential.** Today a client uses the master token, which is
   unrestricted, host-independent and impossible to revoke individually.
2. **A way to get that credential onto the phone** without typing 64 hex
   characters.
3. **Live updates.** Polling `list_sessions` every few seconds is the only
   option today; a phone needs to be told when a session changes.
4. **The conversation.** The desktop renders structured turns from a Tauri
   command; over the API only the flat `session_transcript` text exists.

A fifth item is setup friction: TLS today means running Caddy beside the hub.
For a single-binary deployment the hub should be able to get its own
certificate.

This is sub-project 2 of five (see the sub-project 1 spec for the list).

## Goals

- A **client token** is a first-class credential: named, listable, revocable,
  with a mode (`full` or `readonly`), stored hashed.
- **Pairing in one scan**: `fleet-hub pair` prints a QR code; the phone scans
  it and ends up holding a client token. The code is single-use and expires.
- **`GET /events`**: a server-sent events stream of the same row changes the
  desktop already consumes, authenticated like every other endpoint.
- **`session_conversation`** as an MCP tool, returning the structured turns.
- **Built-in TLS**: `--tls auto` obtains and renews a certificate; `--tls
  cert` uses files the operator supplies; the Caddy path keeps working.

## Non-goals

- The mobile app itself (sub-project 3).
- OAuth, accounts, or multi-user authorization. A client token is a bearer
  credential for the fleet's owner; modes are a blast-radius limit, not a
  permission system for other people.
- A durable event log or replay. A client that reconnects re-reads state with
  the list tools; events are a liveness hint, not a source of truth.
- Push notifications to a phone when the app is closed. That needs a vendor
  service and is its own decision.
- Changing how the desktop app talks to its embedded server (sub-project 5).

## Architecture

### 1. Client tokens

New table (`crates/fleet-core/migrations/032_client_tokens.sql`), registered in
`store/schema.rs` `MIGRATIONS`:

| Column | Meaning |
|---|---|
| `id` | integer primary key |
| `name` | operator-chosen label, unique, 1–64 chars |
| `token_sha256` | lowercase hex of the token's SHA-256; the token itself is never stored |
| `mode` | `full` or `readonly`, same vocabulary as host tokens |
| `created_at`, `last_seen_at` | unix seconds; `last_seen_at` updated at most once a minute |
| `revoked_at` | non-null once revoked; a revoked row is kept for the audit trail |

Unlike host tokens (plaintext, as documented), client tokens are stored
**hashed**: they live on phones, they are minted far more often, and nothing
needs to display them after pairing.

`Caller` today is `{ host_alias: Option<String>, mode: TokenMode }`, where
`host_alias: None` means the master token. It gains a third kind without
breaking that shape: a `client: Option<ClientRef>` field (`{ id, name }`),
so a client caller has `host_alias: None` **and** `client: Some(..)`.
`is_master()` becomes "no host alias and no client", which is the one line
that decides whether a client can reach the fleet-admin tools — it must not.
Only three production sites call it (`support.rs` raw-marker and admin gates,
`messaging.rs`), so the change is contained and each gets a test.
`label()` returns `client:<name>`, which keeps the audit rows, the rate-limit
bucket and the long-poll bucket distinct per client.

Resolution in `auth::resolve_token` stays constant-time and
non-short-circuiting: master, then per-host, then client — the presented token
is hashed once and compared against every non-revoked row. `authorize` already
reloads host tokens per request; client rows load the same way.

Hooks stay host-only: `service::hooks::caller_host` maps a master caller to
`local`, which is meaningless for a phone, so `/hook` refuses a client caller
with `403` rather than attributing its events to a host.

Authorization: a client token is refused the fleet-admin tools (the same list
the master token guards today) and, in `readonly` mode, every mutating tool —
exactly the existing `readonly` gate. Everything else is allowed, on any host.
Audit rows and rate-limit buckets label the caller `client:<name>`.

### 2. Pairing

```
fleet-hub pair --name "phone" [--mode full|readonly] [--ttl 600]
```

prints a QR code to the terminal (UTF-8 half-blocks) plus the URL underneath,
and exits. The QR encodes `<public-url>/pair#<code>`, where `<code>` is 8
crockford-base32 characters from the CSPRNG. Pending codes live in memory
only, so a restart invalidates them; each carries the requested name, mode and
expiry.

`POST /pair { "code": "..." }` (unauthenticated, rate-limited to 10 attempts
per minute per address, constant-time code comparison) consumes the code once
and answers `{ "token": "...", "name": "...", "mode": "...", "hub": "<public
url>" }`. A wrong, used or expired code answers `404` with no detail.

Why an exchange rather than a token in the QR: the QR is displayed on a
terminal that may be shared, screenshotted or logged, and a code that expires
in ten minutes and dies on first use is a much smaller thing to leak.

Management, master-token only:

- CLI: `fleet-hub client list|revoke <name>`.
- MCP tools: `list_clients`, `revoke_client` — so the desktop and an agent can
  see and cut off a phone. Minting stays CLI-only (pairing needs a terminal).

### 3. Events stream

`GET /events` (SSE), behind the same auth layer as `/mcp`:

- Each `RowChange` becomes one SSE message: `event: <row-change name>`,
  `data: <the same JSON payload the desktop event carries>`.
- A comment heartbeat every 15 s keeps proxies from idling the connection out.
- The stream starts with `event: ready` carrying the hub version and the
  server's unix time, so a client can detect clock skew.
- `?kinds=` filters by event-name prefix (`session`, `host`, `worktree`,
  `project`, `task`, `usage`); absent means everything.
- At most 8 concurrent streams per caller; a ninth gets `429` (matching the
  existing bounded-wait limit's spirit).

`fleet-core` gains `BroadcastEventBus`, a `tokio::sync::broadcast` implementation
of the existing `EventBus` trait (one required method, `emit`). The desktop keeps
`AppHandleEventBus`, which already queues through a channel so a store write never
blocks on delivery; the broadcast bus follows that shape — `emit` is a
non-blocking `send` that ignores "no subscribers".

`fleet-hub` today passes `NoopEventBus` at every store-open site
(`serve.rs` `open_store` and four others), so every row change the services emit
inside the daemon is currently dropped. `serve` switches to the broadcast bus;
the other sites (`init`, `token`, one-shot paths) keep the noop, since nothing
subscribes there.

A slow client that falls behind the channel capacity is dropped with a final
`event: lagged` rather than stalling the bus — `broadcast` already reports the
lag to the receiver, and events are a hint, not a source of truth.

Volume is modest: `session:updated` fires about once per session per reconcile
pass (20 s by default) plus once per hook, and the reconcile pass already
batches its emits until after the transaction commits, so a busy fleet produces
a burst every 20 s rather than a steady stream.

### 4. `session_conversation` tool

A thin MCP wrapper over `service::transcript::fetch_conversation`, the same
function the desktop's Tauri command calls. Parameters `{ session_id, turns? }`,
with the turn and character budgets derived by the existing
`transcript::conv_limits(turns)` (default 10 turns and 64,000 characters, up to
100 turns, hard ceiling 512,000 characters) rather than a second knob the caller
can set independently. It returns the structured `Conversation`: turns carrying
the prompt, its timestamp, the turn's end timestamp and items, where an item is
either text or a one-line tool summary flagged when that tool call failed.

It follows the existing tool shape exactly: declared in one of the
`#[tool_router]` blocks summed in `mcp/tools/mod.rs`, every parameter field
documented (a test enforces that), `audit` first, `ok_json` out, `to_mcp_err`
for errors. It joins `READONLY_TOOLS` and the lifecycle deadline class, next to
`session_transcript`, because it reads over SSH. The generated reference is
regenerated in the same commit.

`list_clients` and `revoke_client` both join `ADMIN_TOOLS`, the master-only set
(as *Open questions* #1 decides: the whole client-credential surface is
master-only, listing included — it names every paired device). `list_clients`
also stays in `READONLY_TOOLS`, since it mutates nothing; the two lists answer
different questions, who may call a tool and whether a readonly token may.

### 5. Built-in TLS

`fleet-hub serve` gains `--tls <mode>`:

- `off` (default) — today's behaviour: plain HTTP, TLS terminated by Caddy or
  a tunnel, or loopback only.
- `auto` — ACME via `rustls-acme`: TLS-ALPN-01 on the bound port (which must
  be 443 for Let's Encrypt to reach it), account and certificate cache under
  `<data-dir>/acme`, `--acme-email` required, `--acme-staging` for testing.
  The public URL's host is the certificate's domain.
- `cert` — `--tls-cert <pem> --tls-key <pem>`, watched for replacement on
  renewal (re-read on SIGHUP; no watcher).

With TLS on, the plaintext refusal from sub-project 1 is satisfied by the TLS
itself. The compose file keeps Caddy as the documented default because it also
serves redirects and HTTP/3; `docs/hub.md` gains a "single binary with its own
certificate" section for the bare-binary path.

Dependency note: `rustls-acme` pulls a TLS stack (`rustls` plus a crypto
provider). The provider is chosen by whichever passes `cargo deny check`
unchanged; if neither does, the item ships as `cert` mode only and `auto` is
deferred, rather than widening the licence allowlist.

## Data flow

```
operator terminal                phone                         hub
-----------------                -----                         ---
fleet-hub pair  ──QR──────────▶ scan
                                POST /pair {code} ───────────▶ consume code
                                ◀── {token, hub, mode} ─────── insert client row (hashed)
                                GET /events (Bearer) ────────▶ subscribe to the bus
                                ◀── event: session:updated ─── every RowChange
                                POST /mcp tools/call ────────▶ same service layer
```

## Error handling

- `/pair` with a bad, used or expired code: `404`, no body detail, attempt
  counted against the rate limit.
- A revoked client token: `401`, like any unknown token.
- `/events` beyond the per-caller limit: `429` with `Retry-After`.
- ACME failure at startup (DNS not pointing here, port 443 blocked): the hub
  logs the error and exits 1 rather than serving plaintext on the TLS port.
- ACME renewal failure while running: logged every attempt, the existing
  certificate keeps serving until it expires.

## Testing

- Store: insert, list, resolve-by-hash, revoke; a revoked row never resolves.
- Auth: a client token resolves to a client caller; readonly gating; the
  fleet-admin refusal; constant-time comparison keeps the no-short-circuit
  property (the existing test style).
- Pairing: code minted, consumed once, second use fails; expiry; rate limit;
  the QR encodes the URL the flow expects (decode it in the test).
- Events: a subscriber receives a row change; the filter drops others; the
  heartbeat arrives; a lagging subscriber gets `lagged` and is dropped; the
  per-caller cap returns 429.
- Conversation tool: returns the same JSON the service produces for a fixture
  transcript.
- TLS: `cert` mode serves HTTPS with a self-signed pair in a test; `auto` is
  covered by a staging-directory integration test marked `#[ignore]` (it needs
  the network and a real domain).
- End to end: `scripts/hub-e2e.sh` grows a pairing round trip (mint, exchange,
  use the token, revoke, confirm 401), an events check (subscribe, cause a row
  change, see the message) and a conversation-tool check.

## Open questions for the plan

1. Whether `list_clients`/`revoke_client` should also be exposed to a `full`
   client token (a phone revoking another phone). Decided: master-only, since
   `revoke_client` is the credential-management surface and `ADMIN_TOOLS` is
   already the master-only set. Revisit when the app has a settings screen.
2. Whether `/events` should carry the full row payload or only ids. Decided:
   full payloads, matching the desktop, because a session row is small and the
   alternative costs a round trip per change. `?ids_only=1` stays available if
   the mobile app measures a problem.
