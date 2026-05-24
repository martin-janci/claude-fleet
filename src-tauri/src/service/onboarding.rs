//! Read-only logic backing the first-run onboarding checklist:
//! local prerequisite detection and a tunnel-status snapshot mapping.

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

/// Tunnel liveness as surfaced to the onboarding UI. There is intentionally no
/// `Starting` state: `TunnelSupervisor::snapshot()` reports a single bool per
/// host (task alive vs. finished), so a supervised task that is up *or*
/// mid-backoff both read as `Up`; `NotStarted` means no task exists (e.g. the
/// Control API is disabled).
#[derive(Serialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Up,
    Down,
    NotStarted,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TunnelStatusRow {
    pub host_alias: String,
    pub state: TunnelState,
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

/// Map a per-host liveness snapshot (from `TunnelSupervisor::snapshot`) onto the
/// non-hidden hosts. Absent host => `NotStarted` (e.g. MCP disabled); present &
/// alive => `Up`; present & finished => `Down`.
pub fn map_tunnel_states(hosts: &[HostRow], alive: &HashMap<String, bool>) -> Vec<TunnelStatusRow> {
    hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| TunnelStatusRow {
            host_alias: h.alias.clone(),
            state: match alive.get(&h.alias) {
                Some(true) => TunnelState::Up,
                Some(false) => TunnelState::Down,
                None => TunnelState::NotStarted,
            },
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
pub async fn local_prereqs() -> LocalPrereqs {
    let (claude_version, tmux_version) = tokio::join!(
        tool_version("claude", "--version"),
        tool_version("tmux", "-V"),
    );

    let base = crate::service::projects::projects_base();
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

    #[test]
    fn maps_tunnel_states() {
        let hosts = vec![
            host("up", false),
            host("dead", false),
            host("none", false),
            host("hidden", true),
        ];
        let mut alive = HashMap::new();
        alive.insert("up".to_string(), true);
        alive.insert("dead".to_string(), false);

        let rows = map_tunnel_states(&hosts, &alive);
        assert_eq!(
            rows,
            vec![
                TunnelStatusRow {
                    host_alias: "up".into(),
                    state: TunnelState::Up
                },
                TunnelStatusRow {
                    host_alias: "dead".into(),
                    state: TunnelState::Down
                },
                TunnelStatusRow {
                    host_alias: "none".into(),
                    state: TunnelState::NotStarted
                },
            ]
        );
    }
}
