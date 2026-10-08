//! Linking this hub to another fleet's hub: redeem a peer pairing code
//! against the other hub's `/pair` and keep the token as a fresh dialer link.
//! One implementation for `fleet-hub peer add` and the `link_peer` tool
//! (Orbit Fleet 11.5), so the URL rule and the token's handling cannot
//! drift apart. The token is never printed, logged or put in an error.

use crate::http_client::{exchange, split_response, Endpoint};
use crate::ipc_error::{codes, lock, IpcError};
use crate::store::Store;
use std::sync::Mutex;

const PAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Whether `url` may be dialed as a peer hub: `https://` always, `http://`
/// only with `--insecure` AND a loopback host — the same rule
/// `fleet-agent --insecure` applies to an agent's hub. Never prints or logs
/// `url` beyond what the caller already typed.
pub fn check_peer_url(url: &str, insecure: bool) -> Result<Endpoint, String> {
    let at = Endpoint::parse(url)?;
    if !at.is_tls() {
        if !insecure {
            return Err(format!(
                "refusing the plain hub {url}: the link token would cross the network in clear. \
                 Use https://, or pass --insecure for a loopback test"
            ));
        }
        if !at.is_loopback() {
            return Err(format!(
                "refusing the plain hub {url}: --insecure is for loopback only. Use https://"
            ));
        }
    }
    Ok(at)
}

/// The other hub's `error`/refusal text is its own words, not ours — never
/// trust it to be one safe line. Scrubbed of anything that could forge a
/// second line downstream (the same treatment an untrusted client's text
/// gets) and capped well short of anything a terminal or a log line minds.
const PAIR_ERROR_MAX_CHARS: usize = 200;

/// Pull the peer token out of a `/pair` answer. On any refusal, the error
/// carries the other hub's own message but NEVER the token — this is the one
/// place a peer token exists as plaintext outside the store, and it must not
/// leak into a returned `Err` that a caller might print or log.
///
/// A code redeemed with the wrong mode still mints a full client on the
/// other hub — `/pair` has no way to know what the caller wanted until it
/// reads `mode` back — so the refusal here names it, so the operator can
/// have it revoked instead of leaving an unheld credential behind.
pub fn token_from_pair_response(raw: &[u8]) -> Result<String, String> {
    let resp = split_response(raw)?;
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
    if resp.status != 200 {
        let why = v["error"].as_str().unwrap_or("pairing refused");
        let why: String = crate::mcp::guard::scrub_line(why)
            .chars()
            .take(PAIR_ERROR_MAX_CHARS)
            .collect();
        return Err(format!("the other hub answered {}: {why}", resp.status));
    }
    if v["mode"].as_str() != Some("peer") {
        let name = v["name"].as_str().unwrap_or("it");
        return Err(format!(
            "that code was not minted with --mode peer; ask for a peer code. \
             The other hub already created a full client for this code — ask its \
             operator to `fleet-hub client revoke {name}`"
        ));
    }
    let token = v["token"]
        .as_str()
        .ok_or_else(|| "the other hub sent no token".to_string())?;
    // Checked here, where it arrives, as well as where it is sent: a token
    // with a CR/LF in it would add headers to every exchange with this peer.
    crate::http_client::check_bearer(token)
        .map_err(|_| "the other hub sent a token no request can carry".to_string())?;
    Ok(token.to_string())
}

/// Redeem `code` against `url`'s `/pair` and answer the peer token. `url`
/// must already have passed [`check_peer_url`].
pub async fn redeem(url: &str, code: &str) -> Result<String, String> {
    // Same request-target/header construction as
    // `src-tauri/src/backend/pairing.rs`'s `TcpPairTransport::post_pair`: the
    // base URL with `/pair` appended, parsed once for the target and
    // authority this request line needs.
    let pair_url = format!("{}/pair", url.trim_end_matches('/'));
    let at = Endpoint::parse(&pair_url)?;
    let body = serde_json::json!({ "code": code }).to_string();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        at.target(),
        at.authority(),
        body.len()
    );
    let raw = tokio::time::timeout(PAIR_TIMEOUT, exchange(&at, &request))
        .await
        .map_err(|_| format!("{url} did not answer within {PAIR_TIMEOUT:?}"))??;
    token_from_pair_response(&raw)
}

/// The `link_peer` tool: link this hub to the hub at `url` with a peer code
/// minted there (`fleet-hub pair --mode peer`). `https://` only: a tool
/// never dials a plain hub, which `fleet-hub peer add --insecure` keeps for
/// a loopback test. Answers the new link's id; the running supervisor
/// connects it within a few seconds.
pub async fn link(store: &Mutex<Store>, url: &str, code: &str) -> Result<i64, IpcError> {
    let url = url.trim();
    check_peer_url(url, false).map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    let code = code.trim();
    if code.is_empty() || code.len() > 128 || code.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a pairing code is one short line",
        ));
    }
    let token = redeem(url, code)
        .await
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    lock(store)?.insert_dialer_link(url.trim_end_matches('/'), &token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_tool_refuses_a_plain_hub_and_a_bad_code_before_dialing() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let e = link(&store, "http://127.0.0.1:7788", "abc")
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        let e = link(&store, "https://b.example", "a\nb").await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(store
            .lock()
            .unwrap()
            .peer_link_summaries()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_plain_peer_url_is_refused_unless_insecure_and_loopback() {
        assert!(check_peer_url("https://b.example", false).is_ok());
        let e = check_peer_url("http://b.example", false).unwrap_err();
        assert!(e.contains("--insecure"), "{e}");
        let e = check_peer_url("http://b.example", true).unwrap_err();
        assert!(e.contains("loopback"), "{e}");
        assert!(check_peer_url("http://127.0.0.1:7788", true).is_ok());
    }

    #[test]
    fn the_pair_answer_yields_its_token_and_nothing_else_is_printed() {
        let raw = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n\
                   {\"token\":\"abc\",\"name\":\"hub-a\",\"mode\":\"peer\",\"trusted\":false,\"hub\":\"https://b\"}";
        assert_eq!(token_from_pair_response(raw.as_bytes()).unwrap(), "abc");
        let e =
            token_from_pair_response(b"HTTP/1.1 404 Not Found\r\n\r\n{\"error\":\"invalid code\"}")
                .unwrap_err();
        assert!(e.contains("invalid code") && !e.contains("abc"), "{e}");
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"token\":\"abc\",\"mode\":\"full\"}";
        assert!(
            token_from_pair_response(raw).unwrap_err().contains("peer"),
            "a non-peer code is refused"
        );
    }

    /// The token goes into a hand-written `Authorization` header on every
    /// exchange with this peer: one carrying a CR/LF is refused on arrival.
    #[test]
    fn a_token_that_would_break_a_header_is_refused() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n\
                    {\"token\":\"abc\\r\\nX-Evil: 1\",\"mode\":\"peer\"}";
        let e = token_from_pair_response(raw).unwrap_err();
        assert!(!e.contains("abc"), "must never repeat the token: {e}");
    }

    /// G15: redeeming a code minted for a `full`/`readonly` client still
    /// leaves an unheld client sitting on the other hub — the refusal must
    /// name it so the operator can reclaim it, instead of just saying "ask
    /// for a peer code" and leaving that credential unaccounted for.
    #[test]
    fn a_wrong_mode_code_names_the_client_to_revoke() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n\
                    {\"token\":\"abc\",\"name\":\"phone-3\",\"mode\":\"full\"}";
        let e = token_from_pair_response(raw).unwrap_err();
        assert!(
            e.contains("fleet-hub client revoke phone-3"),
            "must name the stranded client by its actual name: {e}"
        );
        assert!(!e.contains("abc"), "must never repeat the token: {e}");
    }

    /// G15: the other hub's own error text is untrusted input to us just as
    /// much as a peer body is — it must not be able to break a log/terminal
    /// line, and it must not be printed unbounded.
    #[test]
    fn the_other_hubs_refusal_text_is_scrubbed_and_capped() {
        let noisy = format!("nope\n{}", "x".repeat(500));
        let raw = format!("HTTP/1.1 404 Not Found\r\n\r\n{{\"error\":{noisy:?}}}");
        let e = token_from_pair_response(raw.as_bytes()).unwrap_err();
        assert!(!e.contains('\n'), "a line break must be scrubbed: {e}");
        assert!(
            e.chars().count() < noisy.chars().count(),
            "must be capped well short of the original: {e}"
        );
    }
}
