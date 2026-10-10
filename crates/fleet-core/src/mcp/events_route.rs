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
use crate::service::view_scope::ViewScope;
use crate::store::Store;
use axum::extract::{Extension, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tokio_util::sync::CancellationToken;

/// Hands out a fresh subscription to the server's event source. A closure
/// rather than the bus itself so `fleet-core` never has to name which bus the
/// embedder built (and the desktop can pass none at all).
pub type EventSubscriber = Arc<dyn Fn() -> Receiver<EventMessage> + Send + Sync>;

/// What a stream needs to answer a `Last-Event-ID`: the events after a
/// sequence number, or `None` when the gap is longer than the history kept.
pub type EventReplay = Arc<dyn Fn(u64, u64) -> Option<Vec<EventMessage>> + Send + Sync>;

/// Everything `/events` needs from a bus: a fresh subscription per
/// connection, and — when the bus keeps one — the history to resume from.
///
/// One value rather than two parameters threaded side by side through
/// `start` and `start_with_listener`, so a caller cannot supply a
/// subscription and forget the history that belongs with it.
#[derive(Clone)]
pub struct EventFeed {
    pub subscribe: EventSubscriber,
    pub history: Option<EventHistory>,
}

impl From<Arc<crate::events::BroadcastEventBus>> for EventFeed {
    fn from(bus: Arc<crate::events::BroadcastEventBus>) -> Self {
        let generation = bus.generation();
        let replay_bus = Arc::clone(&bus);
        Self {
            subscribe: {
                let bus = Arc::clone(&bus);
                Arc::new(move || bus.subscribe())
            },
            history: Some(EventHistory {
                generation,
                replay: Arc::new(move |g, seq| replay_bus.replay_after(g, seq)),
            }),
        }
    }
}

/// A bus that can be resumed from.
///
/// Separate from [`EventSubscriber`] because a stream works without it: a
/// route built with no history simply never replays, and a reconnecting
/// client re-lists exactly as it always has.
#[derive(Clone)]
pub struct EventHistory {
    /// Identifies this process's sequence, so a restarted hub refuses a
    /// `Last-Event-ID` minted by the last one instead of replaying the wrong
    /// events under the right numbers.
    pub generation: u64,
    pub replay: EventReplay,
}

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
    /// `None` on a bus that keeps no history; the stream then never replays.
    history: Option<EventHistory>,
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
            source: Some(EventSource {
                subscribe,
                store,
                history: None,
            }),
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

    /// Let a reconnecting client resume from its `Last-Event-ID` instead of
    /// re-listing everything.
    pub fn with_history(mut self, history: EventHistory) -> Self {
        if let Some(src) = self.source.as_mut() {
            src.history = Some(history);
        }
        self
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

    /// [`EventsState::enabled`] when a feed was configured, else
    /// [`EventsState::disabled`].
    pub fn new(feed: Option<EventFeed>, store: Arc<Mutex<Store>>) -> Self {
        match feed {
            Some(f) => {
                let state = Self::enabled(f.subscribe, store);
                match f.history {
                    Some(h) => state.with_history(h),
                    None => state,
                }
            }
            None => Self::disabled(),
        }
    }

    /// The limiter this route counts streams against, so `/metrics` reports
    /// the same gauge the cap enforces rather than a second tally that could
    /// disagree with it.
    pub fn stream_limiter(&self) -> Arc<LongPollLimiter> {
        Arc::clone(&self.streams)
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
    /// `?fields=id,claude_status,…` — keep only these keys in each frame's
    /// payload. A phone decodes about half the columns a session row carries
    /// and pays for all of them, ~1 200 times an hour.
    fields: Option<String>,
    /// `?since=<generation>-<seq>` — the `Last-Event-ID` fallback, for the
    /// proxies that strip the header.
    since: Option<String>,
}

/// Keep only `fields` at the top level of a frame payload.
///
/// Top level only, and only when the payload is an object: nested values
/// belong to the field that carries them, and a scalar payload (`{"id":7}`
/// from `session:killed`) has nothing to project.
fn project(payload: &serde_json::Value, fields: &[String]) -> serde_json::Value {
    match payload {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .filter(|(k, _)| fields.iter().any(|f| f == *k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Split a `?fields=` value the way [`wanted_kinds`] splits `?kinds=`.
///
/// Unlike kinds there is no closed vocabulary to check against — a field name
/// is whatever a row type happens to serialize, and that set differs per event
/// — so every non-empty entry is accepted and the `ready` frame echoes the
/// list back. A client that misspells one sees it in the echo rather than in a
/// column that is silently always absent.
///
/// Bounded: at most [`QUERY_LIST_MAX`] distinct entries of at most
/// [`QUERY_ENTRY_MAX`] bytes. The request line may run to ~400 KB, and every
/// frame of the stream is projected against this list key by key, so ~200k
/// entries turned each session event into ~10M string compares per stream.
/// What is kept is what the `ready` frame echoes.
fn wanted_fields(q: &EventsQuery) -> Option<Vec<String>> {
    let mut asked: Vec<String> = Vec::new();
    for f in q.fields.as_deref().unwrap_or("").split(',').map(str::trim) {
        if asked.len() == QUERY_LIST_MAX {
            break;
        }
        if !f.is_empty() && f.len() <= QUERY_ENTRY_MAX && !asked.iter().any(|a| a == f) {
            asked.push(f.to_string());
        }
    }
    (!asked.is_empty()).then_some(asked)
}

/// The most distinct entries `?fields=` / `?kinds=` keep. A session row has
/// about sixty columns, and there are fewer kinds than that.
const QUERY_LIST_MAX: usize = 128;
/// The longest `?fields=` / `?kinds=` entry kept; no column or kind comes near.
const QUERY_ENTRY_MAX: usize = 64;

/// A frame id: this hub's generation and the event's position in it.
///
/// **`seq` is PER CONNECTION for every caller but the hub's own reader**
/// (multi-user M1, T9e). The bus's global counter used to be stamped here
/// for everybody, so a caller that may see one session out of fifty saw the
/// gaps between the ids it received and could measure how many frames it
/// was not shown — the fleet's aggregate activity RATE, at frame
/// granularity. The fence drops the frame; it could not drop the number.
///
/// T9b accepted that residual for one reason: the global counter is the key
/// of `BroadcastEventBus::replay_after`'s single shared ring, and a
/// per-caller sequence would have broken the resume. **The replay rule
/// (T9d) removed that reason.** No non-internal caller is replayed to at
/// all — [`handle_events`] builds a replay only for
/// [`ViewScope::is_internal`], and such a caller's `Last-Event-ID` is
/// answered `resumed: false` — so the seq it sends back is never read, and
/// the only thing the global number still did on its wire was leak. The one
/// reader that resumes keeps the bus's own `seq`, so the ring contract is
/// untouched.
///
/// [`ViewScope::is_internal`]: crate::service::view_scope::ViewScope::is_internal
fn frame_id(generation: u64, seq: u64) -> String {
    format!("{generation}-{seq}")
}

/// Undo [`frame_id`]. `None` for anything that is not one — including the
/// empty string a browser's `EventSource` sends before it has seen an id.
fn parse_frame_id(raw: &str) -> Option<(u64, u64)> {
    let (gen, seq) = raw.trim().split_once('-')?;
    Some((gen.parse().ok()?, seq.parse().ok()?))
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
    // Bounded as `wanted_fields` is: the unknown ones are written to the log
    // on every request, and a ~400 KB `?kinds=` of junk was a ~400 KB WARN
    // line each time, in a log that rotates by day, not by size.
    let mut asked: Vec<String> = Vec::new();
    for k in q.kinds.as_deref().unwrap_or("").split(',').map(str::trim) {
        if asked.len() == QUERY_LIST_MAX {
            break;
        }
        if k.is_empty() {
            continue;
        }
        let k = if k.len() > QUERY_ENTRY_MAX {
            let mut end = QUERY_ENTRY_MAX;
            while !k.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &k[..end])
        } else {
            k.to_string()
        };
        let k = k.to_ascii_lowercase();
        if !asked.contains(&k) {
            asked.push(k);
        }
    }
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

/// Kinds a host-bound caller (a per-host token: every in-session Claude)
/// never receives. `work` frames name tracker tickets from every host and
/// carry no session to scope them by; a host's token reads its own orgs'
/// tickets through `work { … }` (the M3 plan's decision 6, kept by M5 —
/// stricter than an org filter, and nothing on a host needs the stream).
/// `update` frames name every target of the fleet; a scoped caller reads
/// its own row through `update_status`.
/// `session` frames are fenced per frame instead ([`fence_frame`]).
/// `settings` frames name operator settings, which only the master token
/// reads (`get_settings`); nothing on a host or an org-bound phone needs them.
/// `grant` frames name a person and a session (multi-user M1, T9): a per-host
/// token has no person and holds no grants, and an org-bound client derives
/// nothing from somebody else's share. `download` frames name files of every
/// host; a scoped caller re-reads its own through `list_downloads`.
pub const HOST_BOUND_HIDDEN_KINDS: &[&str] = &[
    "work",
    "settings",
    "update",
    "grant",
    "download",
    "local_workspace",
    "confirm",
    "handoff",
];

/// Narrow the requested kinds for a host-bound caller; everyone else keeps
/// what they asked for.
///
/// **The predicate stays [`Caller::is_scoped`], deliberately** (multi-user
/// M1, T9). The plan's draft moved it to "is not the hub's own reader",
/// which would have stripped `work`, `settings` and `update` from the master
/// token and from a person's own paired desktop — three kinds the desktop
/// reads to redraw Settings, the work graph and Updates. That is a
/// regression, not a fence. The reason no person-wide hidden set is needed
/// is that every kind turned out to be either fenced per frame or free of
/// session-scoped content; [`KIND_FENCES`] says which, for all of them, and
/// `every_event_kind_is_fenced_or_declared_content_free` holds the list to
/// [`crate::events::EVENT_KINDS`].
pub(crate) fn fence_host_bound(caller: &Caller, kinds: Option<Vec<String>>) -> Option<Vec<String>> {
    // A client bound to an org (work graph M14) is fenced like a host: a
    // `work` frame names tickets of every org and carries no session to
    // fence it by; it reads its work through `work { … }`.
    // This is the org boundary, not a privacy fence: HOST_BOUND_HIDDEN_KINDS is the list an ORG-
    // bound or host-bound stream does not get, and the paragraph above argues why the predicate
    // stays `is_scoped`. The person's own fence is per FRAME, in `KIND_FENCES` and
    // `fence_frame`.
    if !caller.is_scoped() {
        return kinds;
    }
    let all = kinds.unwrap_or_else(|| {
        crate::events::EVENT_KINDS
            .iter()
            .map(|k| (*k).to_string())
            .collect()
    });
    Some(
        all.into_iter()
            .filter(|k| !HOST_BOUND_HIDDEN_KINDS.contains(&k.as_str()))
            .collect(),
    )
}

/// How a person's `/events` stream treats each [`EVENT_KINDS`] entry
/// (multi-user M1, T9).
///
/// The table exists because the hole T9 closed was not a wrong predicate —
/// it was a kind nobody had thought about. `fence_frame` returned early for
/// every kind that was not `session`, so `task:updated` (a whole `TaskRow`,
/// `prompt` / `result` / `error` included), `move:progress` and
/// `worktree:updated` were broadcast to every open stream, unexamined, and
/// nothing in the suite said so. Every kind now has to be classified here,
/// and `every_event_kind_is_fenced_or_declared_content_free` fails when one
/// is missing — so the NEXT kind somebody adds is a test failure rather than
/// a silent leak.
///
/// [`EVENT_KINDS`]: crate::events::EVENT_KINDS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KindFence {
    /// Judged per frame by [`fence_frame`] against the caller's
    /// [`ViewScope`]. The enumerating test proves, for each of these, that a
    /// stranger's frame is dropped and the owner's is not.
    PerFrame,
    /// Carries no session-scoped content at all. The string is the reason,
    /// and it has to be a claim a reader can check against the payload type
    /// — not "probably fine".
    NoSessionContent(&'static str),
}

/// See [`KindFence`]. One row per [`crate::events::EVENT_KINDS`] entry.
pub(crate) const KIND_FENCES: &[(&str, KindFence)] = &[
    ("session", KindFence::PerFrame),
    (
        "host",
        KindFence::NoSessionContent(
            "a `HostRow` and the `host:pinged` health sample. It has two session-derived \
            fields and neither reaches a stream as session metadata. \
            `unclaimed_sessions` is computed per REQUEST by `service::hosts::list_hosts` and \
            is never stored, so a row straight out of the store — which is all a frame ever \
            carries — holds `None` there. `last_hook_at` IS stored, written from hook \
            traffic (`service::hooks`) and throttled to HOST_HOOK_STAMP_EVERY_SECS: one \
            stamp per host, naming no session, and `list_hosts` serves it to everyone in \
            the org already, so fencing the frame would make the stream stricter than the \
            tool",
        ),
    ),
    (
        "account",
        KindFence::NoSessionContent("an `AccountRow`: a Claude account, its plan and its limits"),
    ),
    (
        "project",
        KindFence::NoSessionContent(
            "a `ProjectRow`: owner, repo, base path — plus `last_session_at`, and the frame \
            IS emitted by reconcile's session observation \
            (`touch_project_last_session_at_in_tx`, on any session's `last_activity_at` \
            advancing, another person's private one included). That one stamp is a \
            per-project aggregate naming no session, and `list_projects` serves it to \
            everyone in the org, so fencing the frame would make the stream stricter than \
            the tool that serves the same row",
        ),
    ),
    (
        "worktree",
        KindFence::NoSessionContent(
            "a `WorktreeRow` (`host_alias`, `project_id`, `name`, `path`, `branch`) or a \
            removed id. It names no session and carries no occupant: T8d's \
            `list_worktrees` fences at the OCCUPANT level and keeps the tree itself for \
            everyone in the org, so fencing the frame would make the stream stricter than \
            the tool that serves the same row",
        ),
    ),
    ("task", KindFence::PerFrame),
    (
        "account_usage",
        KindFence::NoSessionContent("an `AccountUsageSnapshot`, keyed by account uuid"),
    ),
    (
        "asset_inventory",
        KindFence::NoSessionContent("one catalog asset's drift state on one host for one harness"),
    ),
    (
        "catalog",
        KindFence::NoSessionContent("the catalog repo's HEAD and counts"),
    ),
    (
        "sync",
        KindFence::NoSessionContent("a `SyncProgress`: plan id, host, harness, done/total"),
    ),
    ("move", KindFence::PerFrame),
    (
        "start",
        KindFence::NoSessionContent(
            "a `StartProgress`: the caller's own opaque `start_token` (validated to \
            `[A-Za-z0-9_-]{1,64}`), one of three fixed step names, its index and a state. \
            It names no host, no session, no person and no path; only the client that \
            minted the token can tell whose start it is",
        ),
    ),
    (
        "work",
        KindFence::NoSessionContent(
            "tracker items, trackers and Work-view structure. Org data, not session data, \
            and fenced by the ORG half it belongs to: `HOST_BOUND_HIDDEN_KINDS` keeps it \
            off every host-bound and org-bound stream entirely",
        ),
    ),
    (
        "settings",
        KindFence::NoSessionContent("an operator setting's key; also host-bound hidden"),
    ),
    (
        "update",
        KindFence::NoSessionContent("an update target's id; also host-bound hidden"),
    ),
    ("grant", KindFence::PerFrame),
    (
        "download",
        KindFence::NoSessionContent(
            "a file download's row id and nothing else (`RowChange::DownloadChanged(i64)`),             the same ids-only shape as `settings` and `update`, and host-bound hidden             besides. NOT `PerFrame`: the frame names no session to fence it BY — the             download id would have to be resolved to one first — and it does not have to,             because what the id unlocks is already fenced at the `own` tier. A person who             is not the session's owner reads `list_downloads` after this frame and gets             nothing, `GET /downloads/<id>` answers 404, and `remove_download` is a no-op             (`service::downloads::visible`, through `ViewScope::may_own`). The residue is             that a bare integer tells another person's device that SOME download changed;             it names no host, no session, no path and no person, and it buys nothing a             poll of `list_downloads` would not already answer",
        ),
    ),
    (
        "local_workspace",
        KindFence::NoSessionContent(
            "a local workspace link's row id and nothing else \
            (`RowChange::LocalWorkspaceChanged(i64)`). Links live on the desktop that made \
            them and are never served by a hub, so a hub's stream never carries one; \
            host-bound hidden besides",
        ),
    ),
    (
        "confirm",
        KindFence::NoSessionContent(
            "nothing: `RowChange::ConfirmChanged` is an empty object. What is waiting is \
            read through `mcp_confirms`, which answers the hub's personal owner on their \
            own paired device only; anyone else learns that SOME confirmation queue moved \
            and nothing more. Host-bound hidden besides",
        ),
    ),
    (
        "handoff",
        KindFence::NoSessionContent(
            "nothing: `RowChange::HandoffChanged` is an empty object. What Control's agent \
            sent where is read through `control_handoffs`, which answers the hub's personal \
            owner on their own paired device only; anyone else learns that SOME receipt was \
            written and nothing more. Host-bound hidden besides",
        ),
    ),
];

/// [`KIND_FENCES`] for one kind, or `None` for a kind the table forgot —
/// which [`fence_frame`] treats as a drop, so a kind added without a row
/// here fails closed as well as failing its test.
fn kind_fence(kind: &str) -> Option<KindFence> {
    KIND_FENCES
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, f)| *f)
}

/// One session row out of the store, for a frame that names its session by
/// id rather than carrying it.
///
/// `Ok(None)` is "no such row" and `Err` is "could not read it", kept apart
/// so a caller cannot read a failed store read as "nothing to check"
/// (multi-user M1, T9c). Every caller here drops the frame on either, which
/// is the fail-closed answer for both.
///
/// **An id is not an identity.** `sessions.id` is `INTEGER PRIMARY KEY`
/// with no `AUTOINCREMENT` and reconcile hard-deletes a ghosted row, so the
/// row this returns for a given id is the frame's subject only if no
/// delete-and-reuse of that id landed between the frame's mint and this
/// call. Never call it for a frame out of the replay ring, where that is
/// simply false; on the LIVE path it is the narrowed — not closed —
/// residual [`Delivery`] measures and attributes.
fn session_row(store: &Mutex<Store>, id: i64) -> Result<Option<crate::store::SessionRow>, ()> {
    let s = store.lock().map_err(|_| ())?;
    s.get_session_by_id(id).map_err(|_| ())
}

/// The ORG redaction the work graph has always applied to a session frame:
/// a row whose work belongs to a company this caller may not read keeps its
/// identity and loses its work fields.
fn redact_work(scope: &ViewScope, payload: &serde_json::Value) -> serde_json::Value {
    let mut v = payload.clone();
    scope.org.redact_json(&mut v, &|m| {
        m.get("org_id").and_then(serde_json::Value::as_i64)
    });
    v
}

/// Is this frame going out LIVE, or out of the replay ring?
///
/// **THE REPLAY RULE (multi-user M1, T9d): the ring is replayed to the
/// hub's own reader and to nobody else.** A caller whose [`ViewScope`] is
/// not [`ViewScope::internal`] is answered `resumed: false` and re-lists —
/// the behaviour `/events` already has for a gap the history cannot cover,
/// so the client path exists and the shape is precedented
/// ([`StreamState::resumed`], and the `ready` frame's own `resumed` key).
/// It is enforced twice on purpose: [`handle_events`] builds no replay for
/// such a caller, and [`fence_frame_as`] drops a `Delivery::Replay` frame
/// for one anyway, so the fence the tests call is the fence the stream
/// gets.
///
/// **Why the clever versions were removed, so nobody re-derives them.**
/// A replayed frame is a frame about the fleet as it WAS, judged now, and
/// the two mechanisms tried before this one both failed on identity:
///
/// * *Judge the frame by the facts it carries.* A session that has since
///   become another person's, or whose host has since moved org, is then
///   described by a frame that says it had not (T9b).
/// * *Judge it against the row that holds its id now, refusing a row born
///   after the frame.* `sessions.id` is `INTEGER PRIMARY KEY` with no
///   `AUTOINCREMENT` (migration 001) and reconcile hard-deletes a ghosted
///   row, so the id names a DIFFERENT session later — and the only birth
///   marker available was `sessions.created_at`, which is tmux's
///   `#{session_created}` read over ssh (`service/sessions/reconcile.rs`
///   binds `created_at: sess.created`, `tmux.rs`'s `SESSIONS_FORMAT`), i.e.
///   the REMOTE HOST's clock, while the frame's mint time was the hub's.
///   Comparing two clocks on two machines with no guaranteed sync fails in
///   both directions: a host clock behind the hub's lets a reused id
///   through, and a host clock ahead of it silently DROPS frames the caller
///   is entitled to while still answering `resumed: true`. No amount of
///   tightening fixes a cross-clock comparison, and `created_at` is also
///   absent from the upsert's `DO UPDATE SET`, so a rediscovered tmux
///   session returns with its original old stamp on a recycled rowid
///   (T9c). This is the mechanism that was deleted, together with
///   `EventMessage.minted_at`, which existed only to serve it.
///
/// Refusing the replay is clock-free and monotonic by construction: there
/// is no identity to confirm, because no frame about the past is judged
/// under a caller's present scope at all. It also makes
/// [`scope_fingerprint`] being content-addressed rather than monotonic
/// harmless — a scope that round-trips to a previous value (revoke then
/// re-grant at the same level) re-validates a `Last-Event-ID`, and gets
/// `resumed: false` and an empty replay for it.
///
/// The LIVE path still judges a row frame from the payload it carries, and
/// must: `?fields=` projection runs after the fence, so the payload is the
/// whole row. That shape — identity CARRIED, never resolved — is sound on
/// its own terms and needs no claim about timing at all.
///
/// **What "live" does NOT mean, said plainly because two comments used to
/// claim it did** (multi-user M1, T9e). The fence does not run when a frame
/// is minted; it runs when the frame is DELIVERED, inside the consumer task
/// (`st.rx.recv()`, then `row_event(…, Delivery::Live)`).
/// `BroadcastEventBus::emit` never blocks and the channel holds
/// [`crate::events::BROADCAST_CAPACITY`] frames, so a frame can sit unread
/// for as long as a slow SSE consumer takes to drain up to that many — a
/// bound in COUNT, never in time (hyper stops polling the body while the
/// client's window is full, and only a subscriber more than a bufferful
/// behind is terminated as `Lagged`). Nothing re-checks the frame's SUBJECT
/// before it goes out either: the live arm re-reads the CALLER's scope, and
/// only when `org_generation` / `grant_generation` moved — and neither
/// counter moves on a session delete.
///
/// So the three arms that resolve `sessions.id` at delivery —
/// [`fence_session_frame`]'s id-only branch (`session:event`,
/// `session:conversations`), [`fence_move_frame`], and
/// [`fence_grant_frame`]'s owner arm — judge a frame about session A
/// against whichever row holds A's rowid by the time the stream is polled.
/// `sessions.id` is `INTEGER PRIMARY KEY` with no `AUTOINCREMENT`, so a
/// deleted row's id is handed to the next insert; the route test
/// `the_replay_ring_is_fenced_off_from_every_caller` asserts the frame
/// reaching the wrong person with no replay involved. **The residual window
/// is bounded by broadcast residency, not by the write** — it is the same
/// identity class as the replay hole with a smaller window, and the replay
/// rule narrowed it rather than closing it.
///
/// **Closing it is an owner decision, and it is not a third clever
/// mechanism.** It is to give the id-only frames the facts they need, the
/// way `session:killed` already carries them
/// (`SessionKilledPayload`: `host_alias` / `org_id` / `visibility` /
/// `owner_person_id`, read in the same transaction as the write) and judge
/// them through [`crate::service::view_scope::ViewScope::sees_session_facts`],
/// the one body the row-bearing branch already uses. Then no arm resolves
/// `sessions.id` and the fence stops depending on delivery latency. This is
/// NOT T9b's rejected mechanism: T9b judged carried facts about the PAST
/// (stale attributes of a frame whose subject may since have changed
/// hands), while here the facts IDENTIFY the subject, so the worst failure
/// becomes "a person gets a stale frame about a row that was theirs"
/// instead of "a stranger is handed another person's row because SQLite
/// recycled a rowid". It is a wire-shape change to `session:event`,
/// `session:conversations`, `move:progress` and `grant:changed`, which is
/// why it is the owner's and not this task's.
///
/// The replay POSITION is unauthenticated, and that is why this is the
/// rule. Nothing constrains the `seq` a caller sends: the gate in
/// [`handle_events`] checks only that the GENERATION half of a
/// `Last-Event-ID` matches, and that half is the value this hub itself
/// stamps on every frame it sends this caller, so the caller can re-send
/// its own generation with any sequence it likes.
/// `BroadcastEventBus::replay_after` then accepts any `seq >= oldest - 1`,
/// and `oldest` is findable by bisection on `ready.resumed`. No per-caller
/// or per-connection high-water mark is kept anywhere — not in
/// [`StreamState`], not in the bus. An unauthenticated position into a
/// GLOBAL ring is safe only while nothing a caller may not see can come out
/// of it; "replay nothing to a caller" is the one form of that which needs
/// no identity argument.
///
/// [`ViewScope`]: crate::service::view_scope::ViewScope
/// [`ViewScope::internal`]: crate::service::view_scope::ViewScope::internal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// Straight off the bus.
    Live,
    /// Out of the replay ring, for a `Last-Event-ID` / `?since=` resume.
    Replay,
}

/// The fence on ONE frame for ONE stream: `None` drops it, otherwise the
/// payload this caller may have.
///
/// **What changed in multi-user M1 (T9), and why the old shape was the
/// hole.** This function used to open with
/// `if scope.is_all() || msg.kind() != "session"`, taking an
/// [`OrgScope`](crate::service::orgs::OrgScope) — and `OrgScope::All`'s own
/// doc comment lists "an unbound paired client" among the callers it
/// covers, which is exactly the shape a person's device has. So the entire
/// T6/T7/T8 person gate was bypassable with one `GET /events`: a second
/// person's phone subscribed and received every `session:created` /
/// `session:updated` in the fleet, each carrying a whole `SessionRow`.
///
/// **The verbatim pass is now [`ViewScope::is_internal`] and nothing else.**
/// The plan's literal inversion —
/// `person.is_none() && host.is_none() && org.is_all()` — would have been a
/// second hole in the opposite direction: that is precisely the REFUSING
/// scope `Caller::view_scope` builds for a device no pairing bound, so it
/// would have passed every frame in the fleet to the one caller that must
/// see nothing. "The hub does its work" and "a caller sees everything"
/// stopped being one value when `ViewScope` was introduced, and this is one
/// of the sites that depends on the difference.
///
/// Per kind, see [`KIND_FENCES`]. The two session shapes:
///
/// * a frame that IS the row (`session:created`, `session:updated`) or
///   carries its facts (`session:killed`, whose row is already deleted) is
///   judged from the payload. That is safe HERE, and only here, because
///   `?fields=` projection runs AFTER this function, in [`row_event`] — so
///   the payload is the whole row, and `visibility` (`NOT NULL` in the
///   schema, therefore never stripped) is always on it. A session frame
///   carrying no `visibility` key and no `session_id` is dropped: fail
///   closed, which for a bare `session:killed` is the documented answer.
/// * a frame that only NAMES its session (`session:event`,
///   `session:conversations`) is resolved in the store and judged as a row.
///   A row that is gone is dropped rather than passed.
pub(crate) fn fence_frame(
    scope: &ViewScope,
    msg: &EventMessage,
    store: &Mutex<Store>,
) -> Option<serde_json::Value> {
    fence_frame_as(scope, msg, store, Delivery::Live)
}

/// [`fence_frame`] for a frame whose [`Delivery`] is not the live path.
pub(crate) fn fence_frame_as(
    scope: &ViewScope,
    msg: &EventMessage,
    store: &Mutex<Store>,
    delivery: Delivery,
) -> Option<serde_json::Value> {
    if scope.is_internal() {
        return Some(msg.payload.clone());
    }
    // THE REPLAY RULE. See [`Delivery`] for the two cleverer mechanisms
    // this replaced and why each was unsound. Everything below judges a
    // frame against CURRENT state, which is the wrong answer for a frame
    // about the fleet as it WAS — and, for the three arms that resolve a
    // recyclable `sessions.id`, an answer whose soundness rests on no
    // delete-and-reuse of that id landing between the mint and the
    // DELIVERY, which is where this runs. `Delivery` spells out that
    // residual and what closes it; the row-bearing arms carry their
    // identity and do not depend on it.
    if delivery == Delivery::Replay {
        return None;
    }
    match kind_fence(msg.kind()) {
        None => {
            tracing::warn!(
                event = msg.name,
                "[events] no KIND_FENCES row for this kind; dropping the frame"
            );
            None
        }
        Some(KindFence::NoSessionContent(_)) => Some(msg.payload.clone()),
        Some(KindFence::PerFrame) => match msg.kind() {
            "session" => fence_session_frame(scope, msg, store),
            "task" => fence_task_frame(scope, msg, store),
            "move" => fence_move_frame(scope, msg, store),
            "grant" => fence_grant_frame(scope, msg, store),
            // Unreachable while `KIND_FENCES` and this match agree, and a
            // drop if they ever stop agreeing.
            other => {
                tracing::warn!(
                    kind = other,
                    "[events] KIND_FENCES says per-frame but no arm fences it; dropping"
                );
                None
            }
        },
    }
}

/// A `session:*` frame. See [`fence_frame`] for the two shapes, and
/// [`Delivery`] for why it is only ever asked about a LIVE one.
fn fence_session_frame(
    scope: &ViewScope,
    msg: &EventMessage,
    store: &Mutex<Store>,
) -> Option<serde_json::Value> {
    let obj = msg.payload.as_object()?;
    let id = obj.get("id").and_then(serde_json::Value::as_i64);
    let host = obj.get("host_alias").and_then(serde_json::Value::as_str);
    let visibility = obj.get("visibility").and_then(serde_json::Value::as_str);
    if let (Some(id), Some(host_alias), Some(visibility)) = (id, host, visibility) {
        let facts = crate::service::view_scope::SessionFacts {
            id,
            host_alias,
            org_id: obj.get("org_id").and_then(serde_json::Value::as_i64),
            visibility,
            // Absent is UNOWNED, never "owned by whoever is asking":
            // `strip_nulls` takes a null key off the wire, so this is the
            // `(None, None)` trap's doorstep and `ViewScope::owns_person`
            // is written so it cannot be stepped through.
            owner_person_id: obj
                .get("owner_person_id")
                .and_then(serde_json::Value::as_i64),
        };
        if !scope.sees_session_facts(&facts).is_visible() {
            return None;
        }
        return Some(redact_work(scope, &msg.payload));
    }
    // The id-only shape: `session:event` and the conversation frames name
    // their session and carry nothing else, so the row is resolved and
    // judged.
    //
    // **This arm is LIVE-ONLY and that is a precondition, not a proof.**
    // The replay rule in [`fence_frame_as`] keeps a frame about the past
    // out of here, which is what makes the arm defensible at all — but the
    // fence runs at DELIVERY, not at mint, so the id names the row that
    // holds it when the stream is polled and not necessarily the row the
    // write touched. `sessions.id` is recyclable. See [`Delivery`] for the
    // measured bound (broadcast residency, counted not timed), the route
    // test that asserts the leak, and the fix (carry the facts, as
    // `session:killed` does) — which is a wire change and the owner's.
    let sid = obj.get("session_id").and_then(serde_json::Value::as_i64)?;
    let row = session_row(store, sid).ok().flatten()?;
    scope
        .sees_session_row(&row)
        .is_visible()
        .then(|| redact_work(scope, &msg.payload))
}

/// `task:updated` — a whole `TaskRow`, `prompt` / `result` / `error`
/// included: the paragraphs the two Claudes wrote. Judged by exactly the
/// predicate `list_tasks` judges the same row with
/// ([`crate::service::tasks::task_visible_in_scope`]), so a task a caller
/// cannot list cannot arrive on its stream either.
///
/// **The row is re-read from the store, not taken from the payload**
/// (multi-user M1, T9d). Two reasons, and the first is a leak that needed
/// no replay at all:
///
/// * the fence's own input is `detached_at`, which is `#[serde(skip)]` and
///   therefore never on a frame — judging the payload would read every task
///   as attached, i.e. would judge a reaped session's ends against whoever
///   holds those recycled ids now. See
///   [`crate::service::tasks::task_visible_in_scope_pure`] and migration
///   097 for the hazard;
/// * `tasks.id` is the one stable identity in this frame (no delete path
///   touches the table), so re-reading by it is the "resolve the subject"
///   shape the session arms take, without the id-reuse question.
///
/// A payload with no `id`, or an id with no row, is dropped: fail closed.
fn fence_task_frame(
    scope: &ViewScope,
    msg: &EventMessage,
    store: &Mutex<Store>,
) -> Option<serde_json::Value> {
    let id = msg.payload.get("id").and_then(serde_json::Value::as_i64)?;
    let s = store.lock().ok()?;
    let task = s.get_task(id).ok().flatten()?;
    crate::service::tasks::task_visible_in_scope(&s, &task, scope)
        .ok()?
        .then(|| msg.payload.clone())
}

/// `move:progress` — the SOURCE session's id, the host it is moving to, and
/// a short `detail`.
///
/// The row is resolved by id, so this arm carries the same precondition as
/// [`fence_session_frame`]'s id-only branch: live only, and the window
/// between mint and delivery is bounded by broadcast residency rather than
/// by the write (see [`Delivery`]). **It does NOT rest on the source row
/// outliving the run.** T9d's version of this comment said `move_session`
/// "GHOSTS the source rather than deleting it, so the row is there to judge
/// for the whole run", and the same change disproved it: a ghost IS reaped —
/// `crate::service::move_session::PartialCtx::from_tmux_name` exists
/// precisely because "a source ghosted and reaped between the partial and
/// the resolution handed `Finish` a brand-new session on the same host to
/// kill". When the row is gone this drops the frame, which is the
/// fail-closed answer and needs no claim about the row's lifetime.
fn fence_move_frame(
    scope: &ViewScope,
    msg: &EventMessage,
    store: &Mutex<Store>,
) -> Option<serde_json::Value> {
    let sid = msg
        .payload
        .get("session_id")
        .and_then(serde_json::Value::as_i64)?;
    // Live only: this frame names its session by id and nothing else, so
    // the id has to be the row the write just touched. See [`Delivery`].
    let row = session_row(store, sid).ok().flatten()?;
    scope
        .sees_session_row(&row)
        .is_visible()
        .then(|| msg.payload.clone())
}

/// `grant:changed` — one share of one session, ids only.
///
/// Two people are entitled to it and nobody else: the person the grant
/// NAMES (who patches their own grant set from it) and the session's OWNER
/// (the only other party to a share — the Share sheet's own view). The
/// owner arm needs the row, so a share of a session that has since gone
/// reaches only the named person, which is the fail-closed direction.
fn fence_grant_frame(
    scope: &ViewScope,
    msg: &EventMessage,
    store: &Mutex<Store>,
) -> Option<serde_json::Value> {
    let obj = msg.payload.as_object()?;
    let person = obj.get("person_id").and_then(serde_json::Value::as_i64)?;
    let session_id = obj.get("session_id").and_then(serde_json::Value::as_i64)?;
    if matches!(scope.person, Some(p) if p == person) {
        return Some(msg.payload.clone());
    }
    // The OWNER arm resolves the session by id: live only, and with the
    // same residual as the other two id-resolving arms — with a REUSED id
    // the owner of the NEW row is handed a share frame about the old one,
    // and the replay rule in [`fence_frame_as`] narrows that window to the
    // broadcast's residency rather than closing it ([`Delivery`]). The
    // payload is ids only, so what leaks here is "somebody shared something
    // with somebody", not a row. The person arm above is the named party's
    // own grant row and needs no session row at all.
    let row = session_row(store, session_id).ok().flatten()?;
    scope.owns(&row).then(|| msg.payload.clone())
}

pub(crate) fn matches(kinds: Option<&Vec<String>>, msg: &EventMessage) -> bool {
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
    /// Draining the replay the client's `Last-Event-ID` asked for. Strictly
    /// in sequence order and never interleaved with live frames: the live
    /// receiver is buffering meanwhile, and anything it holds that the replay
    /// already covered is dropped by sequence number in [`Phase::Live`].
    Replay,
    Live,
    Done,
}

/// Everything one connection owns. The permit rides in here — not merely in
/// the handler — so a slot is held for the LIFE of the stream and released
/// when the client disconnects and axum drops it.
struct StreamState {
    rx: Receiver<EventMessage>,
    kinds: Option<Vec<String>>,
    /// `?fields=` — keep only these keys of each row payload.
    fields: Option<Vec<String>>,
    /// This hub's sequence generation, stamped into every `id:`.
    generation: u64,
    /// The events the resume asked for, oldest first, popped front to back.
    replay: std::collections::VecDeque<EventMessage>,
    /// Whether the client's `Last-Event-ID` was honoured; echoed in `ready`.
    ///
    /// `false` for every caller but the hub's own reader, by the replay rule
    /// — see [`Delivery`].
    resumed: bool,
    /// The highest sequence number already written to this connection.
    /// A live frame at or below it was covered by the replay and is dropped
    /// rather than sent twice — which for a `session:killed` followed by a
    /// re-create would otherwise be sent out of order.
    ///
    /// The BUS's number, always: it is matched against `msg.seq`, not
    /// against what goes out as an `id:`.
    sent_through: u64,
    /// The `seq` half of the last `id:` written to this connection.
    ///
    /// Equal to `sent_through` for the hub's own reader, which resumes off
    /// the shared ring and must get the bus's own numbers. For everybody
    /// else it counts the frames THIS connection was served, so the id no
    /// longer reports the fleet's frame rate to a caller who may see one
    /// session of fifty. See [`frame_id`].
    wire_seq: u64,
    /// `true` when `id:` carries [`Self::wire_seq`] instead of the bus's
    /// `seq`: every caller but the hub's own reader. Read off the scope once
    /// at open, because a scope that CHANGES ends the stream rather than
    /// being swapped in place — so this cannot go stale under us.
    per_connection_ids: bool,
    phase: Phase,
    _permit: LongPollPermit,
    label: String,
    shutdown: CancellationToken,
    /// The paired client behind this stream, the store to re-read it in, and
    /// the binding it opened under: `None` for the master token and for a
    /// per-host token, which are not revocable this way.
    client: Option<(Arc<Mutex<Store>>, i64, crate::store::ClientBinding)>,
    /// A per-host token's host and the SHA-256 of the token the stream
    /// opened with (review r04 K2): a rotation or a removal ends the stream
    /// on the next beat, as `agent/ws.rs` already does for an agent.
    host: Option<(String, String)>,
    /// Fires on the keep-alive beat; each tick re-checks [`StreamState::client`]
    /// and [`StreamState::host`].
    heartbeat: tokio::time::Interval,
    /// The caller's view scope (work graph M5's org half, multi-user M1's
    /// person half), applied to every frame.
    scope: ViewScope,
    /// The store and the caller to re-read [`Self::scope`] from, on every
    /// beat and before any frame whose generation moved. A scope that moved
    /// **ENDS** the stream — it is not re-scoped in place — so one grant
    /// change disconnects every affected stream and each reconnects under
    /// its new scope. That is fail-closed and deliberate; the price is a
    /// reconnect, and the ≤15 s bound in `docs/hub.md` is this beat.
    ///
    /// **Not an `Option`.** It was `caller.is_scoped().then(…)`, which is
    /// FALSE for an unbound paired client — a person's device — and both
    /// consumers sat behind `if let Some(…)`, so for the one caller
    /// multi-user M1 fences a revoked grant was noticed on the stream
    /// never.
    store: Arc<Mutex<Store>>,
    caller: Caller,
    /// The org generation `scope` was read at (`service::orgs::org_generation`).
    scope_generation: u64,
    /// Which session ids this CONNECTION has actually served a frame about
    /// (multi-user M1, T10) — the memory a departure announcement needs.
    ///
    /// A session can leave a stream's fence without the stream's own scope
    /// moving: a host's org move, an ownership change and a `visibility`
    /// change all move the SESSION. The scope re-read on the keep-alive
    /// beat and the two generation counters only end a stream when the
    /// CALLER changed, so before this set existed the fence simply started
    /// dropping that row's frames and nothing on the wire said so — a
    /// paired phone kept displaying the row for ever, because the desktop
    /// bridge re-lists only on `resumed: false`
    /// (`src-tauri/src/backend/events.rs`).
    ///
    /// Only ids this connection was actually SERVED go in, so the
    /// announcement is never the first thing a caller hears about a
    /// session: a row the fence refused from the start is not in the set
    /// and its departure is not announced. That is what keeps the set from
    /// being an existence oracle, and it is why the insert happens where
    /// the frame goes OUT rather than where it is received.
    ///
    /// Bounded by the sessions this caller may see, not by the fleet's
    /// frame rate: ids, inserted once each, and removed on the way out.
    served: BTreeSet<i64>,
    /// The grant generation `scope` was read at
    /// ([`crate::store::grant_generation`]).
    ///
    /// Its own counter, never `auth_epoch` (rule 8): revoking a DEVICE and
    /// revoking a GRANT are different events with different mechanisms, and
    /// riding the token cache would rebuild every token's cache for one
    /// row's change. Both counters are process-local atomics, so they are
    /// an OPTIMISATION — the guarantee is the keep-alive re-read above.
    grant_generation: u64,
}

/// Is that `client_tokens` row still live (present, not revoked, and bound
/// to what it was bound to when the stream opened)?
///
/// The lock is taken and dropped inside this synchronous function, so no
/// guard ever crosses an `.await`. Anything but a clear "yes" — a poisoned
/// lock, a failed read — ends the stream: a client that reconnects loses
/// nothing but a round trip, while a revoked one kept on the feed is the
/// failure this check exists to prevent.
///
/// This is the DEVICE mechanism (multi-user M1, rule 8). Revoking a device
/// and revoking a SHARE are different events: a grant change never drops a
/// stream here — it travels as its own `grant:changed` frame — and a device
/// re-bound to another person drops it here, exactly as an org re-bind
/// already did, because everything that device may see changed at once.
fn client_is_live(store: &Mutex<Store>, id: i64, opened_as: crate::store::ClientBinding) -> bool {
    let Ok(s) = store.lock() else {
        tracing::warn!("[events] store lock poisoned; ending the stream");
        return false;
    };
    match s.client_token_binding(id) {
        Ok(live) => live == Some(opened_as),
        Err(e) => {
            tracing::warn!(error = %e.message, "[events] could not re-check the client; ending the stream");
            false
        }
    }
}

/// Does `alias`'s token row still hold the token the stream opened with?
///
/// The per-host twin of [`client_is_live`]: a rotation replaces the token
/// and removing the host deletes the row, and either one ends the stream.
/// A narrowing to `readonly` does not, since `/events` only reads. Anything
/// but a clear yes ends it.
fn host_token_is_live(store: &Mutex<Store>, alias: &str, credential: &str) -> bool {
    let Ok(s) = store.lock() else {
        tracing::warn!("[events] store lock poisoned; ending the stream");
        return false;
    };
    match s.get_host_token(alias) {
        Ok(Some(row)) => super::auth::constant_time_eq(
            super::auth::sha256_hex(&row.token).as_bytes(),
            credential.as_bytes(),
        ),
        _ => false,
    }
}

fn sse_event(name: &str, payload: &serde_json::Value) -> Event {
    // `to_string` on a `Value` cannot fail.
    Event::default().event(name).data(payload.to_string())
}

/// One row frame: fenced for this caller, projected if the client asked for
/// fields, and carrying the `id:` a reconnect resumes from. `None` when the
/// fence drops it.
///
/// The projection runs AFTER the fence, which is what lets
/// [`fence_session_frame`] judge a row frame from its own payload — see
/// [`fence_frame`].
fn row_event(
    msg: &EventMessage,
    fields: Option<&Vec<String>>,
    generation: u64,
    id_seq: u64,
    scope: &ViewScope,
    store: &Mutex<Store>,
    delivery: Delivery,
) -> Option<Event> {
    let fenced = match delivery {
        // By its own name on the live path, so the function the fence tests
        // call is the function the stream calls.
        Delivery::Live => fence_frame(scope, msg, store)?,
        Delivery::Replay => fence_frame_as(scope, msg, store, delivery)?,
    };
    let payload = match fields {
        Some(f) => project(&fenced, f),
        None => fenced,
    };
    // `id_seq`, not `msg.seq`: the caller's own position on this
    // connection unless it is the one reader that resumes. See [`frame_id`].
    Some(sse_event(msg.name, &payload).id(frame_id(generation, id_seq)))
}

/// The session a `session:*` frame is ABOUT, in whichever of the two shapes
/// it names it (multi-user M1, T10).
///
/// `session:created` / `session:updated` / `session:killed` ARE the row (or
/// its carried facts), so the key is `id`; `session:event` and
/// `session:conversations` only name their session, so the key is
/// `session_id`. Every other kind answers `None` — a `host:*` or `work:*`
/// frame has no session to depart.
///
/// Deliberately not `kind()`-agnostic: `move:progress` and `grant:changed`
/// also carry a `session_id`, and neither is a statement about the row's
/// existence; `work:item` and friends spell their OWN id as `id`. Only a
/// `session:*` frame says anything about a session.
///
/// This is the RECORDING side ([`record_served`]). The announcing side
/// ([`departure_for`]) is stricter still: it reads `id` only, i.e. the
/// shapes whose fence decision was made on the row's own facts.
fn frame_session_id(msg: &EventMessage) -> Option<i64> {
    if msg.kind() != "session" {
        return None;
    }
    let obj = msg.payload.as_object()?;
    obj.get("id")
        .or_else(|| obj.get("session_id"))
        .and_then(serde_json::Value::as_i64)
}

/// Record a frame that actually went OUT, so a later departure can be
/// announced (multi-user M1, T10). Pure over the set, so the rule is
/// testable without a live stream.
///
/// A `session:killed` that went out IS a departure, so its id LEAVES the
/// set rather than entering it: otherwise a kill followed by a reused rowid
/// would make the next refusal announce a session the caller has already
/// been told is gone.
fn record_served(served: &mut BTreeSet<i64>, msg: &EventMessage) {
    let Some(sid) = frame_session_id(msg) else {
        return;
    };
    if msg.name == "session:killed" {
        served.remove(&sid);
    } else {
        served.insert(sid);
    }
}

/// What a FENCED-OUT frame leaves behind: `Some(session_id)` when this
/// connection owes a departure announcement for it, `None` when the answer
/// is silence (multi-user M1, T10). Pure over the set; takes the id out, so
/// a departure is announced at most once per session per connection.
///
/// Two conditions, and each removes a way of being wrong:
///
/// * the session must be one this connection was SERVED a frame about.
///   That is what keeps the set from being an existence oracle: a row the
///   fence refused from the start is never announced as having left.
/// * the frame must be one whose fence decision was made on the row's own
///   FACTS — `session:created` / `session:updated` / `session:killed`, which
///   spell the session as `id` — and never one of the shapes that had to
///   RESOLVE the id in the store (`session:event`,
///   `session:conversations`). A resolution can fail for reasons that have
///   nothing to do with visibility (a poisoned lock, a read error, a row
///   reaped in the microsecond between two reads), and "the store was
///   briefly unreadable" must not reach a client as "the session is gone" —
///   a removal it can only undo by re-listing. The triggers this
///   announcement exists for all emit a row frame anyway
///   (`Store::emit_session` on an ownership or visibility change), and a
///   real delete carries its facts on `session:killed`.
fn departure_for(served: &mut BTreeSet<i64>, msg: &EventMessage) -> Option<i64> {
    if msg.kind() != "session" {
        return None;
    }
    let sid = msg
        .payload
        .as_object()?
        .get("id")
        .and_then(serde_json::Value::as_i64)?;
    served.remove(&sid).then_some(sid)
}

/// **The departure announcement** (multi-user M1, T10): the frame a stream
/// sends about a session that has LEFT its fence.
///
/// It is a `session:killed` with the id and nothing else, and that is the
/// design rather than a convenience:
///
/// * [`crate::service::view_scope::Visibility::None`]'s own contract is
///   that the row "answers exactly as an id that does not exist". A frame
///   saying *this session no longer exists for you* is therefore not a lie
///   about the fleet, it is the fence's own statement about this caller's
///   world — and `session:killed`'s id-only payload is already exactly that
///   sentence. A new frame kind would have said the same thing in a shape
///   nothing understands.
/// * It is **indistinguishable from a real kill**, which is the privacy
///   property. A distinct `session:left` would tell a former watcher "the
///   session is still running, you just lost access" — an oracle on a row
///   the fence has stopped serving.
/// * No consumer changes. `src/lib/events.ts` maps `session:killed` to
///   `{ type: 'killed', id }` and removes the row; the desktop bridge's
///   `payload_fits` requires `id` and nothing else.
///
/// It is sent at most once per session per connection, because the id is
/// taken OUT of [`StreamState::served`] as the frame is built, and it is
/// sent only for a session this connection had already been served a frame
/// about — so it is never a caller's first word about a row (see
/// [`StreamState::served`]).
///
/// Projected like any other frame when the client asked for `?fields=`,
/// for the indistinguishability above. A client that asks for fields
/// without `id` cannot apply a real `session:killed` either.
fn departure_event(
    session_id: i64,
    fields: Option<&Vec<String>>,
    generation: u64,
    id_seq: u64,
) -> Event {
    let payload = serde_json::json!({ "id": session_id });
    let payload = match fields {
        Some(f) => project(&payload, f),
        None => payload,
    };
    sse_event("session:killed", &payload).id(frame_id(generation, id_seq))
}

/// The `seq` to stamp into the next `id:` this connection writes.
///
/// The bus's own number for the hub's own reader — the only caller that
/// resumes off the shared ring, so the only one for whom the id has to mean
/// a ring position. One more than the last id written for everybody else.
/// See [`frame_id`].
fn next_id_seq(st: &StreamState, msg: &EventMessage) -> u64 {
    if st.per_connection_ids {
        st.wire_seq + 1
    } else {
        msg.seq
    }
}

/// The scope a stream holds, refreshed on the keep-alive beat and whenever
/// an org or grant generation moved.
///
/// `None` — a poisoned lock, a failed read — ends the stream rather than
/// streaming on under a scope nobody could confirm.
fn read_scope(store: &Mutex<Store>, caller: &Caller) -> Option<ViewScope> {
    let s = store.lock().ok()?;
    caller.view_scope(&s).ok()
}

/// A fingerprint of everything [`ViewScope`]'s own `PartialEq` compares,
/// folded into the `id:` every frame carries (multi-user M1, T9).
///
/// **Why the id and not the bus's generation.** The stream ENDS when a
/// caller's scope moves; it does not re-scope in place. The client then
/// reconnects with its last `Last-Event-ID` — and without this the hub
/// would replay the gap under the OLD scope's eyes, answer `resumed: true`,
/// and the desktop bridge (which re-lists only on `resumed: false`) would
/// keep the revoked row in the Svelte store indefinitely. Bumping the bus's
/// own generation instead would fix that by forcing EVERY phone on the hub
/// to re-list for one person's grant change.
///
/// Since T9d it is belt-and-braces rather than the load-bearing check: no
/// caller is replayed to at all ([`Delivery`]), so a `Last-Event-ID` minted
/// under any scope buys the same empty replay and `resumed: false`. It stays
/// because it costs one hash per connection and because the reason above is
/// still the reason a resume must not be honoured across a scope change if
/// replay ever returns for some caller.
///
/// `Debug` rather than a hand-written tuple, and that is the point: it
/// renders every field, including the private `sole_person` and `internal`,
/// so it is exactly as discriminating as the `PartialEq` the keep-alive
/// comparison uses. A field added to `ViewScope` is in the fingerprint the
/// day it is added, with nobody having to remember this function exists.
fn scope_fingerprint(scope: &ViewScope) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{scope:?}").hash(&mut h);
    h.finish()
}

/// `GET /events`.
pub(super) async fn handle_events(
    State(state): State<EventsState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<EventsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    // A hub link's only door is `peer_exchange` on `/mcp`; this route is not
    // it. Checked before anything else — a stream slot, a subscription — is
    // taken.
    if let Some(refusal) = super::auth::refuses_peer(&caller) {
        return refusal;
    }
    let resume_header = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
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
    let fields = wanted_fields(&query);
    let RequestedKinds { accepted, unknown } = wanted_kinds(&query);
    let accepted = fence_host_bound(&caller, accepted);
    if !unknown.is_empty() {
        tracing::warn!(
            caller = %label,
            unknown = ?unknown,
            known = ?crate::events::EVENT_KINDS,
            "[events] ignoring unrecognised ?kinds= values"
        );
    }
    // The boundary: the org half (work graph M5) and the person half
    // (multi-user M1). A scope that cannot be read ends the request rather
    // than streaming unfenced. Both generations are read BEFORE the scope,
    // so a change that lands between the two reads shows up as "moved" on
    // the next frame and costs a reconnect, never a stale fence.
    let scope_generation = crate::service::orgs::org_generation();
    let grant_generation = crate::store::grant_generation();
    let scope = match read_scope(&source.store, &caller) {
        Some(sc) => sc,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "the caller's scope could not be read",
            )
                .into_response()
        }
    };
    // A paired client's authorization was checked once, at connect; the
    // stream then outlives it, so it re-checks the row on every beat.
    let client = caller.client.as_ref().map(|c| {
        (
            Arc::clone(&source.store),
            c.id,
            crate::store::ClientBinding {
                org_id: c.org_id,
                person_id: c.person_id,
            },
        )
    });
    // A per-host token is checked the same way, against the token it opened
    // with, so a rotation (how an operator answers a stolen token) ends it.
    let host = match (&caller.client, &caller.host_alias) {
        (None, Some(alias)) => {
            super::auth::bearer_token(headers.get(axum::http::header::AUTHORIZATION))
                .map(|t| (alias.clone(), super::auth::sha256_hex(t)))
        }
        _ => None,
    };
    // Subscribe BEFORE the first frame goes out: a change emitted between the
    // client's request and its first read must still reach it.
    let rx = (source.subscribe)();
    // Resume, if the client asked to and the bus can. `Last-Event-ID` is the
    // standard header; `?since=` is the same value for the proxies that strip
    // it. A gap the history cannot cover is not an error — the client gets
    // today's behaviour, told plainly in the `ready` frame.
    let asked_from = resume_header
        .as_deref()
        .or(query.since.as_deref())
        .and_then(parse_frame_id);
    //
    // The generation half of the id is this hub's sequence generation XOR a
    // fingerprint of what THIS caller may see ([`scope_fingerprint`]), so a
    // resume minted under a different scope is refused — and only that
    // caller's is, rather than every phone on the hub. The bus still
    // replays against its own generation; the fingerprint is checked here.
    let base_generation = source.history.as_ref().map(|h| h.generation).unwrap_or(0);
    let generation = base_generation ^ scope_fingerprint(&scope);
    // THE REPLAY RULE (multi-user M1, T9d): the ring is replayed to the
    // hub's own reader and to nobody else. `Caller::view_scope` never builds
    // an internal scope, so in practice every `/events` caller re-lists on
    // reconnect — `resumed: false`, which is exactly what the `ready` frame
    // already says for a gap the history cannot cover. [`Delivery`] carries
    // the argument: a frame about the fleet as it WAS cannot be judged under
    // a caller's present scope without an identity for its subject, and the
    // two mechanisms that tried (carried facts; a birth-time comparison
    // against the host's tmux clock) were both unsound.
    let (replayed, resumed_from) = match (&source.history, asked_from) {
        (Some(h), Some((gen, seq))) if gen == generation && scope.is_internal() => {
            match (h.replay)(h.generation, seq) {
                Some(events) => (events, Some(seq)),
                None => (Vec::new(), None),
            }
        }
        _ => (Vec::new(), None),
    };
    tracing::debug!(
        caller = %label,
        kinds = ?accepted,
        fields = ?fields,
        replayed = replayed.len(),
        "[events] stream opened"
    );

    let stream = futures_util::stream::unfold(
        StreamState {
            rx,
            kinds: accepted,
            fields,
            generation,
            resumed: resumed_from.is_some(),
            sent_through: resumed_from.unwrap_or(0),
            wire_seq: resumed_from.unwrap_or(0),
            per_connection_ids: !scope.is_internal(),
            replay: replayed.into_iter().collect(),
            phase: Phase::Ready,
            _permit: permit,
            label,
            shutdown,
            client,
            host,
            heartbeat: {
                let start = tokio::time::Instant::now() + keepalive;
                let mut i = tokio::time::interval_at(start, keepalive);
                // A beat missed while a burst of events was being written is
                // caught up on the next one, not replayed in a tight loop.
                i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                i
            },
            scope,
            store: Arc::clone(&source.store),
            caller,
            scope_generation,
            grant_generation,
            served: BTreeSet::new(),
        },
        |mut st| async move {
            // A loop, so a phase that finishes can re-dispatch on the next
            // one within the same poll: the replay running dry has no frame
            // of its own to yield and must fall through to the live branch.
            loop {
                match st.phase {
                    Phase::Done => return None,
                    Phase::Ready => {
                        st.phase = if st.replay.is_empty() {
                            Phase::Live
                        } else {
                            Phase::Replay
                        };
                        let ready = serde_json::json!({
                            "version": crate::app_version::get(),
                            "now": unix_now(),
                            "kinds": accepted_list(st.kinds.as_ref()),
                            // The wire-contract revision (see `wire_contract`
                            // for what moves it): purely additive next to
                            // `version` and `now` above, so an older client
                            // that has never heard of it just ignores it.
                            "contract": crate::wire_contract::CONTRACT_REVISION,
                            // What this stream actually honoured, echoed the way
                            // `kinds` is: a misspelled field name is then visible
                            // here rather than as a column that is always absent.
                            "fields": st.fields,
                            // Whether the `Last-Event-ID` was honoured. `false`
                            // means re-list: the caller is not the hub's own
                            // reader (the replay rule — see `Delivery`), the gap
                            // was longer than the history kept, or this hub has
                            // restarted since the id was minted. Saying so is the
                            // point — a client that assumed continuity would show
                            // a stale fleet.
                            "resumed": st.resumed,
                        });
                        return Some((Ok::<Event, Infallible>(sse_event("ready", &ready)), st));
                    }
                    // One replayed frame per poll, in sequence order.
                    Phase::Replay => {
                        let mut next = None;
                        while let Some(msg) = st.replay.pop_front() {
                            if matches(st.kinds.as_ref(), &msg) {
                                next = Some(msg);
                                break;
                            }
                        }
                        match next {
                            Some(msg) => {
                                st.sent_through = msg.seq;
                                let store = Arc::clone(&st.store);
                                // A replay reaches only the hub's own
                                // reader, which keeps the bus's numbers.
                                let id_seq = next_id_seq(&st, &msg);
                                match row_event(
                                    &msg,
                                    st.fields.as_ref(),
                                    st.generation,
                                    id_seq,
                                    &st.scope,
                                    &store,
                                    Delivery::Replay,
                                ) {
                                    Some(ev) => {
                                        st.wire_seq = id_seq;
                                        return Some((Ok(ev), st));
                                    }
                                    None => continue,
                                }
                            }
                            // Nothing left to replay: go live in this same poll,
                            // rather than yielding a frame that does not exist.
                            None => {
                                st.phase = Phase::Live;
                                continue;
                            }
                        }
                    }
                    Phase::Live => loop {
                        // Cloned out of `st` so the two select branches do not
                        // borrow it at once.
                        let cancel = st.shutdown.clone();
                        let label = st.label.clone();
                        let client = st.client.clone();
                        let host = st.host.clone();
                        let rescope = (Arc::clone(&st.store), st.caller.clone());
                        let received = tokio::select! {
                            // The server is stopping: end the body now rather
                            // than hold its drain open.
                            _ = cancel.cancelled() => {
                                tracing::debug!(caller = %label, "[events] stream closed: server stopping");
                                return None;
                            }
                            // One beat: is the paired client still paired?
                            _ = st.heartbeat.tick() => {
                                if let Some((store, id, opened_as)) = &client {
                                    if !client_is_live(store, *id, *opened_as) {
                                        tracing::info!(
                                            caller = %label,
                                            "[events] stream closed: the client was revoked or re-bound"
                                        );
                                        return None;
                                    }
                                }
                                if let Some((alias, credential)) = &host {
                                    if !host_token_is_live(&rescope.0, alias, credential) {
                                        tracing::info!(
                                            caller = %label,
                                            "[events] stream closed: the host token was rotated or removed"
                                        );
                                        return None;
                                    }
                                }
                                // The GRANT mechanism, beside (never merged
                                // into) the device one above: rule 8. This
                                // beat is the guarantee — the two generation
                                // counters below only make the common case
                                // immediate.
                                let (store, c) = &rescope;
                                if read_scope(store, c).as_ref() != Some(&st.scope) {
                                    tracing::info!(
                                        caller = %label,
                                        "[events] stream closed: the caller's scope changed \
                                         (an org move, or a grant created, narrowed or revoked)"
                                    );
                                    return None;
                                }
                                continue;
                            }
                            r = st.rx.recv() => r,
                        };
                        match received {
                            // Already written to this connection by the replay:
                            // the receiver was subscribed before the replay was
                            // taken, so the overlap is expected, and sending it
                            // again would put a `session:killed` after the
                            // re-create that followed it.
                            Ok(msg) if msg.seq <= st.sent_through => continue,
                            Ok(msg) if matches(st.kinds.as_ref(), &msg) => {
                                st.sent_through = msg.seq;
                                // An org move or a grant change since the
                                // scope was read: re-read it before this
                                // frame goes out, and end the stream if it
                                // moved. Two counters, because revoking a
                                // device and revoking a share are different
                                // events (rule 8) and neither may ride the
                                // other's mechanism.
                                let orgs_now = crate::service::orgs::org_generation();
                                let grants_now = crate::store::grant_generation();
                                if orgs_now != st.scope_generation
                                    || grants_now != st.grant_generation
                                {
                                    let (store, c) = (&st.store, &st.caller);
                                    if read_scope(store, c).as_ref() != Some(&st.scope) {
                                        tracing::info!(
                                            caller = %st.label,
                                            "[events] stream closed: the caller's scope changed \
                                             (an org move, or a grant created, narrowed or revoked)"
                                        );
                                        return None;
                                    }
                                    st.scope_generation = orgs_now;
                                    st.grant_generation = grants_now;
                                }
                                let store = Arc::clone(&st.store);
                                let id_seq = next_id_seq(&st, &msg);
                                match row_event(
                                    &msg,
                                    st.fields.as_ref(),
                                    st.generation,
                                    id_seq,
                                    &st.scope,
                                    &store,
                                    Delivery::Live,
                                ) {
                                    Some(ev) => {
                                        // Only a frame that actually goes
                                        // out takes a number, so the gaps a
                                        // fence leaves are invisible on the
                                        // wire — which is the whole point.
                                        st.wire_seq = id_seq;
                                        // Remember what this connection was
                                        // served, so a later departure can
                                        // be announced (multi-user M1, T10;
                                        // see `StreamState::served`). A
                                        // `session:killed` that went out IS
                                        // the departure, so its id leaves
                                        // the set rather than entering it.
                                        record_served(&mut st.served, &msg);
                                        return Some((Ok(ev), st));
                                    }
                                    // Fenced out.
                                    //
                                    // **A session that LEAVES this stream's
                                    // fence is announced** (multi-user M1,
                                    // T10), and that is this arm's whole
                                    // substance. The stream ends when the
                                    // CALLER's scope moves (the beat and the
                                    // two generation counters above), which
                                    // covers a grant revoke — but a host's
                                    // org move, an ownership change and a
                                    // `visibility` change move the SESSION,
                                    // not the caller's `ViewScope`, so the
                                    // stream stays up and the fence starts
                                    // dropping that row's frames. Before
                                    // this, nothing on the wire said so and
                                    // a paired phone kept displaying the row
                                    // for ever (the desktop bridge re-lists
                                    // only on `resumed: false`).
                                    //
                                    // So: a frame about a session this
                                    // connection HAS been served, now
                                    // refused, becomes one id-only
                                    // `session:killed` —
                                    // [`departure_event`], which is where the
                                    // reasoning lives. Everything else is
                                    // still silence: a row the fence refused
                                    // from the start is not in `served`, so
                                    // the announcement can never be a
                                    // caller's first word about a session.
                                    None => match departure_for(&mut st.served, &msg) {
                                        Some(sid) => {
                                            st.wire_seq = id_seq;
                                            return Some((
                                                Ok(departure_event(
                                                    sid,
                                                    st.fields.as_ref(),
                                                    st.generation,
                                                    id_seq,
                                                )),
                                                st,
                                            ));
                                        }
                                        None => continue,
                                    },
                                }
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
                                // `n` is the BUS's lag — a count of frames
                                // across the whole fleet, most of which
                                // this caller was never entitled to. It is
                                // the same volume oracle `frame_id` closed,
                                // and sampleable on purpose (open a stream,
                                // do not read it, read the count), so only
                                // the one caller that may see everything
                                // gets the number. For everybody else the
                                // FRAME is the signal — there is a hole,
                                // re-list — and the count is not part of it.
                                let body = if st.scope.is_internal() {
                                    serde_json::json!({ "skipped": n })
                                } else {
                                    serde_json::json!({})
                                };
                                let ev = sse_event("lagged", &body);
                                return Some((Ok(ev), st));
                            }
                            // The bus went away (the process is shutting down).
                            Err(RecvError::Closed) => return None,
                        }
                    },
                }
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
            fields: None,
            since: None,
        }
    }

    fn ev(name: &'static str) -> EventMessage {
        EventMessage {
            name,
            payload: serde_json::Value::Null,
            seq: 1,
        }
    }

    /// A frame id names the sequence AND the process that minted it, so a
    /// restarted hub cannot be resumed against as though it were the same one.
    #[test]
    fn a_frame_id_round_trips_and_a_malformed_one_is_none() {
        assert_eq!(parse_frame_id(&frame_id(7, 42)), Some((7, 42)));
        // A browser's EventSource sends an empty id before it has seen one.
        for bad in ["", "   ", "42", "abc-1", "7-", "-7", "7-x"] {
            assert_eq!(parse_frame_id(bad), None, "{bad:?} is not a frame id");
        }
    }

    #[test]
    fn project_keeps_asked_for_keys_and_leaves_a_scalar_payload_alone() {
        let row = serde_json::json!({"id": 7, "claude_status": "working", "tmux_pane_id": "%3"});
        let fields = vec!["id".to_string(), "claude_status".to_string()];
        assert_eq!(
            project(&row, &fields),
            serde_json::json!({"id": 7, "claude_status": "working"})
        );
        // A key that does not exist on this event is simply absent, not an error.
        assert_eq!(
            project(&row, &["nothing".to_string()]),
            serde_json::json!({})
        );
        // Not every payload is a row: `sync:progress` and friends are scalars
        // and have nothing to project.
        let scalar = serde_json::json!(3);
        assert_eq!(project(&scalar, &fields), scalar);
    }

    #[test]
    fn a_query_list_is_bounded_and_distinct() {
        let many = (0..10_000)
            .map(|i| format!("f{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let q = EventsQuery {
            kinds: Some(format!("{many},session,session,{}", "x".repeat(10_000))),
            fields: Some(format!("id,id,{},{many}", "y".repeat(500))),
            since: None,
        };
        let fields = wanted_fields(&q).unwrap();
        assert_eq!(fields.len(), QUERY_LIST_MAX);
        assert_eq!(fields[0], "id");
        assert_eq!(
            fields[1], "f0",
            "a duplicate and an over-long entry are dropped"
        );
        let RequestedKinds { accepted, unknown } = wanted_kinds(&q);
        assert!(unknown.len() <= QUERY_LIST_MAX);
        assert!(unknown
            .iter()
            .all(|k| k.len() <= QUERY_ENTRY_MAX + '…'.len_utf8()));
        assert!(accepted.unwrap().len() <= 1);
        // Few and short: unchanged.
        let q = EventsQuery {
            kinds: Some("session,host,session".into()),
            fields: None,
            since: None,
        };
        assert_eq!(
            wanted_kinds(&q).accepted,
            Some(vec!["session".to_string(), "host".to_string()])
        );
    }

    #[test]
    fn wanted_fields_drops_empties_and_none_means_the_whole_row() {
        let with = |f: &str| EventsQuery {
            kinds: None,
            fields: Some(f.to_string()),
            since: None,
        };
        assert_eq!(
            wanted_fields(&with("id, claude_status ,")),
            Some(vec!["id".to_string(), "claude_status".to_string()])
        );
        for empty in ["", ",", "  "] {
            assert_eq!(
                wanted_fields(&with(empty)),
                None,
                "an empty filter must mean the whole row, not an empty one"
            );
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
    fn a_host_bound_stream_never_carries_work_frames() {
        let host = Caller {
            api: None,
            host_alias: Some("mefistos".into()),
            client: None,
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        };
        let fenced = fence_host_bound(&host, None).unwrap();
        assert!(!fenced.iter().any(|k| k == "work"));
        assert!(fenced.iter().any(|k| k == "session"));
        assert!(!matches(Some(&fenced), &ev("work:item")));
        let asked = fence_host_bound(&host, Some(vec!["work".into()])).unwrap();
        assert!(!matches(Some(&asked), &ev("work:tracker")));
        // Master and paired clients see everything they asked for.
        assert_eq!(fence_host_bound(&Caller::master(), None), None);
        // Multi-user M1: a device bound to a PERSON and to no org is not
        // host-bound, and still gets `work` — the person half of the fence
        // has nothing to say about a tracker ticket, and narrowing this
        // predicate would blank the work graph on a person's own desktop.
        let persons_device = Caller {
            api: None,
            host_alias: None,
            client: Some(super::super::auth::ClientRef {
                id: 1,
                name: "phone".into(),
                trusted: false,
                org_id: None,
                person_id: Some(7),
            }),
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        };
        assert_eq!(fence_host_bound(&persons_device, None), None);
        assert!(matches(
            fence_host_bound(&persons_device, Some(vec!["work".into()])).as_ref(),
            &ev("work:item")
        ));
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
        bus.session_killed(7.into());
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.kind(), "session");
        assert_eq!(msg.payload["id"], 7);
        bus.emit(&RowChange::HostRemoved("box".into()));
        assert_eq!(rx.recv().await.unwrap().kind(), "host");
    }

    // ---- the person fence (multi-user M1, T9) ----------------------------

    /// One private session and the four people a share has: its OWNER, a
    /// `watch` grantee, a `drive` grantee and a stranger.
    ///
    /// The hub's own personal owner (migration 100) is a FIFTH person, so
    /// `Store::sole_enabled_person` answers `None` and nobody gets the
    /// single-person carve-out. That is deliberate: with it in play every
    /// assertion below would be about the carve-out rather than about the
    /// fence.
    struct Fence {
        store: Arc<Mutex<Store>>,
        session: i64,
        ada: i64,
        bob: i64,
        dar: i64,
        cleo: i64,
    }

    impl Fence {
        fn new() -> Self {
            let s = Store::open_in_memory().unwrap();
            s.upsert_host("h").unwrap();
            let ada = s.create_person("ada", None).unwrap().id;
            let bob = s.create_person("bob", None).unwrap().id;
            let dar = s.create_person("dar", None).unwrap().id;
            let cleo = s.create_person("cleo", None).unwrap().id;
            let session = s
                .upsert_session("s-1", "h", None, None, 1, 1, "running", None)
                .unwrap();
            assert!(s.claim_if_unclaimed(session, Some(ada)).unwrap());
            s.grant_session(
                session,
                crate::store::GrantRecipient::Person(bob),
                crate::store::GRANT_WATCH,
                ada,
            )
            .unwrap();
            s.grant_session(
                session,
                crate::store::GrantRecipient::Person(dar),
                crate::store::GRANT_DRIVE,
                ada,
            )
            .unwrap();
            assert!(
                s.sole_enabled_person().unwrap().is_none(),
                "the fixture must not hand anybody the single-person carve-out"
            );
            Fence {
                store: Arc::new(Mutex::new(s)),
                session,
                ada,
                bob,
                dar,
                cleo,
            }
        }

        /// The scope of one person's own device, through the one constructor
        /// (`Caller::view_scope`).
        fn scope(&self, person: i64) -> ViewScope {
            let caller = Caller {
                api: None,
                host_alias: None,
                client: Some(super::super::auth::ClientRef {
                    id: 1,
                    name: "phone".into(),
                    trusted: false,
                    org_id: None,
                    person_id: Some(person),
                }),
                mode: crate::mcp::auth::TokenMode::Full,
                pane: None,
                is_personal_owner: false,
            };
            let s = self.store.lock().unwrap();
            caller.view_scope(&s).unwrap()
        }

        fn row(&self) -> crate::store::SessionRow {
            let s = self.store.lock().unwrap();
            s.get_session_by_id(self.session).unwrap().unwrap()
        }

        fn frame(&self, name: &'static str, payload: serde_json::Value) -> EventMessage {
            EventMessage {
                name,
                payload,
                seq: 1,
            }
        }

        /// One representative frame of each per-frame-fenced kind, carrying
        /// `self.session`.
        ///
        /// The task is INSERTED, not invented: `fence_task_frame` re-reads
        /// the row by `tasks.id` rather than trusting the payload, because
        /// the fence's own input (`detached_at`) is `#[serde(skip)]` and so
        /// is never on a frame.
        fn per_frame_samples(&self) -> Vec<(&'static str, EventMessage)> {
            let row = self.row();
            let task = {
                let s = self.store.lock().unwrap();
                let t = s
                    .insert_task(
                        Some(self.session),
                        Some(self.session),
                        "SECRET prompt",
                        "abcd1234",
                    )
                    .unwrap();
                s.finish_task(t.id, "done", Some("SECRET result"), None)
                    .unwrap()
                    .0
                    .expect("the task is there")
            };
            vec![
                (
                    "session",
                    self.frame("session:updated", serde_json::to_value(&row).unwrap()),
                ),
                // The `session` kind has TWO payload shapes and the table is
                // only worth what it covers, so both are here (T9e): before
                // this, every enumerating test exercised the row-bearing
                // branch alone, and `session:conversations` was exercised by
                // nothing at all — while the id-only branch is the one with
                // a stated residual ([`Delivery`]).
                (
                    "session",
                    self.frame(
                        "session:event",
                        serde_json::json!({
                            "id": 7, "session_id": self.session,
                            "kind": "prompt_sent", "detail": "ada's private prompt",
                            "at": 1
                        }),
                    ),
                ),
                (
                    "session",
                    self.frame(
                        "session:conversations",
                        serde_json::json!({ "session_id": self.session }),
                    ),
                ),
                (
                    "task",
                    self.frame("task:updated", serde_json::to_value(&task).unwrap()),
                ),
                (
                    "move",
                    self.frame(
                        "move:progress",
                        serde_json::json!({
                            "session_id": self.session, "to_host": "other",
                            "step": "check", "index": 1, "total": 9, "state": "started"
                        }),
                    ),
                ),
                (
                    "grant",
                    self.frame(
                        "grant:changed",
                        serde_json::json!({
                            "session_id": self.session, "person_id": self.bob,
                            "level": "watch"
                        }),
                    ),
                ),
            ]
        }
    }

    /// **The enumerating test this area needs.** Every
    /// [`crate::events::EVENT_KINDS`] entry is either fenced per frame by
    /// [`fence_frame`] or explicitly declared to carry no session-scoped
    /// content — and for the per-frame half, a representative frame really
    /// is dropped for a stranger and really does reach the owner.
    ///
    /// Without it the next kind somebody adds is a silent leak: that is
    /// exactly how `task:updated` (a whole `TaskRow`, `prompt` / `result` /
    /// `error` included), `move:progress` and `worktree:updated` came to be
    /// broadcast unexamined to every open stream.
    #[test]
    fn every_event_kind_is_fenced_or_declared_content_free() {
        // The table and the vocabulary agree, in both directions.
        for kind in crate::events::EVENT_KINDS {
            assert!(
                KIND_FENCES.iter().any(|(k, _)| *k == kind),
                "KIND_FENCES has no row for the {kind:?} event kind: decide \
                 whether it is fenced per frame or carries no session-scoped \
                 content, and say which"
            );
        }
        for (kind, fence) in KIND_FENCES {
            assert!(
                crate::events::EVENT_KINDS.contains(kind),
                "KIND_FENCES names {kind:?}, which is not an event kind"
            );
            if let KindFence::NoSessionContent(why) = fence {
                assert!(
                    why.len() > 20,
                    "{kind:?}'s exemption must say what the frame carries \
                     INSTEAD of a session, in words a reader can check"
                );
            }
        }

        let fx = Fence::new();
        let owner = fx.scope(fx.ada);
        let stranger = fx.scope(fx.cleo);
        let declared: Vec<&str> = KIND_FENCES
            .iter()
            .filter(|(_, f)| *f == KindFence::PerFrame)
            .map(|(k, _)| *k)
            .collect();
        let samples = fx.per_frame_samples();
        for kind in &declared {
            let of_kind: Vec<&EventMessage> = samples
                .iter()
                .filter(|(k, _)| k == kind)
                .map(|(_, m)| m)
                .collect();
            assert!(
                !of_kind.is_empty(),
                "{kind:?} is fenced per frame but has no sample here"
            );
            // Per SHAPE, not per kind: `session` carries a row in one shape
            // and an id in another, and they go through different arms.
            for msg in of_kind {
                assert!(
                    fence_frame(&owner, msg, &fx.store).is_some(),
                    "{}: the owner's own frame must arrive",
                    msg.name
                );
                assert!(
                    fence_frame(&stranger, msg, &fx.store).is_none(),
                    "{}: a stranger must not receive it",
                    msg.name
                );
            }
        }
        // And nothing claims to be per-frame without an arm: `fence_frame`
        // logs and drops in that case, which the loop above would catch as
        // "the owner's own frame must arrive".
        for (kind, msg) in &samples {
            assert!(
                declared.contains(kind),
                "{}: a sample for {kind:?}, which is not fenced per frame",
                msg.name
            );
        }
        // Both `session` shapes, by name, so dropping one is a failure here
        // rather than a quiet narrowing of what this test covers.
        for name in ["session:updated", "session:event", "session:conversations"] {
            assert!(
                samples.iter().any(|(_, m)| m.name == name),
                "{name} is one of the shapes the session arm fences and has                  no sample"
            );
        }
    }

    /// The hub's own readers keep their old reach; every caller is fenced.
    ///
    /// The old early return was `scope.is_all()`, and `OrgScope::All`'s own
    /// doc comment lists "an unbound paired client" among the callers it
    /// covers — which is the shape a person's device has, i.e. the whole
    /// T6/T7/T8 gate was bypassable with one `GET /events`.
    #[test]
    fn only_the_hubs_own_reader_passes_every_frame() {
        let fx = Fence::new();
        let msg = fx.frame("session:updated", serde_json::to_value(fx.row()).unwrap());
        assert!(fence_frame(&ViewScope::internal(), &msg, &fx.store).is_some());
        // A device no pairing bound carries no person: the plan's literal
        // inversion (`person.is_none() && host.is_none() && org.is_all()`)
        // would have passed every frame in the fleet to exactly this caller.
        let unbound = Caller {
            api: None,
            host_alias: None,
            client: Some(super::super::auth::ClientRef {
                id: 2,
                name: "no-person".into(),
                trusted: false,
                org_id: None,
                person_id: None,
            }),
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        };
        let scope = {
            let s = fx.store.lock().unwrap();
            unbound.view_scope(&s).unwrap()
        };
        assert!(
            fence_frame(&scope, &msg, &fx.store).is_none(),
            "a token that names no person must see no session frame"
        );
    }

    /// A grant is what a second person's stream rests on, at each level, and
    /// the two levels differ on the pane and not on the frame: a `watch`
    /// grantee sees the row (that is the whole point of watching) and a
    /// `drive` grantee sees the same row.
    #[test]
    fn a_grantee_receives_the_shared_rows_frames_and_a_revoked_one_does_not() {
        let fx = Fence::new();
        let msg = fx.frame("session:updated", serde_json::to_value(fx.row()).unwrap());
        for (who, person) in [("watcher", fx.bob), ("driver", fx.dar)] {
            assert!(
                fence_frame(&fx.scope(person), &msg, &fx.store).is_some(),
                "the {who}'s stream must carry the shared row"
            );
        }
        {
            let s = fx.store.lock().unwrap();
            s.revoke_session_grant(fx.session, fx.bob, fx.ada).unwrap();
        }
        // The scope is re-read, which is what the stream does on its beat
        // and before any frame whose grant generation moved.
        assert!(
            fence_frame(&fx.scope(fx.bob), &msg, &fx.store).is_none(),
            "a revoked share must stop being delivered"
        );
        assert!(
            fence_frame(&fx.scope(fx.dar), &msg, &fx.store).is_some(),
            "and must not take anybody else's grant with it"
        );
    }

    /// `session:killed` is the one session frame whose row is already gone,
    /// so it carries the facts instead — and one that carries none is
    /// dropped rather than broadcast, because an id alone tells a stranger
    /// that a session with that id existed and has ended.
    #[test]
    fn a_session_killed_frame_is_judged_by_its_carried_facts() {
        let fx = Fence::new();
        let row = fx.row();
        let with_facts = fx.frame(
            "session:killed",
            RowChange::SessionKilled(crate::events::SessionKilledPayload::of_row(&row)).payload(),
        );
        assert!(fence_frame(&fx.scope(fx.ada), &with_facts, &fx.store).is_some());
        assert!(fence_frame(&fx.scope(fx.bob), &with_facts, &fx.store).is_some());
        assert!(
            fence_frame(&fx.scope(fx.cleo), &with_facts, &fx.store).is_none(),
            "a stranger must not learn that this session ended"
        );
        // The id-only form: fail closed for everybody but the hub itself.
        let bare = fx.frame(
            "session:killed",
            RowChange::SessionKilled(row.id.into()).payload(),
        );
        assert_eq!(bare.payload, serde_json::json!({ "id": row.id }));
        for person in [fx.ada, fx.bob, fx.cleo] {
            assert!(
                fence_frame(&fx.scope(person), &bare, &fx.store).is_none(),
                "an unjudgeable kill frame must be dropped"
            );
        }
        assert!(fence_frame(&ViewScope::internal(), &bare, &fx.store).is_some());
    }

    /// The store really does put the facts on the frame — the half the
    /// fence test above assumes. Without this the two tests would agree with
    /// each other and both be wrong.
    #[test]
    fn deleting_a_session_emits_a_kill_frame_carrying_its_facts() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("h").unwrap();
        let ada = s.create_person("ada", None).unwrap().id;
        let id = s
            .upsert_session("s-1", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert!(s.claim_if_unclaimed(id, Some(ada)).unwrap());
        let killed = s.killed_payload(id);
        assert_eq!(killed.id, id);
        assert_eq!(killed.host_alias.as_deref(), Some("h"));
        assert_eq!(killed.visibility.as_deref(), Some("private"));
        assert_eq!(killed.owner_person_id, Some(ada));
        // …and after the row is gone there is nothing left to read, which is
        // why every emitter takes the facts first.
        s.delete_session(id).unwrap();
        assert_eq!(s.killed_payload(id), id.into());
    }

    /// A `grant:changed` frame is for the two people a share is between.
    #[test]
    fn a_grant_frame_reaches_the_recipient_and_the_owner_only() {
        let fx = Fence::new();
        let msg = fx.frame(
            "grant:changed",
            serde_json::json!({ "session_id": fx.session, "person_id": fx.bob, "level": "watch" }),
        );
        assert!(
            fence_frame(&fx.scope(fx.bob), &msg, &fx.store).is_some(),
            "the person the grant names patches their own grant set from it"
        );
        assert!(
            fence_frame(&fx.scope(fx.ada), &msg, &fx.store).is_some(),
            "and the owner, who is the other party to the share"
        );
        for (who, person) in [("another grantee", fx.dar), ("a stranger", fx.cleo)] {
            assert!(
                fence_frame(&fx.scope(person), &msg, &fx.store).is_none(),
                "{who} has no business in somebody else's share"
            );
        }
    }

    /// A frame that names its session only by id (`session:event`,
    /// `session:conversations`) is resolved in the store; one whose row has
    /// gone is dropped rather than passed.
    #[test]
    fn a_frame_that_only_names_its_session_is_resolved_and_fails_closed() {
        let fx = Fence::new();
        for name in ["session:event", "session:conversations"] {
            let msg = fx.frame(
                name,
                serde_json::json!({ "session_id": fx.session, "kind": "prompt_sent" }),
            );
            assert!(
                fence_frame(&fx.scope(fx.ada), &msg, &fx.store).is_some(),
                "{name}"
            );
            assert!(
                fence_frame(&fx.scope(fx.cleo), &msg, &fx.store).is_none(),
                "{name}"
            );
            let unknown = fx.frame(name, serde_json::json!({ "session_id": 9_999 }));
            assert!(
                fence_frame(&fx.scope(fx.ada), &unknown, &fx.store).is_none(),
                "{name}: a row nobody can resolve is not passed"
            );
        }
    }

    /// **The order inside [`row_event`] is load-bearing**, and this pins it:
    /// the fence runs BEFORE `?fields=` projection. That is the whole
    /// licence [`fence_session_frame`] has to judge a row frame from its own
    /// payload — a projection that dropped `visibility` would otherwise make
    /// a private row read as unclaimed, which is exactly the trap T8's
    /// result gate avoids by resolving in the store. Move the projection
    /// earlier and this test fails rather than the fence quietly weakening.
    #[test]
    fn a_projection_cannot_hide_the_key_the_fence_judges_by() {
        let fx = Fence::new();
        let msg = fx.frame("session:updated", serde_json::to_value(fx.row()).unwrap());
        let only_id = vec!["id".to_string()];
        assert!(
            row_event(
                &msg,
                Some(&only_id),
                1,
                1,
                &fx.scope(fx.cleo),
                &fx.store,
                Delivery::Live
            )
            .is_none(),
            "a stranger asking for `?fields=id` must still be refused the row"
        );
        let ev = row_event(
            &msg,
            Some(&only_id),
            1,
            1,
            &fx.scope(fx.ada),
            &fx.store,
            Delivery::Live,
        )
        .expect("the owner's own row");
        // …and the projection did run, after the fence: the frame kept the
        // one key that was asked for and dropped the rest of the row.
        let rendered = format!("{ev:?}");
        assert!(
            rendered.contains("session:updated") && !rendered.contains("tmux_name"),
            "the projection must still apply, and after the fence: {rendered}"
        );
    }

    /// The resume fingerprint: equal scopes agree, and any difference in
    /// what a caller may see mints a different generation — so a reconnect
    /// after a grant change re-lists instead of resuming the gap under the
    /// old scope's eyes, and only THAT caller's resume is invalidated.
    #[test]
    fn the_frame_ids_generation_follows_the_callers_scope() {
        let fx = Fence::new();
        let before = scope_fingerprint(&fx.scope(fx.bob));
        assert_eq!(
            before,
            scope_fingerprint(&fx.scope(fx.bob)),
            "an unchanged scope must keep its resume"
        );
        assert_ne!(
            before,
            scope_fingerprint(&fx.scope(fx.dar)),
            "two people are two scopes"
        );
        {
            let s = fx.store.lock().unwrap();
            s.revoke_session_grant(fx.session, fx.bob, fx.ada).unwrap();
        }
        assert_ne!(
            before,
            scope_fingerprint(&fx.scope(fx.bob)),
            "a revoked grant must invalidate the recipient's resume"
        );
        assert_eq!(
            scope_fingerprint(&fx.scope(fx.dar)),
            scope_fingerprint(&fx.scope(fx.dar)),
            "…and nobody else's"
        );
    }

    /// **THE REPLAY RULE** (multi-user M1, T9d): the ring is replayed to the
    /// hub's own reader and to nobody else.
    ///
    /// Two mechanisms came before this one and both were unsound, which is
    /// why the test is a blanket one rather than a case analysis:
    ///
    /// 1. T9b judged a replayed row frame by the facts it CARRIED — so a
    ///    session that had since become another person's, or whose host had
    ///    since moved org, was described by a frame saying it had not.
    /// 2. T9c judged it against the row holding its id NOW, refusing a row
    ///    whose `created_at` was after the frame's mint time. That compares
    ///    the remote host's tmux clock with the hub's: skew one way lets a
    ///    reused id through, skew the other drops frames the caller is
    ///    entitled to while still answering `resumed: true`. The test that
    ///    pinned it had to hand-write `UPDATE sessions SET created_at` to
    ///    manufacture the only state in which the guard fired.
    ///
    /// So: every per-frame kind, both the row-bearing and the id-only
    /// shape, for the owner as well as a stranger — on the replay path
    /// nobody but the hub's own reader gets anything.
    #[test]
    fn the_replay_ring_is_fenced_off_from_every_caller() {
        let fx = Fence::new();
        let samples = fx.per_frame_samples();
        assert!(!samples.is_empty());
        for (kind, msg) in &samples {
            for (who, scope) in [
                ("the owner", fx.scope(fx.ada)),
                ("a grantee", fx.scope(fx.bob)),
                ("a stranger", fx.scope(fx.cleo)),
            ] {
                assert!(
                    fence_frame_as(&scope, msg, &fx.store, Delivery::Replay).is_none(),
                    "{kind}: {who} must re-list instead of being replayed to"
                );
            }
            // Live, the fence is the scope — the rule narrows the replay
            // path and nothing else.
            assert!(
                fence_frame(&fx.scope(fx.ada), msg, &fx.store).is_some(),
                "{kind}: the owner's LIVE frame still arrives"
            );
            // And the hub's own reader still replays: GC, reconcile and the
            // hub-to-hub paths read the ring.
            assert!(
                fence_frame_as(&ViewScope::internal(), msg, &fx.store, Delivery::Replay).is_some(),
                "{kind}: the hub's own reader is the one scope that replays"
            );
        }
    }

    /// The id-only shapes — `session:event`, the conversation frames,
    /// `move:progress`, `grant:changed` — resolve their session by id, and
    /// `sessions.id` is REUSED (`INTEGER PRIMARY KEY`, no `AUTOINCREMENT`;
    /// reconcile hard-deletes a ghosted row). The replay rule closes that
    /// by construction for a frame about the PAST, which is what this test
    /// asserts.
    ///
    /// **It does not close it on the live path, and the assertion in the
    /// middle of this test is the proof** (read it before trusting any
    /// sentence that says "live, so sound"): `fence_frame` hands Ada's
    /// `session:event` — `detail: "ada's private prompt"` — to Bob's device
    /// view, because the fence resolves the id when the frame is DELIVERED
    /// and Bob's new row holds that id by then. The window is bounded by
    /// broadcast residency rather than by the write; see [`Delivery`] for
    /// the measurement and for the owner decision that closes it (carry the
    /// facts, as `session:killed` does).
    ///
    /// This is also the scenario T9c's clock comparison was meant to catch
    /// and did not: Ada's session is reaped, Bob's new session is handed the
    /// rowid, and a replayed frame about Ada's is judged against Bob's.
    #[test]
    fn a_replayed_frame_about_a_reused_session_id_is_dropped() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        assert!(s.sole_enabled_person().unwrap().is_none());
        let ada_session = s
            .upsert_session("s-ada", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert!(s.claim_if_unclaimed(ada_session, Some(ada)).unwrap());
        let ada_row = s.get_session_by_id(ada_session).unwrap().unwrap();
        let store = Arc::new(Mutex::new(s));

        // Two frames about Ada's session, minted while it existed: the
        // row-bearing one and the id-only one.
        let updated = EventMessage {
            name: "session:updated",
            payload: serde_json::to_value(&ada_row).unwrap(),
            seq: 1,
        };
        let timeline = EventMessage {
            name: "session:event",
            payload: serde_json::json!({
                "id": 7,
                "session_id": ada_session,
                "kind": "prompt_sent",
                "detail": "ada's private prompt",
            }),
            seq: 2,
        };

        // The row is reaped and SQLite hands its id to Bob's new session.
        // Nothing is back-dated and no clock is touched: that is the point
        // — the previous guard needed a `created_at` the real writers never
        // produce, and this one needs no identity at all.
        let bob_row = {
            let s = store.lock().unwrap();
            s.delete_session(ada_session).unwrap();
            let id = s
                .upsert_session("s-bob", "h", None, None, 1, 1, "running", None)
                .unwrap();
            assert_eq!(id, ada_session, "SQLite reuses the rowid: the hazard");
            assert!(s.claim_if_unclaimed(id, Some(bob)).unwrap());
            s.get_session_by_id(id).unwrap().unwrap()
        };
        let bobs = {
            let s = store.lock().unwrap();
            crate::mcp::auth::device_view(&s, bob)
        };
        assert!(
            bobs.sees_session_row(&bob_row).is_visible(),
            "Bob owns the NEW row, which is what made the old frames pass"
        );
        // The live path judges the row-bearing frame by its payload — Ada's
        // facts — and resolves the id-only one, so BOTH would reach Bob if
        // a frame about the past could get onto the replay path.
        assert!(
            fence_frame(&bobs, &timeline, &store).is_some(),
            "the live fence resolves the id, which is why the ring must not \
             carry a stale one to him — and is the residual the doc above \
             states, bounded by broadcast residency and not by the write"
        );

        for msg in [&updated, &timeline] {
            assert!(
                fence_frame_as(&bobs, msg, &store, Delivery::Replay).is_none(),
                "{} about Ada's deleted session must not be replayed to Bob",
                msg.name
            );
        }
    }

    /// **A task whose session was reaped is not inherited with its rowid**
    /// (multi-user M1, T9d) — on the LIVE path, with no replay involved.
    ///
    /// `tasks.requester_session_id` / `worker_session_id` have no foreign
    /// key and no delete path ever touched them, so a task outlived its
    /// sessions holding ids SQLite then handed to somebody else's new
    /// session. `task_visible_in_scope` resolved both ends by `get_session_
    /// by_id`, so `sweep_open_tasks`' `task:updated` for the stale task was
    /// judged against the stranger's own row and delivered — `prompt` and
    /// `result` included. `list_tasks` leaked the same row through the same
    /// predicate.
    ///
    /// Migration 101's trigger NULLs the ids and stamps `detached_at`, and
    /// the predicate refuses a detached task to everyone but the hub's own
    /// reader.
    #[test]
    fn a_task_whose_session_was_reaped_is_not_visible_to_the_next_holder() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        assert!(s.sole_enabled_person().unwrap().is_none());
        let ada_session = s
            .upsert_session("s-ada", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert!(s.claim_if_unclaimed(ada_session, Some(ada)).unwrap());
        let task = s
            .insert_task(None, Some(ada_session), "SECRET prompt", "abcd1234")
            .unwrap();
        let frame = EventMessage {
            name: "task:updated",
            payload: serde_json::to_value(&task).unwrap(),
            seq: 1,
        };
        let store = Arc::new(Mutex::new(s));
        // Ada reads her own task, live.
        {
            let s = store.lock().unwrap();
            let adas = crate::mcp::auth::device_view(&s, ada);
            drop(s);
            assert!(fence_frame(&adas, &frame, &store).is_some());
        }

        // Reaped, and SQLite hands the rowid to Bob's new session.
        let bobs = {
            let s = store.lock().unwrap();
            s.delete_session(ada_session).unwrap();
            let id = s
                .upsert_session("s-bob", "h", None, None, 1, 1, "running", None)
                .unwrap();
            assert_eq!(id, ada_session, "SQLite reuses the rowid: the hazard");
            assert!(s.claim_if_unclaimed(id, Some(bob)).unwrap());
            let stored = s
                .get_task(task.id)
                .unwrap()
                .expect("tasks are never deleted");
            assert_eq!(
                (stored.worker_session_id, stored.detached_at.is_some()),
                (None, true),
                "the trigger de-identified the task's end and stamped it"
            );
            crate::mcp::auth::device_view(&s, bob)
        };
        assert!(
            fence_frame(&bobs, &frame, &store).is_none(),
            "Ada's task must not arrive on Bob's stream because he inherited \
             her rowid"
        );
        // And it is nobody's but the hub's own now — not even Ada's, whose
        // session is over: `detached_at` fails closed rather than widening
        // to the surviving end.
        let adas = {
            let s = store.lock().unwrap();
            crate::mcp::auth::device_view(&s, ada)
        };
        assert!(fence_frame(&adas, &frame, &store).is_none());
        assert!(fence_frame(&ViewScope::internal(), &frame, &store).is_some());
    }

    /// **A task that lost ONE end is not widened to the other end's
    /// grantee** (multi-user M1, T9d).
    ///
    /// This is the half the NULLing alone does not close, and the reason
    /// `detached_at` is a stamp rather than just an absent id.
    /// `task_visible_in_scope_pure` requires BOTH named ends to be visible
    /// and reads an absent end as "no claim to check"
    /// (`(None, _) => true`). So de-identifying a reaped requester would
    /// hand the task — `prompt` and `result` included, written in the
    /// requester's own private session — to anybody who can see the WORKER:
    /// here a `drive` grantee of the worker, who could not see the task a
    /// moment earlier. Fail closed instead: both sessions are over or gone,
    /// and there is nothing left for a person to drive.
    #[test]
    fn a_task_that_lost_one_end_is_not_widened_to_the_other_ends_grantee() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        assert!(s.sole_enabled_person().unwrap().is_none());
        let requester = s
            .upsert_session("s-req", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let worker = s
            .upsert_session("s-work", "h", None, None, 1, 1, "running", None)
            .unwrap();
        for id in [requester, worker] {
            assert!(s.claim_if_unclaimed(id, Some(ada)).unwrap());
        }
        // Bob drives the WORKER and holds nothing on the requester.
        s.grant_session(
            worker,
            crate::store::GrantRecipient::Person(bob),
            crate::store::GRANT_DRIVE,
            ada,
        )
        .unwrap();
        let task = s
            .insert_task(Some(requester), Some(worker), "SECRET prompt", "abcd1234")
            .unwrap();
        let store = Arc::new(Mutex::new(s));
        let frame = EventMessage {
            name: "task:updated",
            payload: serde_json::to_value(&task).unwrap(),
            seq: 1,
        };
        let bobs = {
            let s = store.lock().unwrap();
            crate::mcp::auth::device_view(&s, bob)
        };
        assert!(
            fence_frame(&bobs, &frame, &store).is_none(),
            "a grant on one end is not a grant on the task: the requester is \
             Ada's private session"
        );

        // The requester is reaped. Its id is NULLed, so nothing inherits it
        // — and the task is not handed to Bob either.
        {
            let s = store.lock().unwrap();
            s.delete_session(requester).unwrap();
            let stored = s.get_task(task.id).unwrap().expect("tasks are not deleted");
            assert_eq!(
                (
                    stored.requester_session_id,
                    stored.worker_session_id,
                    stored.detached_at.is_some()
                ),
                (None, Some(worker), true),
                "one end de-identified, the other intact, and the task stamped"
            );
        }
        assert!(
            fence_frame(&bobs, &frame, &store).is_none(),
            "losing the end Bob could not see must not be what lets him see \
             the task"
        );
        // Not the owner's either, and that is the deliberate cost: the
        // conversation that asked for the work is gone.
        let adas = {
            let s = store.lock().unwrap();
            crate::mcp::auth::device_view(&s, ada)
        };
        assert!(fence_frame(&adas, &frame, &store).is_none());
        assert!(fence_frame(&ViewScope::internal(), &frame, &store).is_some());
    }

    /// A `session:killed` frame is the one shape with no row left to judge
    /// against, so on the LIVE path its carried facts are the answer. On the
    /// replay path it is dropped like everything else — the owner learns the
    /// session ended from the re-list `resumed: false` asks for.
    #[test]
    fn a_kill_frame_rests_on_its_carried_facts_live_and_is_not_replayed() {
        let fx = Fence::new();
        let row = fx.row();
        let killed = fx.frame(
            "session:killed",
            RowChange::SessionKilled(crate::events::SessionKilledPayload::of_row(&row)).payload(),
        );
        {
            let s = fx.store.lock().unwrap();
            s.delete_session(row.id).unwrap();
            assert!(s.get_session_by_id(row.id).unwrap().is_none());
        }
        assert!(
            fence_frame(&fx.scope(fx.ada), &killed, &fx.store).is_some(),
            "live, the owner must still be told the session ended"
        );
        assert!(
            fence_frame(&fx.scope(fx.cleo), &killed, &fx.store).is_none(),
            "and a stranger must not be"
        );
        // A GRANTEE is a different matter, and not this test's: the row's
        // deletion cascaded their grant away, so their scope no longer
        // carries it and the owner arm above is the one that proves the
        // carried facts are still consulted.
        assert!(fence_frame(&fx.scope(fx.bob), &killed, &fx.store).is_none());
        assert!(
            fence_frame_as(&fx.scope(fx.ada), &killed, &fx.store, Delivery::Replay).is_none(),
            "the replay rule has no exception for a frame whose row is gone"
        );
    }

    /// `grant:changed` is a `grant`-kind frame, so it is also on the list a
    /// host-bound or org-bound stream never receives: a per-host token has
    /// no person and holds no grants.
    #[test]
    fn a_host_bound_stream_never_carries_grant_frames() {
        let host = Caller {
            api: None,
            host_alias: Some("h".into()),
            client: None,
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        };
        let fenced = fence_host_bound(&host, None).unwrap();
        assert!(!fenced.iter().any(|k| k == "grant"));
        assert!(!matches(Some(&fenced), &ev("grant:changed")));
        // A person's own device keeps it — and keeps `work`, `settings` and
        // `update`, which is why `fence_host_bound`'s predicate is still
        // `is_scoped()`.
        for keep in ["grant", "work", "settings", "update"] {
            assert!(
                fence_host_bound(&Caller::master(), Some(vec![keep.to_string()]))
                    .is_none_or(|k| k.iter().any(|x| x == keep)),
                "{keep} must still reach a person's own device"
            );
        }
    }

    /// `handle_events` needs a live `EventFeed` to drive end to end, so there
    /// is no handler-level HTTP harness in this file (unlike `report_route`'s
    /// `app()`); the peer gate is a pure fn precisely so it can be checked
    /// here without building one. `handle_events` itself calls this same fn
    /// first thing, so a caller refused here is refused before a stream ever
    /// opens.
    #[test]
    fn a_peer_caller_is_refused_and_anyone_else_is_not() {
        use super::super::auth::{refuses_peer, ClientRef, TokenMode};
        let peer = Caller {
            api: None,
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "hub-b".into(),
                trusted: false,
                org_id: None,
                person_id: None,
            }),
            mode: TokenMode::Peer,
            pane: None,
            is_personal_owner: false,
        };
        let resp = refuses_peer(&peer).expect("a peer must be refused");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(refuses_peer(&Caller::master()).is_none());
    }

    // ---- multi-user M1 (T10): the departure announcement -------------------

    /// **A session that LEAVES a live stream's fence is announced, once.**
    ///
    /// T9c and T9d both recorded this as open: the stream ends when the
    /// CALLER's scope moves (a grant revoke, an org re-bind), but a host's
    /// org move, an ownership change and a `visibility` change move the
    /// SESSION — so the stream stayed up, the fence simply started dropping
    /// that row's frames, and a paired phone displayed the row for ever
    /// (the desktop bridge re-lists only on `resumed: false`).
    ///
    /// The rule is in two pure functions so it can be asserted without a
    /// live SSE body, which is the whole reason they are not inline in the
    /// stream loop.
    #[test]
    fn a_session_that_leaves_the_fence_is_announced_once_and_only_if_it_was_served() {
        let fx = Fence::new();
        let row = fx.row();
        let updated = fx.frame("session:updated", serde_json::to_value(&row).unwrap());
        let mut served: BTreeSet<i64> = BTreeSet::new();

        // Nothing was served yet, so a refusal is SILENCE: a row the fence
        // refused from the start must never be announced as having left,
        // or the announcement is an existence oracle.
        assert_eq!(
            departure_for(&mut served, &updated),
            None,
            "a session this connection never saw does not depart"
        );

        // Served once; a later refusal about it is the departure.
        record_served(&mut served, &updated);
        assert!(served.contains(&fx.session));
        assert_eq!(
            departure_for(&mut served, &updated),
            Some(fx.session),
            "the row left the fence and the caller is told"
        );
        // At most once per session per connection: the id is taken OUT.
        assert_eq!(
            departure_for(&mut served, &updated),
            None,
            "the second refusal is silence again"
        );

        // A `session:killed` that went OUT is itself the departure, so it
        // takes the id out rather than putting it in — otherwise a kill
        // followed by a reused rowid would announce a session the caller
        // has already been told is gone.
        record_served(&mut served, &updated);
        let killed = fx.frame("session:killed", serde_json::json!({ "id": fx.session }));
        record_served(&mut served, &killed);
        assert!(!served.contains(&fx.session));

        // The id-only shapes COUNT as having been served — the session is
        // recorded — but never ANNOUNCE a departure: their fence had to
        // resolve the id in the store, and a resolution that failed for a
        // reason other than visibility must not reach a client as a removal.
        // See `departure_for`.
        let ev = fx.frame(
            "session:event",
            serde_json::json!({ "session_id": fx.session, "detail": "x" }),
        );
        record_served(&mut served, &ev);
        assert_eq!(frame_session_id(&ev), Some(fx.session), "it is recorded");
        assert!(served.contains(&fx.session));
        assert_eq!(
            departure_for(&mut served, &ev),
            None,
            "an id-only shape records, and never announces"
        );
        // And the recorded id is still there for the row frame to announce.
        assert_eq!(departure_for(&mut served, &updated), Some(fx.session));

        // Kinds that are not a statement about the row's existence carry no
        // departure, even though two of them spell `session_id`.
        for (name, payload) in [
            (
                "move:progress",
                serde_json::json!({ "session_id": fx.session }),
            ),
            (
                "grant:changed",
                serde_json::json!({ "session_id": fx.session, "person_id": fx.bob }),
            ),
            ("host:probed", serde_json::json!({ "alias": "h" })),
            // A row-shaped frame of another KIND: `work:item` spells its own
            // id and must not be read as a session's.
            ("work:item", serde_json::json!({ "id": fx.session })),
        ] {
            let mut s2: BTreeSet<i64> = BTreeSet::new();
            s2.insert(fx.session);
            let msg = fx.frame(name, payload);
            assert_eq!(
                frame_session_id(&msg),
                None,
                "{name} is not a statement about whether the row exists"
            );
            assert_eq!(departure_for(&mut s2, &msg), None, "{name}");
        }
    }

    /// The departure frame is **indistinguishable from a real kill**, which
    /// is the privacy property: a distinct `session:left` would tell a
    /// former watcher "it is still running, you just lost access". It is
    /// also why no consumer changes — `src/lib/events.ts` maps
    /// `session:killed` to `{ type: 'killed', id }` and the desktop
    /// bridge's `payload_fits` requires `id` and nothing else.
    #[test]
    fn a_departure_frame_is_shaped_exactly_like_a_kill() {
        let ev = departure_event(42, None, 7, 3);
        let real = sse_event("session:killed", &serde_json::json!({ "id": 42 })).id(frame_id(7, 3));
        assert_eq!(format!("{ev:?}"), format!("{real:?}"));
        // Projected like any other frame, for the same reason.
        let projected = departure_event(42, Some(&vec!["id".to_string()]), 7, 3);
        assert_eq!(format!("{projected:?}"), format!("{real:?}"));
    }
}
