//! Supervised reverse SSH tunnels: expose the central localhost MCP server on
//! each remote host's localhost via `ssh -R`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;

/// First restart delay after a tunnel exits; doubles per exit up to
/// `MAX_BACKOFF`.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The delay to wait after `current` before the next restart attempt.
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// Spawns one `ssh` tunnel process from its argv and resolves when it exits
/// (with the exit code, `None` if the spawn itself failed). Injected so the
/// supervisor's restart loop is testable without a real `ssh`.
pub type TunnelSpawner =
    Arc<dyn Fn(Vec<String>) -> Pin<Box<dyn Future<Output = Option<i32>> + Send>> + Send + Sync>;

/// Production spawner: `ssh <argv>`, killed if the supervising task is
/// aborted.
fn ssh_spawner() -> TunnelSpawner {
    Arc::new(|argv: Vec<String>| {
        Box::pin(async move {
            let status = tokio::process::Command::new("ssh")
                .args(&argv)
                .kill_on_drop(true)
                .status()
                .await;
            match status {
                // `code()` is None when ssh was killed by a signal.
                Ok(s) => s.code(),
                // Log it here: once mapped to None, "ssh is missing" would
                // look the same as "killed by a signal" in the restart line.
                Err(e) => {
                    tracing::error!(error = %e, "[tunnel] failed to spawn ssh");
                    None
                }
            }
        })
    })
}

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
        // End-of-options marker: `host` is validated at add_host, but make
        // sure ssh can never read it as an option regardless.
        "--".into(),
        host.into(),
    ]
}

/// Owns one supervised `ssh -R` task per remote host. Held in Tauri state as
/// `Arc<TunnelSupervisor>`. Each task loops: spawn `ssh -R … host`, await exit,
/// and (unless aborted) restart after a capped backoff.
pub struct TunnelSupervisor {
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    spawner: TunnelSpawner,
    initial_backoff: Duration,
}

impl Default for TunnelSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelSupervisor {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            spawner: ssh_spawner(),
            initial_backoff: INITIAL_BACKOFF,
        }
    }

    /// A supervisor whose tunnels are `spawner` calls instead of `ssh`
    /// processes, restarting after `initial_backoff` (doubling, capped as in
    /// production). Tests use it to observe the restart loop; nothing else
    /// should.
    #[cfg(test)]
    pub(crate) fn with_spawner(spawner: TunnelSpawner, initial_backoff: Duration) -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            spawner,
            initial_backoff,
        }
    }

    /// Ensure a tunnel for `host` is running (idempotent — no-op if already up).
    pub fn ensure(&self, host: &str, remote_port: u16, mcp_port: u16) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if tasks.get(host).map(|h| !h.is_finished()).unwrap_or(false) {
            return;
        }
        let host_s = host.to_string();
        let spawner = Arc::clone(&self.spawner);
        let initial_backoff = self.initial_backoff;
        let handle = tokio::spawn(async move {
            let mut backoff = initial_backoff;
            loop {
                let argv = tunnel_argv(&host_s, remote_port, mcp_port);
                let status = spawner(argv).await;
                tracing::warn!(
                    host = %host_s,
                    exit_code = ?status,
                    restart_in = ?backoff,
                    "[tunnel] ssh exited; restarting"
                );
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
        });
        tasks.insert(host.to_string(), handle);
    }

    /// Stop a single host's tunnel.
    #[allow(dead_code)]
    pub fn stop(&self, host: &str) {
        if let Some(h) = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(host)
        {
            h.abort();
        }
    }

    /// Per-host liveness for the onboarding/status UI. `true` = the supervised
    /// task is still running (tunnel up or mid-backoff), `false` = the task
    /// exited unexpectedly and stays in the map until the next `ensure` replaces
    /// it. A deliberately stopped host is removed via `stop`/`stop_all` and
    /// therefore absent (callers map absence to "not started", e.g. MCP disabled).
    pub fn snapshot(&self) -> HashMap<String, bool> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks
            .iter()
            .map(|(host, handle)| (host.clone(), !handle.is_finished()))
            .collect()
    }

    /// Stop all tunnels (app exit / MCP disable).
    pub fn stop_all(&self) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        // `--` must immediately precede the host so ssh treats it as an operand.
        let n = a.len();
        assert_eq!(a[n - 2], "--");
        assert_eq!(a[n - 1], "mefistos");
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
        assert!(!snap.contains_key("never"));

        sup.stop_all();
    }

    #[test]
    fn next_backoff_doubles_and_caps_at_30s() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(
            next_backoff(Duration::from_secs(8)),
            Duration::from_secs(16)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(16)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    /// Spawn log for the fake tunnel spawner: `(argv, when)` per spawn.
    type SpawnLog = Arc<Mutex<Vec<(Vec<String>, std::time::Instant)>>>;

    /// A spawner whose "ssh" exits immediately with 255 (forward failed),
    /// recording every spawn.
    fn exiting_spawner(log: SpawnLog) -> TunnelSpawner {
        Arc::new(move |argv: Vec<String>| {
            log.lock().unwrap().push((argv, std::time::Instant::now()));
            Box::pin(async { Some(255) })
        })
    }

    async fn wait_for_spawns(log: &SpawnLog, n: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while log.lock().unwrap().len() < n {
            assert!(
                std::time::Instant::now() < deadline,
                "expected {n} spawns, got {}",
                log.lock().unwrap().len()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn supervisor_restarts_an_exited_tunnel_with_growing_backoff() {
        // (c) restart-on-exit, observed without a real ssh: every exit is
        // followed by a re-spawn of the SAME argv after a delay that doubles.
        let log: SpawnLog = Arc::new(Mutex::new(Vec::new()));
        let sup = TunnelSupervisor::with_spawner(
            exiting_spawner(Arc::clone(&log)),
            Duration::from_millis(20),
        );
        sup.ensure("mefistos", 4180, 4180);
        wait_for_spawns(&log, 3).await;
        let spawns = log.lock().unwrap().clone();
        for (argv, _) in &spawns {
            assert_eq!(argv, &tunnel_argv("mefistos", 4180, 4180));
        }
        // Lower bounds only — an upper bound would flake under CI load.
        let gap1 = spawns[1].1 - spawns[0].1;
        let gap2 = spawns[2].1 - spawns[1].1;
        assert!(gap1 >= Duration::from_millis(20), "first backoff: {gap1:?}");
        assert!(
            gap2 >= Duration::from_millis(40),
            "second backoff: {gap2:?}"
        );
        // Still supervised (mid-backoff counts as up).
        assert_eq!(sup.snapshot().get("mefistos"), Some(&true));

        // `stop` aborts the loop: at most one spawn that was already past
        // its await point can land afterwards, then nothing.
        sup.stop("mefistos");
        let n = log.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(log.lock().unwrap().len() <= n + 1, "no restarts after stop");
        assert!(!sup.snapshot().contains_key("mefistos"));
    }

    #[tokio::test]
    async fn ensure_is_a_no_op_while_the_tunnel_task_runs() {
        // A tunnel that never exits: a second `ensure` must not spawn a
        // second process for the same host.
        let log: SpawnLog = Arc::new(Mutex::new(Vec::new()));
        let log_for = Arc::clone(&log);
        let pending: TunnelSpawner = Arc::new(move |argv: Vec<String>| {
            log_for
                .lock()
                .unwrap()
                .push((argv, std::time::Instant::now()));
            Box::pin(std::future::pending())
        });
        let sup = TunnelSupervisor::with_spawner(pending, Duration::from_millis(1));
        sup.ensure("h", 4180, 4180);
        sup.ensure("h", 4180, 4180);
        wait_for_spawns(&log, 1).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(log.lock().unwrap().len(), 1);
        // A different host gets its own tunnel.
        sup.ensure("other", 4181, 4180);
        wait_for_spawns(&log, 2).await;
        assert_eq!(log.lock().unwrap()[1].0, tunnel_argv("other", 4181, 4180));
        sup.stop_all();
        assert!(sup.snapshot().is_empty());
    }
}
