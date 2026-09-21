# Hub error channel: reports from every participant, in one place

**Date:** 2026-09-21
**Status:** Approved design, awaiting implementation plan
**Priority:** low — a debugging aid; it must never get in the way of the work
it observes.

## Why

Every participant of a fleet logs to itself alone. The hub and the desktop
write hourly-rotated files through `fleet_core::logging`; a `fleet-agent`
writes to journald on a host the hub cannot even SSH into; a paired phone has
nothing. When something goes wrong on a hub-client desktop or on an agent
host, the operator has to reach that machine to learn what. The desktop's
"Copy diagnostics" bundle helps for one machine and only for the desktop.

The hub already sits in the middle of every one of these connections. This
design makes it the one place errors are collected: the desktop in hub-client
mode, every agent, the hub itself, and (documented, not built here) a phone
client send their error-level events to the hub, which keeps a bounded,
redacted table of them and lets the operator read it from the CLI.

## Goals

- **One place to look.** `fleet-hub reports` shows the recent errors of every
  participant, newest first, naming where each came from.
- **Never in the way.** Reporting is fire-and-forget on every sender: a
  bounded in-memory queue, batched flushes, no `await` on the path that
  produced the error, and dropped — counted, not blocked — when the queue is
  full or the hub is away.
- **Bounded on the hub.** A row cap pruned on insert, an age sweep on the
  reconcile tick, a per-origin rate limit, a body cap, and a cap on every
  string. Worst case the table is about 30 MB; typical is kilobytes.
- **No secrets, no bodies.** Every string is run through the existing
  redaction at ingest. Prompts, transcripts and pane text never reach a
  tracing event today, and this design adds no path for them.
- **Old and new can mix.** A hub older than this design ignores an agent's
  report frame (the protocol's unknown-kind rule) and answers a desktop's
  `POST /report` with `404`, which the desktop treats as "not supported" and
  stops trying for the rest of its run.

## Non-goals

- A desktop panel or an MCP tool for reading reports. The channel is for the
  operator with shell access to the hub; an assistant that needs it can run
  the CLI. (Both can be added later without changing anything here.)
- Warnings. Only `error`-level tracing events and frontend crashes are sent.
  The reconcile tick warns once per unreachable host per pass, which would
  drown the channel.
- Metrics, tracing spans or a log-shipping pipeline. This is a debug channel
  for a fleet of a handful of machines, not observability infrastructure.
- Reporting from a standalone desktop. It has no hub; its log file is its
  channel.

## Vocabulary

- **Report** — one record: when, how bad, from which component, an optional
  `E_*` code, a message, a small JSON context.
- **Origin** — who sent it, derived on the hub from the caller's token, never
  from the body: `client:<name>` for a paired desktop or phone,
  `host:<alias>` for an agent, `hub` for the hub's own events.
- **Batch** — what travels: up to a fixed number of reports plus a count of
  reports the sender dropped since its previous batch.

## Architecture

```
desktop (hub client)                 fleet-agent                   fleet-hub
┌───────────────────────┐   ┌──────────────────────┐   ┌──────────────────────────┐
│ tracing error! ──┐    │   │ tracing error! ──┐   │   │ tracing error! ──┐       │
│ frontend crash ──┤    │   │                  │   │   │                  ▼       │
│ pushError toast ─┤    │   │           ReportRing │   │           ReportRing     │
│                  ▼    │   │           (256, cap) │   │           (256, cap)     │
│           ReportRing  │   │                  │   │   │                  │ tick  │
│           (256, cap)  │   │   heartbeat, ≤16 ▼   │   │                  ▼       │
│      5 s / 20 queued  │   │  AgentFrame::Report ─┼───┼──▶ ingest ──▶ error_reports
│               ▼       │   └──────────────────────┘   │       ▲         (≤ max_rows,
│  POST /report (≤50) ──┼──────────────────────────────┼───────┘          ≤ max_age)
└───────────────────────┘                              │                    │
                                                       │  GET /reports ◀────┘
phone (documented contract): POST /report              │  fleet-hub reports
                                                       └──────────────────────────┘
```

Three senders, one ingest function, one table, one reader.

## The record and the batch (`fleet-proto`)

Both the HTTP body and the agent frame carry the same shapes, so they live in
`fleet-proto`, which both ends already depend on. `fleet-proto` gains no
dependency: these are two serde structs and one bounded queue over `std`.

```rust
// crates/fleet-proto/src/report.rs
pub struct Report {
    /// The sender's clock, unix seconds.
    pub at: i64,
    /// `"error"`; `"warn"` is accepted on the wire for a future sender that
    /// opts into it, and nothing else is.
    pub level: String,
    /// Where in the sender it happened: the tracing target
    /// (`fleet_core::ssh`), or `frontend` / `frontend:unhandled` for the
    /// desktop's web view. At most `COMPONENT_MAX` (64) chars.
    pub component: String,
    /// The `code` field of the tracing event when it has one, or the
    /// `IpcError` code of a frontend error. `E_`-shaped or absent.
    pub code: Option<String>,
    /// The event's message plus its other fields as ` key=value` pairs. At
    /// most `MESSAGE_MAX` (2 048) chars; the sender truncates.
    pub message: String,
    /// Small structured extras (a frontend stack, a URL). At most
    /// `CONTEXT_MAX` (4 096) bytes serialized; the sender drops it and sets
    /// `truncated` when larger.
    pub context: Option<serde_json::Value>,
    /// Set by the sender when `message` or `context` was cut.
    pub truncated: bool,
}

pub struct ReportBatch {
    pub reports: Vec<Report>,
    /// Reports the sender's queue dropped since the previous batch.
    pub dropped: u32,
}
```

Constants next to them, so every end agrees: `COMPONENT_MAX = 64`,
`MESSAGE_MAX = 2_048`, `CONTEXT_MAX = 4_096`, `HTTP_BATCH_MAX = 50`,
`FRAME_BATCH_MAX = 16`, `RING_CAP = 256`, `BODY_MAX = 64 * 1024`.

`Report::clamp(&mut self)` applies the caps (truncating `message` at a char
boundary, replacing an oversize `context` with `None`, setting `truncated`)
and is called by every sender before queueing and by the hub at ingest, so a
sender that skipped it cannot push an oversize row.

### `ReportRing`

A `Mutex<VecDeque<Report>>` with a hard cap of `RING_CAP` and a `dropped: u32`
counter. `push` evicts the oldest when full and bumps `dropped`. `drain(n)`
takes up to `n` oldest reports and the current `dropped` count, resetting it.
The critical section is a push or a pop: nothing formats, allocates
unpredictably, or logs while holding the lock — a tracing layer calls `push`
from inside an event, and a layer that logs re-enters the subscriber.

### The tracing layer

Each binary installs a `ReportLayer` (a `tracing_subscriber::Layer`) that, on
an event at `Level::ERROR` only, visits the fields once into a `Report`
(`message` from the `message` field, `code` from a `code` field, every other
field appended as ` key=value`, `component` from the event's target), clamps
it, and pushes it into the process-wide ring. It is installed alongside the
file layer in `fleet_core::logging::init_in_with` (desktop and hub) and next
to the `fmt` subscriber in `fleet-agent`'s `main`. The same filter that
governs the file applies, so a dependency's `error!` under `warn` for
dependencies still arrives, and `RUST_LOG=off` silences both.

The layer is ~40 lines; `fleet-agent` gets its own copy (it depends on
`fleet-proto` only, and `fleet-proto` stays free of `tracing-subscriber`).
The visitor and clamp it calls are in `fleet-proto`, so the copy is the
`Layer` impl and nothing else.

## Ingest on the hub

One function, `service::reports::ingest(store, origin: &str, batch:
ReportBatch) -> Result<Ingested, IpcError>`, used by both routes in:

1. Refuses a batch over the batch cap for its transport, or a report whose
   `level` is not `error`/`warn`, with `E_VALIDATE`.
2. Clamps every report (the caps above), then runs `component`, `code`,
   `message` and the serialized `context` through `logging::redact` — the
   same bearer-token, `?token=` and 64-hex masking every log line gets.
3. Inserts the batch in one transaction, then prunes the table to
   `reports.max_rows` newest rows with the timeline's indexed-subselect
   pattern (`DELETE … WHERE id NOT IN (SELECT id … ORDER BY received_at
   DESC, id DESC LIMIT ?)`).
4. Writes one line per report to the hub's own log, at `warn`, under target
   `fleet_core::report`, with `origin`, `level`, `component`, `code` and the
   message — so the log file the operator already tails carries the channel
   too, and `journalctl -u fleet-hub | grep report` works. A batch's
   `dropped > 0` is one more `warn` line naming the origin and the count.

`Ingested { stored: usize, dropped_by_sender: u32 }` is what the route
answers with.

### Rate limit

`ReportState` holds a `Mutex<HashMap<String, (window_start: i64, count:
u32)>>` keyed by origin: a fixed one-minute window of `RATE_PER_MINUTE = 60`
reports. A batch that would cross it is refused whole with `429` (HTTP) or
dropped with one `warn` per window (agent frame), and nothing of it is stored.
The map is pruned of windows older than a minute on every insert, so it
cannot grow past the number of live origins. The hub's own ring is exempt.

### The table (migration 040)

```sql
CREATE TABLE IF NOT EXISTS error_reports (
  id          INTEGER PRIMARY KEY,
  received_at INTEGER NOT NULL,   -- the hub's clock
  at          INTEGER NOT NULL,   -- the sender's clock
  origin      TEXT    NOT NULL,   -- client:<name> | host:<alias> | hub
  level       TEXT    NOT NULL,
  component   TEXT    NOT NULL,
  code        TEXT,
  message     TEXT    NOT NULL,
  context     TEXT,               -- JSON, already clamped and redacted
  truncated   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS error_reports_recent
  ON error_reports(received_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS error_reports_by_origin
  ON error_reports(origin, received_at DESC);
INSERT OR IGNORE INTO schema_version (version) VALUES (40);
```

Store helpers in a new `store/reports.rs`: `insert_reports(origin, &[Report],
now) -> usize`, `prune_reports_to(max_rows)`, `sweep_reports_older_than(cutoff)
-> usize`, `list_reports(filter: ReportFilter) -> Vec<ReportRow>` where
`ReportFilter { limit, since, origin, level }`. All hold the store guard for
one statement; none awaits.

### Retention

Two settings in `service::settings::SPECS`:

| Key | Kind | Default | Meaning |
|---|---|---|---|
| `reports.max_rows` | `Int { min: 100, max: 100_000 }` | `5000` | Newest rows kept; pruned on every insert. |
| `reports.max_age_secs` | `Secs` | `604800` (7 days) | Rows older than this are deleted on the reconcile tick; `0` disables the age sweep (the row cap still holds). |

The age sweep is one statement under the store lock in `service::tick`, next
to the open-task sweep, and logs at `info` only when it deleted something.
The hub's own ring is drained into the table on the same tick, with origin
`hub`, so `fleet-hub reports` shows the hub's errors beside everyone else's.

Both settings are the hub's (`fleet-hub`'s `state.db`) and are set the way
every other setting is: `set_setting` over the control API, or by the
desktop's Settings when it owns the fleet. The desktop in hub-client mode
does not see them; it has its own off switch below.

## Senders

### The desktop, in hub-client mode

Three sources feed the process-wide ring:

- **Backend errors:** the `ReportLayer` installed by `logging::init`, which
  runs before the backend resolves, so nothing is missed — a startup error
  waits in the ring until the flusher starts.
- **Frontend crashes:** `src/lib/error_report.ts` installs a `window`
  `error` listener and an `unhandledrejection` listener from `main.ts`, and
  `toasts.ts`'s `pushError` calls its `reportError(error, context)` too, so
  every error the user sees is also reported. Each becomes an
  `invokeCmd('report_client_error', { level, component, code, message,
  context })`. The frontend applies its own guard first: at most 20 reports
  per minute, and an identical `component + message` within 60 s is counted,
  not resent (the count rides in `context.repeats` of the next distinct one)
  — a render loop must not become a request loop.
- **The command:** `report_client_error` (in `src-tauri/src/commands/hub.rs`)
  clamps the report and pushes it into the ring. Its verdict is
  `SameInBoth { why: "about this window: a standalone desktop has no hub to
  report to and the push is a no-op" }`. It is a new Tauri command, so
  `tests_routing.rs` gets its row, `REGEN_HUB_VERDICTS=1` regenerates the
  verdict table, and `REGEN_DOCS=1` regenerates the reference.

**The flusher** (`src-tauri/src/backend/report.rs`) is spawned from
`start_background_tasks` only for `Backend::Remote`, and not at all when the
environment variable `CLAUDE_FLEET_HUB_REPORTS` is `0` or `false` (the same
shape as `CLAUDE_FLEET_LOG_STDERR`; there is no setting, because in
hub-client mode the settings commands route to the hub and a local key would
have nothing to flip it). Every 5 s, or as soon as the ring
holds 20, it drains up to `HTTP_BATCH_MAX`, runs each string through
`logging::redact`, and POSTs one `ReportBatch` to `<hub>/report` through the
same `HubTransport` the tool calls use, bearer = the client token, with a
10 s timeout. It never awaits anything else, and the result only steers
itself:

| Answer | Then |
|---|---|
| `204` | Done. |
| `404` | The hub predates this route: log once at `info`, stop the flusher for this run. |
| `401` / `403` | The token is dead or the client is refused: log once, stop — the window is already showing the banner for this. |
| `429` | Over budget: keep the batch, wait one full minute before the next flush. |
| Anything else, or no answer | Keep the batch (the ring caps it), back off 5 s → 10 s → 20 s → 40 s → 60 s until an answer. |

The flusher's own errors go to the log at `warn`, and — because the layer
only captures `error` — never back into the ring. A `RemoteConfig` value,
the token above all, is never in any of its messages.

### The agent

`fleet-agent`'s `main` installs its `ReportLayer` beside the `fmt`
subscriber. The config file gains `report_errors: bool` (serde default
`true`); with it off the layer is not installed and nothing else changes.

The flush point is the heartbeat branch of `conn::serve`: on every beat, once
`welcomed`, drain up to `FRAME_BATCH_MAX` reports from the ring and, if there
are any (or `dropped > 0`), send one `AgentFrame::Report { reports, dropped }`
through the existing writer channel. The frame is encoded with
`encode_agent_frame_within(MAX_FRAME_BYTES)`; 16 clamped reports are under
100 KB, far inside the cap, and an encode failure drops that batch with one
`warn`. Nothing waits on an answer: the hub sends none.

An error that happened while the agent was *disconnected* — why it could not
reach the hub, above all — waits in the ring and is flushed on the first beat
after the next successful handshake. That is the case the channel exists
for.

### The protocol change (`fleet-proto`)

```rust
AgentFrame::Report {
    #[serde(default)] reports: Vec<Report>,
    #[serde(default)] dropped: u32,
}
```

`PROTO_VERSION` does not move: the crate doc's rule is that an unknown kind
after the handshake is ignorable, and a hub that predates this frame logs
`unknown frame kind; skipping` once per connection and carries on. Both
`agent_frame_id` (registry) and `answered_id` (ws) return `None` for it, and
`ws.rs` handles it *before* `registry.deliver` — which would otherwise drop a
frame with no id — by calling `service::reports::ingest` with origin
`host:<alias>` and the store the `AgentWsState` already holds. The store
lock is taken inside `ingest` for the insert only; the WebSocket read loop
never awaits on it.

### The routes (`crates/fleet-core/src/mcp/report_route.rs`)

Both sit behind `authorize`, merged into `build_app` the way `/events` is,
with their own `ReportState { store, buckets }`.

**`POST /report`** — any authenticated caller, `readonly` included: reporting
an error changes nothing about the fleet, and a read-only phone is exactly
the kind of client the operator cannot otherwise see into. A per-host token
is accepted too (a script on a host may use it), giving origin `host:<alias>`.
`DefaultBodyLimit::max(BODY_MAX)` on the route; the body is a `ReportBatch`.

| Status | When |
|---|---|
| `204` | Stored. |
| `400` | Not a `ReportBatch`, more than `HTTP_BATCH_MAX` reports, or a level other than `error`/`warn`. |
| `413` | Body over `BODY_MAX`. |
| `429` | The origin's minute is spent. Nothing of the batch is stored. |
| `500` | The store failed; logged at `error` on the hub (which its own ring will then carry). |

**`GET /reports`** — master token only (`403` for a client or host token:
messages from every client are the operator's to read, not each other's).
Query: `limit` (default 100, max 1 000), `since` (unix seconds), `origin`,
`level`. Answers a JSON array of rows, newest first, each
`{ id, received_at, at, origin, level, component, code, message, context,
truncated }`.

### `fleet-hub reports`

```
fleet-hub reports [--limit N] [--since <duration|unix>] [--origin <label>] [--json]
```

Drives the running hub over loopback like `client list` does, but through
`GET /reports` rather than a tool. `--since` accepts `30m`, `2h`, `3d` or a
unix timestamp. The table:

```
RECEIVED           ORIGIN            LEVEL  COMPONENT                 CODE               MESSAGE
2026-09-21 10:41Z  host:build-box    error  fleet_agent::conn         -                  dial https://fleet.example.com: connection refused
2026-09-21 10:40Z  client:mac-desk   error  frontend:unhandled        -                  TypeError: Cannot read properties of undefined (reading 'id')  [trunc]
2026-09-21 10:38Z  hub               error  fleet_core::ssh           E_SSH              ssh mefistos: Host key verification failed
```

`MESSAGE` is cut to the terminal's width with an ellipsis; `[trunc]` marks a
row whose sender already truncated it; `--json` prints the rows as the route
returned them. `context` is shown only with `--json`.

## Documentation

`docs/hub.md` gains an **Error reports** section after *Events*: what is
sent and by whom, the two retention settings, the CLI, the `POST /report`
contract for a phone client (the JSON body, the caps, the status codes, and
that a `readonly` token may call it), and the privacy note (redacted at
ingest; no prompt, transcript or pane text has a path here). The
*Configuration* table gains the two settings; *What is different from
standalone* gains one bullet on the desktop's reporting and the
`CLAUDE_FLEET_HUB_REPORTS` switch; *Troubleshooting* gains "`fleet-hub
reports` is empty" (the hub predates the route, the desktop was started with
`CLAUDE_FLEET_HUB_REPORTS=0`, the agent's `report_errors` is off, or the
sender's filter is `RUST_LOG=off`). The `fleet-agent` config section names
`report_errors`.

`docs/control-api-reference.md` is regenerated for the new desktop command.

## Testing

- **`fleet-proto`:** `Report::clamp` cuts at a char boundary and sets
  `truncated`; `ReportRing` evicts the oldest at the cap and counts drops;
  `AgentFrame::Report` round-trips, and an encoded batch of 16 maximal
  reports is under `MAX_FRAME_BYTES`. The existing unknown-kind tests already
  prove an old hub skips it.
- **Layer:** an `error!` with a `code` field becomes one report with that
  code and the target as component; a `warn!` does not; the ring is reachable
  after `init_in_with` in a temp dir.
- **Ingest:** redaction masks a bearer token in a message; a batch over the
  cap is `E_VALIDATE`; the row cap prunes to `reports.max_rows`; the age
  sweep deletes only older rows; the rate limit refuses the 61st report in a
  minute and admits it in the next; `dropped` produces its log line.
- **Routes:** a readonly client may `POST`; a client gets `403` on `GET`;
  `413` over `BODY_MAX`; `429` answers carry nothing stored; `GET` filters
  by `since`, `origin`, `level` and caps `limit`.
- **Desktop flusher:** against a recorded `HubTransport`: batches at 20 or
  5 s, stops after `404`, backs off after a transport error and keeps the
  batch, holds a minute after `429`, never runs in `Backend::Local`, never
  runs with `CLAUDE_FLEET_HUB_REPORTS=0`. The routing tests get the
  `report_client_error` row and its verdict.
- **Agent:** with the fake hub in `conn`'s tests, an error pushed before the
  handshake arrives as a `Report` frame on the first beat after `welcome`;
  with `report_errors: false` no layer is installed.
- **Frontend (Vitest):** the listeners call `report_client_error` with the
  expected component; the same message twice within 60 s is sent once with
  `repeats`; the 21st report in a minute is not sent; `pushError` reports.
- **CLI:** `--since 2h` parses to `now - 7200`; the table renders the
  sample above; `--json` passes rows through.
- **Generated files:** `REGEN_HUB_VERDICTS=1`, `REGEN_DOCS=1` and the hub
  contract golden are regenerated in the same change, so CI's currency tests
  pass.

## Rollout and compatibility

- Additive on every wire: no `wire_contract` bump, no `PROTO_VERSION` bump,
  no change to any existing frame or route.
- Old hub, new desktop: `404` once, then quiet. Old hub, new agent: one
  `unknown frame kind` warning per connection on the hub, nothing else. New
  hub, old senders: an empty table until they are updated; `fleet-hub
  reports` still shows the hub's own errors.
- A hub deployed from the release image picks the migration up on first
  start like every other.

## Files

| Area | File |
|---|---|
| Wire shapes, ring, clamp | `crates/fleet-proto/src/report.rs` (new), `lib.rs` (the frame variant, `pub mod report`) |
| Layer | `crates/fleet-core/src/logging.rs`; `crates/fleet-agent/src/report.rs` (new) |
| Ingest, sweep | `crates/fleet-core/src/service/reports.rs` (new), `service/tick.rs`, `service/settings.rs` |
| Store | `crates/fleet-core/src/store/reports.rs` (new), `store/schema.rs`, `migrations/040_error_reports.sql` (new) |
| Routes | `crates/fleet-core/src/mcp/report_route.rs` (new), `mcp/mod.rs` |
| Agent frame on the hub | `crates/fleet-core/src/agent/ws.rs`, `agent/registry.rs` |
| Agent sender | `crates/fleet-agent/src/conn.rs`, `config.rs`, `main.rs` |
| Desktop | `src-tauri/src/backend/report.rs` (new), `backend/startup.rs`, `backend/mod.rs`, `backend/verdicts.rs`, `backend/tests_routing.rs`, `commands/hub.rs`, `lib.rs` |
| Frontend | `src/lib/error_report.ts` (new), `src/main.ts`, `src/lib/toasts.ts` |
| CLI | `crates/fleet-hub/src/main.rs`, `crates/fleet-hub/src/reports.rs` (new) |
| Docs | `docs/hub.md`, `docs/control-api-reference.md` (regenerated), `src/lib/hub_verdicts.generated.json` (regenerated) |
