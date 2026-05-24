//! Supervised reverse SSH tunnels: expose the central localhost MCP server on
//! each remote host's localhost via `ssh -R`.

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::task::JoinHandle;

/// Build the `ssh` argv for a reverse tunnel that makes the central machine's
/// `127.0.0.1:<mcp_port>` reachable at `127.0.0.1:<remote_port>` on `host`.
/// `-N` (no command), fail fast if the forward can't bind, keepalives so a
/// dropped link is detected.
pub fn tunnel_argv(host: &str, remote_port: u16, mcp_port: u16) -> Vec<String> {
    vec![
        "-N".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-R".into(),
        format!("127.0.0.1:{remote_port}:127.0.0.1:{mcp_port}"),
        host.into(),
    ]
}

/// Owns one supervised `ssh -R` task per remote host. Held in Tauri state as
/// `Arc<TunnelSupervisor>`. Each task loops: spawn `ssh -R … host`, await exit,
/// and (unless aborted) restart after a capped backoff.
#[derive(Default)]
pub struct TunnelSupervisor {
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl TunnelSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure a tunnel for `host` is running (idempotent — no-op if already up).
    pub fn ensure(&self, host: &str, remote_port: u16, mcp_port: u16) {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.get(host).map(|h| !h.is_finished()).unwrap_or(false) {
            return;
        }
        let host_s = host.to_string();
        let handle = tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            loop {
                let argv = tunnel_argv(&host_s, remote_port, mcp_port);
                let status = tokio::process::Command::new("ssh")
                    .args(&argv)
                    .kill_on_drop(true)
                    .status()
                    .await;
                eprintln!("[tunnel] {host_s} ssh exited: {status:?}; restarting in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
            }
        });
        tasks.insert(host.to_string(), handle);
    }

    /// Stop a single host's tunnel.
    #[allow(dead_code)]
    pub fn stop(&self, host: &str) {
        if let Some(h) = self.tasks.lock().unwrap().remove(host) {
            h.abort();
        }
    }

    /// Per-host liveness for the onboarding/status UI. `true` = the supervised
    /// task is still running (tunnel up or mid-backoff), `false` = the task
    /// exited unexpectedly and stays in the map until the next `ensure` replaces
    /// it. A deliberately stopped host is removed via `stop`/`stop_all` and
    /// therefore absent (callers map absence to "not started", e.g. MCP disabled).
    pub fn snapshot(&self) -> HashMap<String, bool> {
        let tasks = self.tasks.lock().unwrap();
        tasks
            .iter()
            .map(|(host, handle)| (host.clone(), !handle.is_finished()))
            .collect()
    }

    /// Stop all tunnels (app exit / MCP disable).
    pub fn stop_all(&self) {
        let mut tasks = self.tasks.lock().unwrap();
        for (_, h) in tasks.drain() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_argv_builds_reverse_forward() {
        let a = tunnel_argv("mefistos", 4180, 4180);
        assert!(a.contains(&"-N".to_string()));
        assert!(a.iter().any(|s| s == "127.0.0.1:4180:127.0.0.1:4180"));
        assert!(a.iter().any(|s| s == "ExitOnForwardFailure=yes"));
        assert_eq!(a.last().unwrap(), "mefistos");
    }

    #[tokio::test]
    async fn snapshot_reports_known_hosts_only() {
        let sup = TunnelSupervisor::new();
        // No tasks yet → empty snapshot.
        assert!(sup.snapshot().is_empty());

        // The supervised task loops forever (spawn ssh → await exit → sleep backoff → repeat),
        // so it never finishes on its own and is_finished() is reliably false right after ensure.
        sup.ensure("mefistos", 4180, 4180);
        let snap = sup.snapshot();
        assert_eq!(snap.get("mefistos"), Some(&true));
        // A host we never started is simply absent (caller maps to NotStarted).
        assert!(snap.get("never").is_none());

        sup.stop_all();
    }
}
