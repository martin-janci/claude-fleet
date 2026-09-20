//! `GET /events` — the fleet's change stream, as server-sent events.
//!
//! A phone client lists what it needs once and then follows this stream
//! instead of polling: every [`RowChange`](crate::events::RowChange) the store
//! emits after the connection opened arrives as one SSE frame, named after the
//! event (`session:updated`, `host:probed`, …) and carrying the same JSON
//! payload the desktop frontend receives.
//!
//! The route sits BEHIND the `authorize` layer — unlike `/healthz` and
//! `/pair`, which are unauthenticated on purpose. A change stream names
//! sessions, hosts, projects and prompts; it is the same data `/mcp` serves
//! and it needs the same bearer token.
//!
//! Only a server that was started with an event source has anything to
//! stream. `fleet-hub serve` opens its store with a
//! [`BroadcastEventBus`](crate::events::BroadcastEventBus) and passes a
//! subscribe handle in; the desktop opens its store with the bus that forwards
//! to the Svelte frontend and passes nothing, so `/events` there answers
//! `503` and says why rather than hanging on a stream that can never produce a
//! frame.

use super::auth::Caller;
use super::guard::{LongPollLimiter, LongPollPermit};
use crate::events::EventMessage;
use crate::store::Store;
use axum::extract::{Extension, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tokio_util::sync::CancellationToken;

/// Hands out a fresh subscription to the server's event source. A closure
/// rather than the bus itself so `fleet-core` never has to name which bus the
/// embedder built (and the desktop can pass none at all).
pub type EventSubscriber = Arc<dyn Fn() -> Receiver<EventMessage> + Send + Sync>;

/// How often an idle stream sends a `:` comment line. Long enough not to be
/// chatter, short enough to hold a connection open through the idle timeouts
/// of a phone's NAT, a reverse tunnel and any proxy in between (the MCP
/// transport picked the same 15 s for its long polls).
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// What `/events` answers on a server started without an event source.
pub const NOT_ENABLED: &str = "events are not enabled on this server";

/// What `/events` answers once a caller holds
/// [`MAX_LONG_POLLS_PER_CALLER`](super::guard::MAX_LONG_POLLS_PER_CALLER)
/// streams.
pub const TOO_MANY_STREAMS: &str = "too many concurrent event streams";

/// What a server needs to serve a stream at all: somewhere to subscribe, and
/// the store — a live stream must keep checking that a paired client still
/// exists (see [`handle_events`]), which is a read, never a write.
#[derive(Clone)]
struct EventSource {
    subscribe: EventSubscriber,
    store: Arc<Mutex<Store>>,
}

/// Per-request state of the `/events` route.
#[derive(Clone)]
pub struct EventsState {
    /// `None` on a server with no event source (the desktop): the route then
    /// answers [`NOT_ENABLED`].
    source: Option<EventSource>,
    /// Stream slots, keyed by [`Caller::label`]. A stream holds a connection
    /// and a subscription indefinitely, exactly the resource the tool-side
    /// long-poll cap exists to bound, so it uses the same limiter type and the
    /// same per-caller ceiling — on its own instance, so that parking eight
    /// phone streams never costs an agent its `wait_for_session` slots.
    streams: Arc<LongPollLimiter>,
    /// How often an idle stream sends its comment line. Always
    /// [`KEEPALIVE_INTERVAL`] in production; a test shortens it so the
    /// heartbeat can be observed without waiting 15 s for it.
    keepalive: Duration,
    /// The server's shutdown signal. A stream is an in-flight request that
    /// never completes on its own, and axum's graceful shutdown waits for
    /// in-flight requests — so without this a hub with one phone attached
    /// would sit out its whole drain timeout on every stop. Each stream ends
    /// itself when the token is cancelled. Never cancelled by default (a
    /// route built without one behaves as before).
    shutdown: CancellationToken,
}

impl EventsState {
    /// A route that streams from `subscribe`, re-reading `store` to notice a
    /// revoked client.
    pub fn enabled(subscribe: EventSubscriber, store: Arc<Mutex<Store>>) -> Self {
        Self {
            source: Some(EventSource { subscribe, store }),
            streams: LongPollLimiter::new(super::guard::MAX_LONG_POLLS_PER_CALLER),
            keepalive: KEEPALIVE_INTERVAL,
            shutdown: CancellationToken::new(),
        }
    }

    /// A route with no event source: every request gets [`NOT_ENABLED`].
    pub fn disabled() -> Self {
        Self {
            source: None,
            streams: LongPollLimiter::new(super::guard::MAX_LONG_POLLS_PER_CALLER),
            keepalive: KEEPALIVE_INTERVAL,
            shutdown: CancellationToken::new(),
        }
    }

    /// End every open stream when `token` is cancelled — the server's own
    /// shutdown token, so stopping does not wait out the drain timeout.
    pub fn with_shutdown(mut self, token: CancellationToken) -> Self {
        self.shutdown = token;
        self
    }

    /// A shorter heartbeat, so a test need not wait [`KEEPALIVE_INTERVAL`].
    #[cfg(test)]
    pub fn with_keepalive(mut self, every: Duration) -> Self {
        self.keepalive = every;
        self
    }

    /// [`EventsState::enabled`] when a source was configured, else
    /// [`EventsState::disabled`].
    pub fn new(subscribe: Option<EventSubscriber>, store: Arc<Mutex<Store>>) -> Self {
        match subscribe {
            Some(s) => Self::enabled(s, store),
            None => Self::disabled(),
        }
    }

    /// Stream slots `label` currently holds. A test uses it to see a slot
    /// released after a stream ends.
    #[cfg(test)]
    pub fn active(&self, label: &str) -> usize {
        self.streams.active(label)
    }
}

/// `?kinds=session,host` — the part of each event name before the `:`.
#[derive(Deserialize, Default)]
pub struct EventsQuery {
    kinds: Option<String>,
}

/// What a `?kinds=` value asked for, split into the kinds this server knows
/// ([`EVENT_KINDS`]) and the ones it does not.
struct RequestedKinds {
    /// `None` for "everything" — no filter was given.
    accepted: Option<Vec<String>>,
    /// Anything that is not an event kind: almost always a typo (`sessions`
    /// for `session`), which would otherwise produce a live stream that only
    /// ever sends `ready` and heartbeats, with nothing to say why.
    unknown: Vec<String>,
}

/// Split a `?kinds=` value. Empty and whitespace-only entries are dropped, so
/// `?kinds=` and `?kinds=,,` mean "everything" rather than "nothing" — a
/// filter that silently matches no event is the harder failure to diagnose.
fn wanted_kinds(q: &EventsQuery) -> RequestedKinds {
    let asked: Vec<String> = q
        .kinds
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if asked.is_empty() {
        return RequestedKinds {
            accepted: None,
            unknown: Vec::new(),
        };
    }
    let (accepted, unknown): (Vec<String>, Vec<String>) = asked
        .into_iter()
        .partition(|k| crate::events::EVENT_KINDS.contains(&k.as_str()));
    RequestedKinds {
        accepted: Some(accepted),
        unknown,
    }
}

/// What the `ready` frame echoes back: the kinds this stream will actually
/// carry, so a client can see what it subscribed to without reading the
/// server's log.
fn accepted_list(kinds: Option<&Vec<String>>) -> Vec<String> {
    match kinds {
        None => crate::events::EVENT_KINDS
            .iter()
            .map(|k| (*k).to_string())
            .collect(),
        Some(ks) => ks.clone(),
    }
}

fn matches(kinds: Option<&Vec<String>>, msg: &EventMessage) -> bool {
    match kinds {
        None => true,
        Some(ks) => ks.iter().any(|k| k == msg.kind()),
    }
}

/// Seconds since the Unix epoch (0 on a clock set before 1970).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Where the stream is in its life. `Lagged` is terminal: a subscriber that
/// fell behind has an incomplete picture, so it is told how many events it
/// missed and the connection closes — reconnecting and re-listing is the only
/// honest recovery, and it beats streaming on with a silent hole in it.
enum Phase {
    Ready,
    Live,
    Done,
}

/// Everything one connection owns. The permit rides in here — not merely in
/// the handler — so a slot is held for the LIFE of the stream and released
/// when the client disconnects and axum drops it.
struct StreamState {
    rx: Receiver<EventMessage>,
    kinds: Option<Vec<String>>,
    phase: Phase,
    _permit: LongPollPermit,
    label: String,
    shutdown: CancellationToken,
    /// The paired client behind this stream, and the store to re-read it in:
    /// `None` for the master token and for a per-host token, which are not
    /// revocable this way.
    client: Option<(Arc<Mutex<Store>>, i64)>,
    /// Fires on the keep-alive beat; each tick re-checks [`StreamState::client`].
    heartbeat: tokio::time::Interval,
}

/// Is that `client_tokens` row still live (present and not revoked)?
///
/// The lock is taken and dropped inside this synchronous function, so no
/// guard ever crosses an `.await`. Anything but a clear "yes" — a poisoned
/// lock, a failed read — ends the stream: a client that reconnects loses
/// nothing but a round trip, while a revoked one kept on the feed is the
/// failure this check exists to prevent.
fn client_is_live(store: &Mutex<Store>, id: i64) -> bool {
    let Ok(s) = store.lock() else {
        tracing::warn!("[events] store lock poisoned; ending the stream");
        return false;
    };
    match s.active_client_tokens() {
        Ok(rows) => rows.iter().any(|r| r.id == id),
        Err(e) => {
            tracing::warn!(error = %e.message, "[events] could not re-check the client; ending the stream");
            false
        }
    }
}

fn sse_event(name: &str, payload: &serde_json::Value) -> Event {
    // `to_string` on a `Value` cannot fail.
    Event::default().event(name).data(payload.to_string())
}

/// `GET /events`.
pub(super) async fn handle_events(
    State(state): State<EventsState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<EventsQuery>,
) -> Response {
    let Some(source) = state.source.clone() else {
        // Deliberately a plain-text 503 with the reason in the body: a client
        // that asked for a stream on a server that has none should be able to
        // print what it got.
        return (StatusCode::SERVICE_UNAVAILABLE, NOT_ENABLED).into_response();
    };
    let label = caller.label();
    let Some(permit) = state.streams.try_acquire(&label) else {
        tracing::warn!(caller = %label, "[events] refused: caller holds the maximum streams");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            TOO_MANY_STREAMS,
        )
            .into_response();
    };
    let keepalive = state.keepalive;
    let shutdown = state.shutdown.clone();
    let RequestedKinds { accepted, unknown } = wanted_kinds(&query);
    if !unknown.is_empty() {
        tracing::warn!(
            caller = %label,
            unknown = ?unknown,
            known = ?crate::events::EVENT_KINDS,
            "[events] ignoring unrecognised ?kinds= values"
        );
    }
    // A paired client's authorization was checked once, at connect; the
    // stream then outlives it, so it re-checks the row on every beat.
    let client = caller
        .client
        .as_ref()
        .map(|c| (Arc::clone(&source.store), c.id));
    // Subscribe BEFORE the first frame goes out: a change emitted between the
    // client's request and its first read must still reach it.
    let rx = (source.subscribe)();
    tracing::debug!(caller = %label, kinds = ?accepted, "[events] stream opened");

    let stream = futures_util::stream::unfold(
        StreamState {
            rx,
            kinds: accepted,
            phase: Phase::Ready,
            _permit: permit,
            label,
            shutdown,
            client,
            heartbeat: {
                let start = tokio::time::Instant::now() + keepalive;
                let mut i = tokio::time::interval_at(start, keepalive);
                // A beat missed while a burst of events was being written is
                // caught up on the next one, not replayed in a tight loop.
                i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                i
            },
        },
        |mut st| async move {
            match st.phase {
                Phase::Done => None,
                Phase::Ready => {
                    st.phase = Phase::Live;
                    let ready = serde_json::json!({
                        "version": crate::app_version::get(),
                        "now": unix_now(),
                        "kinds": accepted_list(st.kinds.as_ref()),
                        // The wire-contract revision (see `wire_contract`
                        // for what moves it): purely additive next to
                        // `version` and `now` above, so an older client
                        // that has never heard of it just ignores it.
                        "contract": crate::wire_contract::CONTRACT_REVISION,
                    });
                    Some((Ok::<Event, Infallible>(sse_event("ready", &ready)), st))
                }
                Phase::Live => loop {
                    // Cloned out of `st` so the two select branches do not
                    // borrow it at once.
                    let cancel = st.shutdown.clone();
                    let label = st.label.clone();
                    let client = st.client.clone();
                    let received = tokio::select! {
                        // The server is stopping: end the body now rather
                        // than hold its drain open.
                        _ = cancel.cancelled() => {
                            tracing::debug!(caller = %label, "[events] stream closed: server stopping");
                            return None;
                        }
                        // One beat: is the paired client still paired?
                        _ = st.heartbeat.tick() => {
                            if let Some((store, id)) = &client {
                                if !client_is_live(store, *id) {
                                    tracing::info!(
                                        caller = %label,
                                        "[events] stream closed: the client was revoked"
                                    );
                                    return None;
                                }
                            }
                            continue;
                        }
                        r = st.rx.recv() => r,
                    };
                    match received {
                        Ok(msg) if matches(st.kinds.as_ref(), &msg) => {
                            let ev = sse_event(msg.name, &msg.payload);
                            return Some((Ok(ev), st));
                        }
                        // A kind this client did not ask for: keep waiting
                        // rather than ending the stream.
                        Ok(_) => continue,
                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!(
                                caller = %st.label,
                                skipped = n,
                                "[events] subscriber fell behind; closing the stream"
                            );
                            st.phase = Phase::Done;
                            let ev = sse_event("lagged", &serde_json::json!({ "skipped": n }));
                            return Some((Ok(ev), st));
                        }
                        // The bus went away (the process is shutting down).
                        Err(RecvError::Closed) => return None,
                    }
                },
            }
        },
    );

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(keepalive))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{BroadcastEventBus, EventBus, RowChange};

    fn q(kinds: Option<&str>) -> EventsQuery {
        EventsQuery {
            kinds: kinds.map(str::to_string),
        }
    }

    fn ev(name: &'static str) -> EventMessage {
        EventMessage {
            name,
            payload: serde_json::Value::Null,
        }
    }

    #[test]
    fn no_filter_means_every_kind() {
        for raw in [None, Some(""), Some(","), Some("  ")] {
            let asked = wanted_kinds(&q(raw));
            assert!(
                asked.accepted.is_none(),
                "{raw:?} must not filter anything out"
            );
            assert!(asked.unknown.is_empty(), "{raw:?} names nothing unknown");
            assert!(matches(asked.accepted.as_ref(), &ev("session:updated")));
            // With no filter, `ready` still says what will come: everything.
            assert_eq!(
                accepted_list(asked.accepted.as_ref()).len(),
                crate::events::EVENT_KINDS.len()
            );
        }
    }

    #[test]
    fn a_filter_matches_the_part_before_the_colon() {
        let asked = wanted_kinds(&q(Some("session, HOST ")));
        let kinds = asked.accepted;
        assert!(matches(kinds.as_ref(), &ev("session:created")));
        assert!(matches(kinds.as_ref(), &ev("session:killed")));
        assert!(matches(kinds.as_ref(), &ev("host:probed")));
        assert!(!matches(kinds.as_ref(), &ev("task:updated")));
        // What the `ready` frame echoes back is what was accepted.
        assert_eq!(accepted_list(kinds.as_ref()), vec!["session", "host"]);
        // `account_usage` is its own kind — `account` must not match it.
        let account = wanted_kinds(&q(Some("account"))).accepted;
        assert!(matches(account.as_ref(), &ev("account:upserted")));
        assert!(!matches(account.as_ref(), &ev("account_usage:updated")));
    }

    #[test]
    fn the_move_kind_is_subscribable() {
        let asked = wanted_kinds(&q(Some("move")));
        assert!(asked.unknown.is_empty(), "{:?}", asked.unknown);
        let kinds = asked.accepted;
        assert!(matches(kinds.as_ref(), &ev("move:progress")));
        assert!(!matches(kinds.as_ref(), &ev("session:updated")));
    }

    /// A plural typo is the easy mistake, and silently yields a stream that
    /// never carries anything: it must be separated out (so it can be logged)
    /// and must not show up as something the client subscribed to.
    #[test]
    fn an_unrecognised_kind_is_reported_not_silently_ignored() {
        let asked = wanted_kinds(&q(Some("sessions,host,nonsense")));
        assert_eq!(asked.unknown, vec!["sessions", "nonsense"]);
        assert_eq!(asked.accepted.as_deref(), Some(&["host".to_string()][..]));
        assert!(!matches(asked.accepted.as_ref(), &ev("session:created")));
        assert!(matches(asked.accepted.as_ref(), &ev("host:probed")));
        // Nothing recognised at all: an empty accepted list, which `ready`
        // shows as such rather than as "everything".
        let none = wanted_kinds(&q(Some("sessions")));
        assert_eq!(none.unknown, vec!["sessions"]);
        assert!(accepted_list(none.accepted.as_ref()).is_empty());
        assert!(!matches(none.accepted.as_ref(), &ev("session:created")));
    }

    /// The ceiling is the tool-side one, and it is per caller: one caller
    /// filling its budget must not touch another's.
    #[test]
    fn stream_slots_are_capped_per_caller() {
        let state = EventsState::disabled();
        let held: Vec<_> = (0..super::super::guard::MAX_LONG_POLLS_PER_CALLER)
            .map(|_| state.streams.try_acquire("client:phone").unwrap())
            .collect();
        assert!(state.streams.try_acquire("client:phone").is_none());
        assert!(state.streams.try_acquire("master").is_some());
        drop(held);
        assert!(state.streams.try_acquire("client:phone").is_some());
    }

    /// The route's own plumbing, without HTTP: a subscription taken from the
    /// bus sees what is emitted after it.
    #[tokio::test]
    async fn a_subscription_carries_the_emitted_event() {
        let bus = Arc::new(BroadcastEventBus::default());
        let sub: EventSubscriber = {
            let bus = Arc::clone(&bus);
            Arc::new(move || bus.subscribe())
        };
        let mut rx = sub();
        bus.session_killed(7);
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.kind(), "session");
        assert_eq!(msg.payload["id"], 7);
        bus.emit(&RowChange::HostRemoved("box".into()));
        assert_eq!(rx.recv().await.unwrap().kind(), "host");
    }
}
