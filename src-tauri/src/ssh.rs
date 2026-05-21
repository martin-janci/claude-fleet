//! ControlMaster-backed SSH client.
//!
//! Why ControlMaster: every list_sessions / kill / rename involves a tmux
//! command on the remote host. Without a persistent socket each call pays
//! the full ssh handshake (~500-2000ms on a LAN, more over WAN). With
//! ControlMaster the first call sets up the master, every subsequent call
//! multiplexes through it and returns in <50ms.
//!
//! Socket path: ~/.cache/claude-fleet/cm-<host>.sock — dedicated to this app
//! so we never collide with a user's global ssh ControlPath setting.
//!
//! ## Master lifecycle
//!
//! Every `run` passes `-o ControlMaster=auto -o ControlPath=... -o
//! ControlPersist=10m`, so ssh itself owns the master: the first connection
//! to a host establishes it, subsequent ones multiplex, and after 10 min
//! idle it self-closes — the next call simply re-establishes it. There is no
//! app-side "is the master spawned" cache to go stale (the previous design
//! cached that in a `OnceCell` and silently lost multiplexing once the master
//! self-closed). Concurrent first-connects are serialised by ssh via the
//! ControlPath.

use crate::ipc_error::IpcError;
use dashmap::DashMap;
use std::path::PathBuf;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct SshClientInner {
    /// Hosts a command has been run against — used only so `shutdown_all`
    /// knows which ControlPaths to close on app exit.
    seen: DashMap<String, ()>,
    /// Per-host `$HOME` cache. A host's home directory does not change for
    /// the lifetime of the app, so the `printenv HOME` round-trip is paid
    /// once and reused (it was previously one SSH round-trip per new_session).
    homes: DashMap<String, String>,
}

/// Cheaply cloneable SSH client. Clones share the same underlying state via
/// `Arc`.
#[derive(Clone)]
pub struct SshClient {
    inner: Arc<SshClientInner>,
}

impl SshClient {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SshClientInner {
                seen: DashMap::new(),
                homes: DashMap::new(),
            }),
        }
    }

    /// Resolve `$HOME` on a remote host, cached for the lifetime of the app.
    /// The first call pays one `printenv HOME` round-trip; later calls return
    /// the cached value. Errors are not cached — a transient failure retries.
    pub async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
        if let Some(home) = self.inner.homes.get(host) {
            return Ok(home.clone());
        }
        let out = self
            .run(host, &["printenv", "HOME"], Duration::from_secs(5))
            .await?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_SSH",
                format!(
                    "couldn't read $HOME on {host}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if home.is_empty() {
            return Err(IpcError::new(
                "E_SSH",
                format!("remote $HOME on {host} is empty"),
            ));
        }
        self.inner.homes.insert(host.to_string(), home.clone());
        Ok(home)
    }

    /// Returns the dedicated ControlPath for a host. Side effect: creates the
    /// parent dir (locked to 0700) if missing.
    pub fn control_path(&self, host: &str) -> PathBuf {
        let dir = cache_dir();
        // best-effort: ignore errors (ssh falls back to a fresh connection
        // if the dir doesn't exist).
        let _ = std::fs::create_dir_all(&dir);
        // Lock the directory to 0700: the ControlMaster sockets inside it are
        // authenticated SSH channels to every configured host. On a shared
        // machine a 0755 dir would let another local user reach them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        dir.join(format!("cm-{host}.sock"))
    }

    /// The `-o` flags shared by every multiplexed ssh invocation. With
    /// `ControlMaster=auto` + `ControlPersist`, ssh creates the master on the
    /// first call and reuses/recreates it as needed — no app-side bookkeeping.
    fn mux_opts(&self, host: &str, timeout: Duration) -> Vec<String> {
        let path = self.control_path(host);
        vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", path.display()),
            "-o".into(),
            "ControlPersist=10m".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            format!("ConnectTimeout={}", timeout.as_secs().max(1)),
        ]
    }

    /// Run a command on `host`, multiplexing through the ControlMaster.
    /// Returns the full Output for callers to inspect stdout/stderr.
    pub async fn run(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, IpcError> {
        self.inner.seen.insert(host.to_string(), ());
        let mut cmd = tokio::process::Command::new("ssh");
        for opt in self.mux_opts(host, timeout) {
            cmd.arg(opt);
        }
        // `--` ends option parsing — the host can never be read as an ssh
        // option even if validation upstream were bypassed.
        cmd.arg("--").arg(host).args(args);
        cmd.output()
            .await
            .map_err(|e| IpcError::new("E_SSH", format!("ssh {host}: {e}")))
    }

    /// Same as `run` but races the SSH child against a `CancellationToken`.
    /// When the token fires before the command finishes, the child is sent
    /// SIGKILL via `start_kill` and explicitly `wait`ed so the OS reaps the
    /// process (no zombie left behind). Returns `Err(E_CANCELLED)`.
    ///
    /// We do NOT rely on `kill_on_drop` alone because tokio's drop guard
    /// only sends the signal — it doesn't await the wait — so the child
    /// would linger as a zombie until init reaps it (or never, if the
    /// runtime keeps running). Explicit kill+wait fixes that.
    pub async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        self.inner.seen.insert(host.to_string(), ());
        let mut cmd = tokio::process::Command::new("ssh");
        for opt in self.mux_opts(host, timeout) {
            cmd.arg(opt);
        }
        cmd.arg("--").arg(host).args(args);
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Belt-and-suspenders: if the cancel arm panics before reaping
            // we still want the OS to clean up the child eventually.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| IpcError::new("E_SSH", format!("ssh spawn {host}: {e}")))?;

        // Take stdout/stderr handles BEFORE moving `child` into the wait —
        // we'll spawn read tasks so the child's pipes don't block when full.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut s) = stdout {
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
            }
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut s) = stderr {
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
            }
            buf
        });

        tokio::select! {
            _ = token.cancelled() => {
                // Send SIGKILL, then wait so the OS reaps the process
                // (otherwise the child becomes a zombie until the runtime
                // exits).
                let _ = child.start_kill();
                let _ = child.wait().await;
                // Abort the pipe-reader tasks. They would normally finish on
                // EOF once the child dies, but a grandchild inheriting the fd
                // could keep a pipe open and leak the task.
                stdout_task.abort();
                stderr_task.abort();
                Err(IpcError::new("E_CANCELLED", format!("ssh {host} cancelled")))
            }
            status = child.wait() => {
                let status = status
                    .map_err(|e| IpcError::new("E_SSH", format!("ssh wait {host}: {e}")))?;
                let stdout = stdout_task.await.unwrap_or_default();
                let stderr = stderr_task.await.unwrap_or_default();
                Ok(Output { status, stdout, stderr })
            }
        }
    }

    /// Tell every touched host's master to exit. Called from Tauri on_exit so
    /// we don't leak persistent ssh processes after the app closes.
    ///
    /// Synchronous (it runs from a sync on_exit hook) and fire-and-forget:
    /// `spawn()` not `status()` so quit isn't serialised on N round-trips,
    /// and ControlPersist would reap an un-exited master anyway.
    pub fn shutdown_all(&self) {
        let hosts: Vec<String> = self.inner.seen.iter().map(|e| e.key().clone()).collect();
        for host in hosts {
            let path = self.control_path(&host);
            let _ = std::process::Command::new("ssh")
                .args([
                    "-o",
                    &format!("ControlPath={}", path.display()),
                    "-O",
                    "exit",
                    "--",
                    &host,
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
}

impl Default for SshClient {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("claude-fleet");
    }
    std::env::temp_dir().join("claude-fleet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_path_lives_under_cache_dir() {
        let c = SshClient::new();
        let p = c.control_path("mefistos");
        assert!(p.ends_with("cm-mefistos.sock"));
        assert!(
            p.to_string_lossy().contains("claude-fleet"),
            "expected path under cache dir, got: {}",
            p.display()
        );
    }

    #[test]
    fn shutdown_when_no_hosts_seen_is_noop() {
        let c = SshClient::new();
        c.shutdown_all(); // must not panic when no host has been touched
    }

    #[test]
    fn mux_opts_carry_controlmaster_auto_and_persist() {
        let c = SshClient::new();
        let opts = c.mux_opts("h", Duration::from_secs(5));
        assert!(opts.iter().any(|o| o == "ControlMaster=auto"));
        assert!(opts.iter().any(|o| o == "ControlPersist=10m"));
        assert!(opts.iter().any(|o| o == "ConnectTimeout=5"));
    }

    #[tokio::test]
    async fn cancel_arm_kills_and_reaps_child() {
        // Replicates the cancel-arm pattern from `run_cancellable`: spawn a
        // long-running child, race a CancellationToken against `child.wait`,
        // and on cancel explicitly `start_kill` + `wait` to reap. After cancel
        // the OS must no longer report the PID as a live process.
        let token = CancellationToken::new();
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sleep");
        let pid = child.id().expect("child has pid");

        let token2 = token.clone();
        tokio::spawn(async move {
            token2.cancel();
        });

        let result: &str = tokio::select! {
            _ = token.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                "cancelled"
            }
            _ = child.wait() => "completed-naturally",
        };
        assert_eq!(result, "cancelled");

        // PID must now be gone (we waited, so no zombie). On Unix, `kill -0
        // <pid>` exits non-zero when the process no longer exists.
        for _ in 0..50 {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                return; // success
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("child pid {pid} still alive 1s after cancel — kill+wait failed to reap");
    }
}
