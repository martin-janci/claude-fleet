//! Tests for [`super`] (`net::http1`): the response-head parsing and
//! chunked-transfer decoding shared by the desktop's `remote.rs` one-shot
//! `POST /mcp`, its `events.rs` streaming `GET /events`, and
//! [`crate::net::https`]'s tracker client. (Lifted from `src-tauri`'s
//! `backend/http1.rs` with work graph M3.0.)
//!
//! Every test here used to live beside one of the two callers, testing the
//! callers' own now-removed copy of this logic directly:
//!
//! - `the_transfer_encoding_header_is_matched_case_insensitively_and_in_a_list`,
//!   `a_truncated_chunked_body_yields_what_arrived`,
//!   `an_unreadable_chunk_size_is_an_error_not_a_guess` and the `dechunk` half
//!   of `a_chunk_may_end_inside_a_character_and_invalid_bytes_never_panic`
//!   moved here from `tests_remote.rs` unchanged.
//! - `the_dechunker_is_transparent_when_the_body_is_not_chunked`,
//!   `the_dechunker_waits_for_a_size_line_that_has_not_finished_arriving` and
//!   `the_dechunker_refuses_a_size_it_cannot_read` moved here from
//!   `tests_events.rs` unchanged.
//!
//! `tests_remote.rs` and `tests_events.rs` keep their own integration-level
//! coverage (`split_response`, `open_stream`, the real chunked socket test)
//! exercising this module through each caller's wiring.

use super::*;

#[test]
fn the_transfer_encoding_header_is_matched_case_insensitively_and_in_a_list() {
    for head in [
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked",
    ] {
        assert!(head_is_chunked(head), "{head:?}");
    }
    for head in [
        "HTTP/1.1 200 OK\r\nContent-Length: 3",
        // The status line is skipped, so a reason phrase cannot match.
        "HTTP/1.1 200 Transfer-Encoding: chunked",
        "HTTP/1.1 200 OK\r\nX-Note: Transfer-Encoding: chunked",
    ] {
        assert!(!head_is_chunked(head), "{head:?}");
    }
}

/// A peer that dies mid-chunk leaves what arrived, matching `remote::speak`'s
/// own tolerance for a half-close. Refusing here would undo that.
#[test]
fn a_truncated_chunked_body_yields_what_arrived() {
    assert_eq!(dechunk(b"5\r\nhel").expect("partial"), b"hel");
    assert_eq!(
        dechunk(b"5\r\nhello\r\n6\r\n wor").expect("partial"),
        b"hello wor"
    );
}

#[test]
fn an_unreadable_chunk_size_is_an_error_not_a_guess() {
    let e = dechunk(b"zz\r\nxx").expect_err("zz is not hex");
    assert!(e.contains("chunk size"), "{e}");
}

/// A chunk size is a BYTE count, so a chunk may end inside a character —
/// that is legal framing, not a malformed body. Bytes that are not UTF-8 at
/// all still cannot panic here: `dechunk` hands back raw bytes, and it is the
/// caller (`split_response`, tested separately) that decodes them lossily.
#[test]
fn a_chunk_may_end_inside_a_character_and_invalid_bytes_never_panic() {
    // "ä" is 0xC3 0xA4: one byte per chunk.
    let joined = dechunk(b"1\r\n\xC3\r\n1\r\n\xA4\r\n0\r\n\r\n").expect("legal framing");
    assert_eq!(joined, "ä".as_bytes());
}

#[test]
fn the_dechunker_is_transparent_when_the_body_is_not_chunked() {
    let mut d = Dechunker::new(false);
    let mut raw = b"data: 1\n\n".to_vec();
    assert_eq!(d.take(&mut raw).unwrap(), b"data: 1\n\n");
    assert!(raw.is_empty());
    assert!(!d.finished());
}

/// THE case that made a whole-body de-chunker useless for a live stream: a
/// size line arriving in one read and its data in the next.
#[test]
fn the_dechunker_waits_for_a_size_line_that_has_not_finished_arriving() {
    let mut d = Dechunker::new(true);
    // 0xb = 11, the length of "data: 12345".
    let mut raw = b"b".to_vec();
    assert!(d.take(&mut raw).unwrap().is_empty(), "'b' might be 'be'");
    raw.extend_from_slice(b"\r\ndata: 12345");
    assert_eq!(d.take(&mut raw).unwrap(), b"data: 12345");
    raw.extend_from_slice(b"\r\n5\r\nabcde\r\n0\r\n\r\n");
    assert_eq!(d.take(&mut raw).unwrap(), b"abcde");
    assert!(d.finished());
}

#[test]
fn the_dechunker_refuses_a_size_it_cannot_read() {
    let mut d = Dechunker::new(true);
    let mut raw = b"zz\r\nxx".to_vec();
    let err = d.take(&mut raw).unwrap_err();
    assert!(err.contains("chunk size"), "{err}");
}

/// `dechunk` is [`Dechunker`] fed the whole body in one call rather than a
/// second parser — this pins that the two agree on the same input rather
/// than merely happening to pass separate tests.
#[test]
fn dechunk_and_the_incremental_dechunker_agree_on_the_same_bytes() {
    let body: &[u8] = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    let whole = dechunk(body).expect("a response");
    let mut incremental = Dechunker::new(true);
    let mut raw = body.to_vec();
    let piecemeal = incremental.take(&mut raw).expect("a response");
    assert_eq!(whole, piecemeal);
    assert_eq!(whole, b"hello world");
}

#[test]
fn parse_status_reads_the_token_after_the_http_version() {
    assert_eq!(parse_status("HTTP/1.1 200 OK\r\nx: y").unwrap(), 200);
    // The status token, not a substring: a reason phrase carrying " 200" must
    // not read as 200, and a header value must not either.
    assert_eq!(
        parse_status("HTTP/1.1 500 Internal Error 200 OK\r\n\r\nboom").unwrap(),
        500
    );
    assert!(parse_status("").is_err());
    assert!(parse_status("not an http response at all").is_err());
}

#[test]
fn find_locates_the_first_occurrence_or_none() {
    assert_eq!(find(b"abc\r\n\r\ndef", b"\r\n\r\n"), Some(3));
    assert_eq!(find(b"no boundary here", b"\r\n\r\n"), None);
    assert_eq!(find(b"", b"\r\n\r\n"), None);
}

#[test]
fn a_size_line_that_never_ends_is_an_error_not_a_buffer() {
    let mut d = Dechunker::new(true);
    let mut raw = b"5\r\nhello\r\n".to_vec();
    assert_eq!(d.take(&mut raw).unwrap(), b"hello");
    let mut grown = Vec::new();
    let mut failed = false;
    for _ in 0..100 {
        grown.extend_from_slice(&[b'a'; 64]);
        let mut raw = grown.clone();
        if d.take(&mut raw).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed, "a 6 KiB size line was still being waited on");
    // A size line split across reads is still fine.
    let mut d = Dechunker::new(true);
    let mut raw = b"3".to_vec();
    assert!(d.take(&mut raw).unwrap().is_empty());
    raw.extend_from_slice(b"\r\nabc\r\n0\r\n\r\n");
    assert_eq!(d.take(&mut raw).unwrap(), b"abc");
    assert!(d.finished());
}

/// A body of very many very small chunks de-chunks in time linear in its
/// size. `take` used to drain `raw` from the front after every chunk and
/// every size line: 600 000 one-byte chunks (3.6 MB on the wire, under the
/// tracker client's 4 MiB cap) moved about 2 TB and took minutes, in a loop
/// that runs after the caller's timeout has already been met. The budget
/// below is three orders of magnitude above what the cursor needs.
#[test]
fn a_body_of_one_byte_chunks_is_dechunked_in_linear_time() {
    const CHUNKS: usize = 600_000;
    let mut body = Vec::with_capacity(CHUNKS * 6 + 5);
    for i in 0..CHUNKS {
        body.extend_from_slice(b"1\r\n");
        body.push(b'a' + (i % 26) as u8);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"0\r\n\r\n");
    let started = std::time::Instant::now();
    let out = dechunk(&body).expect("a response");
    let took = started.elapsed();
    assert_eq!(out.len(), CHUNKS);
    assert!(out
        .iter()
        .enumerate()
        .all(|(i, b)| *b == b'a' + (i % 26) as u8));
    assert!(
        took < std::time::Duration::from_secs(20),
        "de-chunking {CHUNKS} one-byte chunks took {took:?}"
    );
    // Fed a read at a time, the same body gives the same bytes and leaves
    // nothing behind.
    let mut d = Dechunker::new(true);
    let mut raw = Vec::new();
    let mut piecemeal = Vec::new();
    for read in body.chunks(16 * 1024) {
        raw.extend_from_slice(read);
        piecemeal.extend(d.take(&mut raw).expect("a response"));
    }
    assert!(d.finished() && raw.is_empty());
    assert_eq!(piecemeal, out);
}

/// What `take` leaves in `raw` is exactly what it has not consumed: the
/// unfinished size line stays for the next read, and a refused one stays
/// where it was found, behind the payload already taken.
#[test]
fn the_dechunker_leaves_only_what_it_has_not_consumed() {
    let mut d = Dechunker::new(true);
    let mut raw = b"3\r\nabc\r\n4\r\nde".to_vec();
    assert_eq!(d.take(&mut raw).unwrap(), b"abcde");
    assert!(raw.is_empty(), "{raw:?}");
    raw.extend_from_slice(b"fg\r\n1");
    assert_eq!(d.take(&mut raw).unwrap(), b"fg");
    assert_eq!(raw, b"\r\n1", "the size line has not ended yet");
    raw.extend_from_slice(b"\r\nh\r\nzz\r\n");
    let err = d.take(&mut raw).unwrap_err();
    assert!(err.contains("chunk size"), "{err}");
    assert_eq!(raw, b"\r\nzz\r\n", "the refused line is where it was found");
    // Past the last chunk everything is dropped, trailers included.
    let mut d = Dechunker::new(true);
    let mut raw = b"1\r\nx\r\n0\r\nTrailer: y\r\n\r\n".to_vec();
    assert_eq!(d.take(&mut raw).unwrap(), b"x");
    assert!(d.finished() && raw.is_empty());
    raw.extend_from_slice(b"anything after the end");
    assert!(d.take(&mut raw).unwrap().is_empty());
    assert!(raw.is_empty());
}
