//! The hub's `GET /events` stream, re-emitted as the frontend's own row
//! events.
//!
//! # What this has to guarantee
//!
//! Standalone, a row change reaches the Svelte stores like this:
//! `Store` mutates a row → [`EventBus::emit`](fleet_core::events::EventBus)
//! → `AppHandleEventBus` sends `(RowChange::name(), RowChange::payload())`
//! down its channel → the drain thread calls `tauri::Emitter::emit`.
//!
//! Pointed at a hub there is no local `Store` and no local `RowChange`. The
//! same change arrives as an SSE frame on a socket. This module's whole job is
//! that **the frontend cannot tell the two apart** — because if it could,
//! `src/lib/*.ts` would need a second code path, and the "no store changes"
//! claim this sub-project rests on would be false.
//!
//! It gets there by joining the two paths as late as possible: a bridged event
//! goes into the *same* channel, as the *same* `(&'static str, Value)` pair,
//! one line before the same `Emitter::emit` call. See [`RemoteEventSink`].
//!
//! Two narrower guarantees hold it up:
//!
//! - **A name off the wire is never an event name.** It is resolved against
//!   [`fleet_core::events::EVENT_NAMES`] first, and what is emitted is the
//!   `&'static str` from that list — so a hub, or anything between it and
//!   here, cannot invent a frontend event, and a name the frontend does not
//!   listen for is dropped here rather than crossing the IPC boundary.
//! - **The payload is passed through, not rebuilt.** It is parsed to
//!   `serde_json::Value` and emitted as it came. Deserialising into the row
//!   struct and re-serialising would look stricter and be worse: every
//!   optional field carries `#[serde(default)]` (the hub's list tools strip
//!   nulls), so a field this build does not know about would be silently
//!   dropped on the way through.
//!
//! # Gaps in the stream
//!
//! `BroadcastEventBus::subscribe` replays nothing — a subscriber sees only
//! what is emitted after it arrives. So every time this bridge connects it has
//! a hole of unknown size behind it, and the only honest recovery is to
//! re-list. [`EventBridge::run`] resyncs once per successful connection,
//! including the first, and a `lagged` frame (the hub saying this client fell
//! behind its ring) is simply another reason to reconnect. See
//! [`FleetResync`] for what a resync can and cannot restore.

use super::connection::{ConnectionReporter, HubConnection, NoReporter};
use super::contract;
use super::remote::{connect, Endpoint, HubBackend};
use super::RemoteConfig;
use fleet_core::events::EVENT_NAMES;
use fleet_core::mcp::wire::SseDecoder;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The frame `/events` opens every stream with: the hub's version and the
/// kinds this subscription will carry. Not a row change.
pub const READY_FRAME: &str = "ready";
/// The frame `/events` sends when this subscriber fell more than
/// `BROADCAST_CAPACITY` events behind. Terminal — the hub closes the stream
/// straight after it — and it means this client's picture has a hole in it.
pub const LAGGED_FRAME: &str = "lagged";

/// Where a bridged event goes.
///
/// Implemented by `AppHandleEventBus`, which pushes `(name, payload)` into the
/// very channel `EventBus::emit` pushes `(e.name(), e.payload())` into. That
/// is the join: from there on a hub event and a local one are the same pair
/// going through the same drain thread into the same `Emitter::emit`.
///
/// `&'static str` rather than `String` is deliberate; the only way to obtain
/// one is [`known_event_name`].
pub trait RemoteEventSink: Send + Sync {
    fn emit_remote(&self, name: &'static str, payload: Value);
}

/// Re-list the fleet and emit what it finds, because a fresh subscription
/// replays nothing.
///
/// A trait so [`EventBridge`] can be driven with a recorder: "exactly one
/// resync per connection" is a property of the loop, and a test of the loop
/// should be able to observe it without a hub. [`HubResync`] is the real one.
#[async_trait::async_trait]
pub trait FleetResync: Send + Sync {
    async fn resync(&self);

    /// A row event the bridge has just emitted. See [`HubResync`]'s
    /// implementation for why a resync needs to hear about them.
    fn observe(&self, _name: &'static str, _payload: &Value) {}
}

/// A live `GET /events` body: the text of the response as it arrives, with
/// any transfer encoding already undone.
#[async_trait::async_trait]
pub trait EventStreamBody: Send {
    /// The next piece of the body. `Ok(None)` means the hub closed the stream
    /// cleanly. A chunk boundary may fall anywhere — including inside a
    /// `data:` line — so a caller must frame what it gets rather than assume
    /// one call is one event.
    async fn next(&mut self) -> Result<Option<String>, String>;
}

/// Opening one `GET /events` stream. A trait so the reconnect loop can be
/// driven by a script of connections instead of a hub.
#[async_trait::async_trait]
pub trait HubEventStream: Send + Sync {
    async fn open(&self) -> Result<Box<dyn EventStreamBody>, String>;
}

/// Waiting. Injected so that no test of the reconnect loop sleeps on the real
/// clock: this suite runs beside `start_paused` tests elsewhere, and one real
/// sleep is jitter in all of them.
#[async_trait::async_trait]
pub trait Delay: Send + Sync {
    async fn sleep(&self, how_long: Duration);
}

/// The real clock.
pub struct RealDelay;

#[async_trait::async_trait]
impl Delay for RealDelay {
    async fn sleep(&self, how_long: Duration) {
        tokio::time::sleep(how_long).await;
    }
}

/// How long to wait before the first reconnection attempt.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(1);
/// The ceiling. A desktop left running overnight against a hub that is down
/// should retry at a steady, cheap cadence rather than hammering it or giving
/// up on it.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// How long a stream may say nothing at all before it is presumed dead.
///
/// The hub writes a keep-alive comment every
/// [`KEEPALIVE_INTERVAL`](fleet_core::mcp::events_route::KEEPALIVE_INTERVAL)
/// (15 s) precisely so a client can tell a quiet fleet from a dead socket.
/// Two and a half intervals is two missed beats plus slack for a slow link.
/// Without this, a laptop that slept and woke on another network blocks in
/// `read()` forever: it never writes on that socket again, so it never gets
/// an RST, and the hub's FIN never reaches it.
pub const IDLE_TIMEOUT: Duration = Duration::from_millis(
    fleet_core::mcp::events_route::KEEPALIVE_INTERVAL.as_millis() as u64 * 5 / 2,
);

/// A connection that stayed up this long counts as working even if it
/// carried no row event — a quiet fleet is not a broken one. Longer than
/// [`IDLE_TIMEOUT`], so a stream that said `ready` and then went silent does
/// not qualify.
pub const HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// The next wait after `previous` failed: doubling, capped.
pub fn next_backoff(previous: Duration) -> Duration {
    std::cmp::min(previous.saturating_mul(2), MAX_BACKOFF)
}

/// The `&'static str` the frontend listens for, or `None` if this is not one
/// of them.
///
/// The `&'static` is the point. What gets emitted is the entry from
/// [`EVENT_NAMES`], never the string that arrived on the socket.
pub fn known_event_name(from_the_wire: &str) -> Option<&'static str> {
    EVENT_NAMES
        .iter()
        .copied()
        .find(|known| *known == from_the_wire)
}

/// What one connection's reader decided.
#[derive(Debug, PartialEq, Eq)]
enum StreamEnd {
    /// The app is shutting down: do not reconnect.
    Cancelled,
    /// Anything else — EOF, a read error, or a `lagged` frame. All of them
    /// mean the same thing to a client: reconnect, and re-list, because the
    /// picture now has a hole of unknown size in it.
    Gap {
        /// Did this connection prove the stream works before it ended?
        ///
        /// It decides whether the backoff resets, and the distinction is not
        /// academic. A hub that accepts the socket, answers 200 and closes
        /// straight away — a `/events` route whose bus has gone, a proxy that
        /// terminates the connection — looks like a success to `open`. Reset
        /// the backoff on that and the loop reconnects every second forever,
        /// and because every connection re-lists, each of those seconds costs
        /// four tool calls against a hub that is already unwell.
        ///
        /// So only two things count: a ROW event, or a connection that stayed
        /// up for [`HEALTHY_AFTER`]. **`ready` does not**: `events_route`
        /// sends it unconditionally, before it ever reads its bus, so a route
        /// whose bus has gone still sends it. **A connection that ends
        /// `lagged` never counts on its events**, only on its lifetime: a
        /// resync's four serial calls leave the stream unread, the hub's ring
        /// overflows, and the next connection opens `lagged` — reset on that
        /// and the loop feeds itself at one second.
        delivered: bool,
        /// Why it ended, for the disconnected banner.
        why: String,
    },
    /// The hub's `ready` frame named a wire-contract revision outside
    /// `[MIN_HUB_CONTRACT, MAX_HUB_CONTRACT]` (`backend::contract`). Never
    /// resynced and never applied a row — see [`Self::pump`]'s handling of
    /// [`READY_FRAME`]. `run` must not overwrite the specific state already
    /// reported (naming both revisions and which side to update) with a
    /// generic `Reconnecting`, and must not reset the backoff: this
    /// connection proved nothing about whether the hub is reachable, only
    /// that it is not yet one to trust.
    ContractSkew,
}

/// What one frame decided.
#[derive(Debug, PartialEq, Eq)]
enum Delivery {
    /// Keep reading this connection; the frame proved nothing about it.
    KeepReading,
    /// A row event reached the frontend: the stream demonstrably works.
    Row,
    /// This connection is over.
    EndOfStream,
}

/// The reconnect loop: subscribe, re-list, re-emit, and do it again when the
/// stream ends.
pub struct EventBridge {
    stream: Arc<dyn HubEventStream>,
    sink: Arc<dyn RemoteEventSink>,
    resync: Arc<dyn FleetResync>,
    delay: Arc<dyn Delay>,
    cancel: CancellationToken,
    /// Told every transition, so the interface can show a banner while the
    /// stream is down. See [`super::connection`].
    status: Arc<dyn ConnectionReporter>,
}

impl EventBridge {
    pub fn new(
        stream: Arc<dyn HubEventStream>,
        sink: Arc<dyn RemoteEventSink>,
        resync: Arc<dyn FleetResync>,
        delay: Arc<dyn Delay>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            stream,
            sink,
            resync,
            delay,
            cancel,
            status: Arc::new(NoReporter),
        }
    }

    /// Report connection state to `status`. The production bridge always
    /// does (`bootstrap/tasks.rs`, held there by a source test); a test about
    /// something else need not.
    pub fn reporting_to(mut self, status: Arc<dyn ConnectionReporter>) -> Self {
        self.status = status;
        self
    }

    /// Run until `cancel` fires. Never returns of its own accord: a hub that
    /// is down is a hub that may come back, and giving up would leave a
    /// desktop showing a frozen fleet with nothing to say why.
    pub async fn run(&self) {
        let mut backoff = FIRST_BACKOFF;
        // Retries since the stream last worked — what the banner counts.
        let mut attempt: u32 = 0;
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            match self.stream.open().await {
                Ok(body) => {
                    // Reporting `Connected` and re-listing both move into
                    // `pump`, gated on the hub's `ready` frame: whether this
                    // hub's rows are trusted at all is not known until that
                    // frame's `contract` field is read (issue #148), and a
                    // resync is exactly the backfill that must not happen
                    // against a hub outside the accepted range.
                    match self.pump(body).await {
                        StreamEnd::Cancelled => return,
                        StreamEnd::ContractSkew => {
                            // Already reported a specific state naming both
                            // revisions and what to do — a generic
                            // `Reconnecting` here would paper right over it.
                            // Never resets: a hub that fails the version
                            // check has proven nothing about the connection.
                            attempt = attempt.saturating_add(1);
                        }
                        StreamEnd::Gap { delivered, why } => {
                            // A connection that carried nothing is not a
                            // working connection, whatever the socket said.
                            if delivered {
                                backoff = FIRST_BACKOFF;
                                attempt = 0;
                            }
                            attempt = attempt.saturating_add(1);
                            self.status.report(HubConnection::Reconnecting {
                                attempt,
                                retry_in_secs: backoff.as_secs(),
                                reason: why,
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        retry_in = ?backoff,
                        "[hub events] could not open the hub's event stream"
                    );
                    attempt = attempt.saturating_add(1);
                    self.status.report(HubConnection::Offline {
                        attempt,
                        retry_in_secs: backoff.as_secs(),
                        reason: e,
                    });
                }
            }
            if self.cancel.is_cancelled() {
                return;
            }
            self.delay.sleep(backoff).await;
            backoff = next_backoff(backoff);
        }
    }

    /// Read one connection to its end, emitting as it goes.
    async fn pump(&self, mut body: Box<dyn EventStreamBody>) -> StreamEnd {
        let opened = tokio::time::Instant::now();
        let stayed_up = || opened.elapsed() >= HEALTHY_AFTER;
        let mut decoder = SseDecoder::new();
        let mut row_events = false;
        // Whether this connection's `ready` frame has been read yet. A real
        // hub always sends it first, before touching its bus, so nothing
        // this loop reads before that is a row worth applying anyway — but
        // some of this suite's scripts skip straight to a row frame for
        // brevity, hence the guard rather than an assumption.
        let mut contract_checked = false;
        loop {
            // Any bytes at all — the hub's keep-alive comment included —
            // make `next` return, so this bounds SILENCE, not the stream.
            let piece = tokio::select! {
                _ = self.cancel.cancelled() => return StreamEnd::Cancelled,
                next = tokio::time::timeout(IDLE_TIMEOUT, body.next()) => next,
            };
            let text = match piece {
                Ok(Ok(Some(text))) => text,
                Ok(Ok(None)) => {
                    tracing::info!("[hub events] the hub closed the event stream; reconnecting");
                    return StreamEnd::Gap {
                        delivered: row_events || stayed_up(),
                        why: "the hub closed the event stream".to_string(),
                    };
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "[hub events] the event stream failed; reconnecting"
                    );
                    return StreamEnd::Gap {
                        delivered: row_events || stayed_up(),
                        why: format!("the event stream failed: {e}"),
                    };
                }
                Err(_) => {
                    tracing::warn!(
                        silent_for = ?IDLE_TIMEOUT,
                        "[hub events] the event stream went silent (not even the hub's \
                         keep-alive); presuming it dead and reconnecting"
                    );
                    return StreamEnd::Gap {
                        delivered: row_events || stayed_up(),
                        why: format!(
                            "the event stream went silent for {}s, not even the hub's keep-alive",
                            IDLE_TIMEOUT.as_secs()
                        ),
                    };
                }
            };
            for frame in decoder.feed(&text) {
                // The FIRST `ready` frame decides whether this connection's
                // hub is trusted at all — see `backend::contract` for the
                // range and `wire_contract` for what moves a hub's revision.
                // Deliberately before `deliver`, and deliberately `.await`s
                // here rather than there: `deliver` is not async, and a
                // resync must happen (or not) before any row frame that
                // follows in this same read is looked at.
                if !contract_checked && frame.name == READY_FRAME {
                    contract_checked = true;
                    tracing::info!(hub = %frame.data, "[hub events] subscribed");
                    let hub_contract = contract::hub_contract_revision(&frame.data);
                    match contract::classify_hub_contract(
                        hub_contract,
                        contract::MIN_HUB_CONTRACT,
                        contract::MAX_HUB_CONTRACT,
                    ) {
                        contract::ContractFit::InRange => {
                            self.status.report(HubConnection::Connected);
                            // A fresh subscription replays nothing, so
                            // everything that happened while this client was
                            // detached is missing. Re-list before reading
                            // any further frame, so whatever the hub sends
                            // from here on applies on top of the re-listed
                            // rows rather than underneath them.
                            self.resync.resync().await;
                        }
                        contract::ContractFit::TooOld => {
                            tracing::warn!(
                                hub_contract,
                                min_contract = contract::MIN_HUB_CONTRACT,
                                "[hub events] the hub's wire contract is older than this \
                                 build requires; not trusting its rows until it is upgraded"
                            );
                            self.status.report(HubConnection::HubTooOld {
                                hub_contract,
                                min_contract: contract::MIN_HUB_CONTRACT,
                            });
                            return StreamEnd::ContractSkew;
                        }
                        contract::ContractFit::TooNew => {
                            tracing::warn!(
                                hub_contract,
                                max_contract = contract::MAX_HUB_CONTRACT,
                                "[hub events] the hub's wire contract is newer than this \
                                 build understands; not trusting its rows until this app \
                                 is upgraded"
                            );
                            self.status.report(HubConnection::HubTooNew {
                                hub_contract,
                                max_contract: contract::MAX_HUB_CONTRACT,
                            });
                            return StreamEnd::ContractSkew;
                        }
                    }
                    continue;
                }
                match self.deliver(&frame.name, &frame.data) {
                    Delivery::KeepReading => {}
                    Delivery::Row => row_events = true,
                    // Lagged: only the lifetime can vouch for this one.
                    Delivery::EndOfStream => {
                        return StreamEnd::Gap {
                            delivered: stayed_up(),
                            why: "this window fell behind the hub's event stream".to_string(),
                        }
                    }
                }
            }
        }
    }

    /// One framed event.
    fn deliver(&self, name: &str, data: &str) -> Delivery {
        match name {
            READY_FRAME => {
                // `pump` handles the first `ready` frame itself (the
                // contract check needs to run before anything else this
                // connection sends is trusted). A real hub sends exactly
                // one per connection, so reaching this arm at all means a
                // second one arrived; nothing to do but note it.
                tracing::debug!(hub = %data, "[hub events] a second `ready` frame; ignoring");
                Delivery::KeepReading
            }
            LAGGED_FRAME => {
                tracing::warn!(
                    detail = %data,
                    "[hub events] this client fell behind the hub's event ring; \
                     reconnecting and re-listing"
                );
                Delivery::EndOfStream
            }
            other => {
                let Some(known) = known_event_name(other) else {
                    // A newer hub with an event this build does not have.
                    // Ignoring it is what keeps a desktop usable against a hub
                    // one version ahead.
                    tracing::debug!(event = %other, "[hub events] ignoring an unknown event");
                    return Delivery::KeepReading;
                };
                match serde_json::from_str::<Value>(data) {
                    Ok(payload) => {
                        if let Err(why) = payload_fits(known, &payload) {
                            tracing::warn!(
                                event = %known,
                                why = %why,
                                "[hub events] dropping an event the frontend could not apply"
                            );
                            return Delivery::KeepReading;
                        }
                        self.resync.observe(known, &payload);
                        self.sink.emit_remote(known, payload);
                        Delivery::Row
                    }
                    Err(e) => {
                        tracing::warn!(
                            event = %known,
                            error = %e,
                            "[hub events] dropping an event whose payload is not JSON"
                        );
                        Delivery::KeepReading
                    }
                }
            }
        }
    }
}

/// Can the frontend apply `payload` under `name` at all?
///
/// The payload is otherwise passed through untouched (see the module docs),
/// and `src/lib/events.ts` reads it unchecked — so a payload that is not an
/// object, or lacks the key its store merges on, is a TypeError inside the
/// batch flush (which loses the whole batch) or a junk entry in a row array.
/// Checked here: an object, plus that key with the right type. Unknown
/// fields are NOT checked, which keeps a desktop working against a newer hub.
pub fn payload_fits(name: &str, payload: &Value) -> Result<(), String> {
    let Some(obj) = payload.as_object() else {
        return Err("the payload is not a JSON object".to_string());
    };
    let integer = |key: &str| match obj.get(key) {
        Some(v) if v.is_i64() => Ok(()),
        _ => Err(format!("`{key}` is missing or not an integer")),
    };
    let string = |key: &str| match obj.get(key) {
        Some(Value::String(_)) => Ok(()),
        _ => Err(format!("`{key}` is missing or not a string")),
    };
    match name {
        "session:created" | "session:updated" | "session:killed" | "project:updated"
        | "worktree:updated" | "worktree:removed" | "task:updated" => integer("id"),
        "host:added" | "host:probed" | "host:removed" => string("alias"),
        "account:upserted" => string("uuid"),
        "account_usage:updated" => string("account_uuid"),
        "asset_inventory:cleared" => string("host_alias").and_then(|()| string("harness")),
        _ => Ok(()),
    }
}

// --- re-listing after a gap ---------------------------------------------------

/// Every session and host the frontend currently holds, as far as this
/// bridge knows — so the next resync can tell what has gone.
///
/// Written by a resync AND by every live `session:*` / `host:*` frame (see
/// [`FleetResync::observe`]). Written by a resync alone, a row that arrived
/// live and vanished during a gap was never in the set and so was never
/// removed: a permanent ghost.
///
/// `None` means "no resync has run yet": the first one cannot know what
/// vanished before the app started, and inventing removals from an empty set
/// would delete the frontend's whole list.
#[derive(Default)]
struct Seen {
    sessions: Option<HashSet<i64>>,
    hosts: Option<HashSet<String>>,
}

/// The real [`FleetResync`]: re-list the hub's rows and emit them as the
/// events the stores already apply.
///
/// # What it covers, and what it does not
///
/// A resync can only emit an event whose payload is the type the list tool
/// answers with. Four are exact:
///
/// | list tool | row type | emitted as |
/// |---|---|---|
/// | `list_sessions` | `SessionRow` | `session:updated`, plus `session:killed` for ids that have gone |
/// | `list_hosts` | `HostRow` | `host:probed`, plus `host:removed` for aliases that have gone |
/// | `list_tasks` | `TaskRow` | `task:updated` (a task is never removed) |
/// | `list_accounts` | `AccountRow` | `account:upserted` (an account is never removed here) |
///
/// **Projects and worktrees are deliberately left out.** `list_projects`
/// answers `ProjectTreeRow` and `list_worktrees` answers `WorktreeOccupancy`,
/// while `project:updated` and `worktree:updated` carry `ProjectRow` and
/// `WorktreeRow`. Emitting the list shapes under those names would not resync
/// those stores, it would corrupt them — a wrong payload is worse than a stale
/// one. A project or worktree changed during a gap therefore stays stale until
/// something else refreshes it; that is a real, named limitation rather than
/// an oversight.
pub struct HubResync {
    hub: Arc<HubBackend>,
    sink: Arc<dyn RemoteEventSink>,
    seen: Mutex<Seen>,
}

impl HubResync {
    pub fn new(hub: Arc<HubBackend>, sink: Arc<dyn RemoteEventSink>) -> Self {
        Self {
            hub,
            sink,
            seen: Mutex::new(Seen::default()),
        }
    }

    /// Emit one row under `name`, exactly as the hub sent it.
    fn emit_row<T: serde::Serialize>(&self, name: &'static str, row: &T) {
        match serde_json::to_value(row) {
            Ok(payload) => self.sink.emit_remote(name, payload),
            // Unreachable for these row types; dropping one row beats
            // panicking a background task.
            Err(e) => tracing::warn!(event = %name, error = %e, "[hub events] unserialisable row"),
        }
    }
}

#[async_trait::async_trait]
impl FleetResync for HubResync {
    /// Keep [`Seen`] in step with what the frontend has been told live.
    fn observe(&self, name: &'static str, payload: &Value) {
        let mut seen = self.seen.lock().expect("resync state");
        let id = || payload.get("id").and_then(Value::as_i64);
        let alias = || {
            payload
                .get("alias")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        match name {
            "session:created" | "session:updated" => {
                if let (Some(set), Some(id)) = (seen.sessions.as_mut(), id()) {
                    set.insert(id);
                }
            }
            "session:killed" => {
                if let (Some(set), Some(id)) = (seen.sessions.as_mut(), id()) {
                    set.remove(&id);
                }
            }
            "host:added" | "host:probed" => {
                if let (Some(set), Some(alias)) = (seen.hosts.as_mut(), alias()) {
                    set.insert(alias);
                }
            }
            "host:removed" => {
                if let (Some(set), Some(alias)) = (seen.hosts.as_mut(), alias()) {
                    set.remove(&alias);
                }
            }
            _ => {}
        }
    }

    async fn resync(&self) {
        // `force: false` — a reconcile pass belongs to whoever owns the fleet,
        // and that is the hub. This asks for what it already knows.
        match self.hub.list_sessions(false).await {
            Ok(rows) => {
                let now: HashSet<i64> = rows.iter().map(|r| r.id).collect();
                for row in &rows {
                    self.emit_row("session:updated", row);
                }
                let gone = {
                    let mut seen = self.seen.lock().expect("resync state");
                    let before = seen.sessions.replace(now.clone());
                    before.map(|b| b.difference(&now).copied().collect::<Vec<_>>())
                };
                for id in gone.unwrap_or_default() {
                    self.sink
                        .emit_remote("session:killed", serde_json::json!({ "id": id }));
                }
            }
            Err(e) => tracing::warn!(
                code = %e.code,
                "[hub events] could not re-list sessions after reconnecting"
            ),
        }
        match self.hub.list_hosts().await {
            Ok(rows) => {
                let now: HashSet<String> = rows.iter().map(|r| r.alias.clone()).collect();
                for row in &rows {
                    self.emit_row("host:probed", row);
                }
                let gone = {
                    let mut seen = self.seen.lock().expect("resync state");
                    let before = seen.hosts.replace(now.clone());
                    before.map(|b| b.difference(&now).cloned().collect::<Vec<_>>())
                };
                for alias in gone.unwrap_or_default() {
                    self.sink
                        .emit_remote("host:removed", serde_json::json!({ "alias": alias }));
                }
            }
            Err(e) => tracing::warn!(
                code = %e.code,
                "[hub events] could not re-list hosts after reconnecting"
            ),
        }
        match self.hub.list_tasks(None, None, None).await {
            Ok(rows) => {
                for row in &rows {
                    self.emit_row("task:updated", row);
                }
            }
            Err(e) => tracing::warn!(
                code = %e.code,
                "[hub events] could not re-list tasks after reconnecting"
            ),
        }
        match self.hub.list_accounts().await {
            Ok(rows) => {
                for row in &rows {
                    self.emit_row("account:upserted", row);
                }
            }
            Err(e) => tracing::warn!(
                code = %e.code,
                "[hub events] could not re-list accounts after reconnecting"
            ),
        }
    }
}

// --- the real stream ----------------------------------------------------------

/// How long to wait for the hub's response head. The body then stays open
/// indefinitely, which is the whole point; past the head, silence is bounded
/// by [`IDLE_TIMEOUT`] in [`EventBridge::pump`] instead, because the 15 s
/// keep-alive comment is what proves a live stream.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest response head accepted, so a listener that never sends the blank
/// line cannot make this buffer without bound.
const MAX_HEAD: usize = 64 * 1024;

/// The real [`HubEventStream`]: `GET /events` on the configured hub.
pub struct HubSse {
    cfg: RemoteConfig,
}

impl HubSse {
    pub fn new(cfg: RemoteConfig) -> Self {
        Self { cfg }
    }
}

/// Hand-written so a `{:?}` cannot spill the token; [`RemoteConfig`]'s own
/// `Debug` redacts it.
impl std::fmt::Debug for HubSse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubSse").field("cfg", &self.cfg).finish()
    }
}

#[async_trait::async_trait]
impl HubEventStream for HubSse {
    async fn open(&self) -> Result<Box<dyn EventStreamBody>, String> {
        let url = format!("{}/events", self.cfg.base_url);
        let at = Endpoint::parse(&url)?;
        let opened = tokio::time::timeout(OPEN_TIMEOUT, open_stream(&at, &self.cfg.token))
            .await
            .map_err(|_| format!("no response head within {OPEN_TIMEOUT:.0?}"))?;
        // The token can appear in a transport error only if something echoed
        // it back; scrubbed here for the same reason `HubBackend::redact`
        // exists, since this string reaches the log.
        opened.map_err(|e| redact(&e, &self.cfg.token))
    }
}

/// Blank `token` out of `text`. A free function because [`HubSse`] and
/// [`SseBody`] both need it and neither owns the other.
fn redact(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "<redacted>")
}

/// Connect, send the request, read the head, hand back the body.
async fn open_stream(at: &Endpoint, bearer: &str) -> Result<Box<dyn EventStreamBody>, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut conn = connect(at).await?;
    // No `Connection: close` — unlike `POST /mcp` this request is supposed to
    // stay open. `Cache-Control: no-cache` is what the SSE spec asks a client
    // to send, and it stops an intermediary buffering the stream into
    // uselessness.
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n\
         Accept: text/event-stream\r\nCache-Control: no-cache\r\n\r\n",
        at.target, at.authority
    );
    conn.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("send to {}:{}: {e}", at.host, at.port))?;
    conn.flush()
        .await
        .map_err(|e| format!("send to {}:{}: {e}", at.host, at.port))?;

    // Read until the blank line that ends the head. Anything past it is the
    // first of the body and must not be thrown away.
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    let split = loop {
        if let Some(at) = find(&head, b"\r\n\r\n") {
            break at + 4;
        }
        if head.len() > MAX_HEAD {
            return Err("the hub sent a response head larger than 64 KiB".to_string());
        }
        let n = conn
            .read(&mut buf)
            .await
            .map_err(|e| format!("read from {}:{}: {e}", at.host, at.port))?;
        if n == 0 {
            return Err("the hub closed the connection before answering".to_string());
        }
        head.extend_from_slice(&buf[..n]);
    };
    let leftover = head.split_off(split);
    let head = String::from_utf8_lossy(&head).into_owned();
    let status = head
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("unreadable status line from {}", at.authority))?;
    if status != 200 {
        // `/events` answers 503 with `events are not enabled on this server`
        // and 429 with `too many concurrent event streams`, both as plain
        // text, and both worth showing verbatim: they name their own fix.
        let said = String::from_utf8_lossy(&leftover);
        let said = said.trim();
        return Err(if said.is_empty() {
            format!("the hub answered {status} to GET /events")
        } else {
            format!("the hub answered {status} to GET /events: {said}")
        });
    }
    let chunked = super::remote::head_is_chunked(&head);
    Ok(Box::new(SseBody {
        conn,
        dechunker: Dechunker::new(chunked),
        // The head read almost always takes the first body bytes with it;
        // they go in as `pending` so they travel the same path as everything
        // that follows.
        pending: leftover,
        payload: Vec::new(),
        where_from: format!("{}:{}", at.host, at.port),
    }))
}

/// The body of a live `GET /events`: raw socket in, decoded text out.
///
/// Two buffers rather than one, because the two framings do not line up: a
/// chunk boundary is a byte count the hub chose, and a character boundary is
/// whatever UTF-8 says. Bytes go through `pending` (raw, awaiting the
/// de-chunker) and then `payload` (de-chunked, awaiting a whole character).
struct SseBody {
    conn: super::remote::HubStream,
    dechunker: Dechunker,
    /// Raw socket bytes the de-chunker has not consumed.
    pending: Vec<u8>,
    /// De-chunked bytes that do not yet form whole characters.
    payload: Vec<u8>,
    where_from: String,
}

#[async_trait::async_trait]
impl EventStreamBody for SseBody {
    async fn next(&mut self) -> Result<Option<String>, String> {
        use tokio::io::AsyncReadExt;
        loop {
            // Whatever is already buffered first — the head read normally
            // leaves the `ready` frame sitting behind the blank line.
            let decoded = self
                .dechunker
                .take(&mut self.pending)
                .map_err(|e| format!("{}: {e}", self.where_from))?;
            self.payload.extend_from_slice(&decoded);
            let text = take_utf8(&mut self.payload);
            if !text.is_empty() {
                return Ok(Some(text));
            }
            if self.dechunker.finished() {
                return Ok(None);
            }
            let mut buf = [0u8; 8192];
            let n = self
                .conn
                .read(&mut buf)
                .await
                .map_err(|e| format!("read from {}: {e}", self.where_from))?;
            if n == 0 {
                return Ok(None);
            }
            self.pending.extend_from_slice(&buf[..n]);
        }
    }
}

/// Decode as much of `bytes` as is whole UTF-8, leaving an incomplete
/// character at the end in place for the next read.
///
/// `from_utf8_lossy` on each socket read would corrupt any multi-byte
/// character a read boundary happens to split — and a session's friendly name
/// is exactly the kind of field that carries one.
fn take_utf8(bytes: &mut Vec<u8>) -> String {
    match std::str::from_utf8(bytes) {
        Ok(whole) => {
            let text = whole.to_string();
            bytes.clear();
            text
        }
        Err(e) => {
            let good = e.valid_up_to();
            // Valid by construction, so this allocates but never replaces.
            let text = String::from_utf8_lossy(&bytes[..good]).into_owned();
            match e.error_len() {
                // An incomplete character at the very end: keep it.
                None => {
                    bytes.drain(..good);
                }
                // Genuinely invalid bytes. Drop them with a replacement
                // character rather than stalling on them forever.
                Some(bad) => {
                    bytes.drain(..good + bad);
                    return format!("{text}\u{FFFD}");
                }
            }
            text
        }
    }
}

/// Undo `Transfer-Encoding: chunked` incrementally.
///
/// The one-shot path can de-chunk a whole body at once
/// (`remote::dechunk`); a stream cannot, because a chunk boundary falls
/// wherever the hub flushed and the next size line may not have arrived yet.
struct Dechunker {
    chunked: bool,
    /// Bytes left in the chunk being read.
    remaining: usize,
    /// The zero-size chunk arrived.
    done: bool,
}

impl Dechunker {
    fn new(chunked: bool) -> Self {
        Self {
            chunked,
            remaining: 0,
            done: false,
        }
    }

    fn finished(&self) -> bool {
        self.done
    }

    /// Take whatever payload `raw` now yields, leaving the rest in place.
    fn take(&mut self, raw: &mut Vec<u8>) -> Result<Vec<u8>, String> {
        if !self.chunked {
            return Ok(std::mem::take(raw));
        }
        let mut out = Vec::new();
        loop {
            if self.done {
                raw.clear();
                break;
            }
            if self.remaining > 0 {
                let take = self.remaining.min(raw.len());
                if take == 0 {
                    break;
                }
                out.extend(raw.drain(..take));
                self.remaining -= take;
                continue;
            }
            // A size line, possibly preceded by the CRLF that ended the
            // previous chunk's data.
            let skip = if raw.starts_with(b"\r\n") { 2 } else { 0 };
            let Some(eol) = find(&raw[skip..], b"\r\n") else {
                // Not a whole size line yet.
                break;
            };
            let line = String::from_utf8_lossy(&raw[skip..skip + eol]).into_owned();
            let token = line.split(';').next().unwrap_or("").trim().to_string();
            let size = usize::from_str_radix(&token, 16)
                .map_err(|_| format!("unreadable chunk size {token:?}"))?;
            raw.drain(..skip + eol + 2);
            if size == 0 {
                self.done = true;
                raw.clear();
                break;
            }
            self.remaining = size;
        }
        Ok(out)
    }
}

/// First offset of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Start the bridge as a background task.
///
/// A named function beside the other three background-task spawns, and NOT
/// inline at the call site, so `tests_startup` can assert the same two things
/// about it that it asserts about them: `lib.rs` does not call it, and
/// `bootstrap/tasks.rs` calls it exactly once.
///
/// `fleet_core::rt::spawn` rather than a bare `tokio::spawn`: this is reached
/// from Tauri's `setup` closure, which runs on the main thread with no runtime
/// entered — a bare spawn there panics inside a callback that cannot unwind,
/// and so aborts the process.
pub fn spawn_event_bridge(bridge: EventBridge) {
    fleet_core::rt::spawn(async move { bridge.run().await });
}

#[cfg(test)]
#[path = "tests_events.rs"]
mod tests;
