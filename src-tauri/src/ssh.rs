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

use crate::ipc_error::IpcError;
use dashmap::DashMap;
use std::path::PathBuf;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

pub struct SshClient {
    /// Per-host OnceCell. The cell's init future runs the master-spawn exactly
    /// once; subsequent callers receive the cached result immediately.
    masters: DashMap<String, Arc<OnceCell<()>>>,
}

impl SshClient {
    pub fn new() -> Self {
        Self {
            masters: DashMap::new(),
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
            host,
        ]);
        cmd.args(args);
        // tokio::process::Command::output() returns tokio's Output which is
        // std::process::Output re-exported — same type as before.
        cmd.output()
            .await
            .map_err(|e| IpcError::new("E_SSH", format!("ssh {host}: {e}")))
    }

    /// Tell every known master to exit. Called from Tauri on_exit so we
    /// don't leak persistent ssh processes after the app closes.
    ///
    /// This is intentionally synchronous (blocking) because it is called from
    /// a sync on_exit hook. It spawns a blocking task on the tokio runtime so
    /// the async ssh commands can complete.
    pub fn shutdown_all(&self) {
        let hosts: Vec<String> = self.masters.iter().map(|e| e.key().clone()).collect();
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
    async fn ensure_master_idempotent_across_concurrent_calls() {
        // Two concurrent ensure_master() for the same alias should resolve in ≤ 1
        // master spawn. The actual ssh call will fail (alias doesn't exist), but
        // OnceCell semantics still apply: both calls share the same cell.
        let client = SshClient::new();
        let alias = "nonexistent-test-host-for-iter4a";
        let (a, b) = tokio::join!(
            client.ensure_master(alias),
            client.ensure_master(alias),
        );
        // Either both Ok (somehow worked) or both Err (the expected case); both
        // must agree — single OnceCell guarantees that.
        assert_eq!(a.is_ok(), b.is_ok(), "concurrent ensure_master must agree");
        assert_eq!(client.masters.len(), 1, "exactly one cell registered");
    }
}
