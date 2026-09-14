//! Plain-text diagnostics bundle for bug reports (Settings → "Copy
//! diagnostics"). Everything here is read from cached state: the store, the
//! tunnel supervisor snapshot, the MCP runtime and the log files. No network.
//!
//! Secrets policy: the bundle NEVER includes a token. It reports whether the
//! master token is set and each host's token *mode*, and the finished text is
//! run through `logging::redact_secrets` with every token the store knows, so
//! a token that leaked into a log line or an error string is masked too.

use crate::ipc_error::IpcError;
use crate::logging;
use crate::store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

/// How many trailing log lines the bundle carries.
pub const LOG_TAIL_LINES: usize = 200;

/// Runtime state the store does not hold, gathered by the caller (the Tauri
/// command) from managed state.
pub struct DiagnosticsInputs<'a> {
    pub data_dir: &'a Path,
    pub log_dir: &'a Path,
    /// `TunnelSupervisor::snapshot()`: host → supervised task alive.
    pub tunnels: HashMap<String, bool>,
    /// `McpRuntime::is_running()`.
    pub mcp_running: bool,
    /// `McpRuntime::last_error()`.
    pub mcp_bind_error: Option<String>,
}

/// Wire shape of `collect_diagnostics`. Mirrored in `src/lib/diagnostics.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsBundle {
    /// The redacted report, ready to paste into an issue.
    pub text: String,
    /// Where the log files live (for "Open log folder" / display).
    pub log_dir: String,
    /// The log file currently being written, if one exists yet.
    pub log_file: Option<String>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn age(at: Option<i64>, now: i64) -> String {
    match at {
        Some(t) => format!("{t} ({}s ago)", (now - t).max(0)),
        None => "never".into(),
    }
}

fn tunnel_state(alias: &str, tunnels: &HashMap<String, bool>) -> &'static str {
    if alias == "local" {
        return "n/a";
    }
    match tunnels.get(alias) {
        Some(true) => "up",
        Some(false) => "exited",
        None => "not started",
    }
}

/// Build the diagnostics bundle. The store mutex is held only while reading
/// rows (no `.await` in here); log files are read after it is released.
pub fn collect(
    store: &Mutex<Store>,
    inputs: DiagnosticsInputs<'_>,
) -> Result<DiagnosticsBundle, IpcError> {
    let now = now_unix();

    // ── 1. Everything from the store, under one short lock. ──
    let (
        schema_version,
        settings,
        mcp_enabled,
        mcp_port,
        confirm,
        master_set,
        hosts,
        tokens,
        sessions,
    ) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let setting = |k: &str| s.get_setting(k).ok().flatten();
        let master = setting(crate::mcp::SETTING_TOKEN).filter(|t| !t.is_empty());
        (
            s.schema_version().unwrap_or(0),
            crate::service::settings::read_all(&s),
            setting(crate::mcp::SETTING_ENABLED).as_deref() == Some("true"),
            setting(crate::mcp::SETTING_PORT)
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(crate::mcp::DEFAULT_PORT),
            setting(crate::mcp::guard::SETTING_CONFIRM_DESTRUCTIVE).as_deref() == Some("true"),
            master,
            s.list_hosts()?,
            s.list_host_tokens()?,
            s.list_all_sessions()?,
        )
    };

    // Every token the store knows — masked literally in the final text.
    let mut secrets: Vec<String> = tokens.iter().map(|t| t.token.clone()).collect();
    if let Some(m) = &master_set {
        secrets.push(m.clone());
    }
    let token_modes: BTreeMap<&str, &str> = tokens
        .iter()
        .map(|t| (t.host_alias.as_str(), t.mode.as_str()))
        .collect();

    let log_file = logging::current_log_file(inputs.log_dir);
    let log_tail = logging::tail_lines(inputs.log_dir, LOG_TAIL_LINES);

    // ── 2. Render. `writeln!` into a String cannot fail. ──
    let mut t = String::new();
    let _ = writeln!(t, "claude-fleet diagnostics");
    let _ = writeln!(t, "generated_at: {now} (unix seconds)");
    let _ = writeln!(t);

    let _ = writeln!(t, "== App ==");
    let _ = writeln!(t, "version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        t,
        "os: {} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        sysinfo::System::long_os_version().unwrap_or_else(|| "unknown".into())
    );
    let _ = writeln!(t, "schema_version: {schema_version}");
    let _ = writeln!(t, "data_dir: {}", inputs.data_dir.display());
    let _ = writeln!(t, "log_dir: {}", inputs.log_dir.display());
    let _ = writeln!(
        t,
        "log_file: {}",
        log_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none yet)".into())
    );
    let _ = writeln!(
        t,
        "RUST_LOG: {}",
        std::env::var("RUST_LOG")
            .unwrap_or_else(|_| format!("(unset; default {})", logging::DEFAULT_FILTER))
    );
    let _ = writeln!(t);

    let _ = writeln!(t, "== Settings ==");
    for (k, v) in &settings {
        let _ = writeln!(t, "{k} = {v}");
    }
    let _ = writeln!(t);

    let _ = writeln!(t, "== Control API (MCP) ==");
    let _ = writeln!(t, "enabled: {mcp_enabled}");
    let _ = writeln!(t, "bound: {}", inputs.mcp_running);
    let _ = writeln!(t, "port: {mcp_port}");
    let _ = writeln!(
        t,
        "bind_error: {}",
        inputs.mcp_bind_error.as_deref().unwrap_or("none")
    );
    let _ = writeln!(t, "confirm_destructive: {confirm}");
    let _ = writeln!(
        t,
        "master_token: {}",
        if master_set.is_some() { "set" } else { "unset" }
    );
    let _ = writeln!(t);

    let _ = writeln!(t, "== Hosts ({}) ==", hosts.len());
    for h in &hosts {
        let _ = writeln!(
            t,
            "{alias}: ssh_alias={ssh} reachable={reach} last_pinged_at={ping} tmux={tmux} claude={claude} \
             provisioned={prov} hidden={hidden} tunnel={tunnel} token_mode={mode}",
            alias = h.alias,
            ssh = h.ssh_alias.as_deref().unwrap_or("-"),
            reach = h.reachable,
            ping = age(h.last_pinged_at, now),
            tmux = h.tmux_version.as_deref().unwrap_or("-"),
            claude = h.claude_version.as_deref().unwrap_or("-"),
            prov = h.provisioned,
            hidden = h.hidden,
            tunnel = tunnel_state(&h.alias, &inputs.tunnels),
            mode = token_modes.get(h.alias.as_str()).copied().unwrap_or("none"),
        );
    }
    // Tunnels for hosts no longer in the table (removed while running).
    for (alias, alive) in &inputs.tunnels {
        if !hosts.iter().any(|h| &h.alias == alias) {
            let _ = writeln!(
                t,
                "{alias}: (not in hosts table) tunnel={}",
                if *alive { "up" } else { "exited" }
            );
        }
    }
    let _ = writeln!(t);

    // host → "status/claude_status" → count
    let mut by_host: BTreeMap<&str, BTreeMap<String, u32>> = BTreeMap::new();
    for s in &sessions {
        let key = format!(
            "{}/{}",
            s.status,
            s.claude_status.as_deref().unwrap_or("unknown")
        );
        *by_host
            .entry(s.host_alias.as_str())
            .or_default()
            .entry(key)
            .or_insert(0) += 1;
    }
    let _ = writeln!(
        t,
        "== Sessions ({} total; status/claude_status) ==",
        sessions.len()
    );
    for (host, counts) in &by_host {
        let parts: Vec<String> = counts.iter().map(|(k, n)| format!("{k}={n}")).collect();
        let _ = writeln!(t, "{host}: {}", parts.join(" "));
    }
    let _ = writeln!(t);

    let _ = writeln!(t, "== Recent log (last {} lines) ==", log_tail.len());
    for line in &log_tail {
        let _ = writeln!(t, "{line}");
    }

    Ok(DiagnosticsBundle {
        text: logging::redact_secrets(&t, &secrets),
        log_dir: inputs.log_dir.display().to_string(),
        log_file: log_file.map(|p| p.display().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HOST_TOK: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";
    /// A token that matches none of the redaction patterns — only the
    /// literal known-secret pass can catch it.
    const ODD_TOK: &str = "odd-shaped-host-token-Zq9";

    fn seeded_store() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(crate::mcp::SETTING_TOKEN, MASTER).unwrap();
        s.set_setting(crate::mcp::SETTING_ENABLED, "true").unwrap();
        s.set_setting(crate::mcp::SETTING_PORT, "4181").unwrap();
        s.upsert_host("mefistos").unwrap();
        s.upsert_host("turanga").unwrap();
        s.upsert_host_token("mefistos", HOST_TOK).unwrap();
        s.upsert_host_token("turanga", ODD_TOK).unwrap();
        s.set_host_token_mode("turanga", "readonly").unwrap();
        Mutex::new(s)
    }

    fn inputs<'a>(data: &'a Path, logs: &'a Path) -> DiagnosticsInputs<'a> {
        DiagnosticsInputs {
            data_dir: data,
            log_dir: logs,
            tunnels: HashMap::from([
                ("mefistos".to_string(), true),
                ("turanga".to_string(), false),
            ]),
            mcp_running: true,
            // An error string that (wrongly) carries a token must still be masked.
            mcp_bind_error: Some(format!("could not bind: Bearer {ODD_TOK}")),
        }
    }

    #[test]
    fn bundle_contains_no_token_material() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = logging::log_dir_in(tmp.path());
        std::fs::create_dir_all(&logs).unwrap();
        // A log file that leaked every token in several shapes.
        std::fs::write(
            logs.join("claude-fleet.2026-09-11.log"),
            format!(
                "INFO hook Authorization: Bearer {MASTER}\n\
                 INFO GET /hook?token={HOST_TOK}\n\
                 WARN odd token {ODD_TOK} seen\n\
                 INFO plain line\n"
            ),
        )
        .unwrap();

        let store = seeded_store();
        let b = collect(&store, inputs(tmp.path(), &logs)).unwrap();

        for secret in [MASTER, HOST_TOK, ODD_TOK] {
            assert!(
                !b.text.contains(secret),
                "bundle leaked {secret}:\n{}",
                b.text
            );
        }
        // The non-secret facts are all there.
        assert!(b
            .text
            .contains(&format!("version: {}", env!("CARGO_PKG_VERSION"))));
        assert!(b.text.contains("schema_version: "));
        assert!(b.text.contains("master_token: set"));
        assert!(b.text.contains("enabled: true"));
        assert!(b.text.contains("bound: true"));
        assert!(b.text.contains("port: 4181"));
        assert!(b.text.contains("tunnel=up token_mode=full"), "{}", b.text);
        assert!(
            b.text.contains("tunnel=exited token_mode=readonly"),
            "{}",
            b.text
        );
        assert!(b.text.contains("reconcile.interval_secs = "));
        assert!(b.text.contains("INFO plain line"));
        assert!(b.text.contains("== Recent log (last 4 lines) =="));
        assert_eq!(b.log_dir, logs.display().to_string());
        assert!(b.log_file.unwrap().ends_with("claude-fleet.2026-09-11.log"));
    }

    #[test]
    fn bundle_counts_sessions_by_host_and_status_and_handles_no_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs-never-created");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("mefistos").unwrap();
            s.upsert_session("t1", "mefistos", None, None, 0, 0, "running", None)
                .unwrap();
            s.upsert_session("t2", "mefistos", None, None, 0, 0, "running", None)
                .unwrap();
        }
        let b = collect(
            &store,
            DiagnosticsInputs {
                data_dir: tmp.path(),
                log_dir: &logs,
                tunnels: HashMap::new(),
                mcp_running: false,
                mcp_bind_error: None,
            },
        )
        .unwrap();
        assert!(b.text.contains("master_token: unset"));
        assert!(b.text.contains("bind_error: none"));
        assert!(b.text.contains("tunnel=not started token_mode=none"));
        assert!(b.text.contains("== Sessions (2 total"));
        assert!(b.text.contains("mefistos: running/unknown=2"), "{}", b.text);
        assert!(b.text.contains("log_file: (none yet)"));
        assert!(b.log_file.is_none());
    }

    #[test]
    fn tunnel_state_maps_snapshot() {
        let snap = HashMap::from([("a".to_string(), true), ("b".to_string(), false)]);
        assert_eq!(tunnel_state("a", &snap), "up");
        assert_eq!(tunnel_state("b", &snap), "exited");
        assert_eq!(tunnel_state("c", &snap), "not started");
        assert_eq!(tunnel_state("local", &snap), "n/a");
    }
}
