//! Reading what the control API's `/mcp` writes back.
//!
//! The streamable-HTTP transport keeps SSE framing even for a one-shot
//! `tools/call` (that is what carries the keep-alive on long polls), so the
//! JSON-RPC envelope arrives on `data:` lines rather than as the body. Every
//! client of a fleet hub has to undo that framing, and there are two in this
//! repository — `fleet-hub`'s operator CLI (`pair.rs`) and the desktop's
//! hub-client backend. This module is the one implementation they share, so
//! a fix to the de-chunking cannot land in one and not the other.
//!
//! What sits *above* the framing — how an HTTP status, a JSON-RPC `error` or
//! an `isError` result becomes that caller's error type — deliberately stays
//! with each caller: the CLI wants an operator sentence, the desktop wants an
//! `IpcError` with the hub's own `E_*` code.

/// The payload of the LAST server-sent event in `body`, or the trimmed body
/// itself when there is no `data:` line (a `json_response` transport).
///
/// This de-chunks properly rather than assuming one event per response and
/// one line per event: per the SSE grammar a blank line ends an event and an
/// event may spread its payload over several `data:` lines, which are joined
/// with `\n`. A keep-alive comment (`: ping`) or a second frame ahead of the
/// answer is therefore skipped, and a long envelope split across `data:`
/// lines is reassembled instead of being truncated to its tail.
pub fn last_event_payload(body: &str) -> String {
    let mut events: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                events.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            // The spec strips ONE leading space after the colon, no more.
            current.push(data.strip_prefix(' ').unwrap_or(data));
        }
        // Every other field (`event:`, `id:`, a `:` comment) is not payload.
    }
    if !current.is_empty() {
        events.push(current.join("\n"));
    }
    events.pop().unwrap_or_else(|| body.trim().to_string())
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
}
