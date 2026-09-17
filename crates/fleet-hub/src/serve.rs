//! The subcommands' bodies: opening the store, starting the same ticks and
//! server the desktop starts, and stopping them on a signal.

use crate::config::{resolve, resolve_data_dir, HubOptions, Resolved};
use crate::out;
use fleet_core::events::NoopEventBus;
use fleet_core::mcp::{self, settings::ensure_master_token, McpGuards};
use fleet_core::service::hub::{
    SETTING_ALLOWED_HOSTS, SETTING_ALLOW_PLAINTEXT, SETTING_BIND, SETTING_LOCAL_HOST,
    SETTING_PUBLIC_URL,
};
use fleet_core::service::projects::LOCAL_HOST;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

/// Open (creating when missing) `<data-dir>/state.db`. The data dir is the
/// only option resolved without the store: everything else reads its stored
/// `hub.*` values.
fn open_store(opts: &HubOptions, env: &HashMap<String, String>) -> Result<Store, String> {
    let data_dir = resolve_data_dir(opts, env);
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create data dir {}: {e}", data_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700));
    }
    let db_path = data_dir.join("state.db");
    let store = Store::open_with_bus(&db_path, Arc::new(NoopEventBus)).map_err(|e| {
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
) -> Result<(Resolved, Arc<Mutex<Store>>), String> {
    let store = open_store(opts, env)?;
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
    set(
        SETTING_ALLOW_PLAINTEXT,
        if r.allow_plaintext { "true" } else { "false" },
    )?;
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
    let (r, store) = resolve_with_store(opts, env)?;
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
    existing_db(&resolve_data_dir(opts, env))?;
    let s = open_store(opts, env)?;
    if regenerate {
        s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())
            .map_err(|e| e.to_string())?;
    }
    out::line(&ensure_master_token(&s).map_err(|e| e.message)?);
    Ok(ExitCode::SUCCESS)
}

/// `<data-dir>/state.db` when it exists; `token` must never create one.
fn existing_db(data_dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
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
    /// The public key is there: print it.
    Print,
    /// Only the private key: derive the public half from it.
    Derive,
    /// Neither: generate a new key pair.
    Generate,
}

fn key_action(private_exists: bool, public_exists: bool) -> KeyAction {
    match (private_exists, public_exists) {
        (_, true) => KeyAction::Print,
        (true, false) => KeyAction::Derive,
        (false, false) => KeyAction::Generate,
    }
}

/// How long `healthcheck` waits to connect, and then for the status line.
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
fn probe(addr: std::net::SocketAddr, timeout: std::time::Duration) -> Result<String, String> {
    use std::io::{Read, Write};
    let mut conn = std::net::TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("connect {addr}: {e}"))?;
    conn.set_read_timeout(Some(timeout))
        .and_then(|()| conn.set_write_timeout(Some(timeout)))
        .map_err(|e| format!("{addr}: {e}"))?;
    let req = format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes())
        .map_err(|e| format!("send to {addr}: {e}"))?;
    // Status line plus the short body is all we need; bound what a stray peer
    // can make us read. Bytes already read survive a later read error.
    let mut raw = Vec::new();
    let read = conn.take(1024).read_to_end(&mut raw);
    if raw.is_empty() {
        read.map_err(|e| format!("read from {addr}: {e}"))?;
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
/// store (it runs next to a live `serve`). Port: flag > `FLEET_HUB_PORT` >
/// default; the stored `mcp.port` is deliberately not read.
pub fn healthcheck(port: Option<u16>, env: &HashMap<String, String>) -> Result<ExitCode, String> {
    let port = match (port, env.get("FLEET_HUB_PORT")) {
        (Some(p), _) => p,
        (None, Some(v)) => v
            .trim()
            .parse::<u16>()
            .map_err(|e| format!("FLEET_HUB_PORT '{v}': {e}"))?,
        (None, None) => mcp::DEFAULT_PORT,
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let status = probe(addr, HEALTHCHECK_TIMEOUT).map_err(|e| format!("unhealthy: {e}"))?;
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
    let (r, store) = resolve_with_store(opts, env)?;
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
    }
    let token = {
        let s = store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())?;
        ensure_master_token(&s).map_err(|e| e.message)?
    };
    let base = r.base()?;

    let ssh = Arc::new(fleet_core::ssh::SshClient::new());
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
    }));

    warn_if_confirm_destructive(&store);

    let (shutdown, serve_task) = mcp::start_with_handle(
        Arc::clone(&store),
        Arc::clone(&ssh),
        Arc::clone(&reg),
        Arc::clone(&tunnels),
        guards,
        r.bind,
        r.port,
        token,
        r.allowed_hosts.clone(),
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
        "fleet-hub serving"
    );

    fleet_core::service::tick::spawn_reconcile_tick(Arc::clone(&store), Arc::clone(&ssh));
    let usage_cache = Arc::new(Mutex::new(
        fleet_core::service::account_usage::UsageCache::new(),
    ));
    fleet_core::service::tick::spawn_account_usage_tick(
        Arc::clone(&store),
        Arc::clone(&ssh),
        usage_cache,
        Arc::new(NoopEventBus),
    );

    wait_for_signal().await?;
    tracing::info!("fleet-hub stopping");
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
    tunnels.stop_all();
    ssh.shutdown_all();
    Ok(ExitCode::SUCCESS)
}

/// How long `serve` waits for in-flight requests after a stop signal.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
            allow_plaintext: false,
            log_dir: "/unused/logs".into(),
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
        let (r, _store) = resolve_with_store(&opts, &HashMap::new()).unwrap();
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
        let (back, _s) = resolve_with_store(&opts, &HashMap::new()).unwrap();
        assert!(back.allow_plaintext);
        assert_eq!(back.bind.to_string(), "0.0.0.0");
        // And `FLEET_HUB_ALLOW_PLAINTEXT=0` turns it off again.
        let off: HashMap<String, String> =
            [("FLEET_HUB_ALLOW_PLAINTEXT".to_string(), "0".to_string())].into();
        assert!(resolve_with_store(&opts, &off).is_err());
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
        assert_eq!(key_action(false, true), KeyAction::Print);
        assert_eq!(key_action(true, false), KeyAction::Derive);
        assert_eq!(key_action(false, false), KeyAction::Generate);
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

    #[test]
    fn healthcheck_probes_healthz_and_accepts_the_liveness_body() {
        let (addr, server) = one_shot(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\n\
              content-length: 13\r\nconnection: close\r\n\r\nfleet-hub ok\n",
        );
        let status = probe(addr, HEALTHCHECK_TIMEOUT).unwrap();
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

    #[test]
    fn healthcheck_rejects_a_200_from_an_unrelated_listener() {
        // The old probe passed on any HTTP status line, so any process that
        // happened to hold the port read as a healthy hub.
        let (addr, server) = one_shot(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 5\r\n\
              connection: close\r\n\r\nhello",
        );
        let err = probe(addr, HEALTHCHECK_TIMEOUT).unwrap_err();
        assert!(err.contains("fleet-hub"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn healthcheck_fails_on_a_closed_port_or_a_non_http_answer() {
        use std::io::Write;
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        assert!(probe(closed, HEALTHCHECK_TIMEOUT).is_err());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").unwrap();
        });
        let err = probe(addr, HEALTHCHECK_TIMEOUT).unwrap_err();
        assert!(err.contains("SSH-2.0"), "{err}");
        server.join().unwrap();
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
}
