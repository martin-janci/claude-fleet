//! Error reports: the record a participant sends the hub, the batch it
//! travels in, and the bounded queue every sender keeps.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const COMPONENT_MAX: usize = 64;
pub const MESSAGE_MAX: usize = 2_048;
/// Bytes of the serialized `context`.
pub const CONTEXT_MAX: usize = 4_096;
/// Reports per `POST /report`.
pub const HTTP_BATCH_MAX: usize = 50;
/// Reports per `AgentFrame::Report`.
pub const FRAME_BATCH_MAX: usize = 16;
/// The most an agent puts in one `report` frame, in encoded bytes.
///
/// Chosen to fit under the hub's smallest inbound allowance
/// (`MIN_INBOUND_BYTES`, 64 KiB — the floor the hub decodes agent frames
/// against when no `exec` is in flight), with headroom for the envelope.
/// [`FRAME_BATCH_MAX`] reports at [`MESSAGE_MAX`] and [`CONTEXT_MAX`] are far
/// larger than this, so a sender batches to fit it rather than to the count
/// alone: a frame past the hub's allowance is not truncated, it closes the
/// connection.
pub const REPORT_FRAME_BYTES: usize = 56 * 1024;
/// Reports a sender queues before dropping the oldest.
pub const RING_CAP: usize = 256;
/// Bytes of a `POST /report` body.
pub const BODY_MAX: usize = 64 * 1024;
/// Reports one origin may store per minute.
pub const RATE_PER_MINUTE: u32 = 60;

/// One error a participant reports. Every string is bounded by
/// [`Report::clamp`], which every sender calls before queueing and the hub
/// calls again at ingest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The sender's clock, unix seconds.
    pub at: i64,
    /// `"error"` or `"warn"`.
    pub level: String,
    /// The tracing target, or `frontend` / `frontend:unhandled`.
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub truncated: bool,
}

/// What travels: the reports and how many the sender dropped since its
/// previous batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportBatch {
    #[serde(default)]
    pub reports: Vec<Report>,
    #[serde(default)]
    pub dropped: u32,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Cut `s` to at most `max` chars, on a char boundary. Returns whether it cut.
fn cut(s: &mut String, max: usize) -> bool {
    match s.char_indices().nth(max) {
        Some((i, _)) => {
            s.truncate(i);
            true
        }
        None => false,
    }
}

impl Report {
    pub fn error(component: &str, message: &str) -> Self {
        Report {
            at: now_unix(),
            level: "error".into(),
            component: component.into(),
            code: None,
            message: message.into(),
            context: None,
            truncated: false,
        }
    }

    /// Apply the caps in place, setting `truncated` when anything was cut.
    pub fn clamp(&mut self) {
        let mut cut_any = cut(&mut self.message, MESSAGE_MAX);
        cut_any |= cut(&mut self.component, COMPONENT_MAX);
        if let Some(c) = &mut self.code {
            cut_any |= cut(c, COMPONENT_MAX);
        }
        if let Some(ctx) = &self.context {
            let bytes = serde_json::to_vec(ctx)
                .map(|v| v.len())
                .unwrap_or(usize::MAX);
            if bytes > CONTEXT_MAX {
                self.context = None;
                cut_any = true;
            }
        }
        self.truncated |= cut_any;
    }
}

/// A bounded queue of reports. The critical section is a push or a pop:
/// nothing here formats, allocates unpredictably or logs, because a tracing
/// layer calls [`ReportRing::push`] from inside an event.
#[derive(Debug, Default)]
pub struct ReportRing {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    queue: VecDeque<Report>,
    dropped: u32,
}

impl ReportRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, report: Report) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.queue.len() >= RING_CAP {
            g.queue.pop_front();
            g.dropped = g.dropped.saturating_add(1);
        }
        g.queue.push_back(report);
    }

    /// Up to `n` oldest reports and the drop count since the last drain.
    pub fn drain(&self, n: usize) -> ReportBatch {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let take = n.min(g.queue.len());
        let reports = g.queue.drain(..take).collect();
        let dropped = std::mem::take(&mut g.dropped);
        ReportBatch { reports, dropped }
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(msg: &str) -> Report {
        Report::error("fleet_core::ssh", msg)
    }

    #[test]
    fn clamp_cuts_the_message_at_a_char_boundary_and_marks_it() {
        let mut x = r(&"é".repeat(MESSAGE_MAX + 5));
        x.clamp();
        assert!(x.truncated);
        assert_eq!(x.message.chars().count(), MESSAGE_MAX);
        assert!(x.message.is_char_boundary(x.message.len()));
    }

    #[test]
    fn clamp_drops_an_oversize_context_and_a_long_component() {
        let mut x = r("m");
        x.context = Some(serde_json::json!({ "stack": "x".repeat(CONTEXT_MAX) }));
        x.component = "c".repeat(COMPONENT_MAX + 1);
        x.clamp();
        assert!(x.context.is_none());
        assert!(x.truncated);
        assert_eq!(x.component.len(), COMPONENT_MAX);
    }

    #[test]
    fn clamp_leaves_a_small_report_alone() {
        let mut x = r("fine");
        x.context = Some(serde_json::json!({ "k": 1 }));
        x.clamp();
        assert!(!x.truncated);
        assert_eq!(x.context, Some(serde_json::json!({ "k": 1 })));
    }

    #[test]
    fn the_ring_evicts_the_oldest_and_counts_drops() {
        let ring = ReportRing::new();
        for i in 0..(RING_CAP + 3) {
            ring.push(r(&format!("m{i}")));
        }
        assert_eq!(ring.len(), RING_CAP);
        let b = ring.drain(2);
        assert_eq!(b.dropped, 3);
        assert_eq!(b.reports[0].message, "m3", "the oldest survivor first");
        assert_eq!(b.reports[1].message, "m4");
        assert_eq!(ring.len(), RING_CAP - 2);
        assert_eq!(ring.drain(1).dropped, 0, "the drop count resets");
    }

    /// A report at the caps, in a multi-byte alphabet: what the frame cap
    /// has to survive.
    fn fat() -> Report {
        let mut x = r(&"\u{20ac}".repeat(MESSAGE_MAX));
        x.context = Some(serde_json::json!({ "stack": "s".repeat(4_000 - 14) }));
        x.clamp();
        assert!(
            !x.truncated,
            "the fixture must sit under the caps, not over"
        );
        x
    }

    /// The hazard [`REPORT_FRAME_BYTES`] exists for: a full [`FRAME_BATCH_MAX`]
    /// of clamped reports does NOT fit the hub's idle inbound allowance, so an
    /// agent batching by count alone would have its connection closed. One
    /// such report fits with room to spare.
    #[test]
    fn a_full_frame_batch_of_clamped_reports_overruns_the_frame_cap() {
        let one = crate::AgentFrame::Report {
            reports: vec![fat()],
            dropped: 0,
        };
        let len = crate::encode_agent_frame(&one).unwrap().len();
        assert!(
            len < REPORT_FRAME_BYTES,
            "one clamped report must fit ({len} bytes)"
        );
        let full = crate::AgentFrame::Report {
            reports: (0..FRAME_BATCH_MAX).map(|_| fat()).collect(),
            dropped: 0,
        };
        let len = crate::encode_agent_frame(&full).unwrap().len();
        assert!(
            len > REPORT_FRAME_BYTES,
            "{FRAME_BATCH_MAX} clamped reports must overrun the cap ({len} bytes)"
        );
    }

    #[test]
    fn a_batch_round_trips_as_json() {
        let b = ReportBatch {
            reports: vec![r("x")],
            dropped: 2,
        };
        let text = serde_json::to_string(&b).unwrap();
        let back: ReportBatch = serde_json::from_str(&text).unwrap();
        assert_eq!(back, b);
        let sparse: ReportBatch = serde_json::from_str(r#"{"reports":[]}"#).unwrap();
        assert_eq!(sparse.dropped, 0, "dropped defaults");
    }
}
