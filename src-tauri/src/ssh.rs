//! Per-host ControlMaster-backed SSH client.
//!
//! Why ControlMaster: every list_sessions / kill / rename involves a tmux
//! command on the remote host. Without a persistent socket each call pays
//! the full ssh handshake (~500-2000ms on a LAN, more over WAN). With
//! ControlMaster the first call sets up a background `ssh -M -N`, every
//! subsequent call multiplexes through it and returns in <50ms.
//!
//! Socket path: ~/.cache/claude-fleet/cm-<host>.sock — dedicated to this app
//! so we never collide with a user's global ssh ControlPath setting.
//!
//! ## Concurrency model (iter 4a)
//!
//! `masters` is a `DashMap<String, Arc<OnceCell<()>>>`. The `OnceCell` for
//! each host guarantees that concurrent first-touches share exactly one master
//! spawn: `get_or_try_init` blocks all waiters until the winner's future
//! resolves, then returns the cached `Ok(())` to every other caller.
//!
//! Error semantics: `tokio::sync::OnceCell::get_or_try_init` does NOT cache
//! `Err`. If the init future fails, the cell stays empty and the next call
//! retries — this is intentional, so a transient ssh failure (e.g. host
//! briefly unreachable) doesn't poison the host's master slot for the lifetime
//! of the process.

use crate::ipc_error::IpcError;
use dashmap::DashMap;
use std::path::PathBuf;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

struct SshClientInner {
    /// Per-host OnceCell. The cell's init future runs the master-spawn exactly
    /// once; subsequent callers receive the cached result immediately.
    masters: DashMap<String, Arc<OnceCell<()>>>,
}

/// Cheaply cloneable SSH client. Each clone shares the same underlying
/// `DashMap` of ControlMaster cells via `Arc`, so the master is only
/// ever spawned once per host even across concurrent clones.
#[derive(Clone)]
pub struct SshClient {
    inner: Arc<SshClientInner>,
}

impl SshClient {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SshClientInner {
                masters: DashMap::new(),
            }),
        }
    }

    /// Returns the dedicated ControlPath for a host. Side effect: creates
    /// the parent dir if missing. The path is used by both `-M` master spawn
    /// and subsequent `-o ControlPath=...` calls.
    pub fn control_path(&self, host: &str) -> PathBuf {
        let dir = cache_dir();
        // best-effort: ignore errors (caller falls back to per-call ssh if
        // dir doesn't exist).
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("cm-{host}.sock"))
    }

    /// Spawn a background master if we haven't already for this host.
    ///
    /// Concurrent calls for the same host share a single `OnceCell`: exactly
    /// one task runs the spawn future; all others await and reuse the result.
    pub async fn ensure_master(&self, host: &str) -> Result<(), IpcError> {
        let cell = self
            .inner
            .masters
            .entry(host.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        cell.get_or_try_init(|| async {
            let path = self.control_path(host);

            // If a stale socket exists, ask any orphan master to exit. Errors
            // are non-fatal — `-O exit` returns 255 if no master is listening.
            let _ = tokio::process::Command::new("ssh")
                .args([
                    "-o",
                    &format!("ControlPath={}", path.display()),
                    "-O",
                    "exit",
                    "--",
                    host,
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            // Spawn the master. -f makes ssh fork into the background after
            // authenticating. -N requests no remote command. ControlPersist
            // keeps the master idle for 10 minutes before self-closing.
            let status = tokio::process::Command::new("ssh")
                .args([
                    "-fN",
                    "-o",
                    "ControlMaster=yes",
                    "-o",
                    &format!("ControlPath={}", path.display()),
                    "-o",
                    "ControlPersist=10m",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=5",
                    "--",
                    host,
                ])
                .status()
                .await
                .map_err(|e| IpcError::new("E_SSH", format!("spawn ssh master: {e}")))?;

            if !status.success() {
                return Err(IpcError::new(
                    "E_SSH",
                    format!("ssh master to {host} failed (status: {status:?})"),
                ));
            }
            Ok(())
        })
        .await
        .map(|_| ())
    }

    /// Run a command on `host`, multiplexing through the established master.
    /// `timeout` is enforced via `-o ConnectTimeout` (handshake only); the
    /// command itself runs to completion — we trust tmux invocations to be
    /// fast. Returns the full Output for callers to inspect stdout/stderr.
    ///
    /// NOTE: callers are converted to async in iter 4a Task 5; until then
    /// this method is async and call sites that haven't been migrated yet
    /// will fail to compile (expected — Task 5 fixes them).
    pub async fn run(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, IpcError> {
        self.ensure_master(host).await?;
        let path = self.control_path(host);
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args([
            "-o",
            &format!("ControlPath={}", path.display()),
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("ConnectTimeout={}", timeout.as_secs().max(1)),
            // `--` ends option parsing — the host can never be read as an
            // ssh option even if validation upstream were bypassed.
            "--",
            host,
        ]);
        cmd.args(args);
        // tokio::process::Command::output() returns tokio's Output which is
        // std::process::Output re-exported — same type as before.
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
        self.ensure_master(host).await?;
        let path = self.control_path(host);
        let mut child = tokio::process::Command::new("ssh")
            .args([
                "-o",
                &format!("ControlPath={}", path.display()),
                "-o",
                "BatchMode=yes",
                "-o",
                &format!("ConnectTimeout={}", timeout.as_secs().max(1)),
                "--",
                host,
            ])
            .args(args)
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

    /// Tell every known master to exit. Called from Tauri on_exit so we
    /// don't leak persistent ssh processes after the app closes.
    ///
    /// This is intentionally synchronous (blocking) because it is called from
    /// a sync on_exit hook. It uses `std::process::Command` directly (NOT
    /// tokio) so it works without a running runtime — best-effort fire-and-
    /// forget cleanup.
    pub fn shutdown_all(&self) {
        let hosts: Vec<String> = self.inner.masters.iter().map(|e| e.key().clone()).collect();
        for host in hosts {
            let path = self.control_path(&host);
            // Fire-and-forget via std::process::Command — shutdown_all is
            // called from a sync context and we just want best-effort cleanup.
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
                .status();
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
    fn shutdown_when_no_masters_is_noop() {
        let c = SshClient::new();
        c.shutdown_all(); // must not panic when masters map is empty
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

    #[tokio::test]
    async fn ensure_master_idempotent_across_concurrent_calls() {
        // Two concurrent ensure_master() for the same alias should resolve in ≤ 1
        // master spawn. The actual ssh call will fail (alias doesn't exist), but
        // OnceCell semantics still apply: both calls share the same cell.
        let client = SshClient::new();
        let alias = "nonexistent-test-host-for-iter4a";
        let (a, b) = tokio::join!(client.ensure_master(alias), client.ensure_master(alias),);
        // Either both Ok (somehow worked) or both Err (the expected case); both
        // must agree — single OnceCell guarantees that.
        assert_eq!(a.is_ok(), b.is_ok(), "concurrent ensure_master must agree");
        assert_eq!(client.inner.masters.len(), 1, "exactly one cell registered");
    }
}
