//! Read-only logic backing the first-run onboarding checklist:
//! local prerequisite detection and a tunnel-status snapshot mapping.

use crate::service::tunnel::TunnelHealth;
use crate::store::HostRow;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Debug, PartialEq)]
pub struct LocalPrereqs {
    pub claude_ok: bool,
    pub claude_version: Option<String>,
    pub tmux_ok: bool,
    pub tmux_version: Option<String>,
    pub projects_path: String,
    pub projects_readable: bool,
    /// Count of top-level entries under the projects base. Note these are the
    /// org/owner subdirectories, not individual repos — purely informational.
    pub projects_count: u32,
}

/// Tunnel liveness as surfaced to the onboarding UI.
///
/// `Up` is deliberately optimistic for a tunnel that has not failed yet (just
/// spawned, still connecting); `Flapping` is the state that used to be missing,
/// where the supervising task is alive but its ssh keeps dying, so the host
/// reported `Up` while never once connecting.
#[derive(Serialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Up,
    /// Supervised, but ssh keeps exiting before the connection is established.
    Flapping,
    Down,
    NotStarted,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TunnelStatusRow {
    pub host_alias: String,
    pub state: TunnelState,
    /// Exits-before-healthy since the last good connection (0 when healthy).
    pub consecutive_failures: u32,
    /// Tail of the last ssh stderr — why it is failing, for the UI tooltip.
    pub last_error: Option<String>,
}

/// Pull a semver-ish token out of a `--version` line. Returns the first
/// whitespace-separated chunk that starts with a digit (`tmux 3.4` -> `3.4`,
/// `1.0.39 (Claude Code)` -> `1.0.39`). `None` if nothing looks like a version.
pub fn parse_tool_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|tok| {
            let s = tok.trim_start_matches('v');
            s.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .map(|tok| tok.trim_start_matches('v').to_string())
}

/// Map per-host tunnel health onto the non-hidden hosts. Absent host =>
/// `NotStarted` (e.g. MCP disabled); task finished => `Down`; supervised and
/// failing => `Flapping`; otherwise `Up`.
pub fn map_tunnel_states(
    hosts: &[HostRow],
    health: &HashMap<String, TunnelHealth>,
) -> Vec<TunnelStatusRow> {
    hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| {
            let t = health.get(&h.alias);
            TunnelStatusRow {
                host_alias: h.alias.clone(),
                state: match t {
                    None => TunnelState::NotStarted,
                    Some(t) if !t.supervised => TunnelState::Down,
                    Some(t) if t.is_flapping() => TunnelState::Flapping,
                    Some(_) => TunnelState::Up,
                },
                consecutive_failures: t.map(|t| t.consecutive_failures).unwrap_or(0),
                last_error: t.and_then(|t| t.last_error.clone()),
            }
        })
        .collect()
}

/// Run a `<bin> <arg>` and return its parsed version if it exits 0. Bounded by a
/// short timeout so a hung binary (e.g. `claude` pausing for a token refresh)
/// can't stall onboarding; a timeout, spawn error, or non-zero exit all yield
/// `None`.
async fn tool_version(bin: &str, arg: &str) -> Option<String> {
    let fut = tokio::process::Command::new(bin).arg(arg).output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(3), fut)
        .await
        .ok()? // timed out
        .ok()?; // spawn / I/O error
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_tool_version(&text)
}

/// Detect local prerequisites: the `claude` CLI, `tmux`, and the projects scan
/// directory. Never errors — a missing tool is reported as `*_ok = false`.
pub async fn local_prereqs(store: &std::sync::Mutex<crate::store::Store>) -> LocalPrereqs {
    let (claude_version, tmux_version) = tokio::join!(
        tool_version("claude", "--version"),
        tool_version("tmux", "-V"),
    );

    // Resolved root (setting → env → default), same as `refresh_projects`.
    // A poisoned store degrades to the historical default rather than
    // failing the whole checklist.
    let base = match store.lock() {
        Ok(s) => crate::service::projects::local_projects_root(&s),
        Err(_) => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            std::path::PathBuf::from(home)
                .join("projects")
                .join("github.com")
        }
    };
    let projects_path = base.to_string_lossy().to_string();
    let (projects_readable, projects_count) = match std::fs::read_dir(&base) {
        Ok(rd) => (true, rd.filter_map(|e| e.ok()).count() as u32),
        Err(_) => (false, 0),
    };

    LocalPrereqs {
        claude_ok: claude_version.is_some(),
        claude_version,
        tmux_ok: tmux_version.is_some(),
        tmux_version,
        projects_path,
        projects_readable,
        projects_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tunnel::TunnelHealth;

    fn host(alias: &str, hidden: bool) -> HostRow {
        HostRow {
            alias: alias.to_string(),
            ssh_alias: None,
            reachable: true,
            claude_version: None,
            tmux_version: None,
            hidden,
            last_pinged_at: None,
            account_uuid: None,
            provisioned: true,
            transport: "ssh".to_string(),
            org_id: None,
        }
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_tool_version("tmux 3.4"), Some("3.4".into()));
        assert_eq!(
            parse_tool_version("1.0.39 (Claude Code)"),
            Some("1.0.39".into())
        );
        assert_eq!(parse_tool_version("git version v2.40"), Some("2.40".into()));
        assert_eq!(parse_tool_version("no digits here"), None);
        assert_eq!(parse_tool_version(""), None);
    }

    /// `TunnelHealth` for a tunnel that is up and has been for a while.
    fn connected() -> TunnelHealth {
        TunnelHealth {
            supervised: true,
            connected: true,
            ..Default::default()
        }
    }

    /// `TunnelHealth` for a supervised tunnel whose ssh keeps dying.
    fn flapping(failures: u32) -> TunnelHealth {
        TunnelHealth {
            supervised: true,
            connected: false,
            consecutive_failures: failures,
            last_exit_code: Some(255),
            last_error: Some("bind [127.0.0.1]:4180: Address already in use".into()),
            ..Default::default()
        }
    }

    #[test]
    fn maps_tunnel_states() {
        let hosts = vec![
            host("up", false),
            host("dead", false),
            host("none", false),
            host("hidden", true),
        ];
        let health = HashMap::from([
            ("up".to_string(), connected()),
            ("dead".to_string(), TunnelHealth::default()),
        ]);

        let rows = map_tunnel_states(&hosts, &health);
        assert_eq!(
            rows.iter().map(|r| r.state).collect::<Vec<_>>(),
            vec![TunnelState::Up, TunnelState::Down, TunnelState::NotStarted]
        );
        assert_eq!(rows[0].host_alias, "up");
    }

    #[test]
    fn a_crash_looping_tunnel_is_flapping_not_up() {
        // The bug this replaces: the supervising task being alive was reported
        // as `Up`, so a host whose ssh had never once connected rendered as a
        // working tunnel in onboarding.
        let hosts = vec![host("trn", false)];
        let health = HashMap::from([("trn".to_string(), flapping(412))]);

        let rows = map_tunnel_states(&hosts, &health);
        assert_eq!(rows[0].state, TunnelState::Flapping);
        assert_eq!(rows[0].consecutive_failures, 412);
        assert_eq!(
            rows[0].last_error.as_deref(),
            Some("bind [127.0.0.1]:4180: Address already in use"),
            "the operator needs the reason, not just the state"
        );
    }

    #[test]
    fn a_tunnel_that_has_not_failed_yet_is_not_flapping() {
        // Freshly spawned, still connecting: `Up` (optimistic) rather than an
        // alarming badge that clears itself a second later.
        let hosts = vec![host("trn", false)];
        let health = HashMap::from([(
            "trn".to_string(),
            TunnelHealth {
                supervised: true,
                ..Default::default()
            },
        )]);
        assert_eq!(map_tunnel_states(&hosts, &health)[0].state, TunnelState::Up);
    }
}
