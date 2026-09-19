//! Tests for [`super`] (`backend::pairing`) against a fake transport. No network.
//!
//! Every fixture is the hub's *real* encoding, read from
//! `crates/fleet-core/src/mcp/pairing.rs::handle_pair`:
//!
//! - success is `200` with `{"token","name","mode","hub"}` and
//!   `Cache-Control: no-store`;
//! - an unknown, spent or expired code is **one** answer, `404` with
//!   `{"error":"invalid code"}` — the hub deliberately does not say which;
//! - too many attempts is `429` with `{"error":"too many attempts"}` and a
//!   `Retry-After` header;
//! - an unreadable body is a bare `400` with no body at all;
//! - a store failure is `500` with `{"error":"pairing failed"}`.

use super::*;
use fleet_core::ipc_error::codes;
use serde_json::json;
use std::sync::{Arc, Mutex};

// --- fixtures ----------------------------------------------------------------

fn answer(status: u16, body: serde_json::Value) -> HubResponse {
    HubResponse {
        status,
        body: body.to_string(),
    }
}

fn paired_ok() -> HubResponse {
    answer(
        200,
        json!({
            "token": "cl_s3cret_token",
            "name": "laptop",
            "mode": "full",
            "hub": "https://fleet.example.com",
        }),
    )
}

// --- the fake transport ------------------------------------------------------

struct Fake {
    answers: Mutex<Vec<Result<HubResponse, String>>>,
    seen: Mutex<Vec<(String, String)>>,
}

impl Fake {
    fn answering(r: Result<HubResponse, String>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(vec![r]),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl PairTransport for Fake {
    async fn post_pair(&self, url: &str, body: String) -> Result<HubResponse, String> {
        self.seen
            .lock()
            .unwrap()
            .push((url.to_string(), body.clone()));
        let mut answers = self.answers.lock().unwrap();
        if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0].clone()
        }
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

// --- the request -------------------------------------------------------------

/// The code goes to `{base}/pair` as `{"code": …}` — the body
/// `fleet_core::mcp::pairing::PairBody` deserialises — and nowhere else.
#[test]
fn a_pairing_posts_the_code_to_the_pair_route() {
    let fake = Fake::answering(Ok(paired_ok()));
    let got = block_on(redeem(&*fake, "https://fleet.example.com", "ABCD1234")).unwrap();
    assert_eq!(got.name, "laptop");
    assert_eq!(got.mode, "full");
    assert_eq!(got.token, "cl_s3cret_token");

    let calls = fake.calls();
    assert_eq!(calls.len(), 1, "exactly one attempt: {calls:?}");
    assert_eq!(calls[0].0, "https://fleet.example.com/pair");
    let body: serde_json::Value = serde_json::from_str(&calls[0].1).expect("a JSON body");
    assert_eq!(body, json!({ "code": "ABCD1234" }));
}

/// A code is read off a terminal and pasted, so it arrives with whitespace
/// and, on a Crockford alphabet, very likely in the wrong case.
#[test]
fn a_pasted_code_is_trimmed_and_upper_cased() {
    let fake = Fake::answering(Ok(paired_ok()));
    block_on(redeem(&*fake, "https://fleet.example.com", "  abcd1234\n")).unwrap();
    let body: serde_json::Value = serde_json::from_str(&fake.calls()[0].1).unwrap();
    assert_eq!(body, json!({ "code": "ABCD1234" }));
}

/// A trailing slash on the base URL must not produce `//pair`.
#[test]
fn a_trailing_slash_does_not_double_the_pair_path() {
    let fake = Fake::answering(Ok(paired_ok()));
    block_on(redeem(&*fake, "https://fleet.example.com/", "ABCD1234")).unwrap();
    assert_eq!(fake.calls()[0].0, "https://fleet.example.com/pair");
}

// --- the answers -------------------------------------------------------------

#[test]
fn a_successful_pairing_carries_the_token_the_name_and_the_mode() {
    let got = read_pair_response("https://fleet.example.com", paired_ok()).unwrap();
    assert_eq!(got.token, "cl_s3cret_token");
    assert_eq!(got.name, "laptop");
    assert_eq!(got.mode, "full");
    assert_eq!(got.hub, "https://fleet.example.com");
}

/// The hub answers one thing for unknown, already-used and expired, and the
/// message must not pretend to know which. It must, though, say what to do.
#[test]
fn a_refused_code_says_what_to_do_without_guessing_why() {
    let e = read_pair_response(
        "https://fleet.example.com",
        answer(404, json!({ "error": "invalid code" })),
    )
    .expect_err("404 must be an error");
    assert_eq!(e.code, codes::E_INVALID);
    let said = e.message.to_lowercase();
    assert!(
        said.contains("mint") || said.contains("fleet-hub pair"),
        "the user needs the next step: {}",
        e.message
    );
    // Unknown / used / expired are indistinguishable; claiming one is a lie.
    assert!(!said.contains("expired,"), "{}", e.message);
}

#[test]
fn too_many_attempts_is_its_own_code_so_the_ui_can_say_wait() {
    let e = read_pair_response(
        "https://fleet.example.com",
        answer(429, json!({ "error": "too many attempts" })),
    )
    .expect_err("429 must be an error");
    assert_eq!(e.code, codes::E_RATE_LIMITED);
}

/// An axum `StatusCode` reply has an empty body. The message must still be
/// usable.
#[test]
fn a_bare_400_is_still_a_readable_failure() {
    let e = read_pair_response(
        "https://fleet.example.com",
        HubResponse {
            status: 400,
            body: String::new(),
        },
    )
    .expect_err("400 must be an error");
    assert_eq!(e.code, codes::E_INVALID);
    assert!(e.message.contains("fleet.example.com"), "{}", e.message);
}

/// A 500 (the hub could not store the client) or a proxy's 502 did not reach
/// a working pairing route. That is not a bad code, and telling the user to
/// mint another one would send them round a loop that cannot end.
#[test]
fn a_server_failure_maps_to_hub_unreachable() {
    for status in [500u16, 502, 503] {
        let e = read_pair_response(
            "https://fleet.example.com",
            answer(status, json!({ "error": "pairing failed" })),
        )
        .expect_err("a 5xx must be an error");
        assert_eq!(e.code, codes::E_HUB_UNREACHABLE, "for {status}");
        assert!(e.message.contains(&status.to_string()), "{}", e.message);
    }
}

#[test]
fn a_transport_failure_is_hub_unreachable() {
    let fake = Fake::answering(Err("connect fleet.example.com:443: refused".into()));
    let e = block_on(redeem(&*fake, "https://fleet.example.com", "ABCD1234"))
        .expect_err("a dead socket must be an error");
    assert_eq!(e.code, codes::E_HUB_UNREACHABLE);
    assert!(e.message.contains("refused"), "{}", e.message);
}

/// A 200 whose body is not the shape the hub documents must not be accepted
/// as a pairing — an empty token would be stored and every later call would
/// 401 with nothing explaining it.
#[test]
fn a_two_hundred_with_no_token_is_refused() {
    for body in [
        json!({ "name": "laptop", "mode": "full" }),
        json!({ "token": "", "name": "laptop", "mode": "full" }),
        json!({ "token": "   ", "name": "laptop" }),
    ] {
        let e = read_pair_response("https://fleet.example.com", answer(200, body.clone()))
            .expect_err("a tokenless 200 must be refused");
        assert_eq!(e.code, codes::E_PARSE, "for {body}");
    }
    let e = read_pair_response(
        "https://fleet.example.com",
        HubResponse {
            status: 200,
            body: "<html>hello</html>".into(),
        },
    )
    .expect_err("an HTML 200 must be refused");
    assert_eq!(e.code, codes::E_PARSE);
}

/// A hub that omits `name`/`mode` (or a proxy that rewrote the body) still
/// pairs, because the token is the only load-bearing field; the rest is what
/// Settings displays and has an honest fallback.
#[test]
fn name_and_mode_fall_back_rather_than_failing_the_pairing() {
    let got = read_pair_response(
        "https://fleet.example.com",
        answer(200, json!({ "token": "cl_abc" })),
    )
    .expect("a token alone is a pairing");
    assert_eq!(got.token, "cl_abc");
    assert_eq!(got.name, "desktop");
    assert_eq!(got.mode, "unknown");
    // `hub` falls back to the URL we actually dialled, never to empty.
    assert_eq!(got.hub, "https://fleet.example.com");
}

// --- the token never leaks ---------------------------------------------------

/// `PairedClient` is the one value in this app that holds a fresh fleet-wide
/// credential. A `{:?}` in a log line, a panic or a test failure must not
/// spell it out — the same rule `RemoteConfig` follows.
#[test]
fn debugging_a_paired_client_redacts_the_token() {
    let got = read_pair_response("https://fleet.example.com", paired_ok()).unwrap();
    let shown = format!("{got:?}");
    assert!(!shown.contains("cl_s3cret_token"), "{shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
}

/// The code is a credential too (it mints a token). Nothing that comes back
/// from a failure may echo it, because these messages reach a toast and the
/// log.
#[test]
fn no_pairing_error_ever_echoes_the_code() {
    let fake = Fake::answering(Ok(answer(404, json!({ "error": "invalid code" }))));
    let e = block_on(redeem(&*fake, "https://fleet.example.com", "SECRETCD")).unwrap_err();
    assert!(!e.message.contains("SECRETCD"), "{}", e.message);
    assert!(!format!("{e:?}").contains("SECRETCD"), "{e:?}");
}

/// A hub (or a proxy in front of it) that echoes the token into an error body
/// must not get it into an error message, which lands in a toast and the log.
#[test]
fn an_error_body_that_echoes_a_token_shaped_string_is_still_capped() {
    let long = "x".repeat(10_000);
    let e = read_pair_response(
        "https://fleet.example.com",
        HubResponse {
            status: 502,
            body: long.clone(),
        },
    )
    .expect_err("a 502 must be an error");
    assert!(
        e.message.len() < 400,
        "a whole proxy error page must not become the message ({} chars)",
        e.message.len()
    );
}

/// `PairedClient` must never gain `Serialize`: it would ride into a command's
/// return value and take a fresh fleet credential to the frontend with it.
#[test]
fn paired_client_is_not_serializable() {
    struct Probe<T>(std::marker::PhantomData<T>);
    trait NotSerialize {
        fn is_serialize(&self) -> bool {
            false
        }
    }
    impl<T> NotSerialize for Probe<T> {}
    impl<T: serde::Serialize> Probe<T> {
        fn is_serialize(&self) -> bool {
            true
        }
    }
    assert!(
        !Probe::<PairedClient>(std::marker::PhantomData).is_serialize(),
        "PairedClient gained Serialize — a freshly minted fleet-wide client \
         token can now reach the frontend"
    );
    assert!(Probe::<String>(std::marker::PhantomData).is_serialize());
}
