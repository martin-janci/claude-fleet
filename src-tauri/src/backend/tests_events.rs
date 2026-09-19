//! Tests for [`super`] (`backend::events`), the hub event bridge.
//!
//! # The shape of the central assertion
//!
//! Every "indistinguishable" test below builds a real [`RowChange`], asks it
//! for its own `name()` and `payload()`, frames *those* as the hub would, and
//! asserts the bridge emitted exactly them. Nothing here hardcodes
//! `"session:updated"` or a field list: if `RowChange` changes its name or its
//! payload, the expectation moves with it and the test still means what it
//! says. That is what makes this the guard on "the stores did not have to
//! change" rather than a restatement of the bridge's own source.
//!
//! The rows come from the contract module's samples, so the two pin the same
//! fixtures rather than two that can drift.
//!
//! # No test here sleeps on the real clock
//!
//! [`Delay`] is injected and the fake returns immediately, recording what it
//! was asked to wait. A real sleep in this file would be jitter in the
//! `start_paused` tests elsewhere in this suite.

use super::*;
use crate::backend::contract::tests::{
    sample_account, sample_host, sample_project_row, sample_session, sample_task,
    sample_worktree_row,
};
use fleet_core::events::{CatalogSummary, RowChange, SyncProgress};
use fleet_core::store::AssetInventoryRow;
use serde_json::json;
use std::sync::Mutex as StdMutex;

/// Each recorded wait belongs to the step of the curve it should: the
/// reconnect backoff is jittered (`fleet_proto::backoff`), so a delay is a
/// point in the upper half of `FIRST_BACKOFF * multiple`, capped at
/// `MAX_BACKOFF` — never an exact number.
#[track_caller]
fn assert_steps(waits: &[Duration], multiples: &[u32], why: &str) {
    assert_eq!(
        waits.len(),
        multiples.len(),
        "{why}: {waits:?} against steps {multiples:?}"
    );
    for (wait, multiple) in waits.iter().zip(multiples) {
        let step = (FIRST_BACKOFF * *multiple).min(MAX_BACKOFF);
        assert!(
            *wait >= step / 2 && *wait <= step,
            "{why}: {wait:?} is not inside the {step:?} step of {waits:?}"
        );
    }
}

// --- doubles -----------------------------------------------------------------

/// Records every `(name, payload)` the bridge emitted.
#[derive(Default)]
struct Recorder {
    seen: StdMutex<Vec<(&'static str, Value)>>,
}

impl Recorder {
    fn events(&self) -> Vec<(&'static str, Value)> {
        self.seen.lock().unwrap().clone()
    }
    fn names(&self) -> Vec<&'static str> {
        self.events().into_iter().map(|(n, _)| n).collect()
    }
}

impl RemoteEventSink for Recorder {
    fn emit_remote(&self, name: &'static str, payload: Value) {
        self.seen.lock().unwrap().push((name, payload));
    }
}

/// Counts resyncs. That is the whole assertion for "one refetch per gap".
/// Also records what the bridge showed it between resyncs.
#[derive(Default)]
struct CountingResync {
    calls: std::sync::atomic::AtomicUsize,
    observed: StdMutex<Vec<&'static str>>,
}

impl CountingResync {
    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn observed(&self) -> Vec<&'static str> {
        self.observed.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl FleetResync for CountingResync {
    async fn resync(&self) {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn observe(&self, name: &'static str, _payload: &Value) {
        self.observed.lock().unwrap().push(name);
    }
}

/// Returns immediately and records what it was asked to wait for.
#[derive(Default)]
struct FakeDelay {
    waits: StdMutex<Vec<Duration>>,
}

impl FakeDelay {
    fn waits(&self) -> Vec<Duration> {
        self.waits.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Delay for FakeDelay {
    async fn sleep(&self, how_long: Duration) {
        self.waits.lock().unwrap().push(how_long);
        // Yield rather than sleep: the loop must still give the runtime a
        // chance to poll `cancel`, but on this suite's clock, not the wall's.
        tokio::task::yield_now().await;
    }
}

/// One scripted connection: either it fails to open, or it delivers these
/// pieces of body and then ends — or, for the idle-timeout tests, goes quiet.
enum Connection {
    Fails(&'static str),
    Delivers(Vec<String>),
    /// Delivers these pieces and then never answers again, and never closes:
    /// a half-open socket after a laptop slept and woke on another network.
    GoesSilent(Vec<String>),
    /// Delivers these pieces, then `beats` keep-alive comments one
    /// `KEEPALIVE_INTERVAL` apart on the tokio clock, then ends. The counter
    /// records how many beats were actually read, which is how a test tells
    /// "the stream ran its course" from "the client cut it short".
    KeepsAlive(Vec<String>, usize, Arc<std::sync::atomic::AtomicUsize>),
}

/// What a scripted body does once its pieces are spent.
enum After {
    End,
    Silence,
    Beats(usize, Arc<std::sync::atomic::AtomicUsize>),
}

/// A script of connections. When it runs out it cancels the bridge, so
/// `run()` returns and the test is deterministic without a timeout.
struct ScriptedStream {
    script: StdMutex<std::collections::VecDeque<Connection>>,
    cancel: CancellationToken,
    opens: std::sync::atomic::AtomicUsize,
}

impl ScriptedStream {
    fn new(script: Vec<Connection>, cancel: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            script: StdMutex::new(script.into()),
            cancel,
            opens: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    fn opens(&self) -> usize {
        self.opens.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl HubEventStream for ScriptedStream {
    async fn open(&self) -> Result<Box<dyn EventStreamBody>, String> {
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(Connection::Fails(why)) => {
                self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(why.to_string())
            }
            Some(Connection::Delivers(pieces)) => {
                self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(ScriptedBody {
                    pieces: pieces.into(),
                    after: After::End,
                }))
            }
            Some(Connection::GoesSilent(pieces)) => {
                self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(ScriptedBody {
                    pieces: pieces.into(),
                    after: After::Silence,
                }))
            }
            Some(Connection::KeepsAlive(pieces, beats, served)) => {
                self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(ScriptedBody {
                    pieces: pieces.into(),
                    after: After::Beats(beats, served),
                }))
            }
            // The script is spent: end the run so the test finishes without
            // waiting on anything.
            None => {
                self.cancel.cancel();
                Err("the script is spent".to_string())
            }
        }
    }
}

struct ScriptedBody {
    pieces: std::collections::VecDeque<String>,
    after: After,
}

#[async_trait::async_trait]
impl EventStreamBody for ScriptedBody {
    async fn next(&mut self) -> Result<Option<String>, String> {
        if let Some(piece) = self.pieces.pop_front() {
            return Ok(Some(piece));
        }
        match &mut self.after {
            After::End => Ok(None),
            After::Silence => std::future::pending().await,
            After::Beats(0, _) => Ok(None),
            After::Beats(left, served) => {
                tokio::time::sleep(fleet_core::mcp::events_route::KEEPALIVE_INTERVAL).await;
                *left -= 1;
                served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // What axum's `KeepAlive` writes: an SSE comment line.
                Ok(Some(":\n\n".to_string()))
            }
        }
    }
}

fn ready() -> String {
    frame(
        READY_FRAME,
        &json!({"version": "0.2.20", "now": 1, "kinds": ["session"]}),
    )
}

/// A `ready` frame naming an explicit wire-contract revision — what a hub
/// built with `fleet_core::wire_contract` sends, as opposed to [`ready`]'s
/// shape, which is what every hub released before that field existed sent
/// (and still sends: the field is additive, never required).
fn ready_with_contract(contract: u32) -> String {
    frame(
        READY_FRAME,
        &json!({"version": "0.2.20", "now": 1, "kinds": ["session"], "contract": contract}),
    )
}

/// `drive`, bounded. Only for `start_paused` tests: the bound is on the tokio
/// clock, which a paused runtime advances the moment everything is idle, so a
/// bridge wedged on a silent socket FAILS here instead of hanging the suite.
async fn drive_bounded(
    script: Vec<Connection>,
) -> (Arc<Recorder>, Arc<CountingResync>, Arc<FakeDelay>, usize) {
    tokio::time::timeout(Duration::from_secs(24 * 3600), drive(script))
        .await
        .expect(
            "the bridge wedged: a day passed on the tokio clock and it never gave up on a stream",
        )
}

/// One SSE frame as `/events` writes it (`sse_event` in
/// `crates/fleet-core/src/mcp/events_route.rs`: `Event::default().event(name)
/// .data(payload.to_string())`, which axum renders as `event: <name>` then
/// `data: <json>` then a blank line).
fn frame(name: &str, payload: &Value) -> String {
    format!("event: {name}\ndata: {payload}\n\n")
}

/// Exactly what the hub sends for `change`: the same name and the same JSON a
/// local emit would have produced.
fn frame_for(change: &RowChange) -> String {
    frame(change.name(), &change.payload())
}

/// Drive the bridge over one script and hand back what it emitted, how many
/// times it re-listed, and how long it was asked to wait.
async fn drive(
    script: Vec<Connection>,
) -> (Arc<Recorder>, Arc<CountingResync>, Arc<FakeDelay>, usize) {
    let cancel = CancellationToken::new();
    let stream = ScriptedStream::new(script, cancel.clone());
    let sink = Arc::new(Recorder::default());
    let resync = Arc::new(CountingResync::default());
    let delay = Arc::new(FakeDelay::default());
    let bridge = EventBridge::new(
        stream.clone(),
        sink.clone(),
        resync.clone(),
        delay.clone(),
        cancel,
    );
    bridge.run().await;
    let opens = stream.opens();
    (sink, resync, delay, opens)
}

/// Records every connection state the bridge reported.
#[derive(Default)]
struct StateLog {
    seen: StdMutex<Vec<crate::backend::connection::HubConnection>>,
}

impl crate::backend::connection::ConnectionReporter for StateLog {
    fn report(&self, state: crate::backend::connection::HubConnection) {
        self.seen.lock().unwrap().push(state);
    }
}

/// `drive`, also recording the connection states reported.
async fn drive_reporting(
    script: Vec<Connection>,
) -> Vec<crate::backend::connection::HubConnection> {
    let cancel = CancellationToken::new();
    let stream = ScriptedStream::new(script, cancel.clone());
    let log = Arc::new(StateLog::default());
    EventBridge::new(
        stream,
        Arc::new(Recorder::default()),
        Arc::new(CountingResync::default()),
        Arc::new(FakeDelay::default()),
        cancel,
    )
    .reporting_to(log.clone())
    .run()
    .await;
    let seen = log.seen.lock().unwrap().clone();
    seen
}

/// The script every "one event" test uses: one connection carrying `ready()`
/// then `body`. `ready()` first because the contract gate (issue #148) drops
/// every frame ahead of it — a real hub always sends it first anyway, so
/// this is what these tests would script even without that gate.
async fn one_connection(body: Vec<String>) -> Arc<Recorder> {
    let mut script = vec![ready()];
    script.extend(body);
    drive(vec![Connection::Delivers(script)]).await.0
}

/// `drive`, but also recording the connection states reported — for the
/// contract-skew tests, which need both halves of the same claim: no row
/// reached the frontend, AND the reported state says why.
async fn drive_watched(
    script: Vec<Connection>,
) -> (
    Arc<Recorder>,
    Vec<crate::backend::connection::HubConnection>,
    Arc<CountingResync>,
) {
    let cancel = CancellationToken::new();
    let stream = ScriptedStream::new(script, cancel.clone());
    let sink = Arc::new(Recorder::default());
    let resync = Arc::new(CountingResync::default());
    let log = Arc::new(StateLog::default());
    EventBridge::new(
        stream,
        sink.clone(),
        resync.clone(),
        Arc::new(FakeDelay::default()),
        cancel,
    )
    .reporting_to(log.clone())
    .run()
    .await;
    let states = log.seen.lock().unwrap().clone();
    (sink, states, resync)
}

// --- the central claim -------------------------------------------------------

#[tokio::test]
async fn a_hub_session_update_is_the_frontend_event_a_local_one_would_be() {
    let change = RowChange::SessionUpdated(sample_session());
    let seen = one_connection(vec![frame_for(&change)]).await;
    assert_eq!(
        seen.events(),
        vec![(change.name(), change.payload())],
        "the frontend must receive the same name and the same JSON it would \
         have received from the local event bus"
    );
}

#[tokio::test]
async fn a_hub_kill_is_the_frontend_event_a_local_one_would_be() {
    let change = RowChange::SessionKilled(12);
    let seen = one_connection(vec![frame_for(&change)]).await;
    assert_eq!(seen.events(), vec![(change.name(), change.payload())]);
    // And the payload really is the `{id}` shape `src/lib/events.ts` types as
    // `{ id: number }` — spelled out, because this is the one variant whose
    // payload is not a row.
    assert_eq!(seen.events()[0].1, json!({ "id": 12 }));
}

/// Every variant this test can build, in one pass. A new `RowChange` variant
/// will not appear here on its own, but `fleet_core::events` already forces a
/// new variant into `EVENT_NAMES` at compile time, and `known_event_name`
/// reads that list — so the bridge cannot fall behind `RowChange` even for a
/// variant this table misses.
#[tokio::test]
async fn every_variant_this_test_can_build_crosses_unchanged() {
    let changes = vec![
        RowChange::SessionCreated(sample_session()),
        RowChange::SessionUpdated(sample_session()),
        RowChange::SessionKilled(7),
        RowChange::HostAdded(sample_host()),
        RowChange::HostProbed(sample_host()),
        RowChange::HostRemoved("trn".into()),
        RowChange::AccountUpserted(sample_account()),
        RowChange::ProjectUpdated(sample_project_row()),
        RowChange::WorktreeUpdated(sample_worktree_row()),
        RowChange::WorktreeRemoved(3),
        RowChange::TaskUpdated(sample_task()),
        RowChange::AssetInventoryUpdated(AssetInventoryRow::default()),
        RowChange::AssetInventoryCleared {
            host_alias: "trn".into(),
            harness: "claude".into(),
        },
        RowChange::CatalogLoaded(CatalogSummary {
            head: "abc123".into(),
            loaded_at: 1,
            asset_count: 2,
            problem_count: 3,
        }),
        RowChange::SyncProgress(SyncProgress {
            plan_id: "plan".into(),
            host_alias: "trn".into(),
            harness: "claude".into(),
            done: 1,
            total: 2,
        }),
    ];
    let body: Vec<String> = changes.iter().map(frame_for).collect();
    let expected: Vec<(&'static str, Value)> =
        changes.iter().map(|c| (c.name(), c.payload())).collect();
    let seen = one_connection(body).await;
    assert_eq!(seen.events(), expected);
}

/// The table above can build 15 of the 16 variants; this closes the last one
/// and every future one by going at the name list directly.
///
/// `EVENT_NAMES` is held to `RowChange` at COMPILE time in
/// `fleet_core::events` (an exhaustive match, each arm's literal const-checked
/// against the list), and to `src/lib/events.ts` by
/// `frontend_declares_every_event_name`. So: a new variant forces a new entry
/// here, a new entry forces the frontend to declare it, and this test proves
/// the bridge carries every entry. The three links leave no gap for an event
/// to exist on one side and not the other.
#[tokio::test]
async fn every_event_name_the_frontend_listens_for_crosses_the_bridge() {
    // Every key any store merges on, so the shape check (SF-5) passes it
    // under every name; `probe` is the unknown field that must survive.
    let payload = json!({
        "probe": 1, "id": 1, "session_id": 1, "alias": "trn", "uuid": "u-1", "account_uuid": "u-1",
        "host_alias": "trn", "harness": "claude"
    });
    let body: Vec<String> = fleet_core::events::EVENT_NAMES
        .iter()
        .map(|name| frame(name, &payload))
        .collect();
    let seen = one_connection(body).await;
    assert_eq!(
        seen.names(),
        fleet_core::events::EVENT_NAMES.to_vec(),
        "every name EVENT_NAMES lists — and so every name src/lib/events.ts \
         subscribes to — must cross the bridge; one missing is a store that \
         silently stops updating in remote mode"
    );
    assert!(seen.events().iter().all(|(_, p)| *p == payload));
}

/// The whole point of resolving against `EVENT_NAMES`: what reaches the
/// frontend is the `&'static str` from that list, not the bytes off the
/// socket.
#[test]
fn a_name_off_the_wire_is_replaced_by_the_one_the_frontend_listens_for() {
    let from_socket = String::from("session:updated");
    let resolved = known_event_name(&from_socket).expect("a known name");
    assert_eq!(resolved, "session:updated");
    assert!(
        !std::ptr::eq(resolved.as_ptr(), from_socket.as_ptr()),
        "the emitted name must be the static from EVENT_NAMES, not the \
         caller's string"
    );
    for lookalike in [
        "session:updated ",
        "Session:Updated",
        "session:updated\n",
        "session:renamed",
        "",
    ] {
        assert!(
            known_event_name(lookalike).is_none(),
            "{lookalike:?} is not an event the frontend listens for"
        );
    }
}

#[tokio::test]
async fn an_unknown_event_name_is_ignored() {
    let known = RowChange::SessionKilled(1);
    let seen = one_connection(vec![
        frame("session:teleported", &json!({ "id": 1 })),
        frame("", &json!({})),
        frame_for(&known),
    ])
    .await;
    assert_eq!(
        seen.events(),
        vec![(known.name(), known.payload())],
        "an event this build does not know must be dropped, and must not \
         stop the ones it does"
    );
}

#[tokio::test]
async fn the_ready_frame_is_not_forwarded_to_the_frontend() {
    let known = RowChange::SessionKilled(1);
    let seen = one_connection(vec![
        frame(
            READY_FRAME,
            &json!({"version":"0.2.20","now":1,"kinds":["session"]}),
        ),
        frame_for(&known),
    ])
    .await;
    assert_eq!(
        seen.names(),
        vec!["session:killed"],
        "`ready` has no frontend listener; forwarding it would need a store change"
    );
}

#[tokio::test]
async fn a_payload_that_is_not_json_is_dropped_without_ending_the_stream() {
    let after = RowChange::SessionKilled(2);
    let seen = one_connection(vec![
        "event: session:updated\ndata: {this is not json\n\n".to_string(),
        frame_for(&after),
    ])
    .await;
    assert_eq!(seen.events(), vec![(after.name(), after.payload())]);
}

/// The hub writes one frame per `send`, but TCP does not preserve that: a
/// read boundary can fall inside a `data:` line.
/// SF-5. The bridge used to emit anything that parsed as JSON under any name
/// the frontend listens for, and `events.ts` reads the payload unchecked:
/// `null` under `session:killed` throws inside the batch flush and loses the
/// whole batch; `42` under `session:updated` is appended to the sessions
/// array. And an opted-in plaintext hub is explicitly supported, where anyone
/// on the path can write these frames — the name allowlist constrains only
/// the name.
#[tokio::test]
async fn a_payload_the_frontend_store_cannot_apply_is_dropped_not_emitted() {
    let good = RowChange::SessionKilled(9);
    let bad = [
        ("session:killed", "null"),
        ("session:updated", "42"),
        ("host:removed", "\"trn\""),
        ("task:updated", "[]"),
        // Objects, but without the key the store merges on, or with it
        // under the wrong type.
        ("session:updated", "{}"),
        ("session:killed", r#"{"id":"9"}"#),
        ("host:probed", r#"{"alias":7}"#),
        ("worktree:removed", r#"{"id":null}"#),
        ("account:upserted", r#"{"email":"a@b"}"#),
        ("account_usage:updated", "{}"),
        ("asset_inventory:cleared", r#"{"host_alias":"trn"}"#),
    ];
    let mut body: Vec<String> = bad
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect();
    body.push(frame_for(&good));
    let seen = one_connection(body).await;
    assert_eq!(
        seen.events(),
        vec![(good.name(), good.payload())],
        "only the well-formed event may reach the frontend, and a bad one must \
         not end the stream"
    );
}

/// The forward-compatibility the passthrough exists for is kept: a field
/// this build does not know still crosses untouched.
#[tokio::test]
async fn an_unknown_field_on_a_well_formed_payload_still_crosses() {
    let mut row = RowChange::SessionUpdated(sample_session()).payload();
    row["from_a_newer_hub"] = json!({ "nested": [1, 2] });
    let seen = one_connection(vec![frame("session:updated", &row)]).await;
    assert_eq!(seen.events(), vec![("session:updated", row)]);
}

#[tokio::test]
async fn a_frame_split_across_reads_still_produces_one_event() {
    let change = RowChange::SessionUpdated(sample_session());
    let whole = frame_for(&change);
    let cut = whole.len() / 2;
    // Do not cut inside a character.
    let cut = (cut..whole.len())
        .find(|i| whole.is_char_boundary(*i))
        .unwrap();
    let seen = one_connection(vec![whole[..cut].to_string(), whole[cut..].to_string()]).await;
    assert_eq!(seen.events(), vec![(change.name(), change.payload())]);
}

// --- gaps --------------------------------------------------------------------

#[tokio::test]
async fn a_dropped_stream_reconnects_with_backoff_and_re_lists_once_per_connection() {
    let first = RowChange::SessionKilled(1);
    let second = RowChange::SessionKilled(2);
    // `ready()` first on both connections: a real hub always sends it before
    // anything else, and issue #148's contract check gates the resync on it.
    let (seen, resync, delay, opens) = drive(vec![
        Connection::Delivers(vec![ready(), frame_for(&first)]),
        Connection::Delivers(vec![ready(), frame_for(&second)]),
    ])
    .await;
    assert_eq!(
        seen.events(),
        vec![
            (first.name(), first.payload()),
            (second.name(), second.payload())
        ],
        "the second connection's events must arrive too"
    );
    assert_eq!(
        resync.count(),
        2,
        "one re-list per connection — a fresh subscription replays nothing, \
         so every reconnection leaves a hole"
    );
    assert_eq!(opens, 2, "it reconnected after the first stream ended");
    assert_steps(
        &delay.waits()[..1],
        &[1],
        "a reconnection waits before retrying rather than spinning",
    );
}

#[tokio::test]
async fn a_lagged_frame_ends_the_stream_and_triggers_a_refetch() {
    let before = RowChange::SessionKilled(1);
    let after = RowChange::SessionKilled(2);
    // `ready()` first on both connections, as a real hub always sends it —
    // issue #148's contract check gates the resync on it.
    let (seen, resync, _, opens) = drive(vec![
        Connection::Delivers(vec![
            ready(),
            frame_for(&before),
            frame(LAGGED_FRAME, &json!({ "skipped": 300 })),
            // The hub closes straight after `lagged`; anything behind it in
            // the same read must NOT be applied, because the picture it
            // belongs to already has a hole in it.
            frame_for(&after),
        ]),
        Connection::Delivers(vec![ready()]),
    ])
    .await;
    assert_eq!(
        seen.events(),
        vec![(before.name(), before.payload())],
        "`lagged` ends the stream"
    );
    assert_eq!(
        resync.count(),
        2,
        "the reconnection after `lagged` re-lists, which is the only honest \
         recovery from a hole of unknown size"
    );
    assert_eq!(opens, 2, "`lagged` really did end the stream and reconnect");
}

#[tokio::test]
async fn a_hub_that_will_not_answer_backs_off_and_keeps_trying() {
    let (_, resync, delay, opens) = drive(vec![
        Connection::Fails("connection refused"),
        Connection::Fails("connection refused"),
        Connection::Fails("connection refused"),
    ])
    .await;
    assert_eq!(opens, 3, "it keeps trying rather than giving up");
    assert_eq!(
        resync.count(),
        0,
        "nothing to re-list against a hub that never answered"
    );
    assert_steps(&delay.waits()[..3], &[1, 2, 4], "the wait grows");
}

/// The failure a `Gap { delivered }` exists for: a hub that accepts the
/// socket, answers 200 and closes straight away — a `/events` route whose bus
/// has gone, or a proxy terminating the connection. `open` calls that a
/// success.
///
/// Reset the backoff on it and the loop reconnects every second forever; and
/// because every connection re-lists, each of those seconds costs four tool
/// calls against a hub that is already unwell. So the wait must keep growing.
#[tokio::test]
async fn a_hub_that_accepts_and_delivers_nothing_does_not_reset_the_backoff() {
    // `ready()` alone, then nothing: `events_route` sends `ready`
    // unconditionally before it ever touches its bus (see the docstring
    // above), so a route whose bus has gone still produces it — this models
    // exactly that half of the scenario, and is why the contract check
    // (issue #148) still lets the resync through here.
    let (_, resync, delay, opens) = drive(vec![
        Connection::Delivers(vec![ready()]),
        Connection::Delivers(vec![ready()]),
        Connection::Delivers(vec![ready()]),
    ])
    .await;
    assert_eq!(opens, 3);
    assert_eq!(
        resync.count(),
        3,
        "it did re-list each time — which is exactly why the wait must grow"
    );
    assert_steps(
        &delay.waits()[..3],
        &[1, 2, 4],
        "an empty connection is not a working connection, whatever the socket \
         said",
    );
}

/// And the other direction: a row event proves the stream works, so the next
/// drop is retried promptly rather than after a minute of doubling.
///
/// This used to be "one frame — even just `ready`", and that was the hole the
/// review found (SF-2): the hub emits `ready` unconditionally, before it
/// touches its bus, so a route whose bus has gone still sends it. See the next
/// two tests.
#[tokio::test]
async fn a_row_event_is_what_calls_a_connection_working() {
    let (_, _, delay, _) = drive(vec![
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![ready(), frame_for(&RowChange::SessionKilled(1))]),
        Connection::Delivers(vec![]),
    ])
    .await;
    assert_steps(
        &delay.waits()[..4],
        &[1, 2, 1, 2],
        "the third connection delivered, so the wait after it starts over",
    );
}

/// SF-2. `events_route` sends `ready` before it ever reads the bus, so "a
/// `/events` route whose bus has gone" — the exact case the backoff guard was
/// written for — answers 200, sends `ready`, and closes. Counting `ready` as
/// delivery reset the wait every time: the review measured 1s, 1s, 1s, 1s.
#[tokio::test]
async fn a_connection_that_only_says_ready_does_not_reset_the_backoff() {
    let (_, resync, delay, _) = drive(vec![
        Connection::Delivers(vec![ready()]),
        Connection::Delivers(vec![ready()]),
        Connection::Delivers(vec![ready()]),
        Connection::Delivers(vec![ready()]),
    ])
    .await;
    assert_eq!(resync.count(), 4);
    assert_steps(
        &delay.waits()[..4],
        &[1, 2, 4, 8],
        "`ready` is sent before the hub touches its bus, so it proves nothing",
    );
}

// --- contract skew: issue #148 ------------------------------------------------
//
// The hub's `ready` frame carries `contract`, the wire-contract revision
// (`fleet_core::wire_contract::CONTRACT_REVISION`). Outside
// `[MIN_HUB_CONTRACT, MAX_HUB_CONTRACT]` (`backend::contract`) this build
// does not trust that hub's rows: no resync, no row event applied, and a
// connection state naming both revisions and which side to update.
//
// `MIN_HUB_CONTRACT` is `0` today, so "too old" cannot be reached through a
// live `u32` here — `backend::contract::tests` covers both directions of
// `classify_hub_contract` directly as a pure function instead. These tests
// cover what only the bridge can prove: that a skewed connection actually
// suppresses application, and that a later, in-range reconnect recovers.

#[tokio::test]
async fn a_ready_frame_with_no_contract_field_is_revision_zero_and_in_range() {
    use crate::backend::connection::HubConnection as C;
    let (sink, states, resync) = drive_watched(vec![Connection::Delivers(vec![
        ready(),
        frame_for(&RowChange::SessionKilled(1)),
    ])])
    .await;
    assert_eq!(states.first(), Some(&C::Connected), "{states:?}");
    assert_eq!(resync.count(), 1);
    assert_eq!(sink.names(), vec!["session:killed"]);
}

#[tokio::test]
async fn a_ready_frame_naming_an_in_range_contract_is_accepted() {
    use crate::backend::connection::HubConnection as C;
    let (_, states, resync) = drive_watched(vec![Connection::Delivers(vec![ready_with_contract(
        crate::backend::contract::MAX_HUB_CONTRACT,
    )])])
    .await;
    assert_eq!(states.first(), Some(&C::Connected), "{states:?}");
    assert_eq!(resync.count(), 1);
}

/// The direction reachable through today's real bounds: a hub ahead of
/// `MAX_HUB_CONTRACT`. Neither the row that follows nor a resync must reach
/// the frontend — stale-but-honest beats fresh-but-wrong.
#[tokio::test]
async fn a_hub_above_the_maximum_contract_is_too_new_and_suppresses_everything() {
    use crate::backend::connection::HubConnection as C;
    let killed = RowChange::SessionKilled(1);
    let too_new = crate::backend::contract::MAX_HUB_CONTRACT + 1;
    let (sink, states, resync) = drive_watched(vec![Connection::Delivers(vec![
        ready_with_contract(too_new),
        frame_for(&killed),
    ])])
    .await;
    assert!(
        sink.events().is_empty(),
        "a row from a too-new hub must not reach the frontend: {:?}",
        sink.events()
    );
    assert_eq!(
        resync.count(),
        0,
        "no backfill from a hub this build cannot trust"
    );
    match states.first() {
        Some(C::HubTooNew {
            hub_contract,
            max_contract,
        }) => {
            assert_eq!(*hub_contract, too_new);
            assert_eq!(*max_contract, crate::backend::contract::MAX_HUB_CONTRACT);
        }
        other => panic!("expected HubTooNew first, got {other:?} in {states:?}"),
    }
}

/// Backoff must keep growing against a hub stuck too-new, exactly like any
/// other connection that proves nothing — not reset every second the way a
/// generic `Reconnecting` would if it papered over the skew state.
#[tokio::test]
async fn a_too_new_hub_does_not_reset_the_backoff() {
    let too_new = crate::backend::contract::MAX_HUB_CONTRACT + 1;
    let (_, resync, delay, _) = drive(vec![
        Connection::Delivers(vec![ready_with_contract(too_new)]),
        Connection::Delivers(vec![ready_with_contract(too_new)]),
        Connection::Delivers(vec![ready_with_contract(too_new)]),
    ])
    .await;
    assert_eq!(resync.count(), 0);
    assert_steps(
        &delay.waits()[..3],
        &[1, 2, 4],
        "a hub outside the accepted range must not tight-loop reconnecting",
    );
}

/// The upgrade path: a hub that was too-new comes back in range on the very
/// next reconnect (re-checked fresh every connection), and the desktop
/// recovers without a restart.
#[tokio::test]
async fn recovery_when_a_later_reconnect_is_back_in_range() {
    use crate::backend::connection::HubConnection as C;
    let killed = RowChange::SessionKilled(9);
    let too_new = crate::backend::contract::MAX_HUB_CONTRACT + 1;
    let (sink, states, resync) = drive_watched(vec![
        Connection::Delivers(vec![ready_with_contract(too_new)]),
        Connection::Delivers(vec![ready(), frame_for(&killed)]),
    ])
    .await;
    assert_eq!(
        sink.events(),
        vec![(killed.name(), killed.payload())],
        "the recovered connection's row must apply"
    );
    assert_eq!(resync.count(), 1, "only the in-range connection re-lists");
    assert!(
        matches!(states.first(), Some(C::HubTooNew { .. })),
        "{states:?}"
    );
    assert!(
        states.contains(&C::Connected),
        "the recovered connection must report Connected: {states:?}"
    );
}

/// The gate must hold for EVERY frame ahead of `ready`, not only the ones
/// this suite's other scripts happen to send. A row frame that somehow beat
/// `ready` to the wire — unreachable against a real hub today (`events_route`'s
/// `Phase::Ready` always precedes `Phase::Live`), but not something the code
/// itself enforces — must not be trusted: no contract has been checked yet,
/// so there is nothing to trust it against.
#[tokio::test]
async fn a_row_frame_ahead_of_ready_is_dropped_and_does_not_trigger_a_resync() {
    use crate::backend::connection::HubConnection as C;
    let early = RowChange::SessionKilled(1);
    let later = RowChange::SessionKilled(2);
    let (sink, states, resync) = drive_watched(vec![Connection::Delivers(vec![
        frame_for(&early),
        ready(),
        frame_for(&later),
    ])])
    .await;
    assert_eq!(
        sink.events(),
        vec![(later.name(), later.payload())],
        "the frame ahead of `ready` must be dropped; the one after must apply"
    );
    assert_eq!(
        resync.count(),
        1,
        "one resync, from `ready` — not one triggered by the early row"
    );
    assert_eq!(states.first(), Some(&C::Connected), "{states:?}");
}

/// The other half: a connection that sends rows and never sends `ready` at
/// all. Nothing it sends is ever trusted, and — just as important — the
/// bridge must never tell the frontend it is `Connected` on the strength of
/// a socket alone. This script's connection simply runs out of frames and
/// closes (`Ok(None)`), which `pump` treats like any other connection that
/// proved nothing: a `Reconnecting` report, not a `Connecting` stuck
/// forever and not a `Connected` that was never earned. Pinned so a future
/// change to this path is a deliberate one.
#[tokio::test]
async fn a_connection_that_never_sends_ready_never_reports_connected() {
    use crate::backend::connection::HubConnection as C;
    let row = RowChange::SessionKilled(7);
    let (sink, states, resync) =
        drive_watched(vec![Connection::Delivers(vec![frame_for(&row)])]).await;
    assert!(
        sink.events().is_empty(),
        "no row is trusted without a `ready` first: {:?}",
        sink.events()
    );
    assert_eq!(
        resync.count(),
        0,
        "no backfill without a validated contract"
    );
    assert!(
        !states.contains(&C::Connected),
        "a connection that never sends `ready` must never report Connected: {states:?}"
    );
    assert!(
        matches!(states.first(), Some(C::Reconnecting { .. })),
        "this script's connection closes normally once spent, which `pump` \
         reports as Reconnecting, not Connected: {states:?}"
    );
}

/// SF-3. `lagged` also reset the wait. And the loop it feeds is
/// self-sustaining: a resync is four serial calls during which nothing reads
/// the stream, so the hub's ring overflows and the first frame of the next
/// connection is `lagged` again — at one second, not at a growing backoff.
#[tokio::test]
async fn a_connection_that_ends_lagged_does_not_reset_the_backoff() {
    let lagged = frame(LAGGED_FRAME, &json!({ "skipped": 300 }));
    let (_, _, delay, _) = drive(vec![
        Connection::Delivers(vec![ready(), lagged.clone()]),
        Connection::Delivers(vec![ready(), lagged.clone()]),
        Connection::Delivers(vec![
            ready(),
            frame_for(&RowChange::SessionKilled(1)),
            lagged,
        ]),
    ])
    .await;
    assert_steps(
        &delay.waits()[..3],
        &[1, 2, 4],
        "a connection that ended by falling behind is the hot loop itself, \
         even if a row squeezed through first",
    );
}

/// B-1, the blocker. A laptop sleeps and wakes on another network: the client
/// never writes on the socket again, so it never gets an RST, and the hub's
/// FIN never reaches it. `read()` blocks forever — no reconnect, no resync, a
/// frozen fleet and not one log line. The hub sends a keep-alive every 15 s
/// precisely so a client can notice this; silence well past that IS the
/// signal.
#[tokio::test(start_paused = true)]
async fn a_stream_that_goes_silent_is_abandoned_and_reconnected() {
    let after = RowChange::SessionKilled(2);
    // `ready()` first on the second connection too, as a real hub always
    // sends it — issue #148's contract check gates the resync on it.
    let (seen, resync, _, opens) = drive_bounded(vec![
        Connection::GoesSilent(vec![ready()]),
        Connection::Delivers(vec![ready(), frame_for(&after)]),
    ])
    .await;
    assert_eq!(opens, 2, "the silent stream must be given up on");
    assert_eq!(resync.count(), 2, "and the new connection re-lists");
    assert_eq!(seen.events(), vec![(after.name(), after.payload())]);
}

/// The other side of B-1: a stream kept alive only by the hub's heartbeat is
/// a healthy, quiet fleet, and must not be cut. Ten beats is 150 s of no
/// events at all.
#[tokio::test(start_paused = true)]
async fn a_stream_kept_alive_by_the_hubs_heartbeat_is_not_cut() {
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_, resync, _, opens) = drive_bounded(vec![Connection::KeepsAlive(
        vec![ready()],
        10,
        served.clone(),
    )])
    .await;
    assert_eq!(
        served.load(std::sync::atomic::Ordering::SeqCst),
        10,
        "every heartbeat was read: the client did not cut a live stream"
    );
    assert_eq!(opens, 1);
    assert_eq!(resync.count(), 1);
}

/// And a quiet stream that stayed up is a working one: a hub with nothing to
/// say for a minute, then a proxy's idle cut, must be retried promptly — the
/// SF-2 fix must not turn every quiet evening into a 30-second outage.
#[tokio::test(start_paused = true)]
async fn a_quiet_connection_that_stayed_up_counts_as_working() {
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_, _, delay, _) = drive_bounded(vec![
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![]),
        Connection::KeepsAlive(vec![ready()], 5, served),
        Connection::Delivers(vec![]),
    ])
    .await;
    assert_steps(
        &delay.waits()[..4],
        &[1, 2, 1, 2],
        "75 s of heartbeats is a stream that worked",
    );
}

// --- SF-8: the banner's signal ------------------------------------------------
//
// The design says a dropped stream reconnects "showing a banner while
// disconnected". Every one of these transitions used to reach the log and
// nothing else.

#[tokio::test]
async fn an_open_stream_reports_connected_and_a_dropped_one_reconnecting() {
    use crate::backend::connection::HubConnection as C;
    let states = drive_reporting(vec![Connection::Delivers(vec![
        ready(),
        frame_for(&RowChange::SessionKilled(1)),
    ])])
    .await;
    assert_eq!(states[0], C::Connected, "{states:?}");
    match &states[1] {
        C::Reconnecting {
            attempt: 1,
            retry_in_secs: 1,
            reason,
        } => assert!(reason.contains("closed"), "{reason}"),
        other => panic!("expected reconnecting #1 in 1 s, got {other:?}"),
    }
}

/// A hub that will not accept a connection is OFFLINE, and the attempt number
/// and the wait grow with the backoff so the banner can say how long.
#[tokio::test]
async fn a_hub_that_refuses_is_offline_with_a_growing_attempt_and_its_reason() {
    use crate::backend::connection::HubConnection as C;
    let states = drive_reporting(vec![
        Connection::Fails("connect hub.example.com:443: connection refused"),
        Connection::Fails("connect hub.example.com:443: connection refused"),
    ])
    .await;
    // The third Offline is the spent script's own refusal; only the two
    // scripted ones are about this hub.
    let offline: Vec<(u32, u64, String)> = states
        .iter()
        .filter_map(|s| match s {
            C::Offline {
                attempt,
                retry_in_secs,
                reason,
            } => Some((*attempt, *retry_in_secs, reason.clone())),
            _ => None,
        })
        .take(2)
        .collect();
    assert_eq!(
        offline.iter().map(|(a, _, _)| *a).collect::<Vec<_>>(),
        [1, 2],
        "{states:?}"
    );
    // The wait the banner names is the jittered one the loop will really
    // sleep, rounded and never zero: a point in the upper half of the 1 s
    // step, then of the 2 s step.
    assert_eq!(offline[0].1, 1, "{states:?}");
    assert!((1..=2).contains(&offline[1].1), "{states:?}");
    assert!(
        offline
            .iter()
            .all(|(_, _, why)| why.contains("connection refused")),
        "{offline:?}"
    );
}

/// The attempt count is "since it last worked": a stream that delivered
/// starts the count over, so the banner does not say "attempt 9" after a
/// single blip on a fleet that has been fine all day.
#[tokio::test]
async fn a_stream_that_worked_starts_the_attempt_count_over() {
    use crate::backend::connection::HubConnection as C;
    // `ready()` first: a real hub always sends it, and the contract gate
    // (issue #148) drops a row that arrives ahead of it, which would
    // otherwise silently stop this connection from counting as delivered.
    let states = drive_reporting(vec![
        Connection::Fails("refused"),
        Connection::Fails("refused"),
        Connection::Delivers(vec![ready(), frame_for(&RowChange::SessionKilled(1))]),
    ])
    .await;
    let attempts: Vec<u32> = states
        .iter()
        .filter_map(|s| match s {
            C::Offline { attempt, .. } | C::Reconnecting { attempt, .. } => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(attempts[..3], [1, 2, 1], "{states:?}");
}

/// A hub client's bridge is always watched: without this the banner would
/// quietly never appear, and every test above would still pass.
#[test]
fn the_production_bridge_reports_its_connection_state() {
    let src = include_str!("../bootstrap/tasks.rs");
    assert!(
        src.contains(".reporting_to("),
        "bootstrap/tasks.rs builds the EventBridge without a connection \
         reporter, so the disconnected banner can never show"
    );
}

/// A hub that accepts a socket and drops it immediately must not become a
/// one-second hot loop on a successful-but-useless connection. The curve is
/// `fleet_proto::backoff`'s and pinned there; this pins THIS client's
/// numbers, and that every draw lands inside its own step.
#[test]
fn the_backoff_doubles_and_is_capped() {
    let mut b = backoff();
    let seen: Vec<Duration> = (0..11).map(|_| b.next(jitter())).collect();
    assert_steps(
        &seen,
        &[1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024],
        "the curve doubles and is capped",
    );
    assert!(seen.iter().all(|w| *w <= MAX_BACKOFF));
    assert!(
        *seen.last().unwrap() >= MAX_BACKOFF / 2,
        "the curve reaches its cap: {seen:?}"
    );
}

#[tokio::test]
async fn cancelling_ends_the_bridge_without_reconnecting() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let stream = ScriptedStream::new(
        vec![Connection::Delivers(vec![frame_for(
            &RowChange::SessionKilled(1),
        )])],
        cancel.clone(),
    );
    let sink = Arc::new(Recorder::default());
    let resync = Arc::new(CountingResync::default());
    let bridge = EventBridge::new(
        stream.clone(),
        sink.clone(),
        resync,
        Arc::new(FakeDelay::default()),
        cancel,
    );
    bridge.run().await;
    assert_eq!(stream.opens(), 0, "a cancelled bridge opens nothing");
    assert!(sink.events().is_empty());
}

// --- decoding the real wire --------------------------------------------------

#[test]
fn take_utf8_keeps_a_character_a_read_boundary_split() {
    // "ä" is two bytes; cut between them.
    let text = "session ä done";
    let bytes = text.as_bytes();
    let cut = text.find('ä').unwrap() + 1;
    let mut buf = bytes[..cut].to_vec();
    let first = take_utf8(&mut buf);
    assert_eq!(first, "session ");
    assert_eq!(buf.len(), 1, "the half character is kept, not mangled");
    buf.extend_from_slice(&bytes[cut..]);
    assert_eq!(take_utf8(&mut buf), "ä done");
    assert!(buf.is_empty());
}

#[test]
fn take_utf8_does_not_stall_on_genuinely_invalid_bytes() {
    let mut buf = b"ok\xffmore".to_vec();
    let out = take_utf8(&mut buf);
    assert_eq!(out, "ok\u{FFFD}");
    assert_eq!(take_utf8(&mut buf), "more");
    assert!(buf.is_empty());
}

#[test]
fn the_dechunker_is_transparent_when_the_body_is_not_chunked() {
    let mut d = Dechunker::new(false);
    let mut raw = b"data: 1\n\n".to_vec();
    assert_eq!(d.take(&mut raw).unwrap(), b"data: 1\n\n");
    assert!(raw.is_empty());
    assert!(!d.finished());
}

/// THE case that made a whole-body de-chunker useless here: a size line
/// arriving in one read and its data in the next.
#[test]
fn the_dechunker_waits_for_a_size_line_that_has_not_finished_arriving() {
    let mut d = Dechunker::new(true);
    // 0xb = 11, the length of "data: 12345".
    let mut raw = b"b".to_vec();
    assert!(d.take(&mut raw).unwrap().is_empty(), "'b' might be 'be'");
    raw.extend_from_slice(b"\r\ndata: 12345");
    assert_eq!(d.take(&mut raw).unwrap(), b"data: 12345");
    raw.extend_from_slice(b"\r\n5\r\nabcde\r\n0\r\n\r\n");
    assert_eq!(d.take(&mut raw).unwrap(), b"abcde");
    assert!(d.finished());
}

#[test]
fn the_dechunker_refuses_a_size_it_cannot_read() {
    let mut d = Dechunker::new(true);
    let mut raw = b"zz\r\nxx".to_vec();
    let err = d.take(&mut raw).unwrap_err();
    assert!(err.contains("chunk size"), "{err}");
}

// --- the real stream, against a real socket ----------------------------------

/// `HubSse` end to end over loopback: a listener that answers exactly as axum
/// does — `200`, `Transfer-Encoding: chunked`, SSE frames — and a bridge that
/// reads it. This is the only test that exercises the hand-written HTTP.
#[tokio::test]
async fn the_real_stream_reads_a_chunked_sse_response_from_a_real_socket() {
    let change = RowChange::SessionUpdated(sample_session());
    let body = frame_for(&change);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(StdMutex::new(String::new()));
    let asked_here = Arc::clone(&asked);
    let serve = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut head = vec![0u8; 2048];
        let n = sock.read(&mut head).await.unwrap();
        *asked_here.lock().unwrap() = String::from_utf8_lossy(&head[..n]).into_owned();
        sock.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
              Transfer-Encoding: chunked\r\n\r\n",
        )
        .await
        .unwrap();
        // Two chunks, cutting the frame in half mid-payload — which is what
        // no naive reader survives.
        let cut = body.len() / 2;
        let cut = (cut..body.len())
            .find(|i| body.is_char_boundary(*i))
            .unwrap();
        for piece in [&body[..cut], &body[cut..]] {
            sock.write_all(format!("{:x}\r\n{piece}\r\n", piece.len()).as_bytes())
                .await
                .unwrap();
            sock.flush().await.unwrap();
        }
        sock.write_all(b"0\r\n\r\n").await.unwrap();
        sock.flush().await.unwrap();
    });

    let cfg = RemoteConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        token: "cl_s3cret".into(),
        client_name: "laptop".into(),
    };
    let mut stream = HubSse::new(cfg).open().await.expect("the stream opens");
    let mut decoder = SseDecoder::new();
    let mut frames = Vec::new();
    while let Some(text) = stream.next().await.expect("a readable stream") {
        frames.extend(decoder.feed(&text));
    }
    serve.await.unwrap();

    assert_eq!(frames.len(), 1, "one frame, rejoined across two chunks");
    assert_eq!(frames[0].name, change.name());
    assert_eq!(
        serde_json::from_str::<Value>(&frames[0].data).unwrap(),
        change.payload()
    );

    let request = asked.lock().unwrap().clone();
    assert!(request.starts_with("GET /events HTTP/1.1\r\n"), "{request}");
    assert!(
        request.contains("Authorization: Bearer cl_s3cret\r\n"),
        "the token goes in the header: {request}"
    );
    assert!(
        request.contains("Accept: text/event-stream\r\n"),
        "{request}"
    );
    assert!(
        request.contains(&format!("Host: 127.0.0.1:{port}\r\n")),
        "the hub matches its allowlist against this: {request}"
    );
    assert!(
        !request.contains("Connection: close"),
        "an event stream is supposed to stay open: {request}"
    );
}

/// The hub's own refusals are plain text with the fix in them
/// (`events_route::NOT_ENABLED`, `TOO_MANY_STREAMS`), and the token must not
/// ride along into the log line that carries them.
#[tokio::test]
async fn a_refused_stream_reports_the_hubs_own_words_and_never_the_token() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serve = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _ = sock.read(&mut [0u8; 2048]).await.unwrap();
        // A body that echoes the Authorization header back, which is exactly
        // what a chatty proxy error page does.
        let said = "events are not enabled on this server (token cl_s3cret)";
        sock.write_all(
            format!(
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\n\r\n{said}",
                said.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        sock.flush().await.unwrap();
    });
    let cfg = RemoteConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        token: "cl_s3cret".into(),
        client_name: "laptop".into(),
    };
    let err = match HubSse::new(cfg).open().await {
        Err(e) => e,
        Ok(_) => panic!("503 must be a failure, not a stream"),
    };
    serve.await.unwrap();
    assert!(err.contains("503"), "{err}");
    assert!(
        err.contains("events are not enabled on this server"),
        "the hub's own words name the fix: {err}"
    );
    assert!(
        !err.contains("cl_s3cret"),
        "the token must never reach an error, a log or the UI: {err}"
    );
}

/// `{:?}` on the real stream must not spill the token either.
#[test]
fn the_real_streams_debug_hides_the_token() {
    let printed = format!(
        "{:?}",
        HubSse::new(RemoteConfig {
            base_url: "https://hub.example.com".into(),
            token: "cl_s3cret".into(),
            client_name: "laptop".into(),
        })
    );
    assert!(!printed.contains("cl_s3cret"), "{printed}");
    assert!(printed.contains("<redacted>"), "{printed}");
}

// --- re-listing after a gap --------------------------------------------------

/// Answers each tool from a table, so one resync's four calls can be scripted
/// independently. Framed exactly as the hub frames a `tools/call` answer:
/// SSE, with the tool's JSON as a *string* in `content[0].text`.
struct ToolTable {
    answers: StdMutex<std::collections::HashMap<String, String>>,
    asked: StdMutex<Vec<String>>,
}

impl ToolTable {
    fn new(answers: &[(&'static str, &str)]) -> Arc<Self> {
        let table = Arc::new(Self {
            answers: StdMutex::new(std::collections::HashMap::new()),
            asked: StdMutex::new(Vec::new()),
        });
        table.set(answers);
        table
    }

    /// Replace every answer — how a test says "and then the fleet changed
    /// while this client was not attached".
    fn set(&self, answers: &[(&'static str, &str)]) {
        let mut a = self.answers.lock().unwrap();
        a.clear();
        for (tool, payload) in answers {
            a.insert((*tool).to_string(), (*payload).to_string());
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::backend::remote::HubTransport for ToolTable {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        body: String,
    ) -> Result<crate::backend::remote::HubResponse, String> {
        let v: Value = serde_json::from_str(&body).expect("a JSON-RPC body");
        let tool = v["params"]["name"]
            .as_str()
            .expect("a tool name")
            .to_string();
        self.asked.lock().unwrap().push(tool.clone());
        let answer = self.answers.lock().unwrap().get(&tool).cloned();
        match answer {
            Some(payload) => Ok(crate::backend::remote::HubResponse {
                status: 200,
                body: format!(
                    "event: message\ndata: {}\n\n",
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": { "content": [{ "type": "text", "text": payload }] },
                    })
                ),
            }),
            None => Err(format!("this hub is down (asked for {tool})")),
        }
    }
}

fn resync_over(answers: &[(&'static str, &str)]) -> (HubResync, Arc<Recorder>, Arc<ToolTable>) {
    let table = ToolTable::new(answers);
    let hub = Arc::new(crate::backend::remote::HubBackend::with_transport(
        RemoteConfig {
            base_url: "http://hub.example.com:4180".into(),
            token: "cl_s3cret".into(),
            client_name: "laptop".into(),
        },
        table.clone(),
    ));
    let sink = Arc::new(Recorder::default());
    (HubResync::new(hub, sink.clone()), sink, table)
}

/// A `list_sessions` answer carrying just the ids given — enough for the diff,
/// and every other field absent the way `ok_json_compact` leaves it.
fn sessions_payload(ids: &[i64]) -> String {
    let rows: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id, "tmux_name": format!("s{id}"), "host_alias": "trn",
                "created_at": 1, "last_activity_at": 2, "status": "running",
                "kind": "tmux", "turn_seq": 0, "tags": [],
                "usage_input_tokens": 0, "usage_output_tokens": 0,
                "usage_cache_write_tokens": 0, "usage_cache_read_tokens": 0,
                "usage_cost_micros": 0
            })
        })
        .collect();
    Value::Array(rows).to_string()
}

fn hosts_payload(aliases: &[&str]) -> String {
    let rows: Vec<Value> = aliases
        .iter()
        // `transport` is not optional on the wire; a host row without it does
        // not parse, and `HubResync` then re-lists no host at all.
        .map(|a| {
            json!({ "alias": a, "reachable": true, "hidden": false,
                    "provisioned": true, "transport": "ssh" })
        })
        .collect();
    Value::Array(rows).to_string()
}

#[tokio::test]
async fn a_resync_emits_the_rows_it_re_listed_as_the_events_the_stores_apply() {
    let (resync, seen, table) = resync_over(&[
        ("list_sessions", &sessions_payload(&[1, 2])),
        ("list_hosts", &hosts_payload(&["trn", "hetzner"])),
        ("list_tasks", "[]"),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;
    assert_eq!(
        seen.names(),
        vec![
            "session:updated",
            "session:updated",
            "host:probed",
            "host:probed"
        ],
        "a re-list has to reach the stores as the events they already apply — \
         there is no `refetch` event and giving them one would be a store change"
    );
    assert_eq!(
        table.asked(),
        vec!["list_sessions", "list_hosts", "list_tasks", "list_accounts"],
        "projects and worktrees are deliberately NOT re-listed: their list \
         tools answer ProjectTreeRow/WorktreeOccupancy while the events carry \
         ProjectRow/WorktreeRow, and a wrong payload is worse than a stale one"
    );
}

/// The resync emits the hub's `list_tasks` rows as `task:updated`, so it
/// inherits audit finding A unless the marker is removed where the rows are
/// read: the Tasks panel would show the marker line after every reconnect.
#[tokio::test]
async fn a_resynced_task_carries_the_workers_words_not_the_hubs_marker() {
    let marked = fleet_core::mcp::guard::mark_untrusted(
        "shipped the fix",
        &fleet_core::service::tasks::result_origin(11, Some(7), Some("trn")),
    );
    let tasks =
        json!([{ "id": 11, "state": "done", "created_at": 1, "result": marked }]).to_string();
    let (resync, seen, _) = resync_over(&[
        ("list_sessions", "[]"),
        ("list_hosts", "[]"),
        ("list_tasks", &tasks),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;
    let events = seen.events();
    let task = events
        .iter()
        .find(|(n, _)| *n == "task:updated")
        .expect("the task is re-emitted");
    assert_eq!(task.1["result"], json!("shipped the fix"), "{events:?}");
}

#[tokio::test]
async fn the_first_resync_never_invents_a_removal() {
    let (resync, seen, _) = resync_over(&[
        ("list_sessions", &sessions_payload(&[1])),
        ("list_hosts", &hosts_payload(&["trn"])),
        ("list_tasks", "[]"),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;
    assert!(
        !seen.names().contains(&"session:killed"),
        "nothing is known to have vanished before the app started; emitting a \
         kill from an empty baseline would delete the frontend's whole list"
    );
    assert!(!seen.names().contains(&"host:removed"));
}

#[tokio::test]
async fn a_later_resync_kills_what_has_gone_while_this_client_was_detached() {
    let (resync, seen, table) = resync_over(&[
        ("list_sessions", &sessions_payload(&[1, 2])),
        ("list_hosts", &hosts_payload(&["trn", "hetzner"])),
        ("list_tasks", "[]"),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;
    let before = seen.events().len();

    // The gap: session 2 was killed and `hetzner` removed while this client
    // was not attached, so no event for either ever reached it.
    table.set(&[
        ("list_sessions", &sessions_payload(&[1])),
        ("list_hosts", &hosts_payload(&["trn"])),
        ("list_tasks", "[]"),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;

    let after: Vec<(&'static str, Value)> = seen.events().into_iter().skip(before).collect();
    assert!(
        after.contains(&("session:killed", json!({ "id": 2 }))),
        "a session that vanished during the gap must be removed, or the \
         sidebar shows a session that is gone: {after:?}"
    );
    assert!(
        after.contains(&("host:removed", json!({ "alias": "hetzner" }))),
        "{after:?}"
    );
    assert!(
        !after
            .iter()
            .any(|(n, p)| *n == "session:killed" && p["id"] == 1),
        "a session that is still there must not be killed: {after:?}"
    );
}

/// SF-4. `Seen` used to be written only by a resync, so a row that arrived
/// as a LIVE event and vanished during a gap was never in `before` and never
/// removed: a permanent ghost in the sidebar, every action on it failing,
/// until a manual refresh.
#[tokio::test]
async fn a_row_that_arrived_live_and_vanished_in_a_gap_is_removed() {
    let (resync, seen, _) = resync_over(&[
        ("list_sessions", &sessions_payload(&[1, 2])),
        ("list_hosts", &hosts_payload(&["trn"])),
        ("list_tasks", "[]"),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;

    // Live, between resyncs: session 3 and host `new` are created, session 2
    // is killed.
    resync.observe("session:created", &json!({ "id": 3 }));
    resync.observe("host:added", &json!({ "alias": "new" }));
    resync.observe("session:killed", &json!({ "id": 2 }));
    let before = seen.events().len();

    // Then a gap, during which 3 and `new` go away too.
    resync.resync().await;
    let after: Vec<(&'static str, Value)> = seen.events().into_iter().skip(before).collect();
    assert!(
        after.contains(&("session:killed", json!({ "id": 3 }))),
        "a session this client learned about live must be removed when it \
         vanishes: {after:?}"
    );
    assert!(
        after.contains(&("host:removed", json!({ "alias": "new" }))),
        "{after:?}"
    );
    assert!(
        !after.contains(&("session:killed", json!({ "id": 2 }))),
        "2's kill already arrived live; the resync must not repeat it: {after:?}"
    );
}

/// The bridge's half of SF-4: every row event it emits is shown to the
/// resync, which is how `Seen` hears about rows no list returned.
#[tokio::test]
async fn the_bridge_shows_the_resync_every_row_event_it_emits() {
    let created = RowChange::SessionCreated(sample_session());
    let removed = RowChange::HostRemoved("trn".into());
    let (_, resync, _, _) = drive(vec![Connection::Delivers(vec![
        ready(),
        frame_for(&created),
        frame_for(&removed),
    ])])
    .await;
    assert_eq!(resync.observed(), vec![created.name(), removed.name()]);
}

#[tokio::test]
async fn a_resync_against_an_unreachable_hub_emits_nothing_rather_than_clearing_the_stores() {
    // No answers at all: every call fails.
    let (resync, seen, _) = resync_over(&[]);
    resync.resync().await;
    assert!(
        seen.events().is_empty(),
        "a failed re-list must leave the stores as they were; emitting an \
         empty fleet would blank the UI on a blip: {:?}",
        seen.events()
    );
}
