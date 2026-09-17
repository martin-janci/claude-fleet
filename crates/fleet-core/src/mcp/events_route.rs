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
use axum::extract::{Extension, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
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

/// Per-request state of the `/events` route.
#[derive(Clone)]
pub struct EventsState {
    /// `None` on a server with no event source (the desktop): the route then
    /// answers [`NOT_ENABLED`].
    subscribe: Option<EventSubscriber>,
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
    /// A route that streams from `subscribe`.
    pub fn enabled(subscribe: EventSubscriber) -> Self {
        Self {
            subscribe: Some(subscribe),
            streams: LongPollLimiter::new(super::guard::MAX_LONG_POLLS_PER_CALLER),
            keepalive: KEEPALIVE_INTERVAL,
            shutdown: CancellationToken::new(),
        }
    }

    /// A route with no event source: every request gets [`NOT_ENABLED`].
    pub fn disabled() -> Self {
        Self {
            subscribe: None,
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
    pub fn new(subscribe: Option<EventSubscriber>) -> Self {
        match subscribe {
            Some(s) => Self::enabled(s),
            None => Self::disabled(),
        }
    }
}

/// `?kinds=session,host` — the part of each event name before the `:`.
#[derive(Deserialize, Default)]
pub struct EventsQuery {
    kinds: Option<String>,
}

/// The requested kinds, or `None` for "everything". Empty and whitespace-only
/// entries are dropped, so `?kinds=` and `?kinds=,,` mean "everything" rather
/// than "nothing" — a filter that silently matches no event is the harder
/// failure to diagnose.
fn wanted_kinds(q: &EventsQuery) -> Option<Vec<String>> {
    let kinds: Vec<String> = q
        .kinds
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    (!kinds.is_empty()).then_some(kinds)
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
    let Some(subscribe) = state.subscribe.clone() else {
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
    let kinds = wanted_kinds(&query);
    // Subscribe BEFORE the first frame goes out: a change emitted between the
    // client's request and its first read must still reach it.
    let rx = subscribe();
    tracing::debug!(caller = %label, kinds = ?kinds, "[events] stream opened");

    let stream = futures_util::stream::unfold(
        StreamState {
            rx,
            kinds,
            phase: Phase::Ready,
            _permit: permit,
            label,
            shutdown,
        },
        |mut st| async move {
            match st.phase {
                Phase::Done => None,
                Phase::Ready => {
                    st.phase = Phase::Live;
                    let ready = serde_json::json!({
                        "version": crate::app_version::get(),
                        "now": unix_now(),
                    });
                    Some((Ok::<Event, Infallible>(sse_event("ready", &ready)), st))
                }
                Phase::Live => loop {
                    // Cloned out of `st` so the two select branches do not
                    // borrow it at once.
                    let cancel = st.shutdown.clone();
                    let label = st.label.clone();
                    let received = tokio::select! {
                        // The server is stopping: end the body now rather
                        // than hold its drain open.
                        _ = cancel.cancelled() => {
                            tracing::debug!(caller = %label, "[events] stream closed: server stopping");
                            return None;
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

    #[test]
    fn no_filter_means_every_kind() {
        for raw in [None, Some(""), Some(","), Some("  ")] {
            let kinds = wanted_kinds(&q(raw));
            assert!(kinds.is_none(), "{raw:?} must not filter anything out");
            let msg = EventMessage {
                name: "session:updated",
                payload: serde_json::Value::Null,
            };
            assert!(matches(kinds.as_ref(), &msg));
        }
    }

    #[test]
    fn a_filter_matches_the_part_before_the_colon() {
        let kinds = wanted_kinds(&q(Some("session, HOST ")));
        let ev = |name| EventMessage {
            name,
            payload: serde_json::Value::Null,
        };
        assert!(matches(kinds.as_ref(), &ev("session:created")));
        assert!(matches(kinds.as_ref(), &ev("session:killed")));
        assert!(matches(kinds.as_ref(), &ev("host:probed")));
        assert!(!matches(kinds.as_ref(), &ev("task:updated")));
        // `account_usage` is its own kind — `account` must not match it.
        let account = wanted_kinds(&q(Some("account")));
        assert!(matches(account.as_ref(), &ev("account:upserted")));
        assert!(!matches(account.as_ref(), &ev("account_usage:updated")));
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
