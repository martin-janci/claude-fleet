//! `fleet-hub peer …`: link management straight on state.db (as `agent-token`
//! does); the running hub's peer supervisor rescans the links every 5 s.

use crate::config::HubOptions;
use crate::out;
use crate::serve::{existing_db, open_store};
use fleet_core::http_client::{exchange, split_response, Endpoint};
use std::collections::HashMap;
use std::process::ExitCode;

const PAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Whether `url` may be dialed as a peer hub: `https://` always, `http://`
/// only with `--insecure` AND a loopback host — the same rule
/// `fleet-agent --insecure` applies to an agent's hub. Never prints or logs
/// `url` beyond what the caller already typed.
fn check_peer_url(url: &str, insecure: bool) -> Result<Endpoint, String> {
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
fn token_from_pair_response(raw: &[u8]) -> Result<String, String> {
    let resp = split_response(raw)?;
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
    if resp.status != 200 {
        let why = v["error"].as_str().unwrap_or("pairing refused");
        let why: String = fleet_core::mcp::guard::scrub_line(why)
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
    v["token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the other hub sent no token".into())
}

/// `fleet-hub peer add <url> <code> [--insecure]`: redeem a peer pairing code
/// against the other hub's `/pair` and save the token as a fresh dialer link.
/// Never prints, logs, or wraps the token in an error.
pub async fn add(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    url: &str,
    code: &str,
    insecure: bool,
) -> Result<ExitCode, String> {
    check_peer_url(url, insecure)?;
    existing_db(&crate::config::resolve_data_dir(opts, env))?;
    // Opened BEFORE the code is redeemed: if this hub's own store cannot be
    // opened, the code must never reach the other hub's `/pair` at all — a
    // code redeemed but then thrown away here would leave an unheld full
    // client sitting on the other side with nothing to reclaim it.
    let store = open_store(opts, env)?;
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
    let token = token_from_pair_response(&raw)?;
    let id = store
        .insert_dialer_link(url.trim_end_matches('/'), &token)
        .map_err(|e| e.message)?;
    out::line(&format!(
        "linked to {url} (link {id}); the running hub connects within a few seconds — \
         `fleet-hub peer list` shows its state"
    ));
    Ok(ExitCode::SUCCESS)
}

/// `fleet-hub peer list`: this hub's links, one per line, never a token.
pub fn list(opts: &HubOptions, env: &HashMap<String, String>) -> Result<ExitCode, String> {
    existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = open_store(opts, env)?;
    let rows = store.peer_link_summaries().map_err(|e| e.message)?;
    out::line(&link_table(&rows));
    Ok(ExitCode::SUCCESS)
}

/// Which live row `target` names: an exact numeric link id first, and only
/// when none matches, a `fleet_id` equal to `target`. A fleet id is an
/// operator-chosen or peer-learned string and can itself look like a small
/// integer (e.g. a link whose peer fleet id happens to be `"2"`), so an id
/// match must never be shadowed by a fleet-id match earlier in `rows`.
fn find_target<'a>(
    rows: &'a [fleet_core::store::PeerLinkSummary],
    target: &str,
) -> Option<&'a fleet_core::store::PeerLinkSummary> {
    let live = || rows.iter().filter(|r| r.revoked_at.is_none());
    if let Ok(id) = target.parse::<i64>() {
        if let Some(r) = live().find(|r| r.id == id) {
            return Some(r);
        }
    }
    live().find(|r| r.fleet_id.as_deref() == Some(target))
}

/// `fleet-hub peer remove <fleet_id|id>`: revoke a live link by fleet id or
/// numeric link id. Its pending outbox rows fail back to their senders (see
/// `Store::revoke_peer_link`); a listener row's client token is revoked too.
pub fn remove(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    target: &str,
) -> Result<ExitCode, String> {
    existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = open_store(opts, env)?;
    let rows = store.peer_link_summaries().map_err(|e| e.message)?;
    let hit = find_target(&rows, target).ok_or_else(|| format!("no live link {target}"))?;
    let failed = store
        .revoke_peer_link(hit.id, fleet_core::store::now_unix())
        .map_err(|e| e.message)?;
    out::line(&format!(
        "removed link {}; {failed} waiting message(s) failed back to their senders",
        hit.id
    ));
    Ok(ExitCode::SUCCESS)
}

/// Render `rows` (live links only — a revoked one is dropped, not shown
/// struck through) as a fixed-width table. Deliberately has no `token`
/// column: the type it reads, `PeerLinkSummary`, does not carry one.
fn link_table(rows: &[fleet_core::store::PeerLinkSummary]) -> String {
    let mut out = String::from(
        "ID  ROLE      FLEET                                 STATE         PENDING  LAST EXCHANGE  ERROR\n",
    );
    for r in rows.iter().filter(|r| r.revoked_at.is_none()) {
        out.push_str(&format!(
            "{:<3} {:<9} {:<37} {:<13} {:<8} {:<14} {}\n",
            r.id,
            r.role,
            r.fleet_id.as_deref().unwrap_or("(handshake pending)"),
            r.state,
            r.pending,
            r.last_exchange_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into()),
            r.last_error.as_deref().unwrap_or(""),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// G22: a fleet id can itself look like a small integer (learned from
    /// the peer, or chosen by its operator), so an exact numeric link id
    /// must win over a fleet-id match — regardless of which row the target
    /// fleet-id string happens to sit on.
    #[test]
    fn an_exact_link_id_wins_over_a_fleet_id_that_looks_like_it() {
        fn row(id: i64, fleet_id: &str) -> fleet_core::store::PeerLinkSummary {
            fleet_core::store::PeerLinkSummary {
                id,
                fleet_id: Some(fleet_id.into()),
                role: "dialer".into(),
                url: None,
                state: "connected".into(),
                last_exchange_at: None,
                last_error: None,
                pending: 0,
                revoked_at: None,
            }
        }
        // The fleet-id-"2" row sorts first, deliberately, so a naive
        // single `.find()` over the OR of both predicates would return it
        // instead of the row whose actual id is 2.
        let rows = vec![row(5, "2"), row(2, "fleet-other")];
        let hit = find_target(&rows, "2").expect("a match");
        assert_eq!(
            hit.id, 2,
            "the exact link id must win, not the row whose fleet_id is \"2\""
        );

        // A target that is not a valid link id at all still falls back to
        // matching by fleet id.
        let hit = find_target(&rows, "fleet-other").expect("a fleet-id match");
        assert_eq!(hit.id, 2);
    }

    #[test]
    fn the_link_table_never_shows_a_token() {
        let rows = vec![fleet_core::store::PeerLinkSummary {
            id: 1,
            fleet_id: Some("fleet-b".into()),
            role: "dialer".into(),
            url: Some("https://b".into()),
            state: "connected".into(),
            last_exchange_at: Some(1),
            last_error: None,
            pending: 2,
            revoked_at: None,
        }];
        let t = link_table(&rows);
        assert!(
            t.contains("fleet-b") && t.contains("connected") && t.contains('2'),
            "{t}"
        );
    }

    /// G15: if this hub's own store cannot be opened, `add` must never have
    /// posted the code to the other hub at all — otherwise a valid code is
    /// redeemed and thrown away, leaving an unheld full client sitting on
    /// the other side with nothing to reclaim it. A real listener stands in
    /// for the other hub, answering instantly with a valid peer-mode
    /// response so the OLD (POST-before-open_store) code path would
    /// complete the round trip well within this test rather than blocking
    /// on `PAIR_TIMEOUT`.
    #[tokio::test]
    async fn the_store_opens_before_the_code_is_redeemed() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let dialed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dialed2 = dialed.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            if let Ok((mut sock, _)) = listener.accept().await {
                dialed2.store(true, std::sync::atomic::Ordering::SeqCst);
                let body = "{\"token\":\"abc\",\"name\":\"hub-a\",\"mode\":\"peer\",\
                             \"trusted\":false,\"hub\":\"https://b\"}";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        // `existing_db` is satisfied (the file is there) but `open_store`
        // must fail: this is not a SQLite database.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.db"), b"not a sqlite database").unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };

        let result = add(
            &opts,
            &HashMap::new(),
            &format!("http://127.0.0.1:{port}"),
            "CODE",
            true,
        )
        .await;
        assert!(
            result.is_err(),
            "a broken local store must fail add(): {result:?}"
        );
        assert!(
            !dialed.load(std::sync::atomic::Ordering::SeqCst),
            "the code must never reach the other hub before this hub's own store opens"
        );
    }
}
