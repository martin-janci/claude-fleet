//! Reading what the control API writes back.
//!
//! The streamable-HTTP transport keeps SSE framing even for a one-shot
//! `tools/call` (that is what carries the keep-alive on long polls), so the
//! JSON-RPC envelope arrives on `data:` lines rather than as the body. Every
//! client of a fleet hub has to undo that framing, and there are three uses in
//! this repository — `fleet-hub`'s operator CLI (`pair.rs`), the desktop's
//! hub-client backend, and that backend's `GET /events` bridge. This module is
//! the one implementation they share, so a fix to the de-chunking cannot land
//! in one and not the others.
//!
//! [`SseDecoder`] is the framing; [`last_event_payload`] is the one-shot
//! convenience built on it. The difference that matters is *time*: a
//! `tools/call` body arrives whole, while `/events` is open for hours and its
//! frames have to come out one at a time, so the decoder keeps the partial
//! tail of whatever has arrived and hands back only the frames a chunk
//! completed.
//!
//! What sits *above* the framing — how an HTTP status, a JSON-RPC `error` or
//! an `isError` result becomes that caller's error type — deliberately stays
//! with each caller: the CLI wants an operator sentence, the desktop wants an
//! `IpcError` with the hub's own `E_*` code.

/// One server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    /// The `event:` field. SSE's default when the server sent none is
    /// `message`, which is what `rmcp` uses for a `tools/call` answer; the
    /// `/events` route always names its frames (`session:updated`, `ready`,
    /// `lagged`).
    pub name: String,
    /// The `data:` field(s), joined with `\n`. Never empty: an event whose
    /// data buffer is empty is not dispatched, per the SSE spec.
    pub data: String,
}

/// Default `event:` name per the SSE spec, when a frame carries none.
pub const DEFAULT_EVENT_NAME: &str = "message";

/// Incremental SSE framing: [`feed`](SseDecoder::feed) whatever bytes arrived,
/// take out whatever frames they completed.
///
/// Written to the grammar rather than to the shape `rmcp` happens to emit:
/// a blank line dispatches an event, one event may spread its payload over
/// several `data:` lines (joined with `\n`), a line beginning with `:` is a
/// comment (the keep-alive), and a field with no value is legal. Crucially a
/// chunk boundary may fall anywhere — including the middle of a `data:` line
/// — so the decoder holds the unterminated tail rather than treating it as a
/// line.
#[derive(Default)]
pub struct SseDecoder {
    /// Everything fed in that is not yet a complete line.
    partial: String,
    /// `data:` values of the event being assembled.
    data: Vec<String>,
    /// Its `event:` name, if one was given.
    name: Option<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk of the stream; returns every frame it completed, in
    /// order. A chunk that completes no frame returns an empty vector, which
    /// is the normal answer for a keep-alive comment or a split line.
    pub fn feed(&mut self, chunk: &str) -> Vec<SseFrame> {
        self.partial.push_str(chunk);
        // Taken out of `self` so the loop can borrow it while calling
        // `&mut self` methods; whatever follows the last '\n' goes back.
        let buf = std::mem::take(&mut self.partial);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(off) = buf[start..].find('\n') {
            let end = start + off;
            let line = &buf[start..end];
            if let Some(frame) = self.line(line.strip_suffix('\r').unwrap_or(line)) {
                out.push(frame);
            }
            start = end + 1;
        }
        self.partial = buf[start..].to_string();
        out
    }

    /// The stream ended. An event that was never terminated by a blank line
    /// is still dispatched, which is what makes a whole-body decode
    /// ([`last_event_payload`]) see the last frame of a response whose body
    /// ends without one.
    pub fn flush(&mut self) -> Option<SseFrame> {
        let tail = std::mem::take(&mut self.partial);
        if !tail.is_empty() {
            // Only a blank line dispatches, and this one is not blank, so
            // this can only add a field to the event being assembled.
            let _ = self.line(tail.strip_suffix('\r').unwrap_or(&tail));
        }
        self.dispatch()
    }

    /// Consume one complete line.
    fn line(&mut self, line: &str) -> Option<SseFrame> {
        if line.is_empty() {
            return self.dispatch();
        }
        // A line starting with ':' is a comment — the keep-alive beat.
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            // The spec strips ONE leading space after the colon, no more.
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            // A field with no colon has an empty value.
            None => (line, ""),
        };
        match field {
            "data" => self.data.push(value.to_string()),
            "event" => self.name = Some(value.to_string()),
            // `id` and `retry` are reconnection bookkeeping this client does
            // not use (it re-lists after a gap rather than replaying), and an
            // unknown field is ignored by the spec.
            _ => {}
        }
        None
    }

    /// End the event being assembled. Per the spec an event with an empty
    /// data buffer is NOT dispatched — that is what makes a lone `event:` line
    /// or a stray comment produce nothing.
    fn dispatch(&mut self) -> Option<SseFrame> {
        let name = self.name.take();
        if self.data.is_empty() {
            return None;
        }
        Some(SseFrame {
            name: name.unwrap_or_else(|| DEFAULT_EVENT_NAME.to_string()),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

/// The payload of the LAST server-sent event in `body`, or the trimmed body
/// itself when there is no `data:` line (a `json_response` transport).
///
/// A whole-body [`SseDecoder`], so the one-shot and streaming paths cannot
/// drift: a keep-alive comment (`: ping`) or a second frame ahead of the
/// answer is skipped, and a long envelope split across `data:` lines is
/// reassembled instead of being truncated to its tail.
pub fn last_event_payload(body: &str) -> String {
    let mut decoder = SseDecoder::new();
    let mut frames = decoder.feed(body);
    frames.extend(decoder.flush());
    frames
        .pop()
        .map(|f| f.data)
        .unwrap_or_else(|| body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_frame_is_unwrapped() {
        assert_eq!(
            last_event_payload("event: message\ndata: {\"a\":1}\n\n"),
            "{\"a\":1}"
        );
    }

    #[test]
    fn a_body_with_no_data_line_is_returned_as_is() {
        assert_eq!(last_event_payload("  {\"a\":1}  "), "{\"a\":1}");
    }

    #[test]
    fn a_keep_alive_comment_and_an_earlier_frame_are_skipped() {
        let body = ": ping\n\nevent: message\ndata: first\n\nevent: message\ndata: second\n\n";
        assert_eq!(last_event_payload(body), "second");
    }

    #[test]
    fn one_events_data_lines_are_joined_with_a_newline() {
        assert_eq!(last_event_payload("data: a\ndata: b\n\n"), "a\nb");
    }

    /// The spec strips exactly one space after the colon.
    #[test]
    fn only_one_leading_space_is_stripped() {
        assert_eq!(last_event_payload("data:  x\n\n"), " x");
        assert_eq!(last_event_payload("data:x\n\n"), "x");
    }

    // --- SseDecoder: the same framing, one chunk at a time ------------------

    fn frame(name: &str, data: &str) -> SseFrame {
        SseFrame {
            name: name.to_string(),
            data: data.to_string(),
        }
    }

    #[test]
    fn a_frame_carries_its_event_name() {
        let mut d = SseDecoder::new();
        assert_eq!(
            d.feed("event: session:updated\ndata: {\"id\":1}\n\n"),
            vec![frame("session:updated", "{\"id\":1}")]
        );
    }

    /// The name is what `last_event_payload` throws away, and the whole
    /// reason the bridge cannot use it.
    #[test]
    fn a_frame_with_no_event_field_is_named_message() {
        let mut d = SseDecoder::new();
        assert_eq!(d.feed("data: x\n\n"), vec![frame(DEFAULT_EVENT_NAME, "x")]);
    }

    /// THE case a whole-body decoder never meets: a chunk boundary inside a
    /// `data:` line. Split naively, `{"id"` would be parsed as a line of its
    /// own and the payload silently corrupted.
    #[test]
    fn a_line_split_across_chunks_is_rejoined() {
        let mut d = SseDecoder::new();
        assert!(d.feed("event: session:up").is_empty());
        assert!(d.feed("dated\ndata: {\"id\"").is_empty());
        assert!(d.feed(":1,\"a\":2}").is_empty());
        assert_eq!(
            d.feed("\n\n"),
            vec![frame("session:updated", "{\"id\":1,\"a\":2}")]
        );
    }

    #[test]
    fn one_chunk_can_complete_several_frames() {
        let mut d = SseDecoder::new();
        assert_eq!(
            d.feed("event: a\ndata: 1\n\nevent: b\ndata: 2\n\n"),
            vec![frame("a", "1"), frame("b", "2")]
        );
    }

    /// The 15 s beat `/events` sends on an idle stream. It must produce
    /// nothing at all — not an empty frame, and not an end to the event being
    /// assembled.
    #[test]
    fn a_keep_alive_comment_produces_no_frame() {
        let mut d = SseDecoder::new();
        assert!(d.feed(": ping\n").is_empty());
        assert!(d.feed(":\n").is_empty());
        assert_eq!(d.feed("event: a\ndata: 1\n\n"), vec![frame("a", "1")]);
    }

    /// Axum writes `\r\n`-terminated headers and hyper may too; `str::lines`
    /// hid this from the whole-body path.
    #[test]
    fn crlf_terminated_lines_frame_the_same() {
        let mut d = SseDecoder::new();
        assert_eq!(d.feed("event: a\r\ndata: 1\r\n\r\n"), vec![frame("a", "1")]);
    }

    /// An event name with no data is not an event.
    #[test]
    fn an_event_with_an_empty_data_buffer_is_not_dispatched() {
        let mut d = SseDecoder::new();
        assert!(d.feed("event: a\n\n").is_empty());
        assert!(d.flush().is_none());
    }

    /// A name that arrived before an un-dispatched event must not leak onto
    /// the next one.
    #[test]
    fn an_abandoned_event_name_does_not_leak_forward() {
        let mut d = SseDecoder::new();
        assert!(d.feed("event: stale\n\n").is_empty());
        assert_eq!(d.feed("data: 1\n\n"), vec![frame(DEFAULT_EVENT_NAME, "1")]);
    }

    #[test]
    fn flush_dispatches_an_event_the_stream_never_terminated() {
        let mut d = SseDecoder::new();
        assert!(d.feed("event: a\ndata: 1").is_empty());
        assert_eq!(d.flush(), Some(frame("a", "1")));
        assert_eq!(d.flush(), None, "flushing twice yields nothing");
    }
}
