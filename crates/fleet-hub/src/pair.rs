//! `fleet-hub pair` and `fleet-hub client list|revoke`: the operator's side
//! of client access.
//!
//! All three drive the **running** hub rather than the database: a pairing
//! code only means something inside the process that will redeem it (the
//! registry is in memory), and revoking through the live server is what makes
//! the change visible on the very next request. So each command reads the
//! master token and the port out of the data dir and then calls the hub's own
//! `/mcp` with them — `pair_client`, `list_clients`, `revoke_client` — instead
//! of opening a second write path into the store.
//!
//! The request is written by hand over a `TcpStream`, the way `serve.rs`'s
//! health probe is: the only endpoint these commands ever talk to is
//! `127.0.0.1`, so an HTTP client crate (and a TLS stack with it) would be a
//! large dependency for one loopback POST.

use crate::config::{resolve_data_dir, HubOptions};
use crate::out;
use crate::serve::{existing_db, open_store};
use fleet_core::mcp::settings::McpSettings;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

/// One whole request/response exchange with the local hub. Generous next to
/// the health probe's 3 s — `pair_client` writes nothing, but `revoke_client`
/// takes the store lock a busy hub may hold for a moment.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What a caller sees when nothing is listening. The hub must be RUNNING:
/// these commands do not (and must not) reach around it into state.db.
const NOT_RUNNING: &str = "start fleet-hub serve first";

/// Largest response read from the hub. `client list` on a big fleet is a few
/// kilobytes; this only bounds what a stray listener could make us buffer.
const MAX_RESPONSE: u64 = 1024 * 1024;

/// Where to reach the running hub, and with what.
struct HubConn {
    addr: SocketAddr,
    token: String,
}

/// Read the master token and port out of the data dir. Never creates a data
/// dir or a database: a token minted into a fresh one is not this hub's.
fn hub_conn(opts: &HubOptions, env: &HashMap<String, String>) -> Result<HubConn, String> {
    existing_db(&resolve_data_dir(opts, env))?;
    let store = open_store(opts, env)?;
    let cfg = McpSettings::read(&store).map_err(|e| e.message)?;
    let token = cfg
        .token
        .ok_or("this hub has no master token yet; run fleet-hub init first")?;
    // Loopback, like `healthcheck`: the CLI runs next to the daemon, and the
    // master token must not travel over anything but the loopback interface.
    Ok(HubConn {
        addr: SocketAddr::from(([127, 0, 0, 1], cfg.port)),
        token,
    })
}

/// Call one MCP tool on the running hub and return its JSON result.
async fn call_tool(
    conn: &HubConn,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
    .to_string();
    let addr = conn.addr;
    // `Accept` carries both types because the transport answers SSE-framed;
    // rmcp refuses a request that does not accept `text/event-stream`.
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        conn.token,
        body.len()
    );
    let raw = match tokio::time::timeout(CALL_TIMEOUT, exchange(addr, &request)).await {
        Ok(r) => r?,
        Err(_) => return Err(format!("{addr} did not answer within {CALL_TIMEOUT:.0?}")),
    };
    parse_tool_response(&raw)
}

/// Write `request` to `addr` and read the whole response back.
async fn exchange(addr: SocketAddr, request: &str) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut conn = tokio::net::TcpStream::connect(addr).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::ConnectionRefused {
            format!("no hub is answering on {addr} — {NOT_RUNNING}")
        } else {
            format!("connect {addr}: {e}")
        }
    })?;
    conn.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("send to {addr}: {e}"))?;
    let mut raw = Vec::new();
    conn.take(MAX_RESPONSE)
        .read_to_end(&mut raw)
        .await
        .map_err(|e| format!("read from {addr}: {e}"))?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// The tool result inside an HTTP response from the hub's `/mcp`.
///
/// The transport keeps SSE framing (that is what carries the keep-alive on
/// long polls), so the JSON-RPC envelope arrives on a `data:` line rather
/// than as the body; a plain JSON body is accepted too. A tool that failed
/// answers with `isError: true` and its `E_*` text — that becomes the CLI's
/// error, so the operator sees the hub's own words.
pub fn parse_tool_response(raw: &str) -> Result<serde_json::Value, String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or("the hub sent a malformed HTTP response")?;
    let status = head.lines().next().unwrap_or_default().trim_end();
    if !status.contains(" 200") {
        return Err(match status.split_whitespace().nth(1) {
            Some("401") | Some("403") => format!(
                "the hub refused this token ({status}); the master token in the data dir is not \
                 the one the running hub started with — restart it, or run fleet-hub token show"
            ),
            _ => format!("the hub answered {status}"),
        });
    }
    // An SSE frame carries the envelope on `data:`; a plain body IS the
    // envelope.
    let payload = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .next_back()
        .unwrap_or_else(|| body.trim());
    let envelope: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| format!("the hub sent unreadable JSON: {e}"))?;
    if let Some(err) = envelope.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(format!("the hub refused the call: {msg}"));
    }
    let result = envelope
        .get("result")
        .ok_or("the hub's answer carried no result")?;
    let text = result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    if result.get("isError").and_then(|e| e.as_bool()) == Some(true) {
        return Err(text.to_string());
    }
    serde_json::from_str(text).map_err(|e| format!("the tool's result was not JSON: {e}"))
}

// --- rendering ---------------------------------------------------------------

/// Render `url` as a QR code of Unicode half-blocks — two QR rows per text
/// row, so a code that needs 33 modules still fits an 80-column terminal.
pub fn render_qr(url: &str) -> Result<String, String> {
    use qrcode::render::unicode;
    let code = qrcode::QrCode::new(url.as_bytes())
        .map_err(|e| format!("could not encode the pairing URL as a QR code: {e}"))?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .module_dimensions(1, 1)
        .build())
}

/// `YYYY-MM-DD HH:MM` (UTC) of a unix timestamp, or `-` for `None`.
pub fn fmt_time(ts: Option<i64>) -> String {
    let Some(ts) = ts else {
        return "-".to_string();
    };
    let day = fleet_core::service::usage::day_string(ts.div_euclid(86_400));
    let secs = ts.rem_euclid(86_400);
    format!("{day} {:02}:{:02}Z", secs / 3600, (secs % 3600) / 60)
}

/// The `client list` table: a header plus one line per row.
pub fn client_table(rows: &[serde_json::Value]) -> String {
    if rows.is_empty() {
        return "no paired clients".to_string();
    }
    let field = |r: &serde_json::Value, k: &str| {
        r.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let time = |r: &serde_json::Value, k: &str| fmt_time(r.get(k).and_then(|v| v.as_i64()));
    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|r| {
            [
                field(r, "name"),
                field(r, "mode"),
                time(r, "created_at"),
                time(r, "last_seen_at"),
                time(r, "revoked_at"),
            ]
        })
        .collect();
    let header = ["NAME", "MODE", "CREATED", "LAST SEEN", "REVOKED"];
    let mut width = header.map(str::len);
    for row in &cells {
        for (w, c) in width.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    // The last column is not padded, so a line never ends in trailing blanks.
    let line = |row: &[String; 5]| {
        let mut s = String::new();
        for (i, (cell, w)) in row.iter().zip(width).enumerate() {
            if i + 1 == row.len() {
                s.push_str(cell);
            } else {
                s.push_str(&format!("{cell:<w$}  ", w = w));
            }
        }
        s
    };
    let head = line(&header.map(str::to_string));
    std::iter::once(head)
        .chain(cells.iter().map(line))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- the commands ------------------------------------------------------------

/// `fleet-hub pair --name <name> [--mode …] [--ttl …]`: mint a code through
/// the running hub and show the URL as a QR for a phone camera.
pub async fn pair(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    name: &str,
    mode: Option<&str>,
    ttl: Option<u64>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    let mut args = serde_json::json!({ "name": name });
    if let Some(m) = mode {
        args["mode"] = serde_json::Value::String(m.to_string());
    }
    if let Some(t) = ttl {
        args["ttl_s"] = serde_json::Value::from(t);
    }
    let v = call_tool(&conn, "pair_client", args).await?;
    let url = v["url"].as_str().unwrap_or_default();
    out::line(&render_qr(url)?);
    out::line(url);
    out::line("");
    out::line(&format!(
        "client:  {} ({})",
        v["name"].as_str().unwrap_or(name),
        v["mode"].as_str().unwrap_or("full")
    ));
    out::line(&format!(
        "expires: in {} s — the code works once, and a hub restart voids it",
        v["expires_in_s"].as_u64().unwrap_or(0)
    ));
    out::line("Scan it with the claude-fleet app on the device you are pairing.");
    Ok(ExitCode::SUCCESS)
}

/// `fleet-hub client list [--include-revoked]`.
pub async fn client_list(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    include_revoked: bool,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    let v = call_tool(
        &conn,
        "list_clients",
        serde_json::json!({ "include_revoked": include_revoked }),
    )
    .await?;
    let rows = v.as_array().cloned().unwrap_or_default();
    out::line(&client_table(&rows));
    Ok(ExitCode::SUCCESS)
}

/// `fleet-hub client revoke <name>`.
pub async fn client_revoke(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    name: &str,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    let v = call_tool(&conn, "revoke_client", serde_json::json!({ "name": name })).await?;
    out::line(&format!(
        "revoked {} (paired {}); its next request is refused and the name is free again",
        v["name"].as_str().unwrap_or(name),
        fmt_time(v["created_at"].as_i64())
    ));
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `tools/call` answer as the hub really frames it: SSE, so the JSON
    /// is on a `data:` line rather than being the whole body.
    fn sse(payload: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
             event: message\r\ndata: {payload}\r\n\r\n"
        )
    }

    #[test]
    fn a_tool_result_is_read_out_of_the_sse_frame() {
        let inner = r#"{\"url\":\"http://127.0.0.1:4180/pair#ABCD1234\",\"code\":\"ABCD1234\"}"#;
        let raw = sse(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":"{inner}"}}]}}}}"#
        ));
        let v = parse_tool_response(&raw).expect("a result");
        assert_eq!(v["code"], "ABCD1234");
        assert_eq!(v["url"], "http://127.0.0.1:4180/pair#ABCD1234");
    }

    #[test]
    fn a_plain_json_body_is_read_too() {
        // `json_response` is off today, but a hub that switched it on must not
        // make the CLI unusable.
        let raw = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\
                   {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[{\"type\":\"text\",\
                   \"text\":\"[]\"}]}}";
        assert_eq!(parse_tool_response(raw).unwrap(), serde_json::json!([]));
    }

    #[test]
    fn a_tool_error_becomes_the_cli_error() {
        let raw = sse(
            r#"{"jsonrpc":"2.0","id":1,"result":{"isError":true,"content":[{"type":"text","text":"E_EXISTS: a live client named 'phone' already exists"}]}}"#,
        );
        let e = parse_tool_response(&raw).expect_err("an error");
        assert!(e.contains("E_EXISTS"), "{e}");
        assert!(e.contains("phone"), "{e}");
    }

    #[test]
    fn a_jsonrpc_error_and_a_bad_status_are_reported_as_such() {
        let raw =
            sse(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"tool not found"}}"#);
        assert!(parse_tool_response(&raw)
            .expect_err("an error")
            .contains("tool not found"));
        // A 401 is the one the operator will actually hit: a stale token.
        let unauth = "HTTP/1.1 401 Unauthorized\r\nconnection: close\r\n\r\n";
        let e = parse_tool_response(unauth).expect_err("401");
        assert!(e.contains("401"), "{e}");
        assert!(
            e.to_lowercase().contains("token"),
            "a 401 must point at the token: {e}"
        );
        // Nothing parseable at all is an error, not a panic.
        assert!(parse_tool_response("").is_err());
        assert!(parse_tool_response("HTTP/1.1 200 OK\r\n\r\nnot json").is_err());
    }

    #[test]
    fn the_qr_renders_as_half_blocks_and_stays_terminal_sized() {
        let url = "http://127.0.0.1:4180/pair#ABCD1234";
        let qr = render_qr(url).expect("a QR");
        let lines: Vec<&str> = qr.lines().collect();
        assert!(lines.len() > 8, "a QR has rows: {}", lines.len());
        // Dense1x2 draws with half-block characters and a quiet zone.
        assert!(
            qr.chars().any(|c| matches!(c, '█' | '▀' | '▄')),
            "expected half-blocks:\n{qr}"
        );
        // It has to fit a terminal next to the URL printed under it.
        let widest = lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(widest <= 80, "QR is {widest} columns wide");
        // Same input, same picture.
        assert_eq!(render_qr(url).unwrap(), qr);
    }

    #[test]
    fn times_render_as_utc_minutes_or_a_dash() {
        assert_eq!(fmt_time(Some(1_700_000_000)), "2023-11-14 22:13Z");
        assert_eq!(fmt_time(Some(0)), "1970-01-01 00:00Z");
        assert_eq!(fmt_time(None), "-");
    }

    #[test]
    fn the_client_table_shows_every_column_and_never_a_digest() {
        let rows = vec![
            serde_json::json!({
                "id": 2, "name": "phone", "mode": "full",
                "created_at": 1_700_000_000, "last_seen_at": 1_700_000_600,
                "revoked_at": serde_json::Value::Null
            }),
            serde_json::json!({
                "id": 1, "name": "old kiosk", "mode": "readonly",
                "created_at": 1_600_000_000, "last_seen_at": serde_json::Value::Null,
                "revoked_at": 1_600_000_900
            }),
        ];
        let t = client_table(&rows);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines.len(), 3, "a header and two rows:\n{t}");
        assert!(
            lines[0].contains("NAME") && lines[0].contains("MODE"),
            "{t}"
        );
        assert!(
            lines[0].contains("CREATED") && lines[0].contains("LAST SEEN"),
            "{t}"
        );
        assert!(lines[0].contains("REVOKED"), "{t}");
        assert!(
            lines[1].contains("phone") && lines[1].contains("full"),
            "{t}"
        );
        assert!(lines[1].contains("2023-11-14 22:13Z"), "{t}");
        // A live client's revoked column is a dash, not an empty gap.
        assert!(lines[1].trim_end().ends_with('-'), "{t}");
        assert!(
            lines[2].contains("old kiosk") && lines[2].contains("readonly"),
            "{t}"
        );
        // The one thing that must never be printed.
        assert!(!t.contains("token"), "{t}");
        // An empty fleet still says something.
        assert_eq!(client_table(&[]), "no paired clients");
    }
}
