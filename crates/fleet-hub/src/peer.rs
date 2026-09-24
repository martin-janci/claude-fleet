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
pub(crate) fn check_peer_url(url: &str, insecure: bool) -> Result<Endpoint, String> {
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

/// Pull the peer token out of a `/pair` answer. On any refusal, the error
/// carries the other hub's own message but NEVER the token — this is the one
/// place a peer token exists as plaintext outside the store, and it must not
/// leak into a returned `Err` that a caller might print or log.
pub(crate) fn token_from_pair_response(raw: &[u8]) -> Result<String, String> {
    let resp = split_response(raw)?;
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
    if resp.status != 200 {
        let why = v["error"].as_str().unwrap_or("pairing refused");
        return Err(format!("the other hub answered {}: {why}", resp.status));
    }
    if v["mode"].as_str() != Some("peer") {
        return Err("that code was not minted with --mode peer; ask for a peer code".into());
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
    let store = open_store(opts, env)?;
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
    let hit = rows
        .iter()
        .find(|r| {
            r.revoked_at.is_none()
                && (r.fleet_id.as_deref() == Some(target) || r.id.to_string() == target)
        })
        .ok_or_else(|| format!("no live link {target}"))?;
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
pub(crate) fn link_table(rows: &[fleet_core::store::PeerLinkSummary]) -> String {
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
}
