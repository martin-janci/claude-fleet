//! The subcommands' bodies: opening the store, starting the same ticks and
//! server the desktop starts, and stopping them on a signal.

use crate::config::{resolve, resolve_data_dir, HubOptions, Resolved};
use crate::out;
use fleet_core::events::{BroadcastEventBus, EventBus, NoopEventBus};
use fleet_core::mcp::{self, settings::ensure_master_token, McpGuards};
use fleet_core::service::hub::{
    SETTING_ALLOWED_HOSTS, SETTING_ALLOW_PLAINTEXT, SETTING_BIND, SETTING_LOCAL_HOST,
    SETTING_PUBLIC_URL, SETTING_TLS, SETTING_TLS_CERT, SETTING_TLS_KEY,
};
use fleet_core::service::operator::{OPERATOR_HOST, SETTING_OPERATOR_HOST};
use fleet_core::service::projects::LOCAL_HOST;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Open (creating when missing) `<data-dir>/state.db`. The data dir is the
/// only option resolved without the store: everything else reads its stored
/// `hub.*` values.
///
/// [`open_store_with_bus`] with the silent bus — every one-shot subcommand
/// (`init`, `token`, `ssh-key`, `healthcheck`, `pair`). Only `serve` has
/// subscribers to fan events out to.
pub(crate) fn open_store(
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<Store, String> {
    open_store_with_bus(opts, env, Arc::new(NoopEventBus))
}

/// Open (creating when missing) `<data-dir>/state.db`, publishing row changes
/// to `bus`.
pub(crate) fn open_store_with_bus(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    bus: Arc<dyn EventBus>,
) -> Result<Store, String> {
    let data_dir = resolve_data_dir(opts, env);
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create data dir {}: {e}", data_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Not fatal — the hub still runs on a dir it cannot chmod (a mounted
        // volume, a dir owned by someone else) — but never silent: the data
        // dir holds state.db and the master token. Printed through `out.rs`
        // rather than logged, because every subcommand opens the store and
        // only `serve` has logging up by this point; on `serve` the same line
        // reaches the journal as the process's stderr.
        if let Err(e) = std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))
        {
            out::error(&format!(
                "could not restrict the data dir {} to 0700: {e}. \
                 It holds state.db and the master token — check its permissions.",
                data_dir.display()
            ));
        }
    }
    let db_path = data_dir.join("state.db");
    let store = Store::open_with_bus(&db_path, bus).map_err(|e| {
        format!(
            "failed to open the claude-fleet database at {}: {e}\n\
             If the file is corrupt, deleting it resets all hub state — hosts, projects and sessions are re-discovered.",
            db_path.display()
        )
    })?;
    fleet_core::service::provision::set_private_mode(&db_path);
    Ok(store)
}

/// Resolve options against the settings in the opened store, so the checks
/// in `resolve` — the plaintext refusal above all — see the stored values.
fn resolve_with_store(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    bus: Arc<dyn EventBus>,
) -> Result<(Resolved, Arc<Mutex<Store>>), String> {
    let store = open_store_with_bus(opts, env, bus)?;
    let resolved = {
        let settings = |k: &str| store.get_setting(k).ok().flatten();
        resolve(opts, env, &settings)?
    };
    Ok((resolved, Arc::new(Mutex::new(store))))
}

/// Persist the resolved `hub.*` values (and force the API on) so MCP tools
/// that read settings — provisioning above all — see what the process runs with.
fn persist(store: &Mutex<Store>, r: &Resolved) -> Result<(), String> {
    let s = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    let set = |k: &str, v: &str| {
        s.set_setting(k, v)
            .map_err(|e| format!("write setting {k}: {e}"))
    };
    set(mcp::SETTING_ENABLED, "true")?;
    set(mcp::SETTING_PORT, &r.port.to_string())?;
    set(SETTING_BIND, &r.bind.to_string())?;
    set(SETTING_PUBLIC_URL, r.public_url.as_deref().unwrap_or(""))?;
    set(SETTING_ALLOWED_HOSTS, &r.allowed_hosts_explicit.join(","))?;
    set(
        SETTING_LOCAL_HOST,
        if r.local_host { "true" } else { "false" },
    )?;
    set(SETTING_OPERATOR_HOST, &r.operator_host)?;
    set(
        SETTING_ALLOW_PLAINTEXT,
        if r.allow_plaintext { "true" } else { "false" },
    )?;
    set(SETTING_TLS, r.tls.as_str())?;
    // "" for "none given", like every other optional value here: `resolve`
    // reads an empty setting as unset.
    let path = |p: &Option<std::path::PathBuf>| {
        p.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    };
    set(SETTING_TLS_CERT, &path(&r.tls_cert))?;
    set(SETTING_TLS_KEY, &path(&r.tls_key))?;
    if !r.local_host {
        // A state.db copied from a desktop carries a `local` row. Hide it so
        // nothing lists it, and mark it unreachable so nothing counts or polls
        // it either (fleet_health, the account-usage tick); reconcile skips
        // it regardless. `update_host_probe` is the only reachability setter;
        // the row's versions and last ping are written back unchanged.
        let hosts = s.list_hosts().map_err(|e| format!("list hosts: {e}"))?;
        if let Some(local) = hosts.iter().find(|h| h.alias == LOCAL_HOST) {
            s.set_host_hidden(LOCAL_HOST, true)
                .map_err(|e| format!("hide the local host: {e}"))?;
            if local.reachable {
                s.update_host_probe(
                    LOCAL_HOST,
                    false,
                    local.claude_version.as_deref(),
                    local.tmux_version.as_deref(),
                    local.last_pinged_at.unwrap_or(0),
                )
                .map_err(|e| format!("mark the local host unreachable: {e}"))?;
            }
        }
    }
    Ok(())
}

pub fn init(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    regenerate: bool,
) -> Result<ExitCode, String> {
    let (r, store) = resolve_with_store(opts, env, Arc::new(NoopEventBus))?;
    persist(&store, &r)?;
    let token = {
        let s = store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())?;
        if regenerate {
            s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())
                .map_err(|e| e.to_string())?;
        }
        ensure_master_token(&s).map_err(|e| e.message)?
    };
    out::line(&format!("data dir: {}", r.data_dir.display()));
    out::line(&format!("listen:   {}:{}", r.bind, r.port));
    out::line(&format!(
        "public:   {}",
        r.public_url
            .as_deref()
            .unwrap_or("(none — loopback + reverse tunnels)")
    ));
    out::line("master token (shown once; `fleet-hub token show` prints it again):");
    out::line(&token);
    Ok(ExitCode::SUCCESS)
}

pub fn token(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    regenerate: bool,
) -> Result<ExitCode, String> {
    // Only the data dir matters here: `token` serves nothing, so the bind /
    // plaintext checks in `resolve` do not apply. It never creates a data dir
    // or a database: a token minted into a fresh one is not the hub's.
    let db = existing_db(&resolve_data_dir(opts, env))?;
    // `token show` against a hub that is RUNNING is the common case, and this
    // binary may be newer than the daemon's: read the stored token read-only
    // and unmigrated, so printing it cannot reshape the live database. Only
    // the two paths that must WRITE — `regenerate`, and minting the first
    // token into a database that has none — open it for real.
    if !regenerate {
        if let Some(token) = Store::open_read_only(&db)
            .ok()
            .and_then(|s| mcp::settings::McpSettings::read(&s).ok())
            .and_then(|cfg| cfg.token)
        {
            out::line(&token);
            return Ok(ExitCode::SUCCESS);
        }
    }
    let s = open_store(opts, env)?;
    if regenerate {
        s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())
            .map_err(|e| e.to_string())?;
    }
    out::line(&ensure_master_token(&s).map_err(|e| e.message)?);
    Ok(ExitCode::SUCCESS)
}

/// `fleet-hub agent-token <host> [--rotate]`: print the token `fleet-agent
/// install` needs on that host. Only the token goes to stdout, so it can be
/// piped; everything else is a note on stderr.
pub fn agent_token(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    host: &str,
    rotate: bool,
) -> Result<ExitCode, String> {
    // Like `token`: never create a data dir or a database.
    existing_db(&resolve_data_dir(opts, env))?;
    let store = std::sync::Mutex::new(open_store(opts, env)?);
    let t = fleet_core::service::provision::agent_host_token(&store, host, rotate)
        .map_err(|e| e.message)?;
    if t.minted {
        out::error(&format!(
            "note: a new token for {host} is saved; an agent still connected on an older one \
             is cut off within a heartbeat. Install this one on {host}: \
             fleet-agent install --hub <url> --token-file -"
        ));
    }
    if t.mode != "full" {
        out::error(&format!(
            "warning: {host}'s token is {}, and /agent refuses it until its mode is full",
            t.mode
        ));
    }
    out::line(&t.token);
    Ok(ExitCode::SUCCESS)
}

/// `fleet-hub host-token-mode <host> <full|readonly>`: set a provisioned
/// host's control-API token mode, the headless counterpart of the desktop's
/// `set_host_token_mode`.
///
/// It exists because a `readonly` token is refused at `/agent` and **a
/// rotation keeps the mode**, so `agent-token --rotate` cannot undo it:
/// without this, a hub with no desktop beside it had no way back to a working
/// agent. Writes to the same database `agent-token` writes to, so the running
/// hub picks it up on its next check — within a heartbeat, or at once for the
/// next call routed to that host.
pub fn host_token_mode(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    host: &str,
    mode: &str,
) -> Result<ExitCode, String> {
    // Both arguments are checked before the database is opened, so a typo in
    // either one cannot be reported as a problem with the host's token.
    let mode = match mode {
        "full" | "readonly" => mode,
        other => {
            return Err(format!(
                "token mode must be 'full' or 'readonly', got '{other}'"
            ))
        }
    };
    // `host_alias_syntax`, not `host_alias`: the `local` guard the latter adds
    // is about running commands on the hub's own machine, which setting a
    // stored mode does not do.
    fleet_core::validate::host_alias_syntax(host).map_err(|e| e.message)?;
    // Like `token` and `agent-token`: never create a data dir or a database.
    // A mode set in a fresh one would belong to no hub.
    existing_db(&resolve_data_dir(opts, env))?;
    let store = open_store(opts, env)?;
    store
        .set_host_token_mode(host, mode)
        .map_err(|e| e.message)?;
    // Only for the note below; a host row that has gone missing under a token
    // that has not is not this command's problem to report.
    let agent_host = store
        .get_host_row(host)
        .ok()
        .flatten()
        .is_some_and(|h| h.transport == "agent");
    if agent_host {
        out::error(&match mode {
            "readonly" => format!(
                "note: {host} is an agent host, and /agent refuses a readonly token — \
                 any agent connected now is cut off within a heartbeat"
            ),
            _ => format!(
                "note: an agent on {host} that was being refused reconnects by itself \
                 within about a minute; restarting it only hurries that along"
            ),
        });
    }
    out::line(&format!("{host}: token mode is now {mode}"));
    Ok(ExitCode::SUCCESS)
}

/// `<data-dir>/state.db` when it exists; `token` must never create one.
pub(crate) fn existing_db(data_dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let db = data_dir.join("state.db");
    if db.is_file() {
        Ok(db)
    } else {
        Err(format!(
            "no hub database at {}; run fleet-hub init first (or pass --data-dir)",
            db.display()
        ))
    }
}

/// What `ssh-key` does, from which halves of `~/.ssh/id_ed25519` exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    /// Both halves are there: print the public key.
    Print,
    /// The public key is there but its private half is NOT: print it, and say
    /// so. The operator is about to install it in a host's `authorized_keys`,
    /// where it would authorize a key this hub can no longer prove it holds.
    PrintOrphaned,
    /// Only the private key: derive the public half from it.
    Derive,
    /// Neither: generate a new key pair.
    Generate,
}

impl KeyAction {
    /// True when the public key is already on disk and only gets printed.
    #[cfg(test)]
    fn prints(self) -> bool {
        matches!(self, KeyAction::Print | KeyAction::PrintOrphaned)
    }
}

fn key_action(private_exists: bool, public_exists: bool) -> KeyAction {
    match (private_exists, public_exists) {
        (true, true) => KeyAction::Print,
        (false, true) => KeyAction::PrintOrphaned,
        (true, false) => KeyAction::Derive,
        (false, false) => KeyAction::Generate,
    }
}

/// The whole `healthcheck` budget: connect, write and read together. Docker's
/// `HEALTHCHECK --timeout=5s` kills the probe at 5 s, so one deadline for the
/// whole sequence — not three that each restart — is what keeps the worst case
/// under it.
const HEALTHCHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The body `fleet-core`'s unauthenticated `/healthz` route answers with. A
/// literal, not an import: `fleet-core` keeps it private, and the string is
/// the wire contract between the two crates.
const HEALTHZ_MARKER: &str = "fleet-hub ok";

/// Send one unauthenticated `GET /healthz` to `addr` and return the response's
/// status line — but only when the answer is really a fleet hub: an HTTP/1.x
/// status line AND [`HEALTHZ_MARKER`] in the body. Any other listener that
/// happens to hold the port (a proxy, a dev server) answers HTTP too, and used
/// to read as healthy.
///
/// `timeout` is ONE budget for the whole exchange. Per-operation timeouts
/// restarted on every read, so a peer that dripped a byte at a time — or
/// stalled after each of connect, write and read — could hold the probe open
/// for multiples of the budget and outlive Docker's own `--timeout`.
///
/// `tls` must match how the hub serves: a hub terminating TLS answers a
/// plaintext probe with an alert or a dropped connection, and a plaintext hub
/// cannot complete a handshake. The whole budget covers the handshake too.
async fn probe(
    addr: std::net::SocketAddr,
    timeout: std::time::Duration,
    tls: bool,
) -> Result<String, String> {
    match tokio::time::timeout(timeout, probe_exchange(addr, tls)).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "{addr} did not answer within {:.0?}",
            timeout.as_secs_f32()
        )),
    }
}

/// Connect (handshaking when `tls`), then run the exchange; [`probe`] puts the
/// deadline on the whole thing.
async fn probe_exchange(addr: std::net::SocketAddr, tls: bool) -> Result<String, String> {
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;
    let conn = maybe_tls(tcp, addr, tls).await?;
    exchange(conn, addr).await
}

/// Either a plain TCP connection or the client half of a TLS handshake over
/// one, behind one `AsyncRead + AsyncWrite` front — so a caller that already
/// has its own `TcpStream` (this probe's [`exchange`], and `pair::exchange`)
/// can read and write it without knowing which transport [`maybe_tls`]
/// picked.
pub(crate) enum Conn {
    Plain(tokio::net::TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>),
}

impl tokio::io::AsyncRead for Conn {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for Conn {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Wrap an already-connected `tcp` in a TLS client handshake when `tls`,
/// using the probe's no-verification trust story (see
/// [`crate::tls::insecure_probe_client`]): every caller of this dials
/// `127.0.0.1` only, and the hub's certificate is issued for its public
/// domain, which `127.0.0.1` can never match. `pair::exchange` reuses this
/// rather than carrying a second TLS client.
pub(crate) async fn maybe_tls(
    tcp: tokio::net::TcpStream,
    addr: std::net::SocketAddr,
    tls: bool,
) -> Result<Conn, String> {
    if !tls {
        return Ok(Conn::Plain(tcp));
    }
    // The server's certificate is issued for its public domain, so it can
    // never match `127.0.0.1`; `insecure_probe_client` is why that is fine
    // for a liveness probe. The name below is only what goes in SNI.
    let name = rustls_pki_types::ServerName::IpAddress(addr.ip().into());
    let conn = crate::tls::insecure_probe_client()
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS handshake with {addr}: {e}"))?;
    Ok(Conn::Tls(Box::new(conn)))
}

/// Write `request` to `conn` and read the whole response back, capped at
/// `cap` bytes — the one write-then-read-to-end both this probe and
/// [`crate::pair`]'s `exchange` need, over whatever transport [`maybe_tls`]
/// produced.
///
/// `tolerate_partial` is what each caller does when the read itself fails
/// AFTER some bytes already arrived (a peer that answers and then resets —
/// e.g. because it never read this connection's own request out of its
/// receive buffer before closing — has still answered, in one caller's eyes
/// but not the other's):
/// - `true` (this probe, [`exchange`] below): keep the bytes, exactly as
///   [`exchange`]'s liveness check has always tolerated — proven by
///   `healthcheck_fails_on_a_closed_port_or_a_non_http_answer`, whose fixture
///   never drains the client's request and so gets reset after answering.
/// - `false` ([`crate::pair::exchange`]): propagate the read error, same as
///   before this write-then-read was shared — `pair`/`client` want the plain
///   network error, not a downstream JSON-RPC parse failure over truncated
///   bytes.
///
/// An error with NOTHING read yet is always a real failure, in both modes.
///
/// The request text, the cap, and what each caller does with the bytes
/// afterward (validate a liveness body here; parse an HTTP response there)
/// also differ and stay with each caller — only this shape is identical
/// between them.
pub(crate) async fn write_and_read(
    mut conn: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    addr: std::net::SocketAddr,
    request: &str,
    cap: u64,
    tolerate_partial: bool,
) -> Result<Vec<u8>, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    conn.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("send to {addr}: {e}"))?;
    // TLS needs an explicit flush: the record is buffered until one.
    conn.flush()
        .await
        .map_err(|e| format!("send to {addr}: {e}"))?;
    let mut raw = Vec::new();
    let read = conn.take(cap).read_to_end(&mut raw).await;
    if !tolerate_partial || raw.is_empty() {
        read.map_err(|e| format!("read from {addr}: {e}"))?;
    }
    Ok(raw)
}

/// The write-read half, over whatever transport [`probe_exchange`] opened,
/// plus the liveness validation only this probe needs.
async fn exchange<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    conn: S,
    addr: std::net::SocketAddr,
) -> Result<String, String> {
    // Status line plus the short body is all we need; bound what a stray peer
    // can make us read.
    let req = format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let raw = write_and_read(conn, addr, &req, 1024, true).await?;
    if raw.is_empty() {
        return Err(format!("{addr} closed without answering"));
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_string();
    if !status.starts_with("HTTP/1.") {
        return Err(format!("{addr} did not answer HTTP: {status:?}"));
    }
    if !text.contains(HEALTHZ_MARKER) {
        return Err(format!(
            "{addr} answered {status} but not a fleet-hub liveness body: \
             something else is listening on this port"
        ));
    }
    Ok(status)
}

/// `fleet-hub healthcheck`: probe the local listener without opening the
/// store (it runs next to a live `serve`).
///
/// Port: flag > `FLEET_HUB_PORT` > default; the stored `mcp.port` is
/// deliberately not read. **TLS is resolved the same way** — flag >
/// `FLEET_HUB_TLS` > `off` — and the stored `hub.tls` likewise is not read,
/// because reading it would mean opening `state.db` from a second process
/// while `serve` holds it, which is exactly what this subcommand promises not
/// to do. That costs nothing in the setup this exists for: the Dockerfile's
/// `CMD ["fleet-hub", "healthcheck"]` inherits the container's environment,
/// so `FLEET_HUB_TLS=cert` reaches the probe as it reaches `serve`. A hub
/// configured only by stored settings needs `--tls` (or the env) on the probe.
pub async fn healthcheck(
    port: Option<u16>,
    tls: Option<String>,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let port = match (port, env.get("FLEET_HUB_PORT")) {
        (Some(p), _) => p,
        (None, Some(v)) => v
            .trim()
            .parse::<u16>()
            .map_err(|e| format!("FLEET_HUB_PORT '{v}': {e}"))?,
        (None, None) => mcp::DEFAULT_PORT,
    };
    let mode = match (tls, env.get("FLEET_HUB_TLS")) {
        (Some(v), _) => crate::config::TlsMode::parse(v.trim())?,
        (None, Some(v)) => crate::config::TlsMode::parse(v.trim())?,
        (None, None) => crate::config::TlsMode::default(),
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let status = probe(addr, HEALTHCHECK_TIMEOUT, mode.terminates_tls())
        .await
        .map_err(|e| format!("unhealthy: {e}"))?;
    out::line(&format!("healthy: {status}"));
    Ok(ExitCode::SUCCESS)
}

pub fn ssh_key() -> Result<ExitCode, String> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or("HOME is not set")?;
    let ssh_dir = home.join(".ssh");
    let key = ssh_dir.join("id_ed25519");
    let pubkey = key.with_extension("pub");
    // `symlink_metadata`: even a dangling symlink counts as an existing
    // private key, so it is never handed to `ssh-keygen` to overwrite.
    let private_exists = std::fs::symlink_metadata(&key).is_ok();
    match key_action(private_exists, pubkey.exists()) {
        KeyAction::Print => {}
        KeyAction::PrintOrphaned => {
            // Both channels: the log, for a hub whose `serve` has logging up,
            // and one line on STDERR so an operator running `fleet-hub ssh-key`
            // interactively sees it. STDERR, not stdout, keeps
            // `fleet-hub ssh-key | ssh host 'cat >> authorized_keys'` exact.
            tracing::warn!(
                private_key = %key.display(),
                "the private key is missing; the public key below cannot authenticate until it is restored"
            );
            out::error(&format!(
                "warning: the private key {} is missing. The public key below is printed as found, \
                 but this hub cannot authenticate with it until the private key is restored.",
                key.display()
            ));
        }
        KeyAction::Derive => {
            let o = std::process::Command::new("ssh-keygen")
                .arg("-y")
                .arg("-f")
                .arg(&key)
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| format!("run ssh-keygen: {e}"))?;
            if !o.status.success() {
                return Err(format!(
                    "ssh-keygen -y -f {} exited with {}: {}",
                    key.display(),
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                ));
            }
            write_public_key(&pubkey, &o.stdout)?;
        }
        KeyAction::Generate => {
            std::fs::create_dir_all(&ssh_dir).map_err(|e| format!("create ~/.ssh: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&ssh_dir, std::fs::Permissions::from_mode(0o700));
            }
            let st = std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "fleet-hub", "-f"])
                .arg(&key)
                .status()
                .map_err(|e| format!("run ssh-keygen: {e}"))?;
            if !st.success() {
                return Err(format!("ssh-keygen exited with {st}"));
            }
        }
    }
    let text =
        std::fs::read_to_string(&pubkey).map_err(|e| format!("read {}: {e}", pubkey.display()))?;
    out::line(text.trim_end());
    Ok(ExitCode::SUCCESS)
}

/// Write a derived public key as `0644`, refusing to replace a file that
/// appeared in the meantime.
fn write_public_key(path: &std::path::Path, text: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o644);
    }
    let mut f = o
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    f.write_all(text)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        // The umask (0077 under the systemd unit) narrowed the create mode.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    }
    Ok(())
}

pub async fn serve(opts: &HubOptions, env: &HashMap<String, String>) -> Result<ExitCode, String> {
    // The one store in the process that publishes: `serve` is where a paired
    // client can be listening on `GET /events`. Every other subcommand is a
    // one-shot with no subscribers and keeps the silent bus.
    let bus = Arc::new(BroadcastEventBus::default());
    let (r, store) = resolve_with_store(opts, env, Arc::clone(&bus) as Arc<dyn EventBus>)?;
    match fleet_core::logging::init_in_with(&r.log_dir, true) {
        Ok(dir) => tracing::info!(log_dir = %dir.display(), "file logging on"),
        Err(e) => {
            fleet_core::logging::init_stderr_fallback();
            tracing::warn!(error = %e, "file logging unavailable; logging to stderr only");
        }
    }
    persist(&store, &r)?;
    if !r.local_host {
        // Before the control API and the ticks start: from here on every
        // tool or command naming host `local` is refused with E_NOTFOUND
        // instead of running on this machine.
        fleet_core::service::hub::disable_local_host();
        if r.operator_host == OPERATOR_HOST {
            // Not an error: the fleet works without the agent panel. But the
            // panel will say "nowhere to start it" until this is set, and
            // the log is where the operator looks first.
            tracing::warn!(
                "the UX agent's operator is homed on `local`, which this hub does not have \
                 (local_host=false); the agent panel is unavailable until \
                 --operator-host / FLEET_HUB_OPERATOR_HOST names a fleet host"
            );
        }
    }
    let token = {
        let s = store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())?;
        ensure_master_token(&s).map_err(|e| e.message)?
    };
    let base = r.base()?;

    // Routed, not SSH-only: a host row whose `transport` is `'agent'` is
    // reached through the `fleet-agent` connected for it, every other host
    // over SSH exactly as before. The registry comes back out of
    // `ssh.agent_registry()` for the `/agent` endpoint to register on. The
    // desktop builds `SshClient::new()` instead and routes nothing.
    let ssh = Arc::new(fleet_core::ssh::SshClient::with_agents(
        fleet_core::agent::AgentRegistry::new(),
        Arc::clone(&store),
    ));
    let reg = fleet_core::cancel::CancellationRegistry::new();
    let tunnels = Arc::new(fleet_core::service::tunnel::TunnelSupervisor::new());
    // No desktop to approve a destructive-call confirmation: log it. The
    // `mcp.confirm_destructive` setting is off by default; docs/hub.md says
    // to leave it off on a hub.
    let guards = McpGuards::new(Arc::new(|req: &fleet_core::mcp::guard::ConfirmRequest| {
        tracing::warn!(
            tool = %req.tool,
            nonce = %req.nonce,
            "confirmation requested but this hub has no approver; disable mcp.confirm_destructive"
        );
    }))
    .without_approver();

    warn_if_confirm_destructive(&store);

    // Before the listener exists: a hub told to serve TLS that cannot build an
    // acceptor must exit 1 with the reason, never fall back to plaintext on
    // the port a client expects to be encrypted.
    let tls = crate::tls::acceptor(&r)?;
    let addr = std::net::SocketAddr::from((r.bind, r.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not bind {addr}: {e}"))?;

    // Task 5: close every unresolved `session_move_waiting` as
    // `hub_restarted` before the listener below can serve a single request —
    // a waiter lives only in this process's memory, so a wait this restarted
    // process cannot possibly still be honouring must not linger for a
    // reopened desktop to show. Must run before `mcp::start_with_listener`,
    // not merely before `serve` returns: that call hands the bound
    // `listener` to the axum server and awaits only its setup, not a client
    // — a request can be served the instant it returns. Logged, never fatal:
    // a store this broken already fails `persist`/`ensure_master_token`
    // above.
    match fleet_core::service::move_session::wait::sweep_unresolved_waits(&store) {
        Ok(0) => {}
        Ok(n) => tracing::info!("closed {n} stale move-wait(s) as hub_restarted at startup"),
        Err(e) => tracing::warn!("sweep_unresolved_waits at startup failed: {e}"),
    }

    let (shutdown, serve_task) = mcp::start_with_listener(
        Arc::clone(&store),
        Arc::clone(&ssh),
        Arc::clone(&reg),
        Arc::clone(&tunnels),
        guards,
        listener,
        token,
        r.allowed_hosts.clone(),
        // One fresh subscription per `GET /events` connection, plus the
        // replay history a reconnecting client resumes from.
        Some(Arc::clone(&bus).into()),
        tls,
    )
    .await?;
    if let Err(e) = fleet_core::service::provision::reestablish_tunnels(&store, &tunnels, &base) {
        tracing::warn!(error = %e.message, "re-establishing host tunnels failed");
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        public = ?r.public_url,
        bind = %r.bind,
        port = r.port,
        local_host = r.local_host,
        operator_host = %r.operator_host,
        tls = r.tls.as_str(),
        "fleet-hub serving"
    );

    // Cancelled on shutdown, below, so both ticks stop between passes rather
    // than being torn down along with the SSH masters they may still be
    // using mid-pass (issue #144). One token for both: SIGTERM has no reason
    // to stop them at different times.
    let ticks_cancel = CancellationToken::new();
    let reconcile_handle = fleet_core::service::tick::spawn_reconcile_tick(
        Arc::clone(&store),
        Arc::clone(&ssh),
        ticks_cancel.clone(),
    );
    let usage_cache = Arc::new(Mutex::new(
        fleet_core::service::account_usage::UsageCache::new(),
    ));
    let usage_handle = fleet_core::service::tick::spawn_account_usage_tick(
        Arc::clone(&store),
        Arc::clone(&ssh),
        usage_cache,
        // The same bus the store publishes to: `account_usage:updated` is a
        // tick-borne event, not a store write, and a client following
        // `/events` wants it like any other.
        Arc::clone(&bus) as Arc<dyn EventBus>,
        ticks_cancel.clone(),
    );
    // Hub↔hub federation: one exchange loop per dialer link in state.db,
    // rescanned every 5 s so a CLI `peer add` / `peer remove` lands without
    // a restart. Stopped with the ticks.
    let peer_handle = fleet_core::service::peer::supervisor::spawn_peer_supervisor(
        Arc::clone(&store),
        Arc::clone(&ssh),
        Arc::new(fleet_core::http_client::TcpTransport),
        ticks_cancel.clone(),
    );
    // Trackers (work graph M3.3): the hub owns its fleet, so it syncs them
    // (`work.sync_interval_secs`, `0` = off). A paired desktop never does.
    // `via_host` / `via_cli` trackers (M6) run `curl` / `gh` on a host over
    // the hub's own SSH.
    fleet_core::service::trackers::install_default_net(
        fleet_core::service::trackers::TrackerNet::real(Some(
            Arc::clone(&ssh) as Arc<dyn fleet_core::ssh::SshExec>
        )),
    );
    let tracker_handle = fleet_core::service::trackers::sync::spawn_tracker_sync(
        Arc::clone(&store),
        fleet_core::service::trackers::default_net(),
        ticks_cancel.clone(),
    );

    wait_for_signal().await?;
    tracing::info!("fleet-hub stopping");
    // Cancel the ticks BEFORE tearing down the MCP server below: cancellation
    // is only observed between passes, so cancelling here is strictly safe
    // (it does not abort a pass already in flight) and it stops a new pass
    // from starting during the drain. That matters because `/agent`
    // websockets are served by the same axum app `shutdown` tears down — an
    // in-flight pass otherwise keeps its SSH masters but loses every
    // fleet-agent connection mid-drain. This way the in-flight pass gets the
    // drain window plus TICK_SHUTDOWN_TIMEOUT below, with agent connections
    // still up.
    ticks_cancel.cancel();
    shutdown.cancel();
    // Let in-flight requests drain before tearing down what they use.
    match tokio::time::timeout(DRAIN_TIMEOUT, serve_task).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "control API task ended abnormally"),
        Err(_) => tracing::warn!(
            timeout_secs = DRAIN_TIMEOUT.as_secs(),
            "in-flight requests did not drain in time; exiting anyway"
        ),
    }
    // The hub-daemon spec's SIGTERM promise (docs/superpowers/specs/2026-09-17-hub-daemon-design.md):
    // the current reconcile pass finishes, then the loop exits. Await the
    // (already-cancelled, above) ticks' handles, bounded, BEFORE tearing down
    // the SSH masters a pass in flight might still be using.
    let mut tick_handles = Vec::new();
    if let Some(h) = reconcile_handle {
        tick_handles.push(h);
    }
    tick_handles.push(usage_handle);
    tick_handles.push(peer_handle);
    if let Some(h) = tracker_handle {
        tick_handles.push(h);
    }
    await_ticks(tick_handles, TICK_SHUTDOWN_TIMEOUT).await;
    tunnels.stop_all();
    ssh.shutdown_all();
    Ok(ExitCode::SUCCESS)
}

/// How long `serve` waits for in-flight requests after a stop signal.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long `serve` waits, after cancelling the reconcile/usage ticks, for
/// their in-flight passes to finish before tearing down the SSH masters they
/// might still be using anyway. Same order of magnitude as [`DRAIN_TIMEOUT`]
/// — a pass this slow is already unusual, and `ssh.shutdown_all()` cannot
/// wait forever on process shutdown.
const TICK_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Await every tick's `JoinHandle`, bounded by `timeout`: a single stuck
/// pass cannot block shutdown forever. Logs at warn (and proceeds) when the
/// bound is hit, and when a tick task itself panicked.
async fn await_ticks(handles: Vec<tokio::task::JoinHandle<()>>, timeout: std::time::Duration) {
    let join_all = async {
        for h in handles {
            if let Err(e) = h.await {
                tracing::warn!(error = %e, "a background tick task panicked");
            }
        }
    };
    if tokio::time::timeout(timeout, join_all).await.is_err() {
        tracing::warn!(
            timeout_secs = timeout.as_secs(),
            "reconcile/usage ticks did not finish their in-flight pass within the timeout; \
             tearing down SSH masters anyway"
        );
    }
}

/// A state.db copied from a desktop can carry `mcp.confirm_destructive=true`;
/// a hub has no approver, so every destructive tool would be refused. Say so
/// once at startup (no behaviour change).
fn warn_if_confirm_destructive(store: &Mutex<Store>) {
    let on = store
        .lock()
        .ok()
        .and_then(|s| mcp::settings::McpSettings::read(&s).ok())
        .is_some_and(|m| m.confirm_destructive);
    if on {
        tracing::warn!(
            setting = "mcp.confirm_destructive",
            "mcp.confirm_destructive is on, but destructive tools cannot be approved on a hub \
             (no desktop approver): they will be refused with E_CONFIRM_REQUIRED; turn the setting off"
        );
    }
}

async fn wait_for_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term =
            signal(SignalKind::terminate()).map_err(|e| format!("install SIGTERM handler: {e}"))?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(local_host: bool) -> Resolved {
        Resolved {
            data_dir: "/unused".into(),
            bind: "0.0.0.0".parse().unwrap(),
            port: 4190,
            public_url: Some("https://fleet.example.com".into()),
            allowed_hosts: vec!["b.example.com:8443".into(), "fleet.example.com".into()],
            allowed_hosts_explicit: vec!["b.example.com:8443".into()],
            local_host,
            operator_host: "devbox".into(),
            allow_plaintext: false,
            log_dir: "/unused/logs".into(),
            tls: crate::config::TlsMode::Off,
            tls_cert: None,
            tls_key: None,
        }
    }

    /// A file-backed store (the in-memory constructor is core-test-only);
    /// the `TempDir` must outlive the store.
    fn store_with_local_row() -> (tempfile::TempDir, Mutex<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
        s.insert_host("local", None).unwrap();
        s.insert_host("devbox", Some("devbox")).unwrap();
        (dir, Mutex::new(s))
    }

    #[test]
    fn persist_writes_hub_settings_and_hides_a_copied_local_row() {
        let (_dir, store) = store_with_local_row();
        persist(&store, &resolved(false)).unwrap();
        let s = store.lock().unwrap();
        let get = |k: &str| s.get_setting(k).unwrap();
        assert_eq!(get(mcp::SETTING_ENABLED).as_deref(), Some("true"));
        assert_eq!(get(mcp::SETTING_PORT).as_deref(), Some("4190"));
        assert_eq!(get(SETTING_BIND).as_deref(), Some("0.0.0.0"));
        assert_eq!(
            get(SETTING_PUBLIC_URL).as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(
            get(SETTING_ALLOWED_HOSTS).as_deref(),
            Some("b.example.com:8443")
        );
        assert_eq!(get(SETTING_LOCAL_HOST).as_deref(), Some("false"));
        assert_eq!(get(SETTING_ALLOW_PLAINTEXT).as_deref(), Some("false"));
        assert_eq!(
            get(fleet_core::service::operator::SETTING_OPERATOR_HOST).as_deref(),
            Some("devbox"),
            "the operator's home is saved like every other resolved value"
        );
        let local = s.get_host_row("local").unwrap().unwrap();
        assert!(local.hidden);
        assert!(!s.get_host_row("devbox").unwrap().unwrap().hidden);
    }

    #[test]
    fn persist_marks_a_copied_local_row_unreachable() {
        let (_dir, store) = store_with_local_row();
        {
            // As copied from a desktop: `local` was reachable there.
            let s = store.lock().unwrap();
            s.update_host_probe("local", true, Some("2.1.0"), Some("3.4"), 1234)
                .unwrap();
            s.update_host_probe("devbox", true, None, None, 1).unwrap();
        }
        persist(&store, &resolved(false)).unwrap();
        let s = store.lock().unwrap();
        let local = s.get_host_row("local").unwrap().unwrap();
        assert!(local.hidden);
        assert!(!local.reachable, "health and usage polling skip it");
        assert_eq!(local.claude_version.as_deref(), Some("2.1.0"));
        assert_eq!(local.last_pinged_at, Some(1234));
        assert!(s.get_host_row("devbox").unwrap().unwrap().reachable);
    }

    #[test]
    fn a_routable_bind_flag_is_checked_against_the_stored_public_url() {
        // `init --public-url https://…` stored the URL; a later
        // `serve --bind 0.0.0.0` must see it before the plaintext check.
        let dir = tempfile::tempdir().unwrap();
        {
            let s =
                Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
            s.set_setting(SETTING_PUBLIC_URL, "https://fleet.example.com")
                .unwrap();
        }
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            bind: Some("0.0.0.0".into()),
            ..HubOptions::default()
        };
        let (r, _store) =
            resolve_with_store(&opts, &HashMap::new(), Arc::new(NoopEventBus)).unwrap();
        assert_eq!(r.public_url.as_deref(), Some("https://fleet.example.com"));
    }

    #[test]
    fn token_ignores_a_stored_plaintext_bind() {
        // `serve --bind 100.64.0.1 --allow-plaintext` persisted `hub.bind`;
        // `token show` serves nothing, so it must not demand the flag.
        let dir = tempfile::tempdir().unwrap();
        {
            let s =
                Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
            s.set_setting(SETTING_BIND, "100.64.0.1").unwrap();
        }
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        assert!(token(&opts, &HashMap::new(), false).is_ok());
    }

    #[test]
    fn a_persisted_plaintext_allowance_survives_a_bare_resolve() {
        // `serve --bind 0.0.0.0 --allow-plaintext` persists; a later bare
        // `serve` against the same data dir must not be refused.
        let (dir, store) = store_with_local_row();
        let mut r = resolved(false);
        r.public_url = None;
        r.allowed_hosts = vec![];
        r.allowed_hosts_explicit = vec![];
        r.allow_plaintext = true;
        persist(&store, &r).unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_setting(SETTING_ALLOW_PLAINTEXT)
                .unwrap()
                .as_deref(),
            Some("true")
        );
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let (back, _s) =
            resolve_with_store(&opts, &HashMap::new(), Arc::new(NoopEventBus)).unwrap();
        assert!(back.allow_plaintext);
        assert_eq!(back.bind.to_string(), "0.0.0.0");
        // And `FLEET_HUB_ALLOW_PLAINTEXT=0` turns it off again.
        let off: HashMap<String, String> =
            [("FLEET_HUB_ALLOW_PLAINTEXT".to_string(), "0".to_string())].into();
        assert!(resolve_with_store(&opts, &off, Arc::new(NoopEventBus)).is_err());
    }

    #[test]
    fn agent_token_mints_for_an_agent_host_and_refuses_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        {
            let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
            s.insert_host("laptop", Some("laptop")).unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            s.insert_host("mefistos", Some("mefistos")).unwrap();
        }
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        assert!(agent_token(&opts, &HashMap::new(), "laptop", false).is_ok());
        let first = Store::open_read_only(&db)
            .unwrap()
            .get_host_token("laptop")
            .unwrap()
            .expect("minted and saved");
        assert!(agent_token(&opts, &HashMap::new(), "laptop", true).is_ok());
        let rotated = Store::open_read_only(&db)
            .unwrap()
            .get_host_token("laptop")
            .unwrap()
            .unwrap();
        assert_ne!(rotated.token, first.token);

        let err = agent_token(&opts, &HashMap::new(), "mefistos", false).unwrap_err();
        assert!(err.contains("not an agent host"), "{err}");
        assert!(agent_token(&opts, &HashMap::new(), "nobody", false).is_err());
        let missing = HubOptions {
            data_dir: Some(dir.path().join("elsewhere")),
            ..HubOptions::default()
        };
        assert!(agent_token(&missing, &HashMap::new(), "laptop", false).is_err());
        assert!(!dir.path().join("elsewhere").exists());
    }

    #[test]
    fn host_token_mode_flips_a_readonly_token_back_to_full() {
        // The command that exists because `/agent` refuses a `readonly`
        // token and rotating keeps the mode: a hub operator with no desktop
        // has to be able to set it back.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        {
            let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
            s.insert_host("laptop", Some("laptop")).unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            s.upsert_host_token("laptop", "t0").unwrap();
            s.set_host_token_mode("laptop", "readonly").unwrap();
            s.insert_host("untokened", Some("untokened")).unwrap();
        }
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let mode_of = |host: &str| {
            Store::open_read_only(&db)
                .unwrap()
                .get_host_token(host)
                .unwrap()
                .map(|t| t.mode)
        };
        assert_eq!(mode_of("laptop").as_deref(), Some("readonly"));

        assert!(host_token_mode(&opts, &HashMap::new(), "laptop", "full").is_ok());
        assert_eq!(mode_of("laptop").as_deref(), Some("full"));
        // The token itself is untouched: this is not a rotation, so an agent
        // already holding it keeps working.
        assert_eq!(
            Store::open_read_only(&db)
                .unwrap()
                .get_host_token("laptop")
                .unwrap()
                .unwrap()
                .token,
            "t0"
        );

        // And back, so it is the mode that is set rather than a one-way fix.
        assert!(host_token_mode(&opts, &HashMap::new(), "laptop", "readonly").is_ok());
        assert_eq!(mode_of("laptop").as_deref(), Some("readonly"));

        // An unknown mode is refused by name, and changes nothing.
        let err = host_token_mode(&opts, &HashMap::new(), "laptop", "Full").unwrap_err();
        assert!(err.contains("full") && err.contains("readonly"), "{err}");
        assert_eq!(mode_of("laptop").as_deref(), Some("readonly"));

        // A host with no token at all, and a host that does not exist.
        assert!(host_token_mode(&opts, &HashMap::new(), "untokened", "full").is_err());
        assert!(mode_of("untokened").is_none());
        assert!(host_token_mode(&opts, &HashMap::new(), "nobody", "full").is_err());

        // Like `token` and `agent-token`: never create a data dir or a database.
        let missing = HubOptions {
            data_dir: Some(dir.path().join("elsewhere")),
            ..HubOptions::default()
        };
        assert!(host_token_mode(&missing, &HashMap::new(), "laptop", "full").is_err());
        assert!(!dir.path().join("elsewhere").exists());
    }

    #[test]
    fn token_refuses_a_missing_database_without_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("hub");
        let err = existing_db(&data_dir).unwrap_err();
        assert!(
            err.contains(&data_dir.join("state.db").display().to_string())
                && err.contains("run fleet-hub init first (or pass --data-dir)"),
            "{err}"
        );
        let opts = HubOptions {
            data_dir: Some(data_dir.clone()),
            ..HubOptions::default()
        };
        assert!(token(&opts, &HashMap::new(), false).is_err());
        assert!(token(&opts, &HashMap::new(), true).is_err());
        assert!(!data_dir.exists(), "token created the data dir");
        // Once init has run, the path is accepted.
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("state.db"), b"").unwrap();
        assert_eq!(existing_db(&data_dir).unwrap(), data_dir.join("state.db"));
    }

    #[test]
    fn ssh_key_derives_from_a_lone_private_key_and_never_regenerates_it() {
        assert_eq!(key_action(true, true), KeyAction::Print);
        assert_eq!(key_action(true, false), KeyAction::Derive);
        assert_eq!(key_action(false, false), KeyAction::Generate);
    }

    #[test]
    fn ssh_key_flags_a_public_key_whose_private_half_is_gone() {
        // Printing it silently invites the operator to install an authorized
        // key this hub cannot authenticate with.
        assert_eq!(key_action(false, true), KeyAction::PrintOrphaned);
        assert!(KeyAction::PrintOrphaned.prints());
        assert!(KeyAction::Print.prints());
    }

    /// An in-memory stream that hands back `first` on its first read, then
    /// fails every read after with `kind` — a deterministic stand-in for a
    /// peer that answers and then resets, independent of any real socket's
    /// RST timing (which is what the fixture below relies on today).
    struct PartialThenError {
        first: Option<Vec<u8>>,
        kind: std::io::ErrorKind,
    }

    impl tokio::io::AsyncRead for PartialThenError {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if let Some(bytes) = self.first.take() {
                buf.put_slice(&bytes);
                return std::task::Poll::Ready(Ok(()));
            }
            std::task::Poll::Ready(Err(std::io::Error::new(self.kind, "peer went away")))
        }
    }

    impl tokio::io::AsyncWrite for PartialThenError {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// `pair::exchange` must keep its original strict behaviour: ANY read
    /// error is a failure, even with bytes already in hand. Before
    /// `tolerate_partial` existed, `write_and_read` had one behaviour shared
    /// by both callers, and this is exactly the case that behaviour got
    /// wrong for `pair` — a reset after partial data used to come back as
    /// `Ok(partial)` instead of the network error.
    #[tokio::test]
    async fn write_and_read_strict_propagates_a_read_error_even_with_bytes_in_hand() {
        let addr: std::net::SocketAddr = "127.0.0.1:4180".parse().unwrap();
        let conn = PartialThenError {
            first: Some(b"partial".to_vec()),
            kind: std::io::ErrorKind::ConnectionReset,
        };
        let e = write_and_read(conn, addr, "GET / HTTP/1.1\r\n\r\n", 1024, false)
            .await
            .expect_err("pair's strict mode must not swallow a read error");
        assert!(e.contains("read from"), "{e}");
    }

    /// The healthcheck probe keeps ITS original tolerance: bytes already read
    /// survive a later read error, which is what lets it read the SSH banner
    /// in `healthcheck_fails_on_a_closed_port_or_a_non_http_answer` below even
    /// though the fixture there resets the connection.
    #[tokio::test]
    async fn write_and_read_tolerant_keeps_bytes_already_read_despite_a_later_error() {
        let addr: std::net::SocketAddr = "127.0.0.1:4180".parse().unwrap();
        let conn = PartialThenError {
            first: Some(b"partial".to_vec()),
            kind: std::io::ErrorKind::ConnectionReset,
        };
        let raw = write_and_read(conn, addr, "GET / HTTP/1.1\r\n\r\n", 1024, true)
            .await
            .expect("the probe's tolerant mode must keep bytes already read");
        assert_eq!(raw, b"partial");
    }

    /// One listener that answers a single request with `reply`, handing back
    /// the raw request it saw.
    fn one_shot(reply: &'static [u8]) -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = conn.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            conn.write_all(reply).unwrap();
            req
        });
        (addr, server)
    }

    #[tokio::test]
    async fn healthcheck_probes_healthz_and_accepts_the_liveness_body() {
        let (addr, server) = one_shot(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\n\
              content-length: 13\r\nconnection: close\r\n\r\nfleet-hub ok\n",
        );
        let status = probe(addr, HEALTHCHECK_TIMEOUT, false).await.unwrap();
        assert_eq!(status, "HTTP/1.1 200 OK");
        let req = server.join().unwrap();
        assert!(req.starts_with("GET /healthz HTTP/1.1\r\n"), "{req}");
        assert!(
            req.contains(&format!("Host: 127.0.0.1:{}\r\n", addr.port())),
            "{req}"
        );
        assert!(
            !req.to_ascii_lowercase().contains("authorization"),
            "the probe must carry no credential: {req}"
        );
    }

    #[tokio::test]
    async fn healthcheck_rejects_a_200_from_an_unrelated_listener() {
        // The old probe passed on any HTTP status line, so any process that
        // happened to hold the port read as a healthy hub.
        let (addr, server) = one_shot(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 5\r\n\
              connection: close\r\n\r\nhello",
        );
        let err = probe(addr, HEALTHCHECK_TIMEOUT, false).await.unwrap_err();
        assert!(err.contains("fleet-hub"), "{err}");
        server.join().unwrap();
    }

    /// Docker's `HEALTHCHECK … --timeout=5s` kills the probe at 5 s, so the
    /// whole connect-write-read sequence must answer inside ONE budget.
    #[tokio::test]
    async fn healthcheck_gives_up_on_a_listener_that_never_answers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (conn, _) = listener.accept().unwrap();
            // Accept and hold: never write, never close.
            std::thread::sleep(std::time::Duration::from_secs(8));
            drop(conn);
        });
        let started = std::time::Instant::now();
        let err = probe(addr, HEALTHCHECK_TIMEOUT, false).await.unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "docker kills the probe at 5 s; this took {elapsed:?} ({err})"
        );
        drop(server);
    }

    /// The three per-operation timeouts each restarted on every read, so a
    /// peer that dripped one byte at a time held the probe open indefinitely.
    #[tokio::test]
    async fn healthcheck_gives_up_within_one_budget_when_the_peer_drips() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            for b in b"HTTP/1.1 200 OK\r\nx: ".iter() {
                if conn.write_all(&[*b]).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        });
        let budget = std::time::Duration::from_millis(300);
        let started = std::time::Instant::now();
        assert!(probe(addr, budget, false).await.is_err());
        let elapsed = started.elapsed();
        assert!(
            elapsed < budget * 4,
            "one budget of {budget:?} must bound the whole probe; took {elapsed:?}"
        );
        drop(server);
    }

    #[tokio::test]
    async fn healthcheck_fails_on_a_closed_port_or_a_non_http_answer() {
        use std::io::Write;
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        assert!(probe(closed, HEALTHCHECK_TIMEOUT, false).await.is_err());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").unwrap();
        });
        let err = probe(addr, HEALTHCHECK_TIMEOUT, false).await.unwrap_err();
        assert!(err.contains("SSH-2.0"), "{err}");
        server.join().unwrap();
    }

    /// The Dockerfile runs `fleet-hub healthcheck` against the hub's own port.
    /// With `FLEET_HUB_TLS=cert` that port speaks only TLS, so a probe that
    /// cannot must report the container permanently unhealthy — and the TLS
    /// probe must succeed where it does.
    #[tokio::test]
    async fn the_probe_speaks_tls_when_the_hub_does() {
        use fleet_core::events::NoopEventBus;

        let dir = tempfile::tempdir().unwrap();
        let (cert, key, _) = crate::tls::tests::self_signed(dir.path());
        let tls = crate::tls::acceptor(&crate::tls::tests::cert_resolved(cert, key))
            .unwrap()
            .expect("cert mode");

        let store =
            Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, task) = mcp::start_with_listener(
            Arc::new(Mutex::new(store)),
            Arc::new(fleet_core::ssh::SshClient::new()),
            fleet_core::cancel::CancellationRegistry::new(),
            Arc::new(fleet_core::service::tunnel::TunnelSupervisor::new()),
            McpGuards::new(Arc::new(|_: &fleet_core::mcp::guard::ConfirmRequest| {})),
            listener,
            "test-token".to_string(),
            vec![],
            None,
            Some(tls),
        )
        .await
        .unwrap();

        // The TLS probe reaches /healthz despite the certificate naming a
        // public domain rather than 127.0.0.1 (verification is off by design).
        let status = probe(addr, HEALTHCHECK_TIMEOUT, true).await.unwrap();
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");

        // The plaintext probe — today's `fleet-hub healthcheck` — does not.
        let err = probe(addr, HEALTHCHECK_TIMEOUT, false).await.unwrap_err();
        assert!(
            !err.is_empty(),
            "a plaintext probe of a TLS hub must report unhealthy"
        );

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    /// ... and the other way round: a plaintext hub answers the plaintext
    /// probe, which is the unchanged default.
    #[tokio::test]
    async fn the_plaintext_probe_still_answers_a_plaintext_hub() {
        let (addr, server) = one_shot(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\n\
              content-length: 13\r\nconnection: close\r\n\r\nfleet-hub ok\n",
        );
        assert_eq!(
            probe(addr, HEALTHCHECK_TIMEOUT, false).await.unwrap(),
            "HTTP/1.1 200 OK"
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn healthcheck_resolves_tls_from_the_flag_then_the_env() {
        // No hub is listening, so every call fails — what is asserted is HOW
        // it fails, which says which transport the probe chose.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // A bad value is named, from the flag and from the env alike.
        let e = healthcheck(Some(port), Some("yes".into()), &env(&[]))
            .await
            .unwrap_err();
        assert!(e.contains("--tls"), "{e}");
        let e = healthcheck(Some(port), None, &env(&[("FLEET_HUB_TLS", "yes")]))
            .await
            .unwrap_err();
        assert!(e.contains("--tls"), "{e}");
        // The flag beats the env.
        assert!(
            healthcheck(
                Some(port),
                Some("off".into()),
                &env(&[("FLEET_HUB_TLS", "yes")])
            )
            .await
            .unwrap_err()
            .contains("unhealthy"),
            "the flag's `off` must win over the env's garbage"
        );
    }

    #[test]
    fn persist_leaves_the_local_row_visible_when_it_is_a_fleet_host() {
        let (_dir, store) = store_with_local_row();
        let mut r = resolved(true);
        r.public_url = None;
        persist(&store, &r).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(
            s.get_setting(SETTING_PUBLIC_URL).unwrap().as_deref(),
            Some("")
        );
        assert_eq!(
            s.get_setting(SETTING_LOCAL_HOST).unwrap().as_deref(),
            Some("true")
        );
        assert!(!s.get_host_row("local").unwrap().unwrap().hidden);
    }

    #[test]
    fn a_persisted_store_resolves_back_to_the_same_values() {
        let (_dir, store) = store_with_local_row();
        let mut r = resolved(false);
        r.bind = "127.0.0.1".parse().unwrap();
        r.allow_plaintext = true;
        persist(&store, &r).unwrap();
        let s = store.lock().unwrap();
        let settings = |k: &str| s.get_setting(k).ok().flatten();
        let opts = HubOptions {
            data_dir: Some("/unused".into()),
            ..HubOptions::default()
        };
        let back = resolve(&opts, &HashMap::new(), &settings).unwrap();
        assert_eq!(back, r);
    }

    // --- shutdown ordering (issue #144) --------------------------------
    //
    // `spawn_reconcile_tick`/`spawn_account_usage_tick` in fleet-core cover
    // the tick loop's own cancellation behaviour (a pass in flight finishes;
    // a pre-cancelled token starts none). What is specific to `serve` is the
    // ORDER: `ticks_cancel.cancel()` fires before `shutdown.cancel()` (so a
    // new pass cannot start during the drain, while an in-flight one keeps
    // its `/agent` websockets through the drain window), and ticks are
    // awaited, bounded, before `ssh.shutdown_all()`. Driving that through a
    // real `serve()` would need a live listener, a real SIGTERM and a real
    // reconcile pass against a fake host — much heavier scaffolding than the
    // ordering itself warrants. `await_ticks` is the piece that gives the
    // ordering its bound, so it is tested directly, with tokio's paused
    // clock so nothing here touches a real socket or a real sleep. The
    // relative order of the two `.cancel()` calls is pinned by this comment
    // and by review; see `serve`'s body.

    #[tokio::test(start_paused = true)]
    async fn await_ticks_returns_once_every_handle_finishes() {
        let h1 = tokio::spawn(async {});
        let h2 = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        });
        let started = tokio::time::Instant::now();
        await_ticks(vec![h1, h2], std::time::Duration::from_secs(5)).await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "must not wait for the full timeout when both handles finish well within it"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn await_ticks_gives_up_after_the_bound_on_a_stuck_handle() {
        // A handle that never completes — the stand-in for a tick loop stuck
        // mid-pass — must not block shutdown past `timeout`.
        let notify = Arc::new(tokio::sync::Notify::new());
        let n2 = Arc::clone(&notify);
        let stuck = tokio::spawn(async move {
            n2.notified().await; // never notified: this task never finishes
        });
        let started = tokio::time::Instant::now();
        await_ticks(vec![stuck], std::time::Duration::from_millis(200)).await;
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(200),
            "must wait out the full bound before giving up on a stuck handle"
        );
    }
}
