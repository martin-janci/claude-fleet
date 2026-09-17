//! The subcommands' bodies: opening the store, starting the same ticks and
//! server the desktop starts, and stopping them on a signal.

use crate::config::{resolve, HubOptions, Resolved};
use crate::out;
use fleet_core::events::NoopEventBus;
use fleet_core::mcp::{self, settings::ensure_master_token, McpGuards};
use fleet_core::service::hub::{
    SETTING_ALLOWED_HOSTS, SETTING_BIND, SETTING_LOCAL_HOST, SETTING_PUBLIC_URL,
};
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

/// Resolve options against the settings in `<data-dir>/state.db` when it
/// exists (a second `resolve` pass: the first has no store to read).
fn resolve_with_store(
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<(Resolved, Arc<Mutex<Store>>), String> {
    let first = resolve(opts, env, &|_| None)?;
    std::fs::create_dir_all(&first.data_dir)
        .map_err(|e| format!("create data dir {}: {e}", first.data_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&first.data_dir, std::fs::Permissions::from_mode(0o700));
    }
    let db_path = first.data_dir.join("state.db");
    let store = Store::open_with_bus(&db_path, Arc::new(NoopEventBus)).map_err(|e| {
        format!(
            "failed to open the claude-fleet database at {}: {e}\n\
             If the file is corrupt, deleting it resets all hub state — hosts, projects and sessions are re-discovered.",
            db_path.display()
        )
    })?;
    fleet_core::service::provision::set_private_mode(&db_path);
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
    if !r.local_host {
        // A state.db copied from a desktop carries a `local` row; hide it so
        // nothing lists or probes it (reconcile skips it regardless).
        let hosts = s.list_hosts().map_err(|e| format!("list hosts: {e}"))?;
        if hosts.iter().any(|h| h.alias == "local") {
            s.set_host_hidden("local", true)
                .map_err(|e| format!("hide the local host: {e}"))?;
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
    let (_r, store) = resolve_with_store(opts, env)?;
    let s = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    if regenerate {
        s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())
            .map_err(|e| e.to_string())?;
    }
    out::line(&ensure_master_token(&s).map_err(|e| e.message)?);
    Ok(ExitCode::SUCCESS)
}

pub fn ssh_key() -> Result<ExitCode, String> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or("HOME is not set")?;
    let ssh_dir = home.join(".ssh");
    let key = ssh_dir.join("id_ed25519");
    let pubkey = key.with_extension("pub");
    if !pubkey.exists() {
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
    let text =
        std::fs::read_to_string(&pubkey).map_err(|e| format!("read {}: {e}", pubkey.display()))?;
    out::line(text.trim_end());
    Ok(ExitCode::SUCCESS)
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

    let shutdown = mcp::start(
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
    tunnels.stop_all();
    ssh.shutdown_all();
    Ok(ExitCode::SUCCESS)
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
        assert!(s.get_host_row("local").unwrap().unwrap().hidden);
        assert!(!s.get_host_row("devbox").unwrap().unwrap().hidden);
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
