//! Plain-text diagnostics bundle for bug reports (Settings → "Copy
//! diagnostics"). Everything here is read from cached state: the store, the
//! tunnel supervisor snapshot, the MCP runtime and the log files. No network.
//!
//! Secrets policy: the bundle NEVER includes a token. It reports whether the
//! master token is set and each host's token *mode*, and the finished text is
//! run through `logging::redact_secrets` with every token the store knows, so
//! a token that leaked into a log line or an error string is masked too.

use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::logging;
use crate::service::tunnel::TunnelHealth;
use crate::store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

/// How many trailing log lines the bundle carries.
pub const LOG_TAIL_LINES: usize = 200;
/// How far back the bundle looks for warnings and errors. The tail above is
/// often all INFO by the time someone copies the bundle, and the ERROR that
/// started the trouble has scrolled out of it; this window brings it back.
pub const LOG_SCAN_LINES: usize = 5_000;
/// Most distinct warning/error lines listed from before the tail.
pub const LOG_PROBLEM_LINES: usize = 40;

/// Runtime state the store does not hold, gathered by the caller (the Tauri
/// command) from managed state.
pub struct DiagnosticsInputs<'a> {
    pub data_dir: &'a Path,
    pub log_dir: &'a Path,
    /// `TunnelSupervisor::health()`: host → what the supervisor knows about
    /// its tunnel.
    pub tunnels: HashMap<String, TunnelHealth>,
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

/// `2026-09-28T10:00:00Z` — the same clock the log lines are stamped with,
/// so a host's last ping can be lined up against the log by eye.
fn utc(t: i64) -> String {
    crate::service::trackers::format_timestamp(t)
}

fn age(at: Option<i64>, now: i64) -> String {
    match at {
        Some(t) => format!("{} ({}s ago)", utc(t), (now - t).max(0)),
        None => "never".into(),
    }
}

/// The level of a log line as the file layer writes it
/// (`<timestamp> <LEVEL> <target>: <message>`), when it is a warning or an
/// error. Anything else — a continuation line, a panic backtrace, INFO — is
/// `None`.
fn problem_level(line: &str) -> Option<&'static str> {
    let mut words = line.split_whitespace();
    let _timestamp = words.next()?;
    match words.next()? {
        "ERROR" => Some("ERROR"),
        "WARN" => Some("WARN"),
        _ => None,
    }
}

/// A line without its leading timestamp: two occurrences of the same warning
/// share it.
fn without_timestamp(line: &str) -> &str {
    let line = line.trim_start();
    match line.find(char::is_whitespace) {
        Some(i) => line[i..].trim_start(),
        None => line,
    }
}

/// The `== Earlier warnings and errors ==` section: every WARN / ERROR line
/// in `earlier` (the scanned log before the tail), repeats folded into one
/// line with a count, most recent last, capped at `LOG_PROBLEM_LINES`.
/// `None` when there are none, so a quiet log adds nothing.
///
/// A flapping tunnel writes the same warning thousands of times; folding it
/// is what lets the one ERROR that preceded it still fit.
fn earlier_problems(earlier: &[String], tail: &[String]) -> Option<String> {
    struct Seen<'a> {
        first: &'a str,
        last: &'a str,
        count: usize,
        order: usize,
    }
    let count = |lines: &[String], level: &str| {
        lines
            .iter()
            .filter(|l| problem_level(l) == Some(level))
            .count()
    };
    let mut seen: HashMap<&str, Seen<'_>> = HashMap::new();
    for (order, line) in earlier.iter().enumerate() {
        if problem_level(line).is_none() {
            continue;
        }
        let e = seen.entry(without_timestamp(line)).or_insert(Seen {
            first: line,
            last: line,
            count: 0,
            order,
        });
        e.last = line;
        e.count += 1;
        e.order = order;
    }
    if seen.is_empty() {
        return None;
    }
    let mut rows: Vec<Seen<'_>> = seen.into_values().collect();
    rows.sort_by_key(|r| r.order);
    let distinct = rows.len();
    let shown = &rows[distinct.saturating_sub(LOG_PROBLEM_LINES)..];

    let mut t = String::new();
    let _ = writeln!(
        t,
        "== Earlier warnings and errors ({} before the tail; {} distinct, last {} shown) ==",
        earlier.len(),
        distinct,
        shown.len()
    );
    let _ = writeln!(
        t,
        "before_tail: ERROR={} WARN={}",
        count(earlier, "ERROR"),
        count(earlier, "WARN")
    );
    let _ = writeln!(
        t,
        "in_tail: ERROR={} WARN={}",
        count(tail, "ERROR"),
        count(tail, "WARN")
    );
    for r in shown {
        if r.count == 1 {
            let _ = writeln!(t, "{}", r.last);
        } else {
            let since = r.first.split_whitespace().next().unwrap_or("?");
            let _ = writeln!(t, "{} [x{} since {since}]", r.last, r.count);
        }
    }
    let _ = writeln!(t);
    Some(t)
}

fn tunnel_state(alias: &str, tunnels: &HashMap<String, TunnelHealth>) -> &'static str {
    if alias == "local" {
        return "n/a";
    }
    match tunnels.get(alias) {
        None => "not started",
        Some(t) if !t.supervised => "exited",
        Some(t) if t.is_flapping() => "flapping",
        Some(_) => "up",
    }
}

/// The follow-up line for a tunnel that is not simply working: how often it has
/// failed, how ssh exited and what it said. `None` when there is nothing to
/// explain, so a healthy fleet's bundle stays short.
fn tunnel_detail(alias: &str, tunnels: &HashMap<String, TunnelHealth>) -> Option<String> {
    let t = tunnels.get(alias)?;
    if t.consecutive_failures == 0 && t.last_error.is_none() {
        return None;
    }
    Some(format!(
        "    {alias}: consecutive_failures={f} restarts={r} last_exit={e} retry_in={b}ms last_error={msg}",
        f = t.consecutive_failures,
        r = t.restarts,
        e = t
            .last_exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into()),
        b = t.backoff_ms,
        msg = t.last_error.as_deref().unwrap_or("-"),
    ))
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
        tracker_secrets,
        decision_secrets,
    ) = {
        let s = lock(store)?;
        let cfg = crate::mcp::settings::McpSettings::read(&s)?;
        (
            s.schema_version().unwrap_or(0),
            crate::service::settings::read_all(&s),
            cfg.enabled,
            cfg.port,
            cfg.confirm_destructive,
            cfg.token,
            s.list_hosts()?,
            s.list_host_tokens()?,
            s.list_all_sessions()?,
            s.tracker_secret_literals()?,
            s.decision_secret_literals()?,
        )
    };

    // Every token the store knows — masked literally in the final text.
    let mut secrets: Vec<String> = tokens.iter().map(|t| t.token.clone()).collect();
    if let Some(m) = &master_set {
        secrets.push(m.clone());
    }
    // Tracker credentials (work graph M3): the token and its Basic-auth
    // encoding, whichever an error string or log line might carry.
    secrets.extend(tracker_secrets);
    // The decision model's API key (Jev evaluation), masked the same way.
    secrets.extend(decision_secrets);
    let token_modes: BTreeMap<&str, &str> = tokens
        .iter()
        .map(|t| (t.host_alias.as_str(), t.mode.as_str()))
        .collect();

    let log_file = logging::current_log_file(inputs.log_dir);
    // One read for both the tail and the wider scan for warnings.
    let scanned = logging::tail_lines(inputs.log_dir, LOG_SCAN_LINES);
    let (log_earlier, log_tail) = scanned.split_at(scanned.len().saturating_sub(LOG_TAIL_LINES));

    // ── 2. Render. `writeln!` into a String cannot fail. ──
    let mut t = String::new();
    let _ = writeln!(t, "claude-fleet diagnostics");
    let _ = writeln!(t, "generated_at: {} ({now} unix seconds)", utc(now));
    let _ = writeln!(t);

    let _ = writeln!(t, "== App ==");
    let _ = writeln!(t, "version: {}", crate::app_version::get());
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
        if let Some(d) = tunnel_detail(&h.alias, &inputs.tunnels) {
            let _ = writeln!(t, "{d}");
        }
    }
    // Tunnels for hosts no longer in the table (removed while running).
    for alias in inputs.tunnels.keys() {
        if !hosts.iter().any(|h| &h.alias == alias) {
            let _ = writeln!(
                t,
                "{alias}: (not in hosts table) tunnel={}",
                tunnel_state(alias, &inputs.tunnels)
            );
            if let Some(d) = tunnel_detail(alias, &inputs.tunnels) {
                let _ = writeln!(t, "{d}");
            }
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

    if let Some(section) = earlier_problems(log_earlier, log_tail) {
        t.push_str(&section);
    }

    let _ = writeln!(t, "== Recent log (last {} lines) ==", log_tail.len());
    for line in log_tail {
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
    use crate::service::tunnel::TunnelHealth;

    const MASTER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HOST_TOK: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";
    /// A token that matches none of the redaction patterns — only the
    /// literal known-secret pass can catch it.
    const ODD_TOK: &str = "odd-shaped-host-token-Zq9";
    /// A tracker credential of no recognisable shape (work graph M3).
    const TRACKER_TOK: &str = "jira-odd-shaped-secret-7Q";

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
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        s.set_tracker_credential(t.id, "basic", Some("me@acme.com"), Some(TRACKER_TOK), None)
            .unwrap();
        Mutex::new(s)
    }

    fn inputs<'a>(data: &'a Path, logs: &'a Path) -> DiagnosticsInputs<'a> {
        DiagnosticsInputs {
            data_dir: data,
            log_dir: logs,
            tunnels: HashMap::from([
                (
                    "mefistos".to_string(),
                    TunnelHealth {
                        supervised: true,
                        connected: true,
                        ..Default::default()
                    },
                ),
                ("turanga".to_string(), TunnelHealth::default()),
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
                 WARN jira sync failed with {TRACKER_TOK}\n\
                 INFO plain line\n"
            ),
        )
        .unwrap();

        let store = seeded_store();
        let b = collect(&store, inputs(tmp.path(), &logs)).unwrap();

        for secret in [MASTER, HOST_TOK, ODD_TOK, TRACKER_TOK] {
            assert!(
                !b.text.contains(secret),
                "bundle leaked {secret}:\n{}",
                b.text
            );
        }
        // The non-secret facts are all there.
        assert!(b
            .text
            .contains(&format!("version: {}", crate::app_version::get())));
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
        assert!(b.text.contains("== Recent log (last 5 lines) =="));
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
    fn tunnel_state_maps_health() {
        let h = HashMap::from([
            (
                "a".to_string(),
                TunnelHealth {
                    supervised: true,
                    connected: true,
                    restarts: 2,
                    ..Default::default()
                },
            ),
            ("b".to_string(), TunnelHealth::default()),
        ]);
        assert_eq!(tunnel_state("a", &h), "up");
        assert_eq!(tunnel_state("b", &h), "exited");
        assert_eq!(tunnel_state("c", &h), "not started");
        assert_eq!(tunnel_state("local", &h), "n/a");
    }

    #[test]
    fn a_flapping_tunnel_reports_why_in_the_bundle() {
        // The whole point of the bundle is that the reason travels with the
        // report: "tunnel=flapping" alone would send the reader back to the
        // logs that the flapping itself had already flooded.
        let h = HashMap::from([(
            "trn".to_string(),
            TunnelHealth {
                supervised: true,
                connected: false,
                consecutive_failures: 412,
                restarts: 412,
                last_exit_code: Some(255),
                last_error: Some("bind [127.0.0.1]:4180: Address already in use".into()),
                backoff_ms: 30_000,
                ..Default::default()
            },
        )]);
        assert_eq!(tunnel_state("trn", &h), "flapping");
        let d = tunnel_detail("trn", &h).expect("a flapping tunnel has detail");
        assert!(d.contains("412"), "failure count: {d}");
        assert!(d.contains("255"), "exit code: {d}");
        assert!(d.contains("Address already in use"), "reason: {d}");
    }

    #[test]
    fn a_healthy_tunnel_adds_no_detail_line() {
        let h = HashMap::from([(
            "ok".to_string(),
            TunnelHealth {
                supervised: true,
                connected: true,
                restarts: 1,
                ..Default::default()
            },
        )]);
        assert_eq!(tunnel_detail("ok", &h), None);
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn problem_level_reads_the_file_layer_format() {
        assert_eq!(
            problem_level("2026-09-28T10:00:00.1Z ERROR fleet_core::ssh: boom"),
            Some("ERROR")
        );
        assert_eq!(
            problem_level("2026-09-28T10:00:00.1Z  WARN fleet_core::ssh: hm"),
            Some("WARN")
        );
        assert_eq!(
            problem_level("2026-09-28T10:00:00.1Z  INFO fleet_core: ERROR in text"),
            None
        );
        assert_eq!(problem_level("    at src/main.rs:3"), None);
        assert_eq!(problem_level(""), None);
    }

    #[test]
    fn earlier_problems_folds_repeats_and_counts_the_tail() {
        let earlier = lines(&[
            "2026-09-28T10:00:00Z ERROR fleet_core::tunnel: bind failed",
            "2026-09-28T10:00:01Z  WARN fleet_core::tunnel: retrying",
            "2026-09-28T10:00:02Z  INFO fleet_core: fine",
            "2026-09-28T10:00:03Z  WARN fleet_core::tunnel: retrying",
            "2026-09-28T10:00:04Z  WARN fleet_core::tunnel: retrying",
        ]);
        let tail = lines(&["2026-09-28T10:05:00Z ERROR fleet_core::x: later"]);
        let s = earlier_problems(&earlier, &tail).expect("has problems");
        assert!(
            s.contains("5 before the tail; 2 distinct, last 2 shown"),
            "{s}"
        );
        assert!(s.contains("before_tail: ERROR=1 WARN=3"), "{s}");
        assert!(s.contains("in_tail: ERROR=1 WARN=0"), "{s}");
        assert!(
            s.contains(
                "2026-09-28T10:00:04Z  WARN fleet_core::tunnel: retrying \
                 [x3 since 2026-09-28T10:00:01Z]"
            ),
            "{s}"
        );
        assert!(s.contains("ERROR fleet_core::tunnel: bind failed\n"), "{s}");
        assert!(!s.contains("fine"), "{s}");
        // Oldest first: the ERROR that started it precedes the retries.
        assert!(s.find("bind failed") < s.find("retrying"), "{s}");
    }

    #[test]
    fn earlier_problems_keeps_the_most_recent_when_capped() {
        let earlier: Vec<String> = (0..LOG_PROBLEM_LINES + 5)
            .map(|i| format!("2026-09-28T10:00:00Z  WARN t: distinct {i}"))
            .collect();
        let s = earlier_problems(&earlier, &[]).unwrap();
        assert!(!s.contains("distinct 4\n"), "{s}");
        assert!(s.contains("distinct 5\n"), "{s}");
        assert!(s.contains(&format!("distinct {}\n", LOG_PROBLEM_LINES + 4)));
    }

    #[test]
    fn a_quiet_log_adds_no_problems_section() {
        let earlier = lines(&["2026-09-28T10:00:00Z  INFO t: ok"]);
        assert_eq!(earlier_problems(&earlier, &[]), None);
        assert_eq!(earlier_problems(&[], &[]), None);
    }

    #[test]
    fn an_error_scrolled_out_of_the_tail_still_reaches_the_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = logging::log_dir_in(tmp.path());
        std::fs::create_dir_all(&logs).unwrap();
        let mut text =
            format!("2026-09-28T09:00:00Z ERROR fleet_core::ssh: master died, token {HOST_TOK}\n");
        for i in 0..LOG_TAIL_LINES + 50 {
            text.push_str(&format!(
                "2026-09-28T09:10:00Z  INFO fleet_core: tick {i}\n"
            ));
        }
        std::fs::write(logs.join("claude-fleet.2026-09-28-09.log"), text).unwrap();
        let store = seeded_store();
        let b = collect(&store, inputs(tmp.path(), &logs)).unwrap();
        let problems = b
            .text
            .find("== Earlier warnings and errors")
            .expect("section present");
        let tail = b.text.find("== Recent log").unwrap();
        assert!(problems < tail, "{}", b.text);
        assert!(b.text.contains("master died"), "{}", b.text);
        assert!(!b.text.contains(HOST_TOK), "the section is redacted too");
        assert!(b
            .text
            .contains(&format!("== Recent log (last {LOG_TAIL_LINES} lines) ==")));
    }

    #[test]
    fn timestamps_are_readable() {
        assert_eq!(
            age(Some(1_790_000_000), 1_790_000_060),
            "2026-09-21T14:13:20Z (60s ago)"
        );
        assert_eq!(age(None, 0), "never");
    }
}
