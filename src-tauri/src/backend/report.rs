//! Flushes `fleet_core::logging::report_ring()` to the hub's `POST /report`.
//! Fire-and-forget: nothing awaits it, and its answers only steer itself.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

use crate::backend::remote::HubTransport;
use crate::backend::RemoteConfig;
use fleet_core::logging::redact;
use fleet_proto::report::{ReportBatch, ReportRing, HTTP_BATCH_MAX};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
pub const FLUSH_AT: usize = 20;
const POST_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_START: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flush {
    Nothing,
    Sent(usize),
    /// The hub predates the route or refuses this client: stop for this run.
    Stop,
    /// Keep the batch and wait this long.
    Backoff(Duration),
}

pub struct ReportFlusher {
    url: String,
    token: String,
    transport: Arc<dyn HubTransport>,
    ring: &'static ReportRing,
    /// A batch the last flush could not deliver, retried before the ring.
    held: Option<ReportBatch>,
    backoff: Duration,
}

impl ReportFlusher {
    pub fn new(
        cfg: RemoteConfig,
        transport: Arc<dyn HubTransport>,
        ring: &'static ReportRing,
    ) -> Self {
        ReportFlusher {
            url: format!("{}/report", cfg.base_url),
            token: cfg.token,
            transport,
            ring,
            held: None,
            backoff: BACKOFF_START,
        }
    }

    fn next_batch(&mut self) -> Option<ReportBatch> {
        if let Some(h) = self.held.take() {
            return Some(h);
        }
        let mut b = self.ring.drain(HTTP_BATCH_MAX);
        if b.reports.is_empty() && b.dropped == 0 {
            return None;
        }
        for r in &mut b.reports {
            r.message = redact(&r.message).into_owned();
            if let Some(ctx) = &r.context {
                if let Ok(text) = serde_json::to_string(ctx) {
                    let red = redact(&text);
                    if red != text {
                        r.context = serde_json::from_str(&red).ok();
                    }
                }
            }
        }
        Some(b)
    }

    fn back_off(&mut self, batch: ReportBatch) -> Flush {
        self.held = Some(batch);
        let wait = self.backoff;
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        Flush::Backoff(wait)
    }

    pub async fn flush_once(&mut self) -> Flush {
        let Some(batch) = self.next_batch() else {
            return Flush::Nothing;
        };
        let n = batch.reports.len();
        let body = match serde_json::to_string(&batch) {
            Ok(b) => b,
            Err(_) => return Flush::Nothing,
        };
        let sent = tokio::time::timeout(
            POST_TIMEOUT,
            self.transport.post_json(&self.url, &self.token, body),
        )
        .await;
        match sent {
            Ok(Ok(resp)) => match resp.status {
                204 => {
                    self.backoff = BACKOFF_START;
                    Flush::Sent(n)
                }
                404 => {
                    tracing::info!("[report] the hub has no /report route; not reporting this run");
                    Flush::Stop
                }
                401 | 403 => {
                    tracing::info!(
                        status = resp.status,
                        "[report] the hub refuses this client; not reporting this run"
                    );
                    Flush::Stop
                }
                429 => {
                    self.held = Some(batch);
                    Flush::Backoff(BACKOFF_MAX)
                }
                other => {
                    tracing::warn!(status = other, "[report] unexpected answer from /report");
                    self.back_off(batch)
                }
            },
            Ok(Err(e)) => {
                tracing::warn!(error = %redact(&e), "[report] could not reach the hub");
                self.back_off(batch)
            }
            Err(_) => {
                tracing::warn!("[report] no answer from /report within {POST_TIMEOUT:?}");
                self.back_off(batch)
            }
        }
    }

    /// Every `FLUSH_INTERVAL`, or as soon as `FLUSH_AT` are queued.
    pub async fn run(mut self, shutdown: CancellationToken) {
        let mut wait = FLUSH_INTERVAL;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
            match self.flush_once().await {
                Flush::Stop => return,
                Flush::Backoff(d) => wait = d,
                Flush::Nothing | Flush::Sent(_) => {
                    wait = if self.ring.len() >= FLUSH_AT {
                        Duration::ZERO
                    } else {
                        FLUSH_INTERVAL
                    };
                }
            }
        }
    }
}

pub fn spawn_report_flusher(flusher: ReportFlusher, shutdown: CancellationToken) {
    fleet_core::rt::spawn(flusher.run(shutdown));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::remote::HubResponse;
    use fleet_proto::report::{Report, ReportRing};
    use std::sync::Mutex;

    struct Fake {
        answers: Mutex<Vec<Result<HubResponse, String>>>,
        seen: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl HubTransport for Fake {
        async fn post_json(
            &self,
            url: &str,
            bearer: &str,
            body: String,
        ) -> Result<HubResponse, String> {
            assert_eq!(bearer, "cl_tok");
            self.seen.lock().unwrap().push((url.to_string(), body));
            self.answers.lock().unwrap().remove(0)
        }
    }

    fn ok() -> Result<HubResponse, String> {
        Ok(HubResponse {
            status: 204,
            body: String::new(),
        })
    }
    fn status(s: u16) -> Result<HubResponse, String> {
        Ok(HubResponse {
            status: s,
            body: String::new(),
        })
    }

    fn flusher(
        answers: Vec<Result<HubResponse, String>>,
        ring: &'static ReportRing,
    ) -> (ReportFlusher, Arc<Fake>) {
        let fake = Arc::new(Fake {
            answers: Mutex::new(answers),
            seen: Mutex::new(vec![]),
        });
        let cfg = RemoteConfig {
            base_url: "https://hub.example.com".into(),
            token: "cl_tok".into(),
            client_name: "desk".into(),
        };
        (ReportFlusher::new(cfg, fake.clone(), ring), fake)
    }

    fn ring() -> &'static ReportRing {
        Box::leak(Box::new(ReportRing::new()))
    }

    #[tokio::test]
    async fn an_empty_ring_sends_nothing() {
        let (mut f, fake) = flusher(vec![], ring());
        assert_eq!(f.flush_once().await, Flush::Nothing);
        assert!(fake.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_batch_is_posted_to_slash_report_redacted() {
        let r = ring();
        let mut rep = Report::error(
            "fleet_core::ssh",
            "Bearer 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef leaked",
        );
        rep.context = Some(
            serde_json::json!({ "url": "https://h/?token=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" }),
        );
        r.push(rep);
        let (mut f, fake) = flusher(vec![ok()], r);
        assert_eq!(f.flush_once().await, Flush::Sent(1));
        let (url, body) = fake.seen.lock().unwrap()[0].clone();
        assert_eq!(url, "https://hub.example.com/report");
        assert!(!body.contains("0123456789abcdef0123456789"), "{body}");
        assert!(body.contains("\"dropped\":0"));
    }

    #[tokio::test]
    async fn a_404_stops_and_a_429_holds_a_minute_keeping_the_batch() {
        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![status(404)], r);
        assert_eq!(f.flush_once().await, Flush::Stop);

        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![status(429), ok()], r);
        assert_eq!(
            f.flush_once().await,
            Flush::Backoff(Duration::from_secs(60))
        );
        assert_eq!(
            f.flush_once().await,
            Flush::Sent(1),
            "the held batch is retried"
        );
    }

    #[tokio::test]
    async fn a_transport_error_backs_off_doubling_to_a_minute() {
        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![Err("refused".into()); 6], r);
        let mut waits = vec![];
        for _ in 0..6 {
            match f.flush_once().await {
                Flush::Backoff(d) => waits.push(d.as_secs()),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
    }

    #[tokio::test]
    async fn a_401_stops_too() {
        let r = ring();
        r.push(Report::error("c", "m"));
        let (mut f, _) = flusher(vec![status(401)], r);
        assert_eq!(f.flush_once().await, Flush::Stop);
    }
}
