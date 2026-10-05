//! `/voice/capture` and `/voice/source` over a real loopback socket, through
//! the real `authorize` layer (`crate::mcp::test_app`).
//!
//! The registry is process-global, so every hub gets a session id no other
//! test uses, and drops whatever claim is left on it when it goes.

use crate::mcp::auth::sha256_hex;
use crate::service::settings;
use crate::service::voice::{registry, PcmTx, VoiceSource};
use crate::store::Store;
use futures_util::{SinkExt, StreamExt};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_util::sync::CancellationToken;

const MASTER: &str = "voice-master-token";
const HOST_A: &str = "voice-h-a-token";
const HOST_B: &str = "voice-h-b-token";
const PHONE: &str = "voice-phone-token";
const VIEWER: &str = "voice-viewer-token";
const STRANGER: &str = "voice-stranger-token";

/// How long a test waits for something before it fails.
const PATIENCE: Duration = Duration::from_secs(5);
const CHUNK: usize = 3200;

static NEXT_SESSION_ID: AtomicI64 = AtomicI64::new(4_700_000);

struct Hub {
    addr: SocketAddr,
    store: Arc<Mutex<Store>>,
    /// The id of session `s1`, running on `h-a`.
    session_id: i64,
}

impl Drop for Hub {
    fn drop(&mut self) {
        // Whatever claim a test left behind: replace it, then release ours.
        let id = registry().claim(self.session_id, "teardown", Arc::new(Pusher::new(0)));
        registry().release(self.session_id, id);
    }
}

/// A hub with hosts `h-a` (org Acme) and `h-b`, session `s1` on `h-a`, a
/// full client `phone`, a readonly client `viewer`, and a full client
/// `stranger` bound to org Other.
async fn hub(voice_enabled: bool) -> Hub {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    {
        let s = store.lock().unwrap();
        let acme = s.add_org("Acme", None, false).unwrap().id;
        let other = s.add_org("Other", None, false).unwrap().id;
        s.upsert_host("h-a").unwrap();
        s.upsert_host_token("h-a", HOST_A).unwrap();
        s.set_host_org("h-a", Some(acme)).unwrap();
        s.upsert_host("h-b").unwrap();
        s.upsert_host_token("h-b", HOST_B).unwrap();
        s.insert_client_token("phone", &sha256_hex(PHONE), "full")
            .unwrap();
        s.insert_client_token("viewer", &sha256_hex(VIEWER), "readonly")
            .unwrap();
        s.insert_client_token("stranger", &sha256_hex(STRANGER), "full")
            .unwrap();
        s.set_client_org("stranger", Some(other)).unwrap();
        let id = s
            .upsert_session("s1", "h-a", None, None, 0, 0, "running", None)
            .unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET id = ?1 WHERE id = ?2",
                rusqlite::params![session_id, id],
            )
            .unwrap();
        let row = s.get_session_by_id(session_id).unwrap().unwrap();
        assert_eq!(row.org_id, Some(acme), "s1 is Acme's, through h-a");
        if voice_enabled {
            settings::set(&s, settings::VOICE_ENABLED, "true").unwrap();
        }
    }
    let app = crate::mcp::test_app(
        Arc::clone(&store),
        MASTER,
        crate::agent::ws::AgentWsState::disabled(),
    );
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    crate::rt::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    Hub {
        addr,
        store,
        session_id,
    }
}

/// A source that pushes `[byte; CHUNK]` every 10 ms while it is started,
/// counting its live guards.
struct Pusher {
    byte: u8,
    live: Arc<AtomicUsize>,
}

impl Pusher {
    fn new(byte: u8) -> Self {
        Self {
            byte,
            live: Arc::new(AtomicUsize::new(0)),
        }
    }
}

struct PushGuard {
    live: Arc<AtomicUsize>,
    stop: CancellationToken,
}

impl Drop for PushGuard {
    fn drop(&mut self) {
        self.stop.cancel();
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl VoiceSource for Pusher {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        self.live.fetch_add(1, Ordering::SeqCst);
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let byte = self.byte;
        crate::rt::spawn(async move {
            loop {
                tokio::select! {
                    _ = stopped.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        if tx.send(vec![byte; CHUNK]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Box::new(PushGuard {
            live: Arc::clone(&self.live),
            stop,
        }))
    }
}

/// An HTTP/1.1 response whose body is read as it arrives.
struct Resp {
    status: u16,
    headers: String,
    reader: BufReader<TcpStream>,
}

async fn get(addr: SocketAddr, path: &str, bearer: &str) -> Resp {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let req =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {bearer}\r\n\r\n");
    tcp.write_all(req.as_bytes()).await.unwrap();
    let mut reader = BufReader::new(tcp);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).await.unwrap();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status line: {status_line:?}"));
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        headers.push_str(&line.to_ascii_lowercase());
    }
    Resp {
        status,
        headers,
        reader,
    }
}

impl Resp {
    fn chunked(&self) -> bool {
        self.headers.contains("transfer-encoding: chunked")
    }

    /// The next chunk of a chunked body; `None` at its end (or a hang-up).
    async fn next_chunk(&mut self) -> Option<Vec<u8>> {
        let mut size = String::new();
        if self.reader.read_line(&mut size).await.ok()? == 0 {
            return None;
        }
        let n = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
        if n == 0 {
            return None;
        }
        let mut buf = vec![0; n + 2];
        self.reader.read_exact(&mut buf).await.ok()?;
        buf.truncate(n);
        Some(buf)
    }

    /// The whole body, as text.
    async fn text(mut self) -> String {
        let body = if self.chunked() {
            let mut all = Vec::new();
            while let Some(c) = self.next_chunk().await {
                all.extend(c);
            }
            all
        } else {
            let len: usize = self
                .headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse().unwrap())
                .unwrap_or(0);
            let mut buf = vec![0; len];
            self.reader.read_exact(&mut buf).await.unwrap();
            buf
        };
        String::from_utf8(body).unwrap()
    }

    /// At least `n` body bytes, or fail after [`PATIENCE`].
    async fn read_at_least(&mut self, n: usize) -> Vec<u8> {
        let mut all = Vec::new();
        tokio::time::timeout(PATIENCE, async {
            while all.len() < n {
                match self.next_chunk().await {
                    Some(c) => all.extend(c),
                    None => panic!("the body ended after {} bytes", all.len()),
                }
            }
        })
        .await
        .expect("the body did not arrive in time");
        all
    }

    /// True when the body ends within `within`.
    async fn ends_within(&mut self, within: Duration) -> bool {
        tokio::time::timeout(within, async { while self.next_chunk().await.is_some() {} })
            .await
            .is_ok()
    }
}

/// Poll `cond` until it holds, or fail after `within`.
async fn eventually(within: Duration, what: &str, cond: impl Fn() -> bool) {
    let until = tokio::time::Instant::now() + within;
    while !cond() {
        assert!(tokio::time::Instant::now() < until, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn capture_path() -> &'static str {
    "/voice/capture?tmux=s1"
}

fn source_path(h: &Hub) -> String {
    format!("/voice/source?session_id={}", h.session_id)
}

type Ws = tokio_tungstenite::WebSocketStream<TcpStream>;

/// Dial `/voice/source`. `Err(status)` is what the HTTP layer answered
/// instead of upgrading.
async fn dial(h: &Hub, bearer: &str) -> Result<Ws, u16> {
    let mut req = format!("ws://{}{}", h.addr, source_path(h))
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {bearer}").parse().unwrap());
    let tcp = TcpStream::connect(h.addr).await.unwrap();
    tcp.set_nodelay(true).unwrap();
    match tokio_tungstenite::client_async(req, tcp).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("unexpected dial failure: {e}"),
    }
}

/// The next text frame on `ws`, as JSON.
async fn next_text(ws: &mut Ws) -> serde_json::Value {
    tokio::time::timeout(PATIENCE, async {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Text(t))) => return serde_json::from_str(&t).unwrap(),
                Some(Ok(_)) => continue,
                other => panic!("the socket ended: {other:?}"),
            }
        }
    })
    .await
    .expect("no text frame in time")
}

#[tokio::test]
async fn capture_refuses_a_person_token() {
    let h = hub(true).await;
    registry().claim(h.session_id, "test", Arc::new(Pusher::new(9)));
    assert_eq!(get(h.addr, capture_path(), MASTER).await.status, 403);
    assert_eq!(get(h.addr, capture_path(), PHONE).await.status, 403);
}

#[tokio::test]
async fn capture_without_voice_enabled_is_403() {
    let h = hub(false).await;
    registry().claim(h.session_id, "test", Arc::new(Pusher::new(9)));
    assert_eq!(get(h.addr, capture_path(), HOST_A).await.status, 403);
}

#[tokio::test]
async fn capture_of_another_hosts_session_is_404() {
    let h = hub(true).await;
    registry().claim(h.session_id, "test", Arc::new(Pusher::new(9)));
    assert_eq!(get(h.addr, capture_path(), HOST_B).await.status, 404);
}

#[tokio::test]
async fn capture_without_a_claim_is_409() {
    let h = hub(true).await;
    let r = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(r.status, 409);
    assert!(r.text().await.contains("🎤"));
}

#[tokio::test]
async fn capture_streams_the_claimed_source() {
    let h = hub(true).await;
    let src = Arc::new(Pusher::new(9));
    registry().claim(h.session_id, "test", src.clone());
    let mut r = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(r.status, 200);
    assert!(r.headers.contains("content-type: application/octet-stream"));
    let got = r.read_at_least(2 * CHUNK).await;
    assert!(got.iter().all(|&b| b == 9));
    assert_eq!(src.live.load(Ordering::SeqCst), 1);
    drop(r);
    eventually(Duration::from_secs(1), "the microphone stays open", || {
        src.live.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test]
async fn a_second_capture_of_the_session_is_409() {
    let h = hub(true).await;
    registry().claim(h.session_id, "test", Arc::new(Pusher::new(9)));
    let mut first = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(first.status, 200);
    first.read_at_least(CHUNK).await;
    assert_eq!(get(h.addr, capture_path(), HOST_A).await.status, 409);
}

#[tokio::test]
async fn releasing_the_claim_ends_a_live_capture() {
    let h = hub(true).await;
    let src = Arc::new(Pusher::new(9));
    let claim = registry().claim(h.session_id, "test", src.clone());
    let mut r = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(r.status, 200);
    r.read_at_least(CHUNK).await;
    assert!(registry().release(h.session_id, claim));
    assert!(
        r.ends_within(Duration::from_secs(1)).await,
        "the capture outlived its claim"
    );
    eventually(Duration::from_secs(1), "the microphone stays open", || {
        src.live.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test]
async fn max_capture_secs_cuts_a_recording_off() {
    let h = hub(true).await;
    settings::set(
        &h.store.lock().unwrap(),
        settings::VOICE_MAX_CAPTURE_SECS,
        "1",
    )
    .unwrap();
    let src = Arc::new(Pusher::new(9));
    registry().claim(h.session_id, "test", src.clone());
    let mut r = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(r.status, 200);
    r.read_at_least(CHUNK).await;
    assert!(
        r.ends_within(Duration::from_secs(3)).await,
        "a 1 s limit did not cut the recording"
    );
    eventually(Duration::from_secs(1), "the microphone stays open", || {
        src.live.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test]
async fn max_capture_secs_zero_is_no_limit() {
    let h = hub(true).await;
    settings::set(
        &h.store.lock().unwrap(),
        settings::VOICE_MAX_CAPTURE_SECS,
        "0",
    )
    .unwrap();
    let src = Arc::new(Pusher::new(9));
    registry().claim(h.session_id, "test", src.clone());
    let mut r = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(r.status, 200);
    let got = r.read_at_least(4 * CHUNK).await;
    assert!(got.iter().all(|&b| b == 9));
    assert_eq!(src.live.load(Ordering::SeqCst), 1, "still recording");
}

#[tokio::test]
async fn source_refuses_host_and_readonly_tokens() {
    let h = hub(true).await;
    assert_eq!(dial(&h, HOST_A).await.err(), Some(403));
    assert_eq!(dial(&h, VIEWER).await.err(), Some(403));
    assert_eq!(registry().owner(h.session_id), None);
}

#[tokio::test]
async fn source_claims_and_relays_end_to_end() {
    let h = hub(true).await;
    let mut ws = dial(&h, PHONE).await.expect("the upgrade");
    let sid = h.session_id;
    eventually(PATIENCE, "the socket never claimed the session", || {
        registry().owner(sid).as_deref() == Some("client:phone")
    })
    .await;

    let addr = h.addr;
    let (bytes_tx, bytes_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let recorder = crate::rt::spawn(async move {
        let mut r = get(addr, capture_path(), HOST_A).await;
        assert_eq!(r.status, 200);
        let got = r.read_at_least(2 * CHUNK).await;
        let _ = bytes_tx.send(got);
        // Hold the capture open until the test says hang up.
        let _ = done_rx.await;
    });

    let start = next_text(&mut ws).await;
    let n = start["start"].as_u64().expect("a start frame");
    ws.send(WsMessage::Binary(vec![5u8; CHUNK].into()))
        .await
        .unwrap();
    ws.send(WsMessage::Binary(vec![5u8; CHUNK].into()))
        .await
        .unwrap();
    let got = tokio::time::timeout(PATIENCE, bytes_rx)
        .await
        .expect("the recorder read nothing")
        .unwrap();
    assert_eq!(got.len(), 2 * CHUNK);
    assert!(got.iter().all(|&b| b == 5));

    drop(done_tx);
    recorder.await.unwrap();
    let stop = next_text(&mut ws).await;
    assert_eq!(stop["stop"].as_u64(), Some(n));

    ws.close(None).await.unwrap();
    eventually(
        Duration::from_secs(1),
        "the claim outlived its socket",
        || registry().owner(sid).is_none(),
    )
    .await;
}

#[tokio::test]
async fn source_for_a_session_outside_the_org_scope_is_404() {
    let h = hub(true).await;
    assert_eq!(dial(&h, STRANGER).await.err(), Some(404));
    assert_eq!(registry().owner(h.session_id), None);
}

#[tokio::test]
async fn a_new_claim_over_a_live_capture_ends_it() {
    let h = hub(true).await;
    let first = Arc::new(Pusher::new(9));
    registry().claim(h.session_id, "test", first.clone());
    let mut r = get(h.addr, capture_path(), HOST_A).await;
    assert_eq!(r.status, 200);
    r.read_at_least(CHUNK).await;
    registry().claim(h.session_id, "other", Arc::new(Pusher::new(7)));
    assert!(
        r.ends_within(Duration::from_secs(1)).await,
        "the capture outlived its replaced claim"
    );
    eventually(
        Duration::from_secs(1),
        "the old microphone stays open",
        || first.live.load(Ordering::SeqCst) == 0,
    )
    .await;
}

#[tokio::test]
async fn a_superseded_source_socket_is_closed_with_4001() {
    let h = hub(true).await;
    let mut ws = dial(&h, PHONE).await.expect("the upgrade");
    let sid = h.session_id;
    eventually(PATIENCE, "the socket never claimed the session", || {
        registry().owner(sid).as_deref() == Some("client:phone")
    })
    .await;
    registry().claim(sid, "other", Arc::new(Pusher::new(7)));
    let frame = tokio::time::timeout(PATIENCE, async {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Close(frame))) => return frame,
                Some(Ok(_)) => continue,
                other => panic!("the socket ended without a close frame: {other:?}"),
            }
        }
    })
    .await
    .expect("no close frame in time")
    .expect("a close frame with a code");
    assert_eq!(u16::from(frame.code), 4001);
    assert_eq!(frame.reason.as_str(), "microphone claimed elsewhere");
}

#[tokio::test]
async fn every_stop_reaches_the_device_however_many_are_queued() {
    // The device's socket is busy (backpressured): nothing drains the
    // command queue while captures start and stop. No stop may be lost, or
    // the device would keep its microphone open with no capture.
    let (source, mut cmds) = super::WsSource::new();
    for _ in 0..16 {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let guard = source.start(tx).expect("start");
        drop(guard);
    }
    let mut stops = 0;
    while let Ok(cmd) = cmds.try_recv() {
        if matches!(cmd, super::SourceCmd::Stop { .. }) {
            stops += 1;
        }
    }
    assert_eq!(stops, 16);
}

#[test]
fn pcm_for_a_capture_that_is_gone_stops_the_device() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let mut live = Some((3, tx));
    assert_eq!(super::relay_pcm(&mut live, vec![1; 4]), None);
    drop(rx);
    assert_eq!(super::relay_pcm(&mut live, vec![1; 4]), Some(3));
    assert!(live.is_none());
    assert_eq!(super::relay_pcm(&mut live, vec![1; 4]), None, "told once");
}
