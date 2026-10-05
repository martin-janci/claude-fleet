//! The microphone relay's two routes. `/voice/capture` is the host side: the
//! `arecord` stand-in reads a session's audio. `/voice/source` is the client
//! side: a person's device that holds the socket open is that session's
//! microphone. Spec: docs/superpowers/specs/2026-10-05-voice-relay-design.md.
//!
//! Both are behind `authorize`. Audio passes through in memory only: no
//! chunk is logged, stored or buffered beyond the registry's bounded queue.

use super::auth::{refuses_peer, Caller, TokenMode};
use super::report_route::ReportState;
use crate::service::settings;
use crate::service::voice::{registry, Capture, CaptureRefusal, PcmTx, VoiceSource};
use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// The largest websocket frame or message a device may send: a 100 ms PCM
/// chunk is 3 200 B, so this leaves room for a slow device's larger ones.
const FRAME_CAP: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct CaptureQuery {
    pub tmux: String,
}

pub async fn handle_capture(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<CaptureQuery>,
) -> Response {
    if let Some(refused) = refuses_peer(&caller) {
        return refused;
    }
    let Some(host) = caller.host_alias.clone() else {
        return (StatusCode::FORBIDDEN, "only a host's recorder captures\n").into_response();
    };
    if crate::validate::tmux_name_lookup(&q.tmux).is_err() {
        return (StatusCode::BAD_REQUEST, "bad tmux name\n").into_response();
    }
    let (session_id, max, ttl) = {
        let Ok(s) = state.store().lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if !settings::get_bool(&s, settings::VOICE_ENABLED) {
            return (
                StatusCode::FORBIDDEN,
                "voice relay is off (Settings → Limits → Voice)\n",
            )
                .into_response();
        }
        let row = match s.get_session(&q.tmux, &host) {
            Ok(Some(r)) => r,
            Ok(None) => {
                return (StatusCode::NOT_FOUND, "no such session on this host\n").into_response()
            }
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        (
            row.id,
            Duration::from_secs(settings::get_secs(&s, settings::VOICE_MAX_CAPTURE_SECS)),
            Duration::from_secs(settings::get_secs(&s, settings::VOICE_CLAIM_TTL_SECS)),
        )
    };
    // A desktop source's `start` blocks until its microphone opens (up to
    // 5 s), so it runs off the async workers.
    let begun =
        tokio::task::spawn_blocking(move || registry().begin_capture(session_id, ttl)).await;
    let Ok(begun) = begun else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let capture = match begun {
        Ok(c) => c,
        Err(CaptureRefusal::NoClaim) => {
            return (
                StatusCode::CONFLICT,
                "no microphone for this session — turn on 🎤 in the app\n",
            )
                .into_response()
        }
        Err(CaptureRefusal::Busy) => {
            return (StatusCode::CONFLICT, "this session is already recording\n").into_response()
        }
        Err(CaptureRefusal::SourceFailed(e)) => {
            tracing::warn!(session_id, error = %e, "[voice] source failed to start");
            return (
                StatusCode::BAD_GATEWAY,
                format!("the microphone did not start: {e}\n"),
            )
                .into_response();
        }
    };
    let mut resp = Response::new(Body::from_stream(capture_stream(session_id, capture, max)));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// The capture's audio as a body. The stream owns the capture: when the
/// recorder hangs up, hyper drops the body, the capture drops, and the
/// microphone closes. It also ends — closing the microphone — when the
/// claim is replaced or released (`Capture::revoked`) or `max` has passed;
/// a `max` of zero is no limit.
fn capture_stream(
    session_id: i64,
    capture: Capture,
    max: Duration,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send {
    let revoked = capture.revoked();
    let deadline = (!max.is_zero()).then(|| Instant::now() + max);
    let live = Some(LiveCapture {
        capture,
        session_id,
        started: Instant::now(),
    });
    futures_util::stream::unfold(live, move |live| {
        let revoked = revoked.clone();
        async move {
            let mut live = live?;
            let chunk = tokio::select! {
                _ = revoked.cancelled() => None,
                _ = until(deadline) => None,
                chunk = live.capture.rx.recv() => chunk,
            };
            chunk.map(|c| (Ok(c), Some(live)))
        }
    })
}

/// Sleep until `deadline`; never wake without one.
async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// A capture being streamed; logs its end (session and duration only),
/// however it came.
struct LiveCapture {
    capture: Capture,
    session_id: i64,
    started: Instant,
}

impl Drop for LiveCapture {
    fn drop(&mut self) {
        tracing::info!(
            session_id = self.session_id,
            secs = self.started.elapsed().as_secs(),
            "[voice] capture ended"
        );
    }
}

#[derive(Deserialize)]
pub struct SourceQuery {
    pub session_id: i64,
}

enum SourceCmd {
    Start { capture: u64, tx: PcmTx },
    Stop { capture: u64 },
}

/// The close code a source socket gets when its claim was replaced or
/// released elsewhere.
const CLOSE_CLAIMED_ELSEWHERE: u16 = 4001;

/// A claim held by a websocket: `start` asks the device to open its
/// microphone; the returned guard asks it to close it. The command queue is
/// unbounded so a stop is never lost while the socket is backpressured — a
/// lost stop would leave the device's microphone open with no capture. It
/// stays small: a session has at most one capture at a time.
struct WsSource {
    cmds: tokio::sync::mpsc::UnboundedSender<SourceCmd>,
    next: AtomicU64,
}

struct StopOnDrop {
    cmds: tokio::sync::mpsc::UnboundedSender<SourceCmd>,
    capture: u64,
}

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.cmds.send(SourceCmd::Stop {
            capture: self.capture,
        });
    }
}

impl WsSource {
    fn new() -> (Arc<Self>, tokio::sync::mpsc::UnboundedReceiver<SourceCmd>) {
        let (cmds, rx) = tokio::sync::mpsc::unbounded_channel();
        let source = Arc::new(WsSource {
            cmds,
            next: AtomicU64::new(1),
        });
        (source, rx)
    }
}

/// Hand a device's PCM chunk to the live capture. Returns the capture whose
/// stop the device must still be told of: one whose receiver is gone
/// although no stop came through the queue (defence in depth).
fn relay_pcm(live: &mut Option<(u64, PcmTx)>, pcm: Vec<u8>) -> Option<u64> {
    let (capture, tx) = live.as_ref()?;
    if tx.is_closed() {
        let capture = *capture;
        *live = None;
        return Some(capture);
    }
    // Full queue: drop the chunk rather than buffer audio.
    let _ = tx.try_send(pcm);
    None
}

impl VoiceSource for WsSource {
    fn start(&self, tx: PcmTx) -> Result<Box<dyn Send>, String> {
        let capture = self.next.fetch_add(1, Ordering::Relaxed);
        self.cmds
            .send(SourceCmd::Start { capture, tx })
            .map_err(|_| "the device's connection is gone".to_string())?;
        Ok(Box::new(StopOnDrop {
            cmds: self.cmds.clone(),
            capture,
        }))
    }
}

pub async fn handle_source(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<SourceQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(refused) = refuses_peer(&caller) {
        return refused;
    }
    if caller.host_alias.is_some() {
        return (
            StatusCode::FORBIDDEN,
            "a host's token records; a person's device supplies the microphone\n",
        )
            .into_response();
    }
    if caller.mode == TokenMode::Readonly {
        return (
            StatusCode::FORBIDDEN,
            "a readonly token cannot supply a microphone\n",
        )
            .into_response();
    }
    {
        let Ok(s) = state.store().lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if !settings::get_bool(&s, settings::VOICE_ENABLED) {
            return (StatusCode::FORBIDDEN, "voice relay is off\n").into_response();
        }
        let scope = match caller.org_scope(&s) {
            Ok(sc) => sc,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let row = s.get_session_by_id(q.session_id).ok().flatten();
        if !row.as_ref().is_some_and(|r| scope.sees_row(r)) {
            return (StatusCode::NOT_FOUND, "no such session\n").into_response();
        }
    }
    let owner = caller.label();
    let session_id = q.session_id;
    ws.max_frame_size(FRAME_CAP)
        .max_message_size(FRAME_CAP)
        .on_upgrade(move |socket| serve_source(socket, session_id, owner))
}

/// Hold the session's claim for as long as the socket is open: relay start
/// and stop to the device as text, and its binary PCM into the live capture.
async fn serve_source(socket: WebSocket, session_id: i64, owner: String) {
    let (mut sink, mut stream) = socket.split();
    let (source, mut cmd_rx) = WsSource::new();
    let claim_id = registry().claim(session_id, &owner, source);
    let mut live: Option<(u64, PcmTx)> = None;
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(SourceCmd::Start { capture, tx }) => {
                    live = Some((capture, tx));
                    let start = format!("{{\"start\":{capture}}}");
                    if sink.send(Message::Text(start.into())).await.is_err() {
                        break;
                    }
                }
                Some(SourceCmd::Stop { capture }) => {
                    // Not live: the device was already told (`relay_pcm`).
                    if !live.as_ref().is_some_and(|(c, _)| *c == capture) {
                        continue;
                    }
                    live = None;
                    let stop = format!("{{\"stop\":{capture}}}");
                    if sink.send(Message::Text(stop.into())).await.is_err() {
                        break;
                    }
                }
                None => {
                    // The claim was replaced or released elsewhere.
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: CLOSE_CLAIMED_ELSEWHERE,
                            reason: "microphone claimed elsewhere".into(),
                        })))
                        .await;
                    break;
                }
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(pcm))) => {
                    if let Some(capture) = relay_pcm(&mut live, pcm.to_vec()) {
                        let stop = format!("{{\"stop\":{capture}}}");
                        if sink.send(Message::Text(stop.into())).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    registry().release(session_id, claim_id);
}

#[cfg(test)]
#[path = "voice_route_tests.rs"]
mod tests;
