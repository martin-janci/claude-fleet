//! `fleet-hub pair` and `fleet-hub client list|revoke`: the operator's side
//! of client access.
//!
//! All three drive the **running** hub rather than the database: a pairing
//! code only means something inside the process that will redeem it (the
//! registry is in memory), and revoking through the live server is what makes
//! the change visible on the very next request. So each command reads the
//! master token out of the data dir, resolves the port AND the TLS mode the
//! daemon runs with (flag > env > stored setting > default — the same
//! precedence `serve`/`init` resolve every `hub.*` value with), and then
//! calls the hub's own `/mcp` — `pair_client`, `list_clients`,
//! `revoke_client` — instead of opening a second write path into the store.
//!
//! The request is written by hand over a `tokio::net::TcpStream` — or, when
//! the hub terminates TLS itself (`--tls cert`), over the same client-side
//! TLS handshake `serve.rs`'s healthcheck probe uses
//! ([`crate::serve::maybe_tls`], [`crate::tls::insecure_probe_client`])
//! rather than a second TLS client: the only endpoint these commands ever
//! talk to is `127.0.0.1`, so an HTTP client crate would be a large
//! dependency for one loopback POST. The write-then-read-to-end is shared
//! too ([`crate::serve::write_and_read`]), parameterised by a
//! `tolerate_partial` flag so this module keeps its original strict
//! behaviour (any read error fails the call) while the healthcheck probe
//! keeps its own tolerance for bytes already read; the connect step (this
//! module's own [`NOT_RUNNING`] wording) and what each side does with the
//! bytes afterward stay separate from the healthcheck probe.

use crate::config::{resolve_data_dir, HubOptions, TlsMode};
use crate::out;
use crate::serve::{existing_db, maybe_tls};
use fleet_core::mcp::settings::McpSettings;
use fleet_core::service::hub::SETTING_TLS;
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
    /// Whether the hub terminates TLS itself (`--tls cert`), resolved the
    /// same way `hub_conn` resolves `addr`'s port.
    tls: bool,
}

/// Read the master token out of the data dir and work out which port the
/// daemon is listening on. Never creates a data dir or a database: a token
/// minted into a fresh one is not this hub's.
///
/// The port follows the same precedence as everywhere else in this binary —
/// **flag > `FLEET_HUB_PORT` > the stored `mcp.port` > the default** — which
/// is what makes the `--port` that `docs/hub.md` documents for these three
/// commands actually work. Reading `mcp.port` alone looked right only because
/// the stored value usually IS the one `serve` runs with; it is not when the
/// daemon was started with a flag or an env value that was never persisted
/// (`healthcheck` resolves its port the same way, minus the store, which it
/// must not open).
///
/// The database is opened **read-only and unmigrated**
/// ([`Store::open_read_only`]). The daemon is running and holds this file; a
/// CLI built from a newer commit than the running `fleet-hub serve` would
/// otherwise apply its own migrations to the live database under the daemon,
/// and nothing here needs more than two `settings` rows.
fn hub_conn(opts: &HubOptions, env: &HashMap<String, String>) -> Result<HubConn, String> {
    let db = existing_db(&resolve_data_dir(opts, env))?;
    let store = fleet_core::store::Store::open_read_only(&db)
        .map_err(|e| format!("failed to read the hub database at {}: {e}", db.display()))?;
    let cfg = McpSettings::read(&store).map_err(|e| e.message)?;
    let token = cfg
        .token
        .ok_or("this hub has no master token yet; run fleet-hub init first")?;
    // `cfg.port` has already applied the default for a missing / unparseable
    // stored value, so it is the last resort here rather than a fourth level.
    let port = match crate::config::pick(
        "--port",
        opts.port.map(|p| p.to_string()),
        env,
        "FLEET_HUB_PORT",
        None,
    )? {
        Some(p) => p.parse::<u16>().map_err(|e| format!("port '{p}': {e}"))?,
        None => cfg.port,
    };
    // TLS: the same precedence `serve`/`init` resolve `hub.tls` with — flag >
    // `FLEET_HUB_TLS` > the stored setting > off. Unlike `healthcheck`, which
    // never opens `state.db` a second time while `serve` holds it for
    // writing, this command already opened it read-only above for the port
    // and the token, so the stored value costs nothing extra to read here.
    let tls = match crate::config::pick(
        "--tls",
        opts.tls.clone(),
        env,
        "FLEET_HUB_TLS",
        store
            .get_setting(SETTING_TLS)
            .map_err(|e| format!("read {SETTING_TLS}: {e}"))?,
    )? {
        Some(v) => TlsMode::parse(&v)?,
        None => TlsMode::default(),
    };
    if tls == TlsMode::Auto {
        return Err(crate::config::ACME_UNAVAILABLE.to_string());
    }
    let tls = tls.terminates_tls();
    // Loopback, like `healthcheck`: the CLI runs next to the daemon, and the
    // master token must not travel over anything but the loopback interface.
    Ok(HubConn {
        addr: SocketAddr::from(([127, 0, 0, 1], port)),
        token,
        tls,
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
    let raw = match tokio::time::timeout(CALL_TIMEOUT, exchange(addr, conn.tls, &request)).await {
        Ok(r) => r?,
        Err(_) => return Err(format!("{addr} did not answer within {CALL_TIMEOUT:.0?}")),
    };
    parse_tool_response(&raw)
}

/// Write `request` to `addr` and read the whole response back — over TLS
/// first when `tls`, reusing the healthcheck probe's own connect/handshake
/// code ([`maybe_tls`]) and its write-then-read-to-end
/// ([`crate::serve::write_and_read`]) rather than a second copy of either.
/// This connect step keeps its own error wording: [`NOT_RUNNING`] is what an
/// operator sees when nothing is listening, which the probe's connect (never
/// pointed at a hub the operator started themselves) has no need to say.
///
/// `tolerate_partial: false` — unlike the probe, this call wants the plain
/// network error when the read itself fails, even if some bytes already
/// arrived: a truncated response would otherwise surface as a confusing
/// downstream JSON-RPC parse failure instead of naming the reset.
async fn exchange(addr: SocketAddr, tls: bool, request: &str) -> Result<String, String> {
    let tcp = tokio::net::TcpStream::connect(addr).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::ConnectionRefused {
            format!("no hub is answering on {addr} — {NOT_RUNNING}")
        } else {
            format!("connect {addr}: {e}")
        }
    })?;
    let conn = maybe_tls(tcp, addr, tls).await?;
    let raw = crate::serve::write_and_read(conn, addr, request, MAX_RESPONSE, false).await?;
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
    // The status token, not a substring: a `contains(" 200")` matched the
    // reason phrase and any header that happened to carry " 200" too.
    let status_code = status.split_whitespace().nth(1);
    if status_code != Some("200") {
        return Err(match status_code {
            Some("401") | Some("403") => format!(
                "the hub refused this token ({status}); the master token in the data dir is not \
                 the one the running hub started with — restart it, or run fleet-hub token show"
            ),
            _ => format!("the hub answered {status}"),
        });
    }
    let payload = fleet_core::mcp::wire::last_event_payload(body);
    let envelope: serde_json::Value =
        serde_json::from_str(&payload).map_err(|e| format!("the hub sent unreadable JSON: {e}"))?;
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

// The SSE de-chunking this used to carry now lives in
// `fleet_core::mcp::wire::last_event_payload`: the desktop's hub-client
// backend has to undo the same framing, and a second copy of a subtle parser
// is how the two drift apart. The tests below still exercise it through
// `parse_tool_response`.

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

/// Terminal columns `s` occupies, so a table of names in any script lines up.
///
/// `chars().count()` is wrong twice over for a client name — a phone is
/// commonly named in the owner's own script, and emoji are ordinary in a
/// device name: a CJK ideograph or an emoji takes two columns, and a
/// combining mark or a zero-width joiner takes none. A full
/// `unicode-width` table is not worth a dependency here (the only consumer is
/// one CLI table), so this covers the ranges a name realistically lands in
/// and falls back to one column, which is what every terminal assumes.
fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn char_width(c: char) -> usize {
    let cp = c as u32;
    let zero_width = matches!(cp,
        0x0300..=0x036F      // combining diacritics
        | 0x200B..=0x200F    // zero-width space … RTL mark
        | 0xFE00..=0xFE0F    // variation selectors (the emoji presentation one)
        | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F // more combining marks
    ) || c == '\u{2060}';
    if zero_width {
        return 0;
    }
    let wide = matches!(cp,
        0x1100..=0x115F      // Hangul Jamo
        | 0x2E80..=0x303E    // CJK radicals, Kangxi, CJK symbols
        | 0x3041..=0x33FF    // kana, Hangul compatibility jamo, CJK compat
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF // CJK, Yi
        | 0xAC00..=0xD7A3    // Hangul syllables
        | 0xF900..=0xFAFF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 // fullwidth forms
        | 0x1F300..=0x1F9FF  // emoji (symbols, pictographs, faces, supplemental)
        | 0x20000..=0x3FFFD  // CJK extensions B…
    );
    if wide {
        2
    } else {
        1
    }
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
            *w = (*w).max(display_width(c));
        }
    }
    // The last column is not padded, so a line never ends in trailing blanks.
    // Padding is counted in terminal columns, not `char`s: `{:<w$}` pads to a
    // char count, which would under-pad a CJK or emoji name (two columns per
    // char) and misalign every column after it.
    let line = |row: &[String; 5]| {
        let mut s = String::new();
        for (i, (cell, w)) in row.iter().zip(width).enumerate() {
            s.push_str(cell);
            if i + 1 == row.len() {
                break;
            }
            s.push_str(&" ".repeat(w.saturating_sub(display_width(cell))));
            s.push_str("  ");
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

    /// A data dir holding a migrated `state.db` with a master token and a
    /// stored `mcp.port`, the way `fleet-hub init` leaves one. The `TempDir`
    /// is returned because it must outlive the calls that read it.
    fn data_dir_with_port(stored: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let store = fleet_core::store::Store::open_with_bus(
            &dir.path().join("state.db"),
            std::sync::Arc::new(fleet_core::events::NoopEventBus),
        )
        .unwrap();
        store
            .set_setting(fleet_core::mcp::SETTING_TOKEN, &"a".repeat(64))
            .unwrap();
        store
            .set_setting(fleet_core::mcp::SETTING_PORT, stored)
            .unwrap();
        dir
    }

    fn opts_for(dir: &tempfile::TempDir, port: Option<u16>) -> HubOptions {
        HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            port,
            ..Default::default()
        }
    }

    /// `docs/hub.md` tells the operator to point `pair` / `client list` /
    /// `client revoke` at "the same data dir and port it runs with
    /// (`--data-dir`, `--port`, or the `FLEET_HUB_*` env)". So the port is
    /// resolved the way `healthcheck` resolves it — flag > env > the stored
    /// `mcp.port` > default — and not read out of the store alone, which
    /// worked only while the stored value happened to be the right one.
    #[test]
    fn the_port_flag_and_env_beat_the_stored_setting() {
        let dir = data_dir_with_port("4180");
        let no_env = HashMap::new();

        // Nothing given: the stored value, as before.
        let conn = hub_conn(&opts_for(&dir, None), &no_env).expect("stored port");
        assert_eq!(conn.addr.port(), 4180);
        assert_eq!(conn.addr.ip(), std::net::IpAddr::from([127, 0, 0, 1]));

        // The flag wins over a DIFFERENT stored value.
        let conn = hub_conn(&opts_for(&dir, Some(4999)), &no_env).expect("--port");
        assert_eq!(conn.addr.port(), 4999, "--port must beat the stored 4180");

        // The env wins over the store, and the flag over the env.
        let env: HashMap<String, String> =
            [("FLEET_HUB_PORT".to_string(), "4998".to_string())].into();
        assert_eq!(
            hub_conn(&opts_for(&dir, None), &env)
                .expect("env")
                .addr
                .port(),
            4998
        );
        assert_eq!(
            hub_conn(&opts_for(&dir, Some(4999)), &env)
                .expect("--port over env")
                .addr
                .port(),
            4999
        );

        // An unparseable env value is an error, not a silent fallback.
        let bad: HashMap<String, String> =
            [("FLEET_HUB_PORT".to_string(), "not-a-port".to_string())].into();
        // (`expect_err` would need `Debug` on `HubConn`, which carries the
        // master token — it deliberately has none.)
        let e = match hub_conn(&opts_for(&dir, None), &bad) {
            Err(e) => e,
            Ok(_) => panic!("an unparseable FLEET_HUB_PORT must be refused"),
        };
        assert!(e.contains("not-a-port"), "{e}");
    }

    /// `hub.tls` resolves the same way `mcp.port` does: flag > env > the
    /// stored setting > off — the precedence `serve`/`init` use for every
    /// `hub.*` value. This is deliberately NOT `healthcheck`'s flag > env >
    /// default, which never reads the store at all because it must not open
    /// `state.db` a second time while `serve` holds it for writing; this
    /// command already opens the store read-only above for the port and the
    /// token, so reading `hub.tls` the same way costs nothing extra.
    #[test]
    fn the_tls_flag_and_env_beat_the_stored_setting() {
        let dir = data_dir_with_port("4180");
        {
            let store = fleet_core::store::Store::open_with_bus(
                &dir.path().join("state.db"),
                std::sync::Arc::new(fleet_core::events::NoopEventBus),
            )
            .unwrap();
            store.set_setting(SETTING_TLS, "cert").unwrap();
        }
        let no_env = HashMap::new();

        // Nothing given: the stored `cert` mode is picked up.
        let conn = hub_conn(&opts_for(&dir, None), &no_env).expect("stored tls");
        assert!(conn.tls, "hub.tls=cert in the store must be picked up");

        // The flag wins over the stored value.
        let mut opts = opts_for(&dir, None);
        opts.tls = Some("off".to_string());
        let conn = hub_conn(&opts, &no_env).expect("--tls off");
        assert!(!conn.tls, "--tls off must beat the stored cert");

        // The env wins over the store.
        let env: HashMap<String, String> =
            [("FLEET_HUB_TLS".to_string(), "off".to_string())].into();
        assert!(!hub_conn(&opts_for(&dir, None), &env).unwrap().tls);

        // A bad value is refused, naming the flag.
        let mut opts = opts_for(&dir, None);
        opts.tls = Some("yes".to_string());
        let e = match hub_conn(&opts, &no_env) {
            Err(e) => e,
            Ok(_) => panic!("--tls yes must be refused"),
        };
        assert!(e.contains("--tls"), "{e}");
    }

    /// `hub.tls=auto` is refused here the same way `config::resolve` refuses
    /// it for `serve`/`init` — `terminates_tls()` is true for both `Auto` and
    /// `Cert`, so without this check `pair`/`client` would silently speak a
    /// client-side TLS handshake `serve` never offered, against a hub that is
    /// actually plaintext.
    #[test]
    fn tls_auto_is_refused_like_resolve_refuses_it() {
        let dir = data_dir_with_port("4180");
        let mut opts = opts_for(&dir, None);
        opts.tls = Some("auto".to_string());
        let no_env = HashMap::new();
        let e = match hub_conn(&opts, &no_env) {
            Err(e) => e,
            Ok(_) => panic!("--tls auto must be refused, not silently treated as cert"),
        };
        assert_eq!(e, crate::config::ACME_UNAVAILABLE);
    }

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

    /// The transport frames one JSON-RPC answer per response today, but SSE
    /// allows a keep-alive comment or a second frame ahead of it, and allows
    /// one event's payload to span several `data:` lines (joined with `\n`).
    /// The old "last `data:` line wins" read both cases wrongly — silently.
    #[test]
    fn several_sse_frames_and_multi_line_data_are_de_chunked() {
        let result = |text: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            )
        };
        // A keep-alive comment and an earlier frame before the answer.
        let raw = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
             : ping\n\nevent: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
            result(r#"[]"#),
            result(r#"{\"code\":\"LATER\"}"#)
        );
        assert_eq!(
            parse_tool_response(&raw).expect("a result")["code"],
            "LATER"
        );

        // One event whose payload is split over several `data:` lines: the
        // spec joins them with a newline, which JSON tolerates inside the
        // envelope. Taking only the last line used to yield unparseable JSON.
        let envelope = result(r#"{\"code\":\"SPLIT\"}"#);
        let (head, tail) = envelope.split_at(envelope.len() / 2);
        let raw = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
             event: message\ndata: {head}\ndata: {tail}\n\n"
        );
        let v = parse_tool_response(&raw).expect("a re-assembled result");
        assert_eq!(v["code"], "SPLIT");
    }

    /// The status line is read as a token. `contains(" 200")` also matched a
    /// reason phrase or a header that happened to carry " 200".
    #[test]
    fn only_a_200_status_token_counts_as_success() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"[]"}]}}"#;
        let raw = format!("HTTP/1.1 500 Internal Error 200 OK\r\n\r\n{body}");
        let e = parse_tool_response(&raw).expect_err("a 500 is not a success");
        assert!(e.contains("500"), "{e}");
        // And a real 200 still parses.
        let ok = format!("HTTP/1.1 200 OK\r\n\r\n{body}");
        assert_eq!(parse_tool_response(&ok).unwrap(), serde_json::json!([]));
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

    /// A phone is commonly named in its owner's own script. Padding by
    /// `chars().count()` under-pads a CJK or emoji name — two terminal
    /// columns per character — and every column after it walks left.
    #[test]
    fn the_table_lines_up_for_wide_and_zero_width_names() {
        let row = |name: &str| {
            serde_json::json!({
                "id": 1, "name": name, "mode": "full",
                "created_at": 1_700_000_000,
                "last_seen_at": serde_json::Value::Null,
                "revoked_at": serde_json::Value::Null
            })
        };
        let t = client_table(&[row("马丁的手机"), row("phone"), row("e\u{301}mile 📱")]);
        let lines: Vec<&str> = t.lines().collect();
        // Every MODE cell starts in the same terminal column.
        let mode_col =
            |l: &str| display_width(&l[..l.find("full").or_else(|| l.find("MODE")).unwrap()]);
        let cols: Vec<usize> = lines.iter().map(|l| mode_col(l)).collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "MODE starts at columns {cols:?}:\n{t}"
        );

        // The width helper itself: wide is 2, combining and ZWJ are 0.
        assert_eq!(display_width("马丁"), 4);
        assert_eq!(display_width("phone"), 5);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width("📱"), 2);
        assert_eq!(display_width("👍\u{fe0f}"), 2);
    }

    /// A minimal, real hub: a `state.db` with a master token, and a
    /// listener speaking the actual MCP transport, optionally behind the
    /// TLS acceptor `tls::acceptor` builds for `--tls cert`. Returns the
    /// `TempDir` (must outlive `pair`/`client_*` calls against it), the
    /// shutdown token and the serve task, which the caller must cancel and
    /// join.
    async fn running_hub(
        tls: Option<crate::tls::HubTls>,
    ) -> (
        tempfile::TempDir,
        tokio_util::sync::CancellationToken,
        tokio::task::JoinHandle<()>,
    ) {
        use fleet_core::events::NoopEventBus;
        use std::sync::{Arc, Mutex};

        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        let token = "b".repeat(64);
        let store = fleet_core::store::Store::open_with_bus(
            &dir.path().join("state.db"),
            Arc::new(NoopEventBus),
        )
        .unwrap();
        store
            .set_setting(fleet_core::mcp::SETTING_TOKEN, &token)
            .unwrap();
        store
            .set_setting(fleet_core::mcp::SETTING_PORT, &port.to_string())
            .unwrap();
        store
            .set_setting(SETTING_TLS, if tls.is_some() { "cert" } else { "off" })
            .unwrap();

        let (shutdown, task) = fleet_core::mcp::start_with_listener(
            Arc::new(Mutex::new(store)),
            Arc::new(fleet_core::ssh::SshClient::new()),
            fleet_core::cancel::CancellationRegistry::new(),
            Arc::new(fleet_core::service::tunnel::TunnelSupervisor::new()),
            fleet_core::mcp::McpGuards::new(Arc::new(
                |_: &fleet_core::mcp::guard::ConfirmRequest| {},
            )),
            listener,
            token,
            vec![],
            None,
            tls,
        )
        .await
        .unwrap();
        (dir, shutdown, task)
    }

    /// The whole point of the task: `fleet-hub pair` against a hub running
    /// `--tls cert` — today's hand-rolled `TcpStream` cannot complete the
    /// handshake at all, so this fails without the fix.
    #[tokio::test]
    async fn pair_reaches_a_hub_that_terminates_tls() {
        let cert_dir = tempfile::tempdir().unwrap();
        let (cert, key, _) = crate::tls::tests::self_signed(cert_dir.path());
        let tls = crate::tls::acceptor(&crate::tls::tests::cert_resolved(cert, key))
            .unwrap()
            .expect("cert mode");

        let (dir, shutdown, task) = running_hub(Some(tls)).await;
        let opts = opts_for(&dir, None);
        let result = pair(&opts, &HashMap::new(), "phone", None, None).await;
        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        assert!(result.is_ok(), "pair over TLS must succeed: {result:?}");
    }

    /// The unchanged path: `fleet-hub pair` against a plaintext hub, which
    /// must keep working exactly as before.
    #[tokio::test]
    async fn pair_reaches_a_plaintext_hub() {
        let (dir, shutdown, task) = running_hub(None).await;
        let opts = opts_for(&dir, None);
        let result = pair(&opts, &HashMap::new(), "phone", None, None).await;
        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        assert!(
            result.is_ok(),
            "pair over plaintext must succeed: {result:?}"
        );
    }
}
