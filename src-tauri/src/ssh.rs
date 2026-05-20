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

use crate::ipc_error::IpcError;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

pub struct SshClient {
    // Set of hosts for which we've already spawned a master process.
    // Backed by a Mutex<HashSet<String>> so concurrent ensure_master calls
    // serialize and a second call is a cheap no-op.
    started: Mutex<std::collections::HashSet<String>>,
}

impl SshClient {
    pub fn new() -> Self {
        Self {
            started: Mutex::new(std::collections::HashSet::new()),
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

    /// Spawn a background master if we haven't already. Idempotent.
    pub fn ensure_master(&self, host: &str) -> Result<(), IpcError> {
        {
            let started = self
                .started
                .lock()
                .map_err(|_| IpcError::new("E_LOCK", "ssh master mutex poisoned"))?;
            if started.contains(host) {
                return Ok(());
            }
        }
        let path = self.control_path(host);
        // If a stale socket exists, ask any orphan master to exit. Errors
        // are non-fatal — `-O exit` returns 255 if no master is listening.
        let _ = Command::new("ssh")
            .args([
                "-o",
                &format!("ControlPath={}", path.display()),
                "-O",
                "exit",
                host,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Spawn the master. -f makes ssh fork into the background after
        // authenticating. -N requests no remote command. ControlPersist
        // keeps the master idle for 10 minutes before self-closing.
        let status = Command::new("ssh")
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
            .map_err(|e| IpcError::new("E_SSH", format!("spawn ssh master: {e}")))?;
        if !status.success() {
            return Err(IpcError::new(
                "E_SSH",
                format!("ssh master to {host} failed (status: {status:?})"),
            ));
        }
        let mut started = self
            .started
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "ssh master mutex poisoned"))?;
        started.insert(host.to_string());
        Ok(())
    }

    /// Run a command on `host`, multiplexing through the established master.
    /// `timeout` is enforced via `-o ConnectTimeout` (handshake only); the
    /// command itself runs to completion — we trust tmux invocations to be
    /// fast. Returns the full Output for callers to inspect stdout/stderr.
    pub fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        self.ensure_master(host)?;
        let path = self.control_path(host);
        let mut cmd = Command::new("ssh");
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
        cmd.output()
            .map_err(|e| IpcError::new("E_SSH", format!("ssh {host}: {e}")))
    }

    /// Tell every known master to exit. Called from Tauri on_exit so we
    /// don't leak persistent ssh processes after the app closes.
    pub fn shutdown_all(&self) {
        let started = match self.started.lock() {
            Ok(s) => s.clone(),
            Err(_) => return,
        };
        for host in started.iter() {
            let path = self.control_path(host);
            let _ = Command::new("ssh")
                .args([
                    "-o",
                    &format!("ControlPath={}", path.display()),
                    "-O",
                    "exit",
                    host,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
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
        c.shutdown_all(); // must not panic when started set is empty
    }
}
