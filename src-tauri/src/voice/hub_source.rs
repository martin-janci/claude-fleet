//! A hub-paired desktop's microphone: hold `/voice/source` open on the hub
//! (that is the claim), answer `{"start":n}` by opening the microphone and
//! sending binary PCM, `{"stop":n}` by closing it.

use fleet_core::service::voice::{VoiceSource, PCM_QUEUE};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http, Message};

/// The close code the hub sends when this claim was replaced or released
/// elsewhere (`mcp/voice_route.rs`).
const CLOSE_CLAIMED_ELSEWHERE: u16 = 4001;

/// How a socket that did not fail came to an end.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// This window released or replaced the claim (`stop`).
    Released,
    /// Another device claimed the session's microphone on the hub.
    ClaimedElsewhere,
    /// The hub closed the socket for any other reason.
    HubClosed,
}

/// The websocket upgrade for `/voice/source` under the hub's `base_url`
/// (scheme, authority and any path prefix, no trailing slash).
pub fn upgrade_request(
    base_url: &str,
    token: &str,
    session_id: i64,
) -> Result<http::Request<()>, String> {
    let ws = if let Some(rest) = base_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(format!("not a hub address: {base_url}"));
    };
    let mut req = format!("{ws}/voice/source?session_id={session_id}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    req.headers_mut().insert(
        http::header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .map_err(|_| "the hub token is not a valid header value".to_string())?,
    );
    Ok(req)
}

/// Hold the claim until `stop` fires or the hub ends the socket.
pub async fn run(
    base_url: String,
    token: String,
    session_id: i64,
    source: std::sync::Arc<dyn VoiceSource>,
    stop: tokio_util::sync::CancellationToken,
) -> Result<Ended, String> {
    let request = upgrade_request(&base_url, &token, session_id)?;
    let at = fleet_core::http_client::Endpoint::parse(&base_url)?;
    let io = tokio::select! {
        _ = stop.cancelled() => return Ok(Ended::Released),
        io = fleet_core::http_client::connect(&at) => io?,
    };
    let (ws, _) = tokio::select! {
        _ = stop.cancelled() => return Ok(Ended::Released),
        ws = tokio_tungstenite::client_async(request, io) => {
            ws.map_err(|e| format!("the hub refused the microphone: {e}"))?
        }
    };
    let (mut sink, mut stream) = ws.split();
    let (pcm_tx, mut pcm_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PCM_QUEUE);
    // The open microphone; dropping it closes the device.
    let mut guard: Option<Box<dyn Send>> = None;
    loop {
        tokio::select! {
            _ = stop.cancelled() => {
                drop(guard.take());
                let _ = sink.send(Message::Close(None)).await;
                return Ok(Ended::Released);
            }
            Some(pcm) = pcm_rx.recv(), if guard.is_some() => {
                sink.send(Message::Binary(pcm.into())).await.map_err(|e| e.to_string())?;
            }
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let v: serde_json::Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                    if v.get("start").is_some() {
                        // One capture at a time: close any earlier one first.
                        drop(guard.take());
                        let src = std::sync::Arc::clone(&source);
                        let tx = pcm_tx.clone();
                        // Opening the device can block for seconds.
                        let started = tokio::task::spawn_blocking(move || src.start(tx))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r);
                        match started {
                            Ok(g) => guard = Some(g),
                            Err(e) => {
                                let _ = sink.send(Message::Close(None)).await;
                                return Err(e);
                            }
                        }
                    } else if v.get("stop").is_some() {
                        drop(guard.take());
                        // What the closed capture left queued is not the next one's.
                        while pcm_rx.try_recv().is_ok() {}
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    return Ok(match frame {
                        Some(f) if u16::from(f.code) == CLOSE_CLAIMED_ELSEWHERE => {
                            Ended::ClaimedElsewhere
                        }
                        _ => Ended::HubClosed,
                    });
                }
                None => return Ok(Ended::HubClosed),
                Some(Err(e)) => return Err(e.to_string()),
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upgrade_request_targets_voice_source_with_the_bearer() {
        let req = upgrade_request("https://hub.example:8443/prefix", "tok", 42).unwrap();
        assert_eq!(
            req.uri().to_string(),
            "wss://hub.example:8443/prefix/voice/source?session_id=42"
        );
        assert_eq!(req.headers()[http::header::AUTHORIZATION], "Bearer tok");
    }

    #[test]
    fn a_plain_http_hub_is_dialled_over_ws_and_anything_else_is_refused() {
        let req = upgrade_request("http://127.0.0.1:7777", "tok", 1).unwrap();
        assert_eq!(
            req.uri().to_string(),
            "ws://127.0.0.1:7777/voice/source?session_id=1"
        );
        assert!(upgrade_request("ftp://hub", "tok", 1).is_err());
    }
}
