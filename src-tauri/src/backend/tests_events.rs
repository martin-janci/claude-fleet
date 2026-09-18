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
#[derive(Default)]
struct CountingResync {
    calls: std::sync::atomic::AtomicUsize,
}

impl CountingResync {
    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl FleetResync for CountingResync {
    async fn resync(&self) {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
/// pieces of body and then ends.
enum Connection {
    Fails(&'static str),
    Delivers(Vec<String>),
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
}

#[async_trait::async_trait]
impl EventStreamBody for ScriptedBody {
    async fn next(&mut self) -> Result<Option<String>, String> {
        Ok(self.pieces.pop_front())
    }
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

/// The script every "one event" test uses: one connection carrying `body`.
async fn one_connection(body: Vec<String>) -> Arc<Recorder> {
    drive(vec![Connection::Delivers(body)]).await.0
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
    let payload = json!({ "probe": 1 });
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
    let (seen, resync, delay, opens) = drive(vec![
        Connection::Delivers(vec![frame_for(&first)]),
        Connection::Delivers(vec![frame_for(&second)]),
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
    assert_eq!(
        delay.waits().first(),
        Some(&FIRST_BACKOFF),
        "a reconnection waits before retrying rather than spinning"
    );
}

#[tokio::test]
async fn a_lagged_frame_ends_the_stream_and_triggers_a_refetch() {
    let before = RowChange::SessionKilled(1);
    let after = RowChange::SessionKilled(2);
    let (seen, resync, _, opens) = drive(vec![
        Connection::Delivers(vec![
            frame_for(&before),
            frame(LAGGED_FRAME, &json!({ "skipped": 300 })),
            // The hub closes straight after `lagged`; anything behind it in
            // the same read must NOT be applied, because the picture it
            // belongs to already has a hole in it.
            frame_for(&after),
        ]),
        Connection::Delivers(vec![]),
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
    assert_eq!(
        delay.waits()[..3],
        [FIRST_BACKOFF, FIRST_BACKOFF * 2, FIRST_BACKOFF * 4],
        "the wait grows"
    );
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
    let (_, resync, delay, opens) = drive(vec![
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![]),
    ])
    .await;
    assert_eq!(opens, 3);
    assert_eq!(
        resync.count(),
        3,
        "it did re-list each time — which is exactly why the wait must grow"
    );
    assert_eq!(
        delay.waits()[..3],
        [FIRST_BACKOFF, FIRST_BACKOFF * 2, FIRST_BACKOFF * 4],
        "an empty connection is not a working connection, whatever the socket \
         said"
    );
}

/// And the other direction: one frame — even just `ready` — proves the route
/// answered, so the next drop is retried promptly rather than after a minute
/// of doubling.
#[tokio::test]
async fn a_single_frame_is_enough_to_call_a_connection_working() {
    let (_, _, delay, _) = drive(vec![
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![]),
        Connection::Delivers(vec![frame(
            READY_FRAME,
            &json!({"version": "0.2.20", "now": 1, "kinds": ["session"]}),
        )]),
        Connection::Delivers(vec![]),
    ])
    .await;
    assert_eq!(
        delay.waits()[..4],
        [
            FIRST_BACKOFF,
            FIRST_BACKOFF * 2,
            FIRST_BACKOFF,
            FIRST_BACKOFF * 2
        ],
        "the third connection delivered, so the wait after it starts over"
    );
}

/// A hub that accepts a socket and drops it immediately must not become a
/// one-second hot loop on a successful-but-useless connection.
#[test]
fn the_backoff_doubles_and_is_capped() {
    let mut wait = FIRST_BACKOFF;
    let mut seen = vec![wait];
    for _ in 0..10 {
        wait = next_backoff(wait);
        seen.push(wait);
    }
    assert_eq!(seen[0], Duration::from_secs(1));
    assert_eq!(seen[1], Duration::from_secs(2));
    assert_eq!(seen[2], Duration::from_secs(4));
    assert_eq!(*seen.last().unwrap(), MAX_BACKOFF);
    assert!(seen.iter().all(|w| *w <= MAX_BACKOFF));
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
        .map(|a| json!({ "alias": a, "reachable": true, "hidden": false, "provisioned": true }))
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
