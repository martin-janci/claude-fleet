//! The smart-caching decision (cycle 2), kept pure so every ordering of
//! stored cursor, head and generation can be tested without a store.
//!
//! The one rule: a cursor that cannot be trusted yields the FULL payload and
//! a stated reason. Never a silent skip.

use crate::store::CursorRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    /// A conversation boundary happened since the last read.
    ConversationChanged,
    /// The cursor is past the head — in practice a reused session id.
    AheadOfHead,
    /// `fresh_for` names no session. Answered, stated, nothing stored.
    ReaderUnknown,
    /// A positional cursor (`session_transcript`'s anchor) named a spot the
    /// current read could not locate — outside the tail window, or the
    /// cursor never recorded one. `turn_seq`/generation alone said `After`,
    /// but the delta cannot be positioned, so it is answered full instead
    /// of guessing.
    TooFarBehind,
}

impl ResetReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ResetReason::ConversationChanged => "conversation_changed",
            ResetReason::AheadOfHead => "ahead_of_head",
            ResetReason::ReaderUnknown => "reader_unknown",
            ResetReason::TooFarBehind => "too_far_behind",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStart {
    Full(Option<ResetReason>),
    After(i64),
    Unchanged,
}

pub fn decide_stream(
    stored: Option<&CursorRow>,
    head: Option<i64>,
    generation: Option<i64>,
) -> StreamStart {
    let Some(c) = stored else {
        return StreamStart::Full(None);
    };
    let Some(w) = c.watermark else {
        // A snapshot row under a stream tool: treat as no cursor.
        return StreamStart::Full(None);
    };
    if c.generation != generation {
        return StreamStart::Full(Some(ResetReason::ConversationChanged));
    }
    match head {
        None => StreamStart::Full(Some(ResetReason::AheadOfHead)),
        Some(h) if w > h => StreamStart::Full(Some(ResetReason::AheadOfHead)),
        Some(h) if w == h => StreamStart::Unchanged,
        Some(_) => StreamStart::After(w),
    }
}

pub fn snapshot_hash(payload: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(payload.as_bytes()))
}

pub fn envelope(
    unchanged: bool,
    reset: Option<ResetReason>,
    more: bool,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "unchanged": unchanged,
        "cursor_reset": reset.map(ResetReason::as_str),
        "more": more,
        "data": data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CursorRow;

    fn cur(w: i64, g: Option<i64>) -> CursorRow {
        CursorRow {
            watermark: Some(w),
            generation: g,
            content_hash: None,
            anchor: None,
        }
    }

    #[test]
    fn no_cursor_is_a_full_read_with_no_reset() {
        assert_eq!(decide_stream(None, Some(9), None), StreamStart::Full(None));
    }

    #[test]
    fn a_cursor_at_the_head_is_unchanged() {
        assert_eq!(
            decide_stream(Some(&cur(9, None)), Some(9), None),
            StreamStart::Unchanged
        );
    }

    #[test]
    fn a_cursor_behind_the_head_reads_after_it() {
        assert_eq!(
            decide_stream(Some(&cur(4, None)), Some(9), None),
            StreamStart::After(4)
        );
    }

    /// Session-id reuse (`sessions.id` has no AUTOINCREMENT): a new session
    /// starts at turn_seq 0 while an old cursor says 50.
    #[test]
    fn a_cursor_ahead_of_the_head_resets() {
        assert_eq!(
            decide_stream(Some(&cur(50, None)), Some(3), None),
            StreamStart::Full(Some(ResetReason::AheadOfHead))
        );
    }

    #[test]
    fn a_cursor_on_an_empty_stream_resets_rather_than_claiming_unchanged() {
        assert_eq!(
            decide_stream(Some(&cur(5, None)), None, None),
            StreamStart::Full(Some(ResetReason::AheadOfHead))
        );
    }

    /// The /clear case: turn_seq kept counting, the transcript file changed.
    #[test]
    fn a_moved_generation_resets_even_when_the_watermark_looks_current() {
        assert_eq!(
            decide_stream(Some(&cur(9, Some(1))), Some(9), Some(2)),
            StreamStart::Full(Some(ResetReason::ConversationChanged))
        );
        assert_eq!(
            decide_stream(Some(&cur(4, Some(1))), Some(9), Some(2)),
            StreamStart::Full(Some(ResetReason::ConversationChanged)),
            "a generation change outranks a readable delta"
        );
    }

    #[test]
    fn a_first_boundary_after_a_cursor_with_none_resets() {
        assert_eq!(
            decide_stream(Some(&cur(4, None)), Some(9), Some(7)),
            StreamStart::Full(Some(ResetReason::ConversationChanged))
        );
    }

    /// Deliberate: tools with no generation pass None on both sides.
    #[test]
    fn none_and_none_is_the_same_generation() {
        assert_eq!(
            decide_stream(Some(&cur(4, None)), Some(9), None),
            StreamStart::After(4)
        );
    }

    #[test]
    fn a_snapshot_cursor_is_not_a_stream_cursor() {
        let snap = CursorRow {
            watermark: None,
            generation: None,
            content_hash: Some("h".into()),
            anchor: None,
        };
        assert_eq!(
            decide_stream(Some(&snap), Some(9), None),
            StreamStart::Full(None)
        );
    }

    #[test]
    fn the_hash_is_stable_and_sensitive() {
        assert_eq!(snapshot_hash("[1,2]"), snapshot_hash("[1,2]"));
        assert_ne!(snapshot_hash("[1,2]"), snapshot_hash("[1,3]"));
        assert_eq!(snapshot_hash("x").len(), 64, "hex sha-256");
    }

    #[test]
    fn the_envelope_carries_every_flag() {
        let v = envelope(
            false,
            Some(ResetReason::ReaderUnknown),
            true,
            serde_json::json!([1]),
        );
        assert_eq!(v["unchanged"], false);
        assert_eq!(v["cursor_reset"], "reader_unknown");
        assert_eq!(v["more"], true);
        assert_eq!(v["data"], serde_json::json!([1]));
        let quiet = envelope(true, None, false, serde_json::Value::Null);
        assert!(quiet["cursor_reset"].is_null());
    }
}
