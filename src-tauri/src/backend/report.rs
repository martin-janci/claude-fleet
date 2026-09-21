//! Flushes `fleet_core::logging::report_ring()` to the hub's `POST /report`.
//! Fire-and-forget: nothing awaits it, and its answers only steer itself.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md
//!
//! What the hub's answer does to the flusher:
//!
//! | Answer | Then |
//! |---|---|
//! | `204` | Sent. Reset the backoff. |
//! | `404` | The hub predates this route: log once at `info`, stop the flusher for this run. |
//! | `401` / `403` | The token is dead or the client is refused: log once, stop. |
//! | `429` | Over budget: keep the batch, wait one full minute before the next flush. |
//! | Other `400..=499` | The hub deterministically refused this exact batch (e.g. `400` from validation, `413` from its own body-size limit) — retrying it forever would poison the flusher, so it is discarded (one `warn`, never the body) and the backoff resets. |
//! | `5xx`, transport error, or no answer within the timeout | Keep the batch (the ring caps it), back off 5 s → 10 s → 20 s → 40 s → 60 s until an answer. |
//!
//! A batch that would itself cross the hub's `BODY_MAX` (a drain of up to
//! `HTTP_BATCH_MAX` clamped reports can, at roughly 6 KiB apiece) is split
//! before it is ever posted: [`ReportFlusher::next_batch`] trims reports off
//! the end until what is left fits, and carries the trimmed tail to go out
//! ahead of anything drained later — see `carry` below.

use crate::backend::remote::HubTransport;
use crate::backend::RemoteConfig;
use fleet_core::logging::redact;
use fleet_proto::report::{Report, ReportBatch, ReportRing, BODY_MAX, HTTP_BATCH_MAX};
use std::collections::VecDeque;
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
    /// The hub deterministically refused this batch — a `400..=499` other
    /// than `401`/`403`/`404`/`429`, which would otherwise repost the same
    /// doomed batch forever and starve every report behind it. Dropped, not
    /// retried.
    Discarded(usize),
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
    /// A batch the last flush could not deliver, retried before anything
    /// else — it already went through redaction and the body-size split, so
    /// it is reposted exactly as built.
    held: Option<ReportBatch>,
    /// The tail trimmed off an oversize batch by [`Self::next_batch`],
    /// oldest first. Consumed before a fresh ring drain, so a report that
    /// arrived earlier is never overtaken by one the ring handed out later.
    carry: VecDeque<Report>,
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
            carry: VecDeque::new(),
            backoff: BACKOFF_START,
        }
    }

    /// Whether `batch`, as JSON, fits the hub's `POST /report` body limit.
    /// An encode failure is not this function's problem to solve — treat it
    /// as fitting so the caller's trim loop terminates and `flush_once`
    /// reports the (harmless) encode error itself.
    fn fits(batch: &ReportBatch) -> bool {
        serde_json::to_string(batch)
            .map(|s| s.len() <= BODY_MAX)
            .unwrap_or(true)
    }

    fn next_batch(&mut self) -> Option<ReportBatch> {
        if let Some(h) = self.held.take() {
            return Some(h);
        }
        let (reports, dropped) = if !self.carry.is_empty() {
            (self.carry.drain(..).collect::<Vec<_>>(), 0)
        } else {
            let b = self.ring.drain(HTTP_BATCH_MAX);
            if b.reports.is_empty() && b.dropped == 0 {
                return None;
            }
            let mut reports = b.reports;
            for r in &mut reports {
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
            (reports, b.dropped)
        };

        // A single clamped report is well under BODY_MAX (`Report::clamp`
        // bounds it to roughly 6.3 KiB), so this always terminates with at
        // least one report left in `batch`.
        let mut batch = ReportBatch { reports, dropped };
        while batch.reports.len() > 1 && !Self::fits(&batch) {
            if let Some(tail) = batch.reports.pop() {
                self.carry.push_front(tail);
            }
        }
        Some(batch)
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
                s @ 400..=499 => {
                    // Deterministic for this exact body (bad shape, or over
                    // the hub's own size limit): holding it would repost the
                    // same doomed batch every backoff forever and starve
                    // every report queued behind it. Never log the body.
                    tracing::warn!(
                        status = s,
                        batch_size = n,
                        "[report] the hub deterministically refused this batch; discarding it"
                    );
                    self.backoff = BACKOFF_START;
                    Flush::Discarded(n)
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
                Flush::Nothing | Flush::Sent(_) | Flush::Discarded(_) => {
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
    use fleet_proto::report::ReportRing;
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

    /// The regression this fix round exists for: without it, a `400` or
    /// `413` — deterministic for this exact body — would be held and
    /// reposted every backoff forever, poisoning the flusher.
    #[tokio::test]
    async fn a_400_or_413_discards_the_batch_and_keeps_draining() {
        let r = ring();
        r.push(Report::error("c", "m1"));
        let (mut f, fake) = flusher(vec![status(413), ok()], r);
        assert_eq!(f.flush_once().await, Flush::Discarded(1));

        r.push(Report::error("c", "m2"));
        assert_eq!(
            f.flush_once().await,
            Flush::Sent(1),
            "the discard did not wedge the flusher behind it"
        );

        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_ne!(
            seen[0].1, seen[1].1,
            "the second post must not just replay the discarded batch"
        );
    }

    #[tokio::test]
    async fn an_oversize_drain_is_split_under_the_body_cap_and_the_tail_goes_first_next_time() {
        let r = ring();
        for i in 0..50u32 {
            // A numbered message so push order is recoverable from the
            // posted bodies, padded out toward `MESSAGE_MAX` so 50 of these
            // cannot possibly fit one `BODY_MAX` body.
            let mut rep = Report::error("c", &format!("{i:03}-{}", "x".repeat(1996)));
            rep.context = Some(serde_json::json!({ "s": "x".repeat(3900) }));
            rep.clamp();
            r.push(rep);
        }
        // More answers than any plausible split count needs; the loop below
        // stops at `Flush::Nothing` well before they run out.
        let (mut f, fake) = flusher(std::iter::repeat_with(ok).take(20).collect(), r);

        let mut posted_bodies = vec![];
        loop {
            match f.flush_once().await {
                Flush::Nothing => break,
                Flush::Sent(_) => {
                    posted_bodies.push(fake.seen.lock().unwrap().last().unwrap().1.clone());
                }
                other => panic!("unexpected {other:?}"),
            }
        }

        assert!(
            posted_bodies.len() > 1,
            "50 reports at ~6 KiB apiece must not fit one BODY_MAX body"
        );
        let mut all_indices = vec![];
        for body in &posted_bodies {
            assert!(
                body.len() <= BODY_MAX,
                "a posted body of {} bytes exceeds BODY_MAX ({BODY_MAX})",
                body.len()
            );
            let batch: ReportBatch = serde_json::from_str(body).unwrap();
            for r in &batch.reports {
                let idx: u32 = r.message[..3].parse().expect("numbered prefix");
                all_indices.push(idx);
            }
        }
        assert_eq!(
            all_indices.len(),
            50,
            "every pushed report must be posted exactly once"
        );
        assert_eq!(
            all_indices,
            (0..50).collect::<Vec<_>>(),
            "reports must go out in push order across posts, tail-of-a-split first"
        );
    }
}
