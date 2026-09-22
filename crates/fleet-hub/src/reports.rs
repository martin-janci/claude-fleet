//! `fleet-hub reports`: read the error channel back from the running hub
//! over `GET /reports` (master token, loopback), like `client list` does.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{display_width, exchange, fmt_time, hub_conn};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

/// One whole request/response exchange with the local hub, same budget as
/// `pair.rs`'s calls. The response-size cap ([`crate::pair`]'s own
/// `MAX_RESPONSE`) is [`exchange`]'s, not this module's — a full 1000-row
/// page of reports, context included, is well under its 1 MiB.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// `30m`, `2h`, `3d`, or a unix timestamp.
pub fn parse_since(s: &str, now: i64) -> Result<i64, String> {
    let s = s.trim();
    if let Ok(unix) = s.parse::<i64>() {
        return Ok(unix);
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .map_err(|_| format!("--since {s:?}: use 30m, 2h, 3d or a unix timestamp"))?;
    let secs = match unit {
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => {
            return Err(format!(
                "--since {s:?}: use 30m, 2h, 3d or a unix timestamp"
            ))
        }
    };
    Ok(now - n * secs)
}

/// The `GET /reports` response: a JSON array on 200, the hub's own error
/// text (never HTML) otherwise.
pub fn parse_json_response(raw: &str) -> Result<serde_json::Value, String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or("the hub sent a malformed HTTP response")?;
    let status = head
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or("");
    if status != "200" {
        return Err(format!("the hub answered {status}: {}", body.trim()));
    }
    serde_json::from_str(body).map_err(|e| format!("the hub's answer is not JSON: {e}"))
}

/// The `reports` table: a header plus one line per row, newest first (the
/// hub already answers in that order). `width` is the terminal width the
/// MESSAGE column is wrapped to fit; the fixed columns are never truncated.
pub fn table(rows: &[serde_json::Value], width: usize) -> String {
    if rows.is_empty() {
        return "no error reports".to_string();
    }
    let field = |r: &serde_json::Value, k: &str| {
        r.get(k).and_then(|v| v.as_str()).unwrap_or("-").to_string()
    };
    let header = [
        "RECEIVED",
        "ORIGIN",
        "LEVEL",
        "COMPONENT",
        "CODE",
        "MESSAGE",
    ];
    let cells: Vec<[String; 6]> = rows
        .iter()
        .map(|r| {
            [
                fmt_time(r.get("received_at").and_then(|v| v.as_i64())),
                field(r, "origin"),
                field(r, "level"),
                field(r, "component"),
                field(r, "code"),
                field(r, "message")
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            ]
        })
        .collect();
    let mut w = header.map(str::len);
    for row in &cells {
        for (i, c) in row.iter().enumerate().take(5) {
            w[i] = w[i].max(display_width(c));
        }
    }
    let fixed: usize = w[..5].iter().sum::<usize>() + 5 * 2;
    let msg_w = width.saturating_sub(fixed).max(20);
    let mut out = String::new();
    let line = |cols: [&str; 6], out: &mut String| {
        for (i, c) in cols.iter().enumerate().take(5) {
            out.push_str(c);
            out.extend(std::iter::repeat_n(
                ' ',
                w[i].saturating_sub(display_width(c)) + 2,
            ));
        }
        out.push_str(cols[5]);
        out.push('\n');
    };
    line(header, &mut out);
    for (row, r) in cells.iter().zip(rows) {
        // The ellipsis (`…`, 3 UTF-8 bytes) is part of the MESSAGE column's
        // budget, not an addition to it — reserving its byte cost up front
        // keeps a truncated row's line within `width`, rather than one
        // display column short of it but bytes over.
        const ELLIPSIS: char = '…';
        let mut msg: String = row[5]
            .chars()
            .take(msg_w.saturating_sub(ELLIPSIS.len_utf8()))
            .collect();
        if msg.chars().count() < row[5].chars().count() {
            msg.push(ELLIPSIS);
        }
        if r.get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            msg.push_str(" [trunc]");
        }
        line(
            [&row[0], &row[1], &row[2], &row[3], &row[4], &msg],
            &mut out,
        );
    }
    out.trim_end_matches('\n').to_string()
}

/// `fleet-hub reports [--limit] [--since] [--origin] [--json]`: `GET
/// /reports` on the running hub, over loopback with the master token, like
/// `pair`/`client` reach it.
///
/// `--origin`'s only special character is `:` (`client:<name>`,
/// `host:<alias>`); names are validated printable ASCII, so a manual
/// percent-encoding of that one character is enough and saves a dependency
/// a single query parameter does not otherwise need.
pub async fn run(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    limit: u32,
    since: Option<String>,
    origin: Option<String>,
    json: bool,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    let now = fleet_proto::report::now_unix();
    let mut query = format!("limit={}", limit.clamp(1, 1000));
    if let Some(s) = since {
        query.push_str(&format!("&since={}", parse_since(&s, now)?));
    }
    if let Some(o) = origin {
        query.push_str(&format!("&origin={}", o.replace(':', "%3A")));
    }
    let request = format!(
        "GET /reports?{query} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\
         Accept: application/json\r\nConnection: close\r\n\r\n",
        conn.addr, conn.token
    );
    let raw =
        match tokio::time::timeout(CALL_TIMEOUT, exchange(conn.addr, conn.tls, &request)).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(format!(
                    "{} did not answer within {CALL_TIMEOUT:.0?}",
                    conn.addr
                ))
            }
        };
    let rows = parse_json_response(&raw)?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    if json {
        out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
    } else {
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(120);
        out::line(&table(&rows, width));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_accepts_durations_and_unix_seconds() {
        assert_eq!(parse_since("30m", 10_000).unwrap(), 10_000 - 1800);
        assert_eq!(parse_since("2h", 10_000).unwrap(), 10_000 - 7200);
        assert_eq!(parse_since("3d", 1_000_000).unwrap(), 1_000_000 - 259_200);
        assert_eq!(parse_since("1700000000", 0).unwrap(), 1_700_000_000);
        assert!(parse_since("soon", 0).is_err());
        assert!(parse_since("5w", 0).is_err());
    }

    #[test]
    fn the_table_renders_newest_first_and_marks_truncation() {
        let rows = vec![
            serde_json::json!({ "received_at": 1_790_000_000, "origin": "host:box", "level": "error",
                "component": "fleet_agent::conn", "code": null, "message": "dial refused", "truncated": false }),
            serde_json::json!({ "received_at": 1_789_999_000, "origin": "client:desk", "level": "error",
                "component": "frontend:unhandled", "code": "E_PARSE", "message": "x".repeat(300), "truncated": true }),
        ];
        let t = table(&rows, 120);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("RECEIVED"));
        assert!(lines[1].contains("host:box") && lines[1].contains("dial refused"));
        assert!(lines[2].contains("E_PARSE") && lines[2].ends_with("[trunc]"));
        assert!(lines[2].len() <= 120 + "[trunc]".len() + 1);
        assert_eq!(table(&[], 80), "no error reports");
    }

    #[test]
    fn a_json_response_is_parsed_and_a_non_200_is_an_error_with_the_body() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"id\":1}]";
        assert_eq!(
            parse_json_response(ok).unwrap(),
            serde_json::json!([{ "id": 1 }])
        );
        let no = "HTTP/1.1 403 Forbidden\r\n\r\nreports are the master token's to read";
        assert!(parse_json_response(no)
            .unwrap_err()
            .contains("master token"));
    }
}
