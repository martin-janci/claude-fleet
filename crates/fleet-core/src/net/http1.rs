//! HTTP/1.1 response-head parsing and chunked-transfer decoding, shared by
//! every hand-rolled client in the workspace: the desktop's one-shot
//! `POST /mcp` (`src-tauri/src/backend/remote.rs`), its long-lived
//! `GET /events` (`backend/events.rs`), and [`super::https`], the outbound
//! client trackers use (work graph M3.0).
//!
//! What each caller does differently is how it gets its bytes — a one-shot
//! call reads until the peer closes, a stream reads only up to the blank line
//! that ends the head and then keeps the socket open — so the read loops stay
//! with each caller. This module owns the part that is genuinely identical:
//! parsing bytes once they are in hand.

/// First offset of `needle` in `haystack`.
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The numeric status code out of an HTTP/1.x response head's status line —
/// the token after the HTTP version, not a substring match: a reason phrase
/// or a header that happens to carry " 200" must not count.
pub fn parse_status(head: &str) -> Result<u16, String> {
    let status_line = head.lines().next().unwrap_or_default();
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("unreadable status line: {status_line:?}"))
}

/// Every header of a response head, in order, names as written. The status
/// line is skipped; a line with no `:` (a folded continuation, which HTTP/1.1
/// deprecates) is ignored rather than guessed at.
pub fn parse_headers(head: &str) -> Vec<(String, String)> {
    head.lines()
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Does this response head declare `Transfer-Encoding: chunked`? Header names
/// are case-insensitive and the value may be a list (`gzip, chunked`).
pub fn head_is_chunked(head: &str) -> bool {
    head.lines().skip(1).any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|t| t.trim().eq_ignore_ascii_case("chunked"))
        })
    })
}

/// Undo `Transfer-Encoding: chunked` incrementally, across as many reads as
/// the bytes need to arrive.
///
/// The whole-body path ([`dechunk`]) can de-chunk everything at once; a live
/// stream cannot, because a chunk boundary falls wherever the hub flushed and
/// the next size line may not have arrived yet — this is what `events.rs`'s
/// `SseBody` feeds one socket read at a time.
pub struct Dechunker {
    chunked: bool,
    /// Bytes left in the chunk being read.
    remaining: usize,
    /// The zero-size chunk arrived.
    done: bool,
}

impl Dechunker {
    pub fn new(chunked: bool) -> Self {
        Self {
            chunked,
            remaining: 0,
            done: false,
        }
    }

    pub fn finished(&self) -> bool {
        self.done
    }

    /// Take whatever payload `raw` now yields, leaving the rest in place.
    pub fn take(&mut self, raw: &mut Vec<u8>) -> Result<Vec<u8>, String> {
        if !self.chunked {
            return Ok(std::mem::take(raw));
        }
        let mut out = Vec::new();
        loop {
            if self.done {
                raw.clear();
                break;
            }
            if self.remaining > 0 {
                let take = self.remaining.min(raw.len());
                if take == 0 {
                    break;
                }
                out.extend(raw.drain(..take));
                self.remaining -= take;
                continue;
            }
            // A size line, possibly preceded by the CRLF that ended the
            // previous chunk's data.
            let skip = if raw.starts_with(b"\r\n") { 2 } else { 0 };
            let Some(eol) = find(&raw[skip..], b"\r\n") else {
                // Not a whole size line yet.
                break;
            };
            let line = String::from_utf8_lossy(&raw[skip..skip + eol]).into_owned();
            // `;` introduces chunk extensions, which nothing here uses.
            let token = line.split(';').next().unwrap_or("").trim().to_string();
            let size = usize::from_str_radix(&token, 16)
                .map_err(|_| format!("unreadable chunk size {token:?}"))?;
            raw.drain(..skip + eol + 2);
            if size == 0 {
                self.done = true;
                raw.clear();
                break;
            }
            self.remaining = size;
        }
        Ok(out)
    }
}

/// Undo `Transfer-Encoding: chunked` over a whole body already read in full —
/// built on [`Dechunker`] rather than a second parser: one `take` call over
/// the whole buffer runs its loop exactly as a stream's repeated calls would,
/// so this is [`Dechunker`] fed everything at once, not a second
/// implementation to keep in step with the first.
///
/// A body that stops mid-chunk (the peer closed early) yields what had
/// arrived rather than an error: `remote::speak` already tolerates a
/// half-closed connection, and failing here would undo that.
///
/// Over BYTES: a chunk size is a byte count and says nothing about character
/// boundaries, so a chunk may legitimately end halfway through a character.
/// The caller decodes the joined result.
pub fn dechunk(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut raw = body.to_vec();
    Dechunker::new(true).take(&mut raw)
}

#[cfg(test)]
#[path = "tests_http1.rs"]
mod tests;
