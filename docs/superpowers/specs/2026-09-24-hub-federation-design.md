# Hub↔hub federation — design

Cycle 3 of the fleet-mesh work. Cycle 1 (`2026-09-22-fleet-mesh-addressing-and-delivery-design.md`)
gave every endpoint a durable address, `<fleet>/session/<host>/<name>`, and
refused a foreign fleet with `E_UNSUPPORTED` ("a hub-to-hub link is not built
yet", `service/messages.rs` `resolve_to_session_id`). Cycle 2
(`2026-09-23-smart-caching-cursors-design.md`) gave readers watermarks. This
cycle builds the link, so a session on one hub can message a session on
another hub, get a reply, and do so about as fast as inside one fleet.

## Goal and scope

- A session on hub A sends to `<B>/session/<host>/<name>`; the message lands in
  that session's inbox on hub B and reaches it the way a local message does
  (hook delivery, wake-up, `wait_for_reply`).
- The recipient replies to the sender's address and the reply comes back the
  same way, with the thread (`reply_to`) intact.
- Latency is one round-trip in either direction, not a polling interval.
- Nothing is lost or duplicated across a crash or restart of either hub. A
  message that cannot be delivered produces `message_undeliverable` for its
  sender rather than silence.

There is no concrete second fleet today. The design is sized for that: the
smallest thing that works end to end between two real hub processes, proven in
`hub-e2e.sh`, and does not rule out a foreign-owned fleet later.

### Non-goals (this cycle)

- Peer discovery. Links are made by an operator, one at a time.
- Transitive routing (A→B→C). A hub never re-forwards a message it received
  from a peer.
- Per-session allow-lists. A linked peer can address any session in the fleet,
  as any local participant can today.
- Addressing clients or hubs across a link. Only `session` addresses cross it,
  matching what a local `send_message` accepts.
- Per-peer rate limiting. Revocation is the remedy for a misbehaving peer.
- Link management from the desktop UI. `peer add` / `peer remove` are CLI-only,
  like `pair`.

## Decisions

| # | Decision | Why |
|---|----------|-----|
| D1 | One side of a link **dials**, the other **listens**. The dialer long-polls one MCP tool, `peer_exchange`, on the listener; each call carries traffic both ways. | Works whether or not the second hub is reachable from the first (NAT). Only the dialer needs a route to its peer. |
| D2 | No persistent socket. Low latency comes from long-polling plus the dialer cancelling its parked poll when it has something to send. | Avoids a new frame protocol, liveness and reconnect machinery (the open problems of `/agent`). The exchange payload is the same either way, so a socket can replace the long-poll later without a schema change. |
| D3 | The hub gains an outbound HTTPS client by **moving** the desktop's hand-written HTTP/1 + `tokio-rustls` client (`src-tauri/src/backend/remote.rs` `HubTransport`/`TcpTransport`, `backend/http1.rs`) into `fleet-core`. | No new dependency and no second TLS stack. `rustls-native-certs` is already in the lockfile and approved by `deny.toml`. |
| D4 | Links are set up with the existing pairing (single-use code, then `POST /pair`) under a new client-token mode, `peer`. | Reuses a flow that has been reviewed and tested. Revocation comes for free. |
| D5 | A `peer` token can call **exactly one** tool, `peer_exchange`. `peer_exchange` refuses every other kind of caller. | A `full` token opens every tool, including `kill_session`. A peer must never get one. |
| D6 | Loop prevention is one rule: a message received from a peer is never forwarded to another peer. | An address names its fleet explicitly, so a message always makes exactly one hop. |
| D7 | Acknowledgements travel as a watermark (`after`), as in cycle 2. Inserts are idempotent on (`remote_fleet_id`, `remote_message_id`). | Makes cancellation, lost responses and crashes safe without a two-phase protocol. |
| D8 | Anything that arrived from a peer is untrusted, always. It never reaches a pane as text, and every rendering carries the untrusted-content marker. | The peer is another operator's fleet. There is no "trusted peer" setting this cycle. |

## 1. Link and identity

### The peer token mode

- `store/clients.rs` `CLIENT_MODES` gains `"peer"`. `mcp/auth.rs` `TokenMode`
  gains `Peer`, and `TokenMode::parse("peer")` maps to it. An unknown string
  still maps to `Readonly` (fail closed, unchanged).
- The mode gate (`mcp/tools/support.rs` `enforce_mode`, and the same check in
  `mcp/tools/present.rs`) refuses a `Peer` caller on every tool except
  `peer_exchange`, with `E_FORBIDDEN`. `peer_exchange` refuses master, `full`
  and `readonly` callers.
- A `peer` token is never trusted: `set_client_trust` refuses a peer row.
- A `peer` token cannot open `GET /events`, `/report` or `/reports`. Those
  routes check the mode and refuse `Peer` with 403.

### The `peer_links` table (migration 045)

One row per link on each hub:

| column | dialer side | listener side |
|---|---|---|
| `id` | yes | yes |
| `fleet_id` TEXT, unique among live rows | the listener's, learned at the handshake | the dialer's, pinned at the first exchange |
| `role` TEXT | `dialer` | `listener` |
| `url` TEXT | the listener's base URL | NULL |
| `token` TEXT | the peer token, plaintext (`state.db` is 0600, as with `hub.client_plaintext_token`) | NULL |
| `client_id` INTEGER | NULL | the `client_tokens` row |
| `after` INTEGER | the highest listener-side message id stored here | NULL |
| `pending_rejects` TEXT (JSON) | rejections of listener messages not yet reported | NULL |
| `state` TEXT | `connected`, `retrying`, `refused`, `incompatible` | `connected` or `refused` |
| `last_exchange_at`, `last_error`, `created_at`, `revoked_at` | yes | yes |

The dialer also stores the listener's outbox watermark in `after`. The listener
needs no watermark of its own, because the dialer's `after` tells it what has
been handed over.

### Setup

1. On B, the listener: `fleet-hub pair --mode peer --name <label>` prints a
   single-use code, as it does for a phone.
2. On A, the dialer: `fleet-hub peer add <B-url> <code>`. A posts the code to B's
   `/pair`, receives the token, inserts its `dialer` row with `fleet_id` NULL
   and `state = retrying`, and the exchange loop starts on its next pass.
3. The first `peer_exchange` is the handshake. The request carries A's
   `fleet_id` (minted with `ensure_local_fleet_id` if unset). B creates its
   `listener` row, pinning that `fleet_id` to that `client_id`, and answers with
   its own `fleet_id`, which A stores.
4. From then on, B refuses an exchange whose `fleet_id` differs from the pin
   with `E_FORBIDDEN`. A refuses a response whose `fleet_id` differs from what
   it stored. Both are terminal (`refused`).
5. A `fleet_id` that equals the receiving hub's own is refused at the handshake
   (a hub linked to itself).

### Revocation

- `fleet-hub peer remove <fleet_id>` on either side stamps `revoked_at`. On the
  listener it also revokes the `client_tokens` row.
- `fleet-hub client revoke` on the listener's peer token has the same effect:
  the next exchange is refused and the dialer goes `refused`.
- On revocation, every pending outbox row on that link becomes
  `message_undeliverable` for its sender (section 3).

### Plaintext only on loopback

The dialer refuses an `http://` URL unless its host is a loopback address, the
same rule as `fleet-agent --insecure` (`crates/fleet-agent/src/conn.rs`). This is
what lets `hub-e2e.sh` link two hubs listening on `127.0.0.1` without TLS. The
flag on `peer add` is `--insecure`, with the same meaning.

## 2. The message path

### Remote participants

A foreign endpoint becomes a participant of a new kind, `remote`. Migration 045
adds `participants.address` (TEXT, unique where not NULL: the full
`<fleet>/session/<host>/<name>`) and `participants.peer_link_id`.

`session_messages.from_session_id` and `to_session_id` are `NOT NULL` from
migration 015. Instead of rebuilding the table, a remote end stores `0` there;
no session has id 0. Its true end is always its participant. Migration 045 also
adds:

- `session_messages.remote_fleet_id` TEXT and `remote_message_id` INTEGER, with
  a unique index where both are set. This is the idempotency key for inbound
  messages.
- `session_messages.peer_state` TEXT, NULL for local messages. For an outbound
  remote message: `pending`, `accepted` or `undeliverable`.

**Plan-time sweep.** Every reader that resolves a message end through
`from_session_id` / `to_session_id` (the inbox and history renderers, the
`message_sent`/`message_received` timeline details, `pane_header`, the hook
packer's `sender_label`, `reply_to` validation, `message_involves_participant`,
the retention sweep) must resolve a `0` end through its participant and render
the remote `address`. The plan lists each reader, and each gets a test.

### A → B (dialer to listener)

1. `send_message(to_addr = "<B>/session/<host>/<name>")` on A parses a foreign
   fleet. With no live link to that fleet, it is refused with `E_UNSUPPORTED`,
   with a message naming the missing link, as today.
2. With a live link:
   - `deliver = true` or `submit = true` is refused with `E_UNSUPPORTED`. A hub
     never types into a peer's panes.
   - A body over `PEER_BODY_MAX` (32 KiB) is refused with `E_VALIDATE`, so the
     sender learns immediately rather than 7 days later.
   - Otherwise the remote participant for `to_addr` is found or created, the
     message is inserted with `to_session_id = 0`,
     `to_participant_id = <remote participant>` and `peer_state = pending`, and
     `send_message` returns its id. The `message_sent` event is written as
     usual.
3. The exchange loop on A (below) sends it on its next call, or cancels its
   parked call to send it now.

### The `peer_exchange` tool

Request:

```json
{
  "proto": 1,
  "fleet_id": "<caller's fleet>",
  "send": [
    { "id": 17, "from_addr": "<A>/session/h/a1", "to_addr": "<B>/session/h/b1",
      "body": "…", "kind": "message", "reply_to": { "fleet": "<A or B>", "id": 12 },
      "sent_at": 1790000000, "wake": true }
  ],
  "after": 40,
  "results": [ { "id": 38, "status": "rejected", "code": "E_PARTICIPANT_UNKNOWN", "message": "…" } ],
  "wait_ms": 25000
}
```

Response:

```json
{
  "proto": 1,
  "fleet_id": "<listener's fleet>",
  "results": [ { "id": 17, "status": "accepted" },
               { "id": 18, "status": "rejected", "code": "E_PARTICIPANT_UNKNOWN", "message": "…" } ],
  "messages": [ /* same shape as send[], the listener's outbox for this link, id > after, oldest first */ ],
  "more": false
}
```

- `send` and `messages` each hold at most `PEER_BATCH_MAX` (50). A request with
  more is refused whole with `E_VALIDATE`. The listener sets `more` when it held
  back rows.
- `wait_ms` is clamped to `PEER_WAIT_MAX_MS` (25 000). The listener returns
  immediately if `send` was non-empty, `messages` is non-empty, or `more` is
  set. Otherwise it waits on the store's `message_notify` (as `wait_for_reply`
  does) until this link has an outbox row past `after`, or the wait expires.
- `proto` other than 1 is refused with `E_UNSUPPORTED`. The dialer treats that
  as terminal (`incompatible`).

### The listener receiving

Each item in `send` is checked, then inserted in its own transaction:

1. `from_addr` parses as a `session` address whose fleet equals the link's
   pinned `fleet_id`. A peer cannot speak for a third fleet, or for the
   listener's own fleet.
2. `to_addr` parses as a `session` address in the listener's own fleet, naming
   a live, non-retired session.
3. `body` is non-empty and at most `PEER_BODY_MAX`.
4. `reply_to`, when set, maps to a local message (below) that involves the
   recipient.
5. The insert is keyed on (`remote_fleet_id = pinned fleet`,
   `remote_message_id = id`). A duplicate is `accepted` again, with no second
   row and no second wake.

A failed check becomes a `rejected` result with its `E_*` code. It never fails
the whole exchange. An accepted message is inserted with
`from_participant_id = <remote participant for from_addr>`,
`from_session_id = 0`, and the `message_received` event naming the address.
From there cycle 1 takes over unchanged, except for the wake (below).

### The dialer receiving

The dialer applies the same checks to each item in `messages`, with the roles
swapped: `from_addr` must be in the listener's fleet and `to_addr` in its own.
In one transaction it inserts the accepted items, records each rejected item
(`id`, `code`, `message`) in the link row's `pending_rejects` (a JSON column),
and sets `after` to the highest id in the batch.

The next request carries `pending_rejects` as its `results` field (same shape
as the response's). The listener applies `results` before `after`:

- a rejected id becomes `undeliverable`, with `message_undeliverable` for its
  sender;
- every other row of this link at or below `after` becomes `accepted`.

The dialer clears `pending_rejects` only when that request succeeds. A lost
request just carries them again, and applying a rejection twice is a no-op.
Either direction therefore ends in an answer for the sender.

For its own `send`, the dialer sets `peer_state` from each result: `accepted`,
or `undeliverable` plus a `message_undeliverable` event for the sender, carrying
the code and message.

### The wake-up for remote messages

Cycle 1's wake pastes `pane_header(id, sender, host, body)`, the full body, and
presses Enter. For a message from a peer that would put a foreign fleet's text
into a live session as a prompt. So, for a message whose sender is a `remote`
participant:

- The wake pastes a fixed line, `[fleet] message #<id> from another fleet is in
  your inbox`, containing only the local message id. It never includes the body
  or the address, both of which the peer controls.
- The `wake_action` guard (never when blocked, stuck or unknown) is unchanged.
- The sender's `wake` flag is honoured, and nothing else about the pane is
  (`deliver` and `submit` never cross the link).

A test asserts that the body of a remote message never reaches
`send_system_prompt`.

### Untrusted rendering

Wherever a remote message's body is rendered (the hook's `additionalContext`
and Stop `reason` from `service/delivery.rs` `pack`/`pack_within`, `inbox`,
`session_history`, the transcript's message view), it is wrapped in the same
untrusted-content marker `mcp::tools::apply_marker` applies to an untrusted
client's text. The label is the remote address. The delivery caps (8000 chars /
200 lines; Stop 2000 / 20) count the marker.

### Replies and threads

The recipient sees the sender as its address and replies with that `to_addr`.
`reply_to` travels as `{fleet, id}`:

- `fleet` equal to the receiving hub's own fleet means `id` is its own message
  id.
- `fleet` equal to the link's peer means the receiver looks up the row with
  `remote_fleet_id = fleet` and `remote_message_id = id`.
- Anything else is refused (`E_INVALID`).

The sending hub fills it the same way in reverse: a parent it originated goes
as `{own fleet, own id}`, and a parent it received goes as
`{remote_fleet_id, remote_message_id}`. `reply_to` validation stays by
participant (cycle 1, final review Important 3), so a remote participant can be
part of a thread.

### The exchange loop (dialer)

One task per live `dialer` link, started at hub boot and by `peer add`:

```
loop:
  batch  = pending outbox rows for this link, oldest first, ≤ 50
  wait   = batch empty ? PEER_WAIT_MAX_MS : 0
  select:
    r = exchange(batch, after, wait)   -> apply results + messages; reset backoff
    local notify with a new pending row for this link, while batch was empty
                                       -> drop the in-flight call; continue
    link revoked / shutdown            -> exit
  on transport error -> backoff (section 3)
  on refusal         -> state = refused | incompatible; exit
```

Dropping the in-flight call is safe (D7): whatever the listener put in that
lost response is re-sent, because `after` did not move. A dropped call may leave
the listener's handler parked for up to `PEER_WAIT_MAX_MS`. That handler only
reads, so a second concurrent call from the same link is harmless.

### Latency

- B → A: the listener answers the parked poll as soon as a row for this link is
  inserted. Cost: the rest of one response.
- A → B: A cancels its parked poll and dials immediately. Cost: one request.

Both are one round-trip, as in D2.

## 3. Failure and retry

### Everything is in SQLite

Pending rows, `peer_state`, the pinned `fleet_id` and `after` all live in
`state.db`. The loop restarts from them at boot.

| Crash point | What happens | Result |
|---|---|---|
| Listener inserted A's message; the response was lost | A resends; the idempotency key matches; `accepted` again | one row |
| Dialer received B's batch and crashed before committing | `after` did not move; B hands the batch over again; A dedupes on the key | one row |
| Dialer cancelled its parked call to send | as the previous row | nothing lost |
| Either hub restarts | the loop resumes from the rows | resumes |

### Retry

- **Transport failure** (connect error, TLS error, a timeout past
  `wait_ms + 10 s`, HTTP 5xx, a malformed response): exponential backoff from
  1 s, doubling to a 60 s cap, with ±20 % jitter. It resets after one
  successful exchange. `state = retrying`, `last_error` set.
- **Refusal** (HTTP 401, a structured tool refusal — `E_FORBIDDEN` /
  `E_UNAUTHORIZED`, `isError` plus a code — a `fleet_id` mismatch, `proto`
  refused, the peer not knowing `peer_exchange`): terminal. `state = refused`,
  or `incompatible` for the last two, and the loop exits. `peer add` again
  (re-pair), or upgrading the older hub, is the way back. A bare HTTP 403 with
  no such body — the shape a proxy in front of the listener sends, not the
  hub's own authorize middleware — is a transport failure instead (retried
  with backoff, above): only the hub's own answer is authoritative that the
  token itself is bad. A terminal link's
  pending rows stay pending until the 7-day sweep, so a re-pair within a week
  still delivers them. When a new token's first exchange returns a `fleet_id`
  that already has a non-revoked `dialer` row in state `refused` or
  `incompatible`, the new `url` and `token` are written onto that existing row
  and the temporary row is dropped. The pending rows therefore stay attached
  to the same link, and the unique `fleet_id` is never violated. A handshake
  claiming a fleet whose live dialer row is in any other state (`connected`,
  `retrying`) is refused: the NEW row goes terminal `refused` with
  `fleet <id> is already linked (link <N>); remove it first with fleet-hub
  peer remove`, and the live row is untouched — otherwise any newly paired
  hub could take over a working link to a third fleet. The dialer checks the
  answered `fleet_id` before anything else: malformed is `incompatible`, this
  hub's own id is `refused`.

### Retention

The sweep that already runs outside `gc.enabled` (the read-cursor and
retired-participant sweeps in `service/gc.rs`) gains one step:

- A `pending` row older than `PEER_PENDING_MAX_SECS` (7 days, the same span as
  `RETIRED_RETENTION_SECS`), or any `pending` row on a revoked link, becomes
  `undeliverable`, with a `message_undeliverable` event for its sender.
- A `remote` participant with no messages left is deleted with the retired
  participants.

### Clocks and ordering

A remote `sent_at` is informational only: it travels on the wire (so a future
consumer could log it) but nothing on the receiving hub stores or shows it —
not the row, not the inbox, not the transcript. Ordering, the inbox, retention
and cursors all use the local id and the local insert time, so a peer's
skewed clock cannot reorder an inbox or dodge the sweep, and there is no
displayed timestamp for it to lie about either.

### Limits a peer cannot bypass

- `PEER_BODY_MAX` = 32 KiB per body, on both sides.
- `PEER_BATCH_MAX` = 50 items per direction per exchange, and
  `PEER_PAGE_MAX_BYTES` = 512 KiB: both the dialer's `send` page and the
  listener's `messages` page are cut once their items' serialized size
  reaches it, always keeping at least one item (the listener sets `more`).
  A count cap alone is not enough: items are double-encoded and SSE-framed,
  control-heavy bodies grow several-fold in JSON, and 50 × 32 KiB can exceed
  the dialer's 8 MiB answer cap or a proxy's body limit — a size failure is
  a transport failure, so the same page would be re-sent until the sweep.
  A proxy in front of a listening hub must allow request bodies of at least
  1 MiB.
- `PEER_ADDR_MAX` = 256 bytes for `from_addr` and `to_addr`, on both sides.
  An address becomes the sender label of the recipient's hook delivery; the
  hook packer also drops a label that cannot fit, so one item can never
  stall a session's delivery queue.
- `kind` `question` does not cross a link (it holds the recipient's `Stop`
  hook): the listener rejects it `E_VALIDATE`, and `send_remote` refuses it.
- `PEER_WAIT_MAX_MS` = 25 000, under common reverse-proxy idle timeouts. The
  plan checks the hub's own MCP request timeout and the NAS proxy
  (`fleet.rlt.sk`) against it.

## 4. What the operator sees

- **`list_peer_links`**, a new hub-only tool. It lists each link's `fleet_id`,
  role, state, `last_exchange_at`, `last_error` and pending count. It is
  master-only (as built): it names every fleet this hub is linked to, which
  no paired client needs. It is not called `peer_status`, because that name
  is the existing session-peer tool.
- **`fleet-hub peer list | add | remove`**, the same information and the
  management.
- **`fleet_health`**, one roll-up line when any live link is outside
  `connected`.
- **Desktop in hub-client mode.** Nothing (as built): `list_peer_links` is
  an MCP tool with no Tauri command, so it has no row in
  `src-tauri/src/backend/verdicts.rs`. No desktop UI this cycle.
- **`docs/hub.md`**, a new section, "Link two hubs": setup, revocation, what a
  peer can and cannot do. `docs/control-api.md` gets a short "Across a hub
  link" paragraph under messaging.
- Tool descriptions stay short (the `BUDGET_BYTES` test in
  `mcp/tools/tests.rs`). Measure it, never guess. Prose goes into the docs.
  `REGEN_DOCS=1` for the reference, and `REGEN_HUB_CONTRACT=1` if a wire type
  changes.

### As built

Controller rulings during the build changed the design above as follows
(each is also reflected where it applies):

- **Listener rebind only after revocation.** A listener row is rebound to a
  new peer client token only when its current token is revoked (or gone);
  a second live token claiming the fleet is refused `E_FORBIDDEN`, naming
  `fleet-hub client revoke`.
- **Dialer merge only into a stopped row.** The dialer-side mirror: a re-pair
  merges into a `refused` / `incompatible` row only (§3 Retry).
- **Link-scoped results.** A peer's `accepted` / `rejected` results settle
  only rows on its own link; the listener ignores `accepted` entries and
  hands rows over by `after` alone.
- **One wake per recipient per exchange,** with the nudge for its last new
  message, on a status re-read after all inserts.
- **Token-fenced dialer writes.** Every state or progress write a dialer loop
  makes applies only while the row still carries the token the loop started
  with; a loop outlived by a re-pair stops as `Superseded`.
- **Store faults retried, not rejected.** A store failure while applying a
  peer's item fails the exchange (non-terminal `E_INTERNAL`) instead of
  rejecting the item; the resend is idempotent.
- **Peer text on the timeline** is tagged `(untrusted, another fleet)` and
  scrubbed to one line; a `reply_to` that does not resolve reads the same
  whether the message is missing or not the recipient's, and a reply across
  a link cannot thread onto a third fleet's message.
- **Size caps:** `PEER_PAGE_MAX_BYTES`, `PEER_ADDR_MAX`, no `question` (§3
  Limits).
- **A reply must thread onto a message both ends actually share.** The
  sender's check is `(involves the sender) AND
  message_involves_participant(parent, recipient)` — a parent received
  from the recipient already satisfies the second half, since its
  `from_participant` is the recipient. The receiver's own-fleet branch of
  `reply_to` accepts only a local id that went outbound *on this link*
  (`peer_state IS NOT NULL` and the recipient's `peer_link_id` matches),
  not any local message the recipient happens to be part of — so a reply
  can thread onto the two fleets' own conversation, never onto a third
  fleet's traffic or a purely local thread that merely shares a
  participant.
- **A forged untrusted-block closer is neutralised, not trusted.** After
  stripping a peer body's own leading marker lines, every remaining line
  that equals or starts with the untrusted-content marker is prefixed
  `> ` before storage, so a body cannot manufacture the marker's closing
  line and have the rest of itself read as fleet's own words.
- **Wakes are bounded per exchange, not per recipient.** Nudging every
  new message's recipient in one exchange is capped at `WAKE_TIMEOUT` (5 s)
  total; a session too busy to respond in time just misses that exchange's
  nudge, not the message.
- **An immediately-empty parked answer still backs off.** A long-poll that
  was actually parked (`wait_ms > 0`) and returns with nothing in it in
  under `EMPTY_POLL_FLOOR` (1 s) — a peer or proxy answering every parked
  call at once — is followed by the ordinary backoff before the next call,
  the same as a transport failure, so two such dialers cannot spin each
  other.
- **`last_exchange_at` moves on success only,** never on a failed or
  refused attempt, so it means what an operator reading `peer list` expects
  it to mean. `fleet_health`'s `peer_links_down` also now counts a live
  *listener* link as down — a dialer link already shows its trouble in
  `state` — when its client token has been revoked or it has gone
  `LISTENER_STALE_SECS` (120 s) with no served exchange.
- **`fleet-hub client revoke` on a listener's peer token acts immediately,
  not just on the next exchange.** A call already parked in
  `peer_exchange` returns at once instead of waiting out `wait_ms`, and a
  further `send_message` to that fleet is refused `E_UNSUPPORTED`. Unlike
  `fleet-hub peer remove`, revoking the token does not touch the link's
  pending rows — they stay attached for a re-pair within the retention
  window, same as any other terminal link (§3 Retry).
- **Fleet-id pinning at mint time was considered and declined.** Trust in a
  link is established once, at the handshake: a pairing code only reaches
  someone who can run commands on the *other* hub, and the first exchange
  pins whichever `fleet_id` that hub answers with. A separate allowlist of
  expected fleet ids was not built, because a fleet id is not secret and
  requiring one in advance would mean the listener's operator already knows
  the dialer's id before pairing — which the code exists to avoid needing.
  This could still be added later as an extra check, not a replacement for
  the handshake pin.

## 5. Components

| Unit | Where | Does | Depends on |
|---|---|---|---|
| HTTP client | `fleet-core/src/http_client/` (moved from `src-tauri/src/backend/{remote.rs,http1.rs}`) | `HubTransport` trait, `TcpTransport`, HTTP/1 parsing | `tokio-rustls`, `rustls-native-certs` |
| Peer wire types | `fleet-core/src/service/peer/wire.rs` | request/response structs, `proto`, limits | serde |
| Validation | `service/peer/validate.rs` (pure) | the receiving checks, `reply_to` mapping | `service/address` |
| Link store | `store/peer_links.rs` | link rows, outbox query, idempotent insert, `after` | migration 045 |
| Listener | `service/peer/listen.rs` + `mcp/tools/peer.rs` | `peer_exchange` handler, long-poll | store, validation |
| Dialer | `service/peer/dial.rs` | the exchange loop, backoff, state | HTTP client, store, validation |
| Mode gate | `mcp/auth.rs`, `mcp/tools/support.rs`, `present.rs` | `TokenMode::Peer` | — |
| CLI | `crates/fleet-hub/src/main.rs` | `peer add/list/remove`, `pair --mode peer` | service |

The desktop keeps using the HTTP client through the moved module.
`src-tauri/src/backend/remote.rs` re-exports or imports it, so its call sites
do not change.

## 6. Testing

1. **Pure units.** Batch validation (the pin, `from_addr` fleet, `to_addr`
   ownership, the body and batch caps); `reply_to` mapping in both directions;
   the backoff schedule; classifying transport failure versus refusal; the
   state transitions.
2. **Store.** Migration 045 on an empty and on a populated database; the outbox
   query; handing over by `after`; the duplicate insert being ignored; the
   remote participant; the pending-to-undeliverable sweep (7 days and
   revocation) with its event; every reader from the section 2 sweep rendering
   a `0` end through its participant.
3. **Two hubs in one process, the main proof.** Two `Store`s and two service
   stacks, wired through a fake `HubTransport` that calls the other side's
   `peer_exchange` handler directly and can drop, delay or fail a call. One
   test per row of the section 3 crash table, plus: a revoked token, a
   `fleet_id` mismatch, an unknown `proto`, a self-link, a peer claiming a third
   fleet, `deliver = true` refused, and a reply thread across the link. Each
   asserts one row, or `undeliverable` with the code.
4. **Security guards.** A `Peer` caller is refused on every tool in the real
   router except `peer_exchange`, iterating the router so a tool added later is
   covered. `peer_exchange` refuses master, `full` and `readonly`. An unknown
   mode parses as `Readonly`. `set_client_trust` refuses a peer row. A remote
   message's wake pastes the fixed line and never the body. A remote body is
   marker-wrapped in the hook output.
5. **The moved HTTP client.** The desktop's `tests_http1` and `tests_remote`
   move with it and pass unchanged.
6. **`scripts/hub-e2e.sh`, a two-hub scenario.** Two fresh hubs with their own
   data dirs and local-host on:
   1. `pair --mode peer` on hub 2, then `peer add --insecure` on hub 1.
   2. A session on hub 1 sends to a session on hub 2 by address. Check the
      inbox row on hub 2, with the marker, and that a real `/hook` call returns
      it in `additionalContext`.
   3. Hub 2 replies, and hub 1's `wait_for_reply` returns the reply within the
      poll window.
   4. Stop hub 2, send, restart hub 2: delivered once.
   5. `peer remove`: pending messages become `message_undeliverable`.
   6. The peer token is refused on `list_sessions`.

## Risks and open points for the plan

- **The move of the HTTP client** touches the desktop's hub-client path.
  Mitigation: a mechanical move first, as its own task, with the existing tests
  as the guard, before anything uses it from the hub.
- **The `0` sentinel** relies on the reader sweep being complete. Mitigation:
  the plan enumerates readers by `grep` on `from_session_id|to_session_id`, and
  the store tests cover each.
- **Long-poll through the MCP layer.** Whether rmcp's streamable HTTP server
  holds a tool call for 25 s without its own timeout, and how it behaves when
  the client disconnects mid-call, is checked in the first listener task
  before the design relies on it. The fallback is a shorter `wait_ms`, which
  costs latency, not correctness.
- **CI clippy is newer than local.** Wait for GitHub CI on the final head.
