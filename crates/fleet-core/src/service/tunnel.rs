//! Supervised reverse SSH tunnels: expose the central localhost MCP server on
//! each remote host's localhost via `ssh -R`.
//!
//! Three things here are load-bearing and were each a live bug:
//!
//! 1. The argv disables ssh multiplexing. Inheriting a user's `ControlMaster
//!    auto` makes our `ssh -N` a slave that hands the forward to the master and
//!    exits 0 immediately — an invisible restart loop around a tunnel we do not
//!    own.
//! 2. A tunnel is reaped before it is started. An app instance killed without
//!    running `stop_all` leaves its `ssh -R` children orphaned onto pid 1, still
//!    holding the remote port, so every later attempt dies with "address already
//!    in use" forever.
//! 3. Health is richer than "the supervising task exists". A host that has never
//!    once connected used to render as `Up`, because the task supervising its
//!    failures was alive.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// First restart delay after a tunnel exits; doubles per exit up to
/// `MAX_BACKOFF`, and returns to this once a tunnel has been healthy.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A child that stays up at least this long counts as a real connection: the
/// forward bound and the link held. Shorter than any plausible healthy tunnel's
/// lifetime, far longer than the sub-second exit of a failed one.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);
/// How much of ssh's stderr is kept for the health record and the log line.
const MAX_STDERR: usize = 400;

/// The delay to wait after `current` before the next restart attempt.
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// The delay after a child that ran for `ran_for`. A tunnel that lived long
/// enough to be healthy starts over at `initial`: a blip after six good hours
/// should reconnect in a second, not inherit the 30s cap from the failures that
/// preceded them.
fn backoff_after(
    current: Duration,
    initial: Duration,
    ran_for: Duration,
    healthy_after: Duration,
) -> Duration {
    if ran_for >= healthy_after {
        initial
    } else {
        next_backoff(current)
    }
}

/// Whether a failure at this count deserves a WARN. Four hosts restarting every
/// 30s wrote ~11,500 lines a day and drowned everything else in the 3-day log
/// window, so past the first few the detail drops to DEBUG and only every 20th
/// failure is shouted about.
fn is_loud(consecutive_failures: u32) -> bool {
    consecutive_failures <= 3 || consecutive_failures.is_multiple_of(20)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How one `ssh` tunnel ended. `code` is `None` when it was killed by a signal
/// or never spawned; `stderr` is what ssh said on the way out, which is the
/// difference between a diagnosable failure and a bare `exit_code=255`.
#[derive(Debug, Clone, Default)]
pub struct TunnelExit {
    pub code: Option<i32>,
    pub stderr: String,
}

impl TunnelExit {
    /// An exit with `code` and nothing on stderr.
    pub fn code(code: i32) -> Self {
        Self {
            code: Some(code),
            stderr: String::new(),
        }
    }
}

/// Spawns one `ssh` tunnel process from its argv and resolves when it exits.
/// Injected so the supervisor's restart loop is testable without a real `ssh`.
pub type TunnelSpawner =
    Arc<dyn Fn(Vec<String>) -> Pin<Box<dyn Future<Output = TunnelExit> + Send>> + Send + Sync>;
/// Enumerates running processes as `(pid, full command line)`. Injected so
/// orphan reaping is testable without a real process table.
pub type ProcessLister = Arc<dyn Fn() -> Vec<(u32, String)> + Send + Sync>;
/// Terminates a pid. Injected alongside [`ProcessLister`].
pub type ProcessKiller = Arc<dyn Fn(u32) + Send + Sync>;

/// Keep the tail of ssh's stderr: the reason is on the last lines, and any
/// banner or motd noise is on the first.
fn tail_stderr(s: &str) -> String {
    let t = s.trim();
    if t.len() <= MAX_STDERR {
        return t.to_string();
    }
    let cut = t.len() - MAX_STDERR;
    let start = t
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= cut)
        .unwrap_or(t.len());
    format!("…{}", &t[start..])
}

/// Production spawner: `ssh <argv>`, killed if the supervising task is
/// aborted, with stderr captured rather than inherited (in a bundled release
/// an inherited stderr goes nowhere, which is why failures used to be logged
/// as a bare exit code with no reason).
fn ssh_spawner() -> TunnelSpawner {
    Arc::new(|argv: Vec<String>| {
        Box::pin(async move {
            let out = tokio::process::Command::new("ssh")
                .args(&argv)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await;
            match out {
                Ok(o) => TunnelExit {
                    // `code()` is None when ssh was killed by a signal.
                    code: o.status.code(),
                    stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                },
                // Report it here: once mapped to None, "ssh is missing" would
                // look the same as "killed by a signal" in the restart line.
                Err(e) => TunnelExit {
                    code: None,
                    stderr: format!("failed to spawn ssh: {e}"),
                },
            }
        })
    })
}

/// Production process lister: `ps -A -o pid=,args=` (same flags on macOS and
/// Linux).
fn ps_lister() -> ProcessLister {
    Arc::new(|| {
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,args="])
            .output();
        let Ok(out) = out else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                let (pid, rest) = line.split_once(char::is_whitespace)?;
                Some((pid.parse::<u32>().ok()?, rest.trim().to_string()))
            })
            .collect()
    })
}

/// Production killer: `SIGTERM`. An `ssh -R` exits cleanly on it and drops its
/// forward, which is all we need before rebinding.
fn signal_killer() -> ProcessKiller {
    Arc::new(|pid: u32| {
        #[cfg(unix)]
        // SAFETY: `kill` with a positive pid and SIGTERM; a stale pid simply
        // returns ESRCH, which we ignore.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = pid;
    })
}

/// Build the `ssh` argv for a reverse tunnel that makes the central machine's
/// `127.0.0.1:<mcp_port>` reachable at `127.0.0.1:<remote_port>` on `host`.
/// `-N` (no command), no multiplexing, fail fast if the forward can't bind,
/// keepalives so a dropped link is detected.
pub fn tunnel_argv(host: &str, remote_port: u16, mcp_port: u16) -> Vec<String> {
    vec![
        "-N".into(),
        // A supervised long-lived tunnel must own its own connection. Without
        // this, a user's `ControlMaster auto` for this host in ~/.ssh/config
        // turns us into a multiplexed slave: ssh hands the -R forward to the
        // master and exits 0 within a fraction of a second, so the supervisor
        // restart-loops forever while the forward is owned by a process it can
        // neither see nor stop. `ssh.rs` keeps its own dedicated ControlPath
        // for the same reason; here we want no multiplexing at all.
        "-o".into(),
        "ControlMaster=no".into(),
        "-o".into(),
        "ControlPath=none".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-R".into(),
        forward_spec(remote_port, mcp_port),
        // End-of-options marker: `host` is validated at add_host, but make
        // sure ssh can never read it as an option regardless.
        "--".into(),
        host.into(),
    ]
}

/// The `-R` argument that identifies one of our tunnels in a command line.
fn forward_spec(remote_port: u16, mcp_port: u16) -> String {
    format!("127.0.0.1:{remote_port}:127.0.0.1:{mcp_port}")
}

/// Pids in `procs` (as `(pid, full command line)`) that are an `ssh -R` tunnel
/// of *ours* for `host` — i.e. an orphan left behind by an app instance that
/// was killed before it could stop its children.
///
/// Deliberately strict, because the consequence of a false positive is killing
/// someone else's ssh: the executable must be `ssh`, `-N` must be present, the
/// `-R` argument must be exactly our loopback-to-loopback forward, and the last
/// operand must be the host. That also matches argv from older builds (which
/// had no `--` marker and no `ControlMaster=no`), since those orphans are
/// precisely the ones that need reaping after an upgrade.
pub fn stale_tunnel_pids(
    procs: &[(u32, String)],
    host: &str,
    remote_port: u16,
    mcp_port: u16,
) -> Vec<u32> {
    let spec = forward_spec(remote_port, mcp_port);
    procs
        .iter()
        .filter(|(_, cmd)| {
            let tokens: Vec<&str> = cmd.split_whitespace().collect();
            let is_ssh = tokens
                .first()
                .map(|t| t.rsplit('/').next().unwrap_or(t) == "ssh")
                .unwrap_or(false);
            let forwards_ours = tokens.windows(2).any(|w| w[0] == "-R" && w[1] == spec);
            is_ssh && forwards_ours && tokens.contains(&"-N") && tokens.last() == Some(&host)
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// Whether ssh's stderr says the remote forward could not bind — the signature
/// of another process (typically an orphan) already holding the port. Worth
/// distinguishing because plain retrying can never clear it.
pub fn looks_like_bind_conflict(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("address already in use") || s.contains("remote port forwarding failed")
}

/// Kill any orphaned tunnel of ours for `host`, returning how many were
/// signalled. The process table read is blocking, so it goes to a blocking
/// worker rather than stalling a runtime thread.
async fn reap_orphans(
    lister: &ProcessLister,
    killer: &ProcessKiller,
    host: &str,
    remote_port: u16,
    mcp_port: u16,
) -> usize {
    let l = Arc::clone(lister);
    let procs = tokio::task::spawn_blocking(move || l())
        .await
        .unwrap_or_default();
    let pids = stale_tunnel_pids(&procs, host, remote_port, mcp_port);
    for pid in &pids {
        tracing::warn!(
            host = %host,
            pid = pid,
            "[tunnel] terminating an orphaned tunnel left by an earlier instance"
        );
        killer(*pid);
    }
    pids.len()
}

/// What the supervisor knows about one host's tunnel. Mirrored in
/// `src/lib/onboarding.ts`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TunnelHealth {
    /// The supervising task exists and has not finished. Says nothing about
    /// whether the tunnel is actually up — see `connected`.
    pub supervised: bool,
    /// An `ssh` child is running and has been for at least `HEALTHY_AFTER`,
    /// i.e. the forward bound and the link is holding.
    pub connected: bool,
    /// Exits-before-healthy since the last good connection. Non-zero with
    /// `connected == false` is a crash loop.
    pub consecutive_failures: u32,
    /// Total child exits since the task started.
    pub restarts: u64,
    /// Exit status of the last child (`None` = killed by a signal).
    pub last_exit_code: Option<i32>,
    /// Tail of the last child's stderr — the actual reason it failed.
    pub last_error: Option<String>,
    /// When the tunnel was last observed healthy.
    pub last_connected_unix: Option<i64>,
    /// The delay before the next restart attempt.
    pub backoff_ms: u64,
}

impl TunnelHealth {
    /// Supervised, not connected, and failing: a crash loop rather than a
    /// tunnel that is merely still coming up.
    pub fn is_flapping(&self) -> bool {
        self.supervised && !self.connected && self.consecutive_failures > 0
    }
}

/// Owns one supervised `ssh -R` task per remote host. Held in Tauri state as
/// `Arc<TunnelSupervisor>`. Each task loops: reap orphans, spawn
/// `ssh -R … host`, await exit, and (unless aborted) restart after a capped
/// backoff.
pub struct TunnelSupervisor {
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    stats: Arc<Mutex<HashMap<String, TunnelHealth>>>,
    spawner: TunnelSpawner,
    lister: ProcessLister,
    killer: ProcessKiller,
    initial_backoff: Duration,
    healthy_after: Duration,
}

/// Test-only injection of every seam the supervisor touches.
#[cfg(test)]
pub(crate) struct TestParts {
    pub spawner: TunnelSpawner,
    pub initial_backoff: Duration,
    pub healthy_after: Duration,
    pub lister: ProcessLister,
    pub killer: ProcessKiller,
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
            stats: Arc::new(Mutex::new(HashMap::new())),
            spawner: ssh_spawner(),
            lister: ps_lister(),
            killer: signal_killer(),
            initial_backoff: INITIAL_BACKOFF,
            healthy_after: HEALTHY_AFTER,
        }
    }

    /// A supervisor whose tunnels are `spawner` calls instead of `ssh`
    /// processes, with an empty process table (nothing to reap). Convenience
    /// over [`Self::with_parts`] for tests that only care about the restart
    /// loop.
    #[cfg(test)]
    pub(crate) fn with_spawner(spawner: TunnelSpawner, initial_backoff: Duration) -> Self {
        Self::with_parts(TestParts {
            spawner,
            initial_backoff,
            healthy_after: HEALTHY_AFTER,
            lister: Arc::new(Vec::new),
            killer: Arc::new(|_| {}),
        })
    }

    /// A supervisor whose tunnels, process table and kills are all injected.
    /// Tests use it to observe the restart loop; nothing else should.
    #[cfg(test)]
    pub(crate) fn with_parts(p: TestParts) -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            stats: Arc::new(Mutex::new(HashMap::new())),
            spawner: p.spawner,
            lister: p.lister,
            killer: p.killer,
            initial_backoff: p.initial_backoff,
            healthy_after: p.healthy_after,
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
        let lister = Arc::clone(&self.lister);
        let killer = Arc::clone(&self.killer);
        let stats = Arc::clone(&self.stats);
        let initial_backoff = self.initial_backoff;
        let healthy_after = self.healthy_after;
        {
            let mut st = stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            st.insert(
                host_s.clone(),
                TunnelHealth {
                    backoff_ms: initial_backoff.as_millis() as u64,
                    ..Default::default()
                },
            );
        }
        let handle = tokio::spawn(async move {
            let mut backoff = initial_backoff;
            // The first attempt always sweeps: anything matching our argv right
            // now belongs to an instance that is gone. Later attempts sweep
            // only after a bind conflict.
            let mut reap_before_attempt = true;
            loop {
                if reap_before_attempt {
                    reap_orphans(&lister, &killer, &host_s, remote_port, mcp_port).await;
                }
                let argv = tunnel_argv(&host_s, remote_port, mcp_port);
                tracing::debug!(host = %host_s, argv = ?argv, "[tunnel] starting ssh");

                // Mark the tunnel connected once the child outlives
                // `healthy_after`, without giving up the await on its exit.
                let started = Instant::now();
                let fut = spawner(argv);
                tokio::pin!(fut);
                let mut marked = false;
                let exit = loop {
                    tokio::select! {
                        e = &mut fut => break e,
                        _ = tokio::time::sleep(healthy_after), if !marked => {
                            marked = true;
                            let mut st = stats.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            if let Some(h) = st.get_mut(&host_s) {
                                h.connected = true;
                                h.consecutive_failures = 0;
                                h.last_connected_unix = Some(now_unix());
                            }
                            tracing::info!(host = %host_s, "[tunnel] connected");
                        }
                    }
                };

                let ran_for = started.elapsed();
                let was_healthy = ran_for >= healthy_after;
                let stderr = tail_stderr(&exit.stderr);
                let conflict = looks_like_bind_conflict(&exit.stderr);
                let delay = backoff_after(backoff, initial_backoff, ran_for, healthy_after);
                backoff = delay;

                let failures = {
                    let mut st = stats
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Absent means `stop` removed us and the abort has not
                    // landed yet — record nothing rather than resurrect a row.
                    st.get_mut(&host_s)
                        .map(|h| {
                            h.connected = false;
                            h.restarts += 1;
                            h.last_exit_code = exit.code;
                            h.last_error = (!stderr.is_empty()).then(|| stderr.clone());
                            h.backoff_ms = delay.as_millis() as u64;
                            if was_healthy {
                                h.consecutive_failures = 0;
                            } else {
                                h.consecutive_failures += 1;
                            }
                            h.consecutive_failures
                        })
                        .unwrap_or(0)
                };

                // A bind conflict cannot clear itself by retrying: something
                // else holds the port, so sweep again before the next attempt.
                reap_before_attempt = conflict;

                let reason = if conflict {
                    "the remote port is already bound (another process holds it)"
                } else if exit.code.is_none() {
                    "killed by a signal, or it never spawned"
                } else if was_healthy {
                    "the connection dropped"
                } else {
                    "it exited before the connection was established"
                };
                // Same fields either way; only the level changes, so a
                // permanent failure stops flooding the log after the first few.
                macro_rules! restart_event {
                    ($level:ident) => {
                        tracing::$level!(
                            host = %host_s,
                            exit_code = ?exit.code,
                            ran_for_ms = ran_for.as_millis() as u64,
                            consecutive_failures = failures,
                            restart_in = ?delay,
                            reason = reason,
                            stderr = %stderr,
                            "[tunnel] ssh exited; restarting"
                        )
                    };
                }
                if is_loud(failures) {
                    restart_event!(warn);
                } else {
                    restart_event!(debug);
                }
                tokio::time::sleep(delay).await;
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
        self.stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(host);
    }

    /// Per-host liveness for callers that only need a bool. `true` = the
    /// supervised task is still running (tunnel up **or** mid-backoff), so this
    /// cannot tell a working tunnel from a crash loop — use [`Self::health`]
    /// for anything the operator reads.
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

    /// Per-host tunnel health. A deliberately stopped host is absent (callers
    /// map absence to "not started", e.g. MCP disabled).
    pub fn health(&self) -> HashMap<String, TunnelHealth> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stats = self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks
            .iter()
            .map(|(host, handle)| {
                let mut h = stats.get(host).cloned().unwrap_or_default();
                h.supervised = !handle.is_finished();
                if !h.supervised {
                    h.connected = false;
                }
                (host.clone(), h)
            })
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
        self.stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Debuggability: log volume, stderr retention, flap detection ----

    #[test]
    fn only_the_first_few_failures_and_then_every_twentieth_are_loud() {
        // Four hosts restarting every 30s wrote ~11,500 WARN lines a day and
        // drowned the 3-day log window; a permanent failure must go quiet.
        assert!(is_loud(1));
        assert!(is_loud(3));
        assert!(!is_loud(4));
        assert!(!is_loud(19));
        assert!(is_loud(20), "a periodic reminder still gets through");
        assert!(!is_loud(21));
        assert!(is_loud(40));
    }

    #[test]
    fn stderr_is_trimmed_and_kept_whole_when_short() {
        assert_eq!(
            tail_stderr("  bind: Address already in use\n"),
            "bind: Address already in use"
        );
        assert_eq!(tail_stderr("   "), "");
    }

    #[test]
    fn a_long_stderr_keeps_its_tail_because_the_reason_is_last() {
        let noisy = format!(
            "{}\nError: remote port forwarding failed",
            "motd ".repeat(400)
        );
        let kept = tail_stderr(&noisy);
        assert!(kept.len() <= MAX_STDERR + 4, "truncated: {}", kept.len());
        assert!(kept.ends_with("Error: remote port forwarding failed"));
        assert!(kept.starts_with('…'), "truncation is marked: {kept}");
    }

    #[test]
    fn flapping_is_supervised_but_never_connected() {
        let flapping = TunnelHealth {
            supervised: true,
            connected: false,
            consecutive_failures: 7,
            ..Default::default()
        };
        assert!(flapping.is_flapping());
        // A healthy tunnel is not flapping...
        assert!(!TunnelHealth {
            supervised: true,
            connected: true,
            ..Default::default()
        }
        .is_flapping());
        // ...nor is one that has not failed yet (just started, mid-connect).
        assert!(!TunnelHealth {
            supervised: true,
            ..Default::default()
        }
        .is_flapping());
        // ...nor a task that has finished.
        assert!(!TunnelHealth {
            supervised: false,
            consecutive_failures: 7,
            ..Default::default()
        }
        .is_flapping());
    }

    // ---- Bug 1: the argv must not inherit the user's ssh multiplexing ----

    #[test]
    fn tunnel_argv_opts_out_of_ssh_multiplexing() {
        // A user's ~/.ssh/config with `ControlMaster auto` for this host would
        // otherwise turn our supervised tunnel into a multiplexed slave: it
        // hands the -R forward to the master and exits 0 in ~0.2s, so the
        // supervisor restarts forever while the forward is owned by a process
        // it cannot see. Pin the opt-out.
        let a = tunnel_argv("mefistos", 4180, 4180);
        let pairs: Vec<(&str, &str)> = a
            .windows(2)
            .map(|w| (w[0].as_str(), w[1].as_str()))
            .collect();
        assert!(
            pairs.contains(&("-o", "ControlMaster=no")),
            "argv must disable multiplexing: {a:?}"
        );
        assert!(
            pairs.contains(&("-o", "ControlPath=none")),
            "argv must ignore a configured ControlPath: {a:?}"
        );
    }

    // ---- Bug 2: recognising orphaned tunnels from a dead app instance ----

    #[test]
    fn stale_tunnel_pids_finds_our_reverse_forward_for_the_host() {
        let procs = vec![(
            4242u32,
            "ssh -N -o ExitOnForwardFailure=yes -R 127.0.0.1:4180:127.0.0.1:4180 -- mefistos"
                .to_string(),
        )];
        assert_eq!(
            stale_tunnel_pids(&procs, "mefistos", 4180, 4180),
            vec![4242]
        );
    }

    #[test]
    fn stale_tunnel_pids_ignores_other_hosts_ports_and_commands() {
        let procs = vec![
            // Same forward, different host.
            (1, "ssh -N -R 127.0.0.1:4180:127.0.0.1:4180 -- other".to_string()),
            // Same host, different port.
            (2, "ssh -N -R 127.0.0.1:4181:127.0.0.1:4181 -- mefistos".to_string()),
            // The app's own multiplexed control connection - must never be killed.
            (
                3,
                "ssh -o ControlMaster=auto -o ControlPath=/x/cm-mefistos.sock -- mefistos bash -lc tmux"
                    .to_string(),
            ),
            // A forward tunnel (-L), not ours.
            (4, "ssh -N -L 127.0.0.1:4180:127.0.0.1:4180 -- mefistos".to_string()),
            // The host name appearing inside an unrelated command.
            (5, "tail -f /var/log/mefistos".to_string()),
        ];
        assert!(stale_tunnel_pids(&procs, "mefistos", 4180, 4180).is_empty());
    }

    #[test]
    fn stale_tunnel_pids_matches_an_absolute_ssh_path() {
        let procs = vec![(
            7u32,
            "/usr/bin/ssh -N -R 127.0.0.1:4180:127.0.0.1:4180 -- mefistos".to_string(),
        )];
        assert_eq!(stale_tunnel_pids(&procs, "mefistos", 4180, 4180), vec![7]);
    }

    #[test]
    fn stale_tunnel_pids_matches_an_older_argv_without_the_end_of_options_marker() {
        // Tunnels orphaned by a build that predates `--` must still be reaped.
        let procs = vec![(
            9u32,
            "ssh -N -o ServerAliveInterval=30 -R 127.0.0.1:4180:127.0.0.1:4180 mefistos"
                .to_string(),
        )];
        assert_eq!(stale_tunnel_pids(&procs, "mefistos", 4180, 4180), vec![9]);
    }

    // ---- Bug 5: recognising why ssh exited ----

    #[test]
    fn bind_conflict_is_recognised_from_ssh_stderr() {
        assert!(looks_like_bind_conflict(
            "bind [127.0.0.1]:4180: Address already in use\nError: remote port forwarding failed for listen port 4180"
        ));
        assert!(looks_like_bind_conflict(
            "Warning: remote port forwarding failed for listen port 4180"
        ));
    }

    #[test]
    fn an_ordinary_connection_failure_is_not_a_bind_conflict() {
        assert!(!looks_like_bind_conflict(
            "ssh: connect to host x port 22: Connection refused"
        ));
        assert!(!looks_like_bind_conflict(""));
    }

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
            Box::pin(async { TunnelExit::code(255) })
        })
    }

    /// A spawner that never returns: the tunnel stays up.
    fn pending_spawner(log: SpawnLog) -> TunnelSpawner {
        Arc::new(move |argv: Vec<String>| {
            log.lock().unwrap().push((argv, std::time::Instant::now()));
            Box::pin(std::future::pending())
        })
    }

    fn no_processes() -> ProcessLister {
        Arc::new(Vec::new)
    }

    /// A killer that records the pids it was asked to terminate.
    fn recording_killer(seen: Arc<Mutex<Vec<u32>>>) -> ProcessKiller {
        Arc::new(move |pid| seen.lock().unwrap().push(pid))
    }

    /// Test supervisor with every seam injected.
    fn supervisor(
        spawner: TunnelSpawner,
        initial_backoff: Duration,
        healthy_after: Duration,
        lister: ProcessLister,
        killer: ProcessKiller,
    ) -> TunnelSupervisor {
        TunnelSupervisor::with_parts(TestParts {
            spawner,
            initial_backoff,
            healthy_after,
            lister,
            killer,
        })
    }

    async fn wait_for_spawns(log: &SpawnLog, n: usize) {
        wait_until(|| log.lock().unwrap().len() >= n, &format!("{n} spawns")).await;
    }

    /// Poll `cond` until it holds, failing the test after 10s.
    async fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn supervisor_restarts_an_exited_tunnel_with_growing_backoff() {
        // (c) restart-on-exit, observed without a real ssh: every exit is
        // followed by a re-spawn of the SAME argv after a delay that doubles.
        let log: SpawnLog = Arc::new(Mutex::new(Vec::new()));
        let sup = supervisor(
            exiting_spawner(Arc::clone(&log)),
            Duration::from_millis(20),
            Duration::from_secs(3600),
            no_processes(),
            recording_killer(Default::default()),
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
        let sup = supervisor(
            pending_spawner(Arc::clone(&log)),
            Duration::from_millis(1),
            Duration::from_secs(3600),
            no_processes(),
            recording_killer(Default::default()),
        );
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

    // ---- Bug 3: health must distinguish "connected" from "crash-looping" ----

    #[test]
    fn backoff_after_a_healthy_run_returns_to_the_initial_delay() {
        let initial = Duration::from_secs(1);
        let healthy = Duration::from_secs(60);
        // Exited before it was ever healthy: keep doubling.
        assert_eq!(
            backoff_after(
                Duration::from_secs(8),
                initial,
                Duration::from_secs(1),
                healthy
            ),
            Duration::from_secs(16)
        );
        // Ran long enough to count as healthy: a later blip must reconnect
        // promptly instead of inheriting a 30s delay from hours ago.
        assert_eq!(
            backoff_after(
                Duration::from_secs(30),
                initial,
                Duration::from_secs(600),
                healthy
            ),
            initial
        );
    }

    #[tokio::test]
    async fn health_reports_connected_once_the_child_outlives_the_healthy_threshold() {
        let log: SpawnLog = Arc::new(Mutex::new(Vec::new()));
        let sup = supervisor(
            pending_spawner(Arc::clone(&log)),
            Duration::from_millis(5),
            Duration::from_millis(30),
            no_processes(),
            recording_killer(Default::default()),
        );
        sup.ensure("mefistos", 4180, 4180);
        // Before the threshold it is supervised but NOT yet connected — this is
        // the distinction the onboarding UI was missing.
        wait_for_spawns(&log, 1).await;
        assert!(!sup.health()["mefistos"].connected);
        wait_until(|| sup.health()["mefistos"].connected, "connected").await;
        let h = sup.health().remove("mefistos").unwrap();
        assert!(h.supervised);
        assert_eq!(h.consecutive_failures, 0);
        sup.stop_all();
    }

    #[tokio::test]
    async fn health_records_the_exit_code_and_ssh_stderr_of_a_failing_tunnel() {
        let sup = supervisor(
            Arc::new(|_| {
                Box::pin(async {
                    TunnelExit {
                        code: Some(255),
                        stderr: "bind [127.0.0.1]:4180: Address already in use".into(),
                    }
                })
            }),
            Duration::from_millis(5),
            Duration::from_secs(3600),
            no_processes(),
            recording_killer(Default::default()),
        );
        sup.ensure("mefistos", 4180, 4180);
        wait_until(
            || {
                sup.health()
                    .get("mefistos")
                    .is_some_and(|h| h.consecutive_failures >= 2)
            },
            "two recorded failures",
        )
        .await;
        let h = sup.health().remove("mefistos").unwrap();
        assert!(!h.connected, "a crash-looping tunnel is not connected");
        assert!(h.supervised, "the supervising task is still running");
        assert_eq!(h.last_exit_code, Some(255));
        assert!(
            h.last_error
                .as_deref()
                .unwrap_or("")
                .contains("Address already in use"),
            "ssh stderr must reach the operator: {:?}",
            h.last_error
        );
        assert!(h.last_connected_unix.is_none());
        sup.stop_all();
    }

    #[tokio::test]
    async fn health_backoff_returns_to_the_initial_delay_after_a_healthy_run() {
        // Third spawn stays up past the healthy threshold, then exits; the
        // supervisor must go back to the initial delay rather than the 4x one
        // the two earlier fast failures had grown.
        let n = Arc::new(Mutex::new(0u32));
        let spawner: TunnelSpawner = Arc::new(move |_| {
            let mut c = n.lock().unwrap();
            *c += 1;
            let nth = *c;
            drop(c);
            Box::pin(async move {
                if nth == 3 {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                }
                TunnelExit::code(255)
            })
        });
        let sup = supervisor(
            spawner,
            Duration::from_millis(20),
            Duration::from_millis(40),
            no_processes(),
            recording_killer(Default::default()),
        );
        sup.ensure("mefistos", 4180, 4180);
        wait_until(
            || {
                sup.health()
                    .get("mefistos")
                    .is_some_and(|h| h.backoff_ms == 80)
            },
            "backoff grown to 80ms by two fast failures",
        )
        .await;
        wait_until(
            || {
                sup.health()
                    .get("mefistos")
                    .is_some_and(|h| h.backoff_ms == 20)
            },
            "backoff reset to 20ms after the healthy run",
        )
        .await;
        sup.stop_all();
    }

    // ---- Bug 2: reaping orphans left by a killed app instance ----

    #[tokio::test]
    async fn ensure_reaps_an_orphaned_tunnel_before_spawning() {
        let orphan = "ssh -N -R 127.0.0.1:4180:127.0.0.1:4180 -- mefistos".to_string();
        let killed: Arc<Mutex<Vec<u32>>> = Default::default();
        let log: SpawnLog = Arc::new(Mutex::new(Vec::new()));
        let sup = supervisor(
            pending_spawner(Arc::clone(&log)),
            Duration::from_millis(5),
            Duration::from_secs(3600),
            Arc::new(move || vec![(4242, orphan.clone())]),
            recording_killer(Arc::clone(&killed)),
        );
        sup.ensure("mefistos", 4180, 4180);
        wait_until(
            || killed.lock().unwrap().contains(&4242),
            "the orphan to be killed",
        )
        .await;
        sup.stop_all();
    }

    #[tokio::test]
    async fn a_bind_conflict_reaps_again_before_the_next_attempt() {
        // Retrying can never clear "address already in use" on its own — the
        // holder has to go. Without this the app restart-loops forever while an
        // orphan owns the port, which is exactly what the logs showed.
        let orphan = "ssh -N -R 127.0.0.1:4180:127.0.0.1:4180 -- mefistos".to_string();
        let killed: Arc<Mutex<Vec<u32>>> = Default::default();
        let sup = supervisor(
            Arc::new(|_| {
                Box::pin(async {
                    TunnelExit {
                        code: Some(255),
                        stderr: "Error: remote port forwarding failed for listen port 4180".into(),
                    }
                })
            }),
            Duration::from_millis(5),
            Duration::from_secs(3600),
            Arc::new(move || vec![(4242, orphan.clone())]),
            recording_killer(Arc::clone(&killed)),
        );
        sup.ensure("mefistos", 4180, 4180);
        wait_until(
            || killed.lock().unwrap().len() >= 3,
            "a reap before the initial spawn and before each retry",
        )
        .await;
        sup.stop_all();
    }

    #[tokio::test]
    async fn an_ordinary_failure_does_not_keep_reaping() {
        // Only a bind conflict implicates another process. A refused connection
        // must not send us hunting for pids to kill on every retry.
        let orphan = "ssh -N -R 127.0.0.1:4180:127.0.0.1:4180 -- mefistos".to_string();
        let killed: Arc<Mutex<Vec<u32>>> = Default::default();
        let log: SpawnLog = Arc::new(Mutex::new(Vec::new()));
        let log_for = Arc::clone(&log);
        let sup = supervisor(
            Arc::new(move |argv| {
                log_for
                    .lock()
                    .unwrap()
                    .push((argv, std::time::Instant::now()));
                Box::pin(async {
                    TunnelExit {
                        code: Some(255),
                        stderr: "ssh: connect to host mefistos port 22: Connection refused".into(),
                    }
                })
            }),
            Duration::from_millis(5),
            Duration::from_secs(3600),
            Arc::new(move || vec![(4242, orphan.clone())]),
            recording_killer(Arc::clone(&killed)),
        );
        sup.ensure("mefistos", 4180, 4180);
        wait_for_spawns(&log, 3).await;
        assert_eq!(
            killed.lock().unwrap().len(),
            1,
            "only the one reap that precedes the first spawn"
        );
        sup.stop_all();
    }
}
