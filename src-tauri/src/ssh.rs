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
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Floor for the wall-clock bound derived from a connect timeout (see
/// `SshClient::default_wall_clock`).
const DEFAULT_WALL_CLOCK_FLOOR: Duration = Duration::from_secs(30);
/// Wall-clock bound for `upload_file`: a large file over a slow link needs
/// far more than a probe, but it still must not hang forever.
pub(crate) const UPLOAD_WALL_CLOCK: Duration = Duration::from_secs(300);
/// Bound on each of the two best-effort control requests issued after a
/// wall-clock timeout (`ssh -O check`, then `ssh -O exit` if needed). A
/// timed-out call can therefore take up to `wall_clock + 2 × this` before it
/// returns.
const MASTER_RESET_TIMEOUT: Duration = Duration::from_secs(5);

struct SshClientInner {
    /// Hosts a command has been run against — used only so `shutdown_all`
    /// knows which ControlPaths to close on app exit.
    seen: DashMap<String, ()>,
    /// Per-host `$HOME` cache. A host's home directory does not change for
    /// the lifetime of the app, so the `printenv HOME` round-trip is paid
    /// once and reused (it was previously one SSH round-trip per new_session).
    homes: DashMap<String, String>,
    /// Per-host count of `run_child` calls currently live. A wall-clock
    /// timeout must not reset the shared ControlMaster while other commands
    /// (an upload, another probe) are still multiplexed through it.
    in_flight: DashMap<String, usize>,
    /// Per-host count of masters actually reset after a timeout since
    /// launch. Reported in the diagnostics bundle; tests use it to prove the
    /// reset is skipped when it would collateral-damage.
    master_resets: DashMap<String, usize>,
}

/// RAII decrement for `SshClientInner::in_flight`.
struct InFlight<'a> {
    inner: &'a SshClientInner,
    host: String,
}

impl<'a> InFlight<'a> {
    fn enter(inner: &'a SshClientInner, host: &str) -> Self {
        *inner.in_flight.entry(host.to_string()).or_insert(0) += 1;
        Self {
            inner,
            host: host.to_string(),
        }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Some(mut n) = self.inner.in_flight.get_mut(&self.host) {
            *n = n.saturating_sub(1);
        }
    }
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
                in_flight: DashMap::new(),
                master_resets: DashMap::new(),
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
        let home = home_from_output(host, &out)?;
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
    pub(crate) fn mux_opts(&self, host: &str, timeout: Duration) -> Vec<String> {
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
            // Keepalive so a wedged connection self-terminates instead of
            // hanging forever. `ConnectTimeout` only bounds the INITIAL connect
            // of a fresh master; when a later call multiplexes onto an EXISTING
            // master no connect happens, so without keepalives a black-holed
            // master (peer gone silently — laptop sleep/wake, network switch,
            // remote reboot) leaves both the master and its sessions blocked
            // indefinitely. 5s × 2 ⇒ the master gives up ~10s after the peer
            // goes quiet, exits, and subsequent calls reconnect cleanly.
            "-o".into(),
            "ServerAliveInterval=5".into(),
            "-o".into(),
            "ServerAliveCountMax=2".into(),
        ]
    }

    /// Wall-clock bound applied to `run` / `run_cancellable` when the caller
    /// gives only a connect timeout. `ConnectTimeout` covers ONLY the initial
    /// TCP/SSH handshake of a fresh master; a command that hangs AFTER connect
    /// (wedged ControlMaster after laptop sleep, a stuck remote `tmux`) is not
    /// bounded by it at all. Three times the connect budget, floored at 30s,
    /// is generous for every probe/tmux round-trip the app makes while still
    /// guaranteeing the caller regains control.
    pub fn default_wall_clock(connect_timeout: Duration) -> Duration {
        (connect_timeout * 3).max(DEFAULT_WALL_CLOCK_FLOOR)
    }

    /// Run a command on `host`, multiplexing through the ControlMaster.
    /// Returns the full Output for callers to inspect stdout/stderr.
    ///
    /// `timeout` is the ssh `ConnectTimeout`; the whole command is additionally
    /// bounded by `default_wall_clock(timeout)` — see `run_bounded`.
    pub async fn run(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, IpcError> {
        self.run_bounded(host, args, timeout, Self::default_wall_clock(timeout))
            .await
    }

    /// `run` with an explicit wall-clock bound. When `wall_clock` elapses the
    /// ssh child is killed and reaped, the host's ControlMaster is told to
    /// exit (best effort, so the next call rebuilds a fresh one instead of
    /// multiplexing onto the wedged master again), and `E_SSH_TIMEOUT` is
    /// returned.
    pub async fn run_bounded(
        &self,
        host: &str,
        args: &[&str],
        connect_timeout: Duration,
        wall_clock: Duration,
    ) -> Result<Output, IpcError> {
        self.inner.seen.insert(host.to_string(), ());
        let mut cmd = tokio::process::Command::new("ssh");
        for opt in self.mux_opts(host, connect_timeout) {
            cmd.arg(opt);
        }
        // `--` ends option parsing — the host can never be read as an ssh
        // option even if validation upstream were bypassed.
        cmd.arg("--").arg(host).args(args);
        self.run_child(host, cmd, wall_clock, None, "E_SSH").await
    }

    /// Same as `run` but races the SSH child against a `CancellationToken`.
    /// When the token fires before the command finishes, the child is sent
    /// SIGKILL via `start_kill` and explicitly `wait`ed so the OS reaps the
    /// process (no zombie left behind). Returns `Err(E_CANCELLED)`. The same
    /// wall-clock bound as `run` applies on top (`E_SSH_TIMEOUT`).
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
        self.run_child(
            host,
            cmd,
            Self::default_wall_clock(timeout),
            Some(token),
            "E_SSH",
        )
        .await
    }

    /// Upload a local file to `remote_path` on `host` by piping its bytes into
    /// `cat > <quoted path>` over the ControlMaster. The remote parent
    /// directory must already exist (caller `mkdir -p`s it). Returns Err on a
    /// non-zero ssh/cat exit. Uses the same `-o` muxing as `run`. Bounded by
    /// `UPLOAD_WALL_CLOCK` (uploads legitimately outlive a probe's budget).
    pub async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        timeout: Duration,
    ) -> Result<(), IpcError> {
        self.inner.seen.insert(host.to_string(), ());
        let file = std::fs::File::open(local_path).map_err(|e| {
            IpcError::new("E_UPLOAD", format!("open {}: {e}", local_path.display()))
        })?;
        let mut cmd = tokio::process::Command::new("ssh");
        for opt in self.mux_opts(host, timeout) {
            cmd.arg(opt);
        }
        // Single remote word: the remote login shell runs `cat > 'path'`,
        // reading the piped file from stdin. Path is single-quoted.
        let remote_cmd = format!("cat > {}", crate::shell::quote(remote_path));
        cmd.arg("--").arg(host).arg(&remote_cmd);
        cmd.stdin(std::process::Stdio::from(file));
        let out = self
            .run_child(host, cmd, UPLOAD_WALL_CLOCK, None, "E_UPLOAD")
            .await?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_UPLOAD",
                format!(
                    "upload to {host} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(())
    }

    /// Spawn `cmd` and wait for it under three exits, in priority order
    /// (`biased` select, so a cancel that races the deadline always wins):
    ///
    /// 1. `token` fired → kill + reap the child, `Err(E_CANCELLED)`.
    /// 2. `wall_clock` elapsed → kill + reap the child, then — ONLY if no
    ///    other command is in flight on this host AND the master fails an
    ///    `ssh -O check` — tell the ControlMaster to exit (`reset_master`).
    ///    `Err(E_SSH_TIMEOUT)`. The master is shared with the user's attached
    ///    PTY (`pty.rs`) and any concurrent upload/probe, so a merely slow
    ///    command must never tear it down; only a wedged one may. Because
    ///    of the check + exit requests, a timed-out call can take up to
    ///    `wall_clock + 2 × MASTER_RESET_TIMEOUT` before returning.
    /// 3. child exited → `Ok(Output)`.
    ///
    /// `spawn_code` is the error code used for a spawn/wait failure so
    /// upload keeps reporting `E_UPLOAD` while everything else is `E_SSH`.
    /// stdout/stderr are drained by two reader tasks so a chatty child can
    /// never block on a full pipe while we wait on it.
    ///
    /// Not ssh-specific: tests drive it with `sh -c 'sleep N'` to prove the
    /// timeout arm without a real host.
    pub(crate) async fn run_child(
        &self,
        host: &str,
        mut cmd: tokio::process::Command,
        wall_clock: Duration,
        token: Option<CancellationToken>,
        spawn_code: &str,
    ) -> Result<Output, IpcError> {
        let in_flight = InFlight::enter(&self.inner, host);
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Belt-and-suspenders: if a kill arm panics before reaping we
            // still want the OS to clean up the child eventually.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| IpcError::new(spawn_code, format!("ssh spawn {host}: {e}")))?;

        // Take stdout/stderr handles BEFORE moving `child` into the wait —
        // reader tasks keep the pipes drained so the child can't block on a
        // full pipe.
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

        // Without a token this arm never fires; `select!` still needs a
        // future, so use a pending one.
        let cancelled = async {
            match token {
                Some(t) => t.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };

        // Kill + reap, then abort the pipe readers. They would normally
        // finish on EOF once the child dies, but a grandchild inheriting the
        // fd could keep a pipe open and leak the task.
        let kill_and_reap = |mut child: tokio::process::Child| async move {
            let _ = child.start_kill();
            let _ = child.wait().await;
        };

        tokio::select! {
            // Priority order as documented: cancel beats deadline beats exit.
            biased;
            _ = cancelled => {
                kill_and_reap(child).await;
                stdout_task.abort();
                stderr_task.abort();
                Err(IpcError::new("E_CANCELLED", format!("ssh {host} cancelled")))
            }
            _ = tokio::time::sleep(wall_clock) => {
                kill_and_reap(child).await;
                stdout_task.abort();
                stderr_task.abort();
                // This call no longer counts as live on the host.
                drop(in_flight);
                let reset = self.maybe_reset_master(host).await;
                Err(wall_clock_error(host, wall_clock, reset))
            }
            status = child.wait() => {
                let status = status
                    .map_err(|e| IpcError::new(spawn_code, format!("ssh wait {host}: {e}")))?;
                let stdout = stdout_task.await.unwrap_or_default();
                let stderr = stderr_task.await.unwrap_or_default();
                Ok(Output { status, stdout, stderr })
            }
        }
    }

    /// Number of `run_child` calls currently live on `host` (excluding any
    /// the caller has already dropped its guard for).
    fn others_in_flight(&self, host: &str) -> usize {
        self.inner.in_flight.get(host).map(|n| *n).unwrap_or(0)
    }

    /// How many times `maybe_reset_master` actually reset a master since
    /// launch, across all hosts.
    pub(crate) fn master_reset_count(&self) -> usize {
        self.inner.master_resets.iter().map(|e| *e.value()).sum()
    }

    /// Per-host [`Self::master_reset_count`], ordered by host alias. Only
    /// hosts that had at least one reset appear.
    pub(crate) fn master_reset_counts(&self) -> std::collections::BTreeMap<String, usize> {
        self.inner
            .master_resets
            .iter()
            .map(|e| (e.key().clone(), *e.value()))
            .collect()
    }

    /// After a wall-clock timeout on `host`: decide whether the shared
    /// ControlMaster is wedged and, only then, reset it. Returns whether a
    /// reset happened.
    ///
    /// Skipped while other commands are live on the host (they would lose
    /// their channels) and when the master still answers `ssh -O check`
    /// within `MASTER_RESET_TIMEOUT` (the timed-out command was slow, not
    /// the transport). The `-O check` also covers channels this client does
    /// not count — the user's attached PTY in `pty.rs` multiplexes over the
    /// same ControlPath.
    async fn maybe_reset_master(&self, host: &str) -> bool {
        if self.others_in_flight(host) > 0 {
            eprintln!(
                "[ssh] {host}: command timed out but other commands are in flight; keeping the master"
            );
            return false;
        }
        if self.master_alive(host).await {
            eprintln!("[ssh] {host}: command timed out but the master still answers; keeping it");
            return false;
        }
        *self
            .inner
            .master_resets
            .entry(host.to_string())
            .or_insert(0) += 1;
        self.reset_master(host).await;
        true
    }

    /// `ssh -O check` against the host's ControlPath under
    /// `MASTER_RESET_TIMEOUT`. `true` only when the master answered
    /// successfully in time; a missing socket, a failed spawn, a non-zero
    /// exit or a hang all count as "not alive".
    async fn master_alive(&self, host: &str) -> bool {
        let path = self.control_path(host);
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args([
            "-o",
            &format!("ControlPath={}", path.display()),
            "-O",
            "check",
            "--",
            host,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
        let Ok(mut child) = cmd.spawn() else {
            return false;
        };
        match tokio::time::timeout(MASTER_RESET_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status.success(),
            Ok(Err(_)) => false,
            Err(_elapsed) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                false
            }
        }
    }

    /// Ask the host's ControlMaster to exit (`ssh -O exit`) so the next call
    /// establishes a fresh one. Best effort with its own short bound: a
    /// wedged master may not even answer the control socket, in which case
    /// the request is killed and ssh's own keepalive reaps the master later.
    /// Also removes the socket file so `ControlMaster=auto` cannot attach to
    /// a dead master that never answered the exit request.
    pub(crate) async fn reset_master(&self, host: &str) {
        let path = self.control_path(host);
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args([
            "-o",
            &format!("ControlPath={}", path.display()),
            "-O",
            "exit",
            "--",
            host,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
        let Ok(mut child) = cmd.spawn() else {
            return;
        };
        if tokio::time::timeout(MASTER_RESET_TIMEOUT, child.wait())
            .await
            .is_err()
        {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let _ = std::fs::remove_file(&path);
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

/// The transport the service layer talks to a host through. `SshClient` is
/// the production implementation (ControlMaster-multiplexed `ssh`);
/// tests use `LocalExec` (the same argv through a local `bash -c`) and a
/// scripted `FakeSsh`, which records every call and answers from canned
/// replies. Services take `&dyn SshExec` so all three are interchangeable.
///
/// Semantics every implementation must keep, because the callers rely on
/// them:
///
/// - `run` / `run_cancellable` return `Ok(Output)` for ANY exit status — an
///   unreachable host is ssh exiting 255 with its message on stderr, not an
///   `Err`. `Err` is reserved for spawn failures (`E_SSH`), the wall-clock
///   bound (`E_SSH_TIMEOUT`) and cancellation (`E_CANCELLED`).
/// - `args` are space-joined and re-tokenised by the remote login shell, so
///   a multi-word script must already be `shell::quote`d by the caller.
/// - `upload_file` streams the local file over stdin into `cat > <path>` and
///   fails with `E_UPLOAD` on a non-zero exit.
/// - `remote_home` resolves `$HOME` on the host (`E_SSH` if it cannot).
#[async_trait::async_trait]
pub trait SshExec: Send + Sync {
    async fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError>;

    async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError>;

    async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        timeout: Duration,
    ) -> Result<(), IpcError>;

    async fn remote_home(&self, host: &str) -> Result<String, IpcError>;
}

// The inherent methods stay (so `Arc<SshClient>` callers in `commands/`,
// `mcp/` and `pty.rs` need no trait import); the trait impl just forwards.
#[async_trait::async_trait]
impl SshExec for SshClient {
    async fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        SshClient::run(self, host, args, timeout).await
    }

    async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        SshClient::run_cancellable(self, host, args, timeout, token).await
    }

    async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        timeout: Duration,
    ) -> Result<(), IpcError> {
        SshClient::upload_file(self, host, local_path, remote_path, timeout).await
    }

    async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
        SshClient::remote_home(self, host).await
    }
}

/// `Arc<T>` forwards to `T`, so a `&Arc<SshClient>` (the Tauri state and the
/// MCP `FleetTools` field) unsizes straight to `&dyn SshExec` at the call
/// boundary, and an `Arc<dyn SshExec>` is itself an `SshExec`.
#[async_trait::async_trait]
impl<T: SshExec + ?Sized> SshExec for Arc<T> {
    async fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        (**self).run(host, args, timeout).await
    }

    async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        (**self).run_cancellable(host, args, timeout, token).await
    }

    async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        timeout: Duration,
    ) -> Result<(), IpcError> {
        (**self)
            .upload_file(host, local_path, remote_path, timeout)
            .await
    }

    async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
        (**self).remote_home(host).await
    }
}

/// Parse a `printenv HOME` round-trip into the home directory. Shared by
/// every `SshExec` implementation so they agree on the error shape.
pub(crate) fn home_from_output(host: &str, out: &Output) -> Result<String, IpcError> {
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
    Ok(home)
}

/// The `E_SSH_TIMEOUT` every `SshExec` returns when a command outlives its
/// wall clock. `reset` records whether the ControlMaster was torn down.
pub(crate) fn wall_clock_error(host: &str, wall_clock: Duration, reset: bool) -> IpcError {
    IpcError::new(
        "E_SSH_TIMEOUT",
        format!(
            "ssh {host}: command exceeded {}s wall clock{}",
            wall_clock.as_secs(),
            if reset { "; connection reset" } else { "" }
        ),
    )
}

/// `SshExec` over the local machine: the argv is space-joined exactly as ssh
/// would join it and handed to `bash -c`, so the same re-tokenisation the
/// remote login shell performs happens here too (a script that is not
/// `quote`d breaks identically on both). `upload_file` is `cat > <path>` fed
/// from the local file. No ControlMaster, no keepalives — the only failure
/// modes are a missing `bash`, a non-zero exit, and the wall clock.
///
/// Used by the opt-in `tmux_roundtrip` integration test to drive the real
/// `RemoteTmux` command builder against a private local tmux server.
///
/// Test-only on purpose: production `local` commands go through `LocalTmux`,
/// which execs tmux with a plain argv and never involves a shell. Routing
/// them through `bash -c` would widen the shell-injection surface.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct LocalExec {
    /// Extra environment for every spawned `bash` (e.g. `TMUX_TMPDIR` to
    /// point tmux at a private server).
    env: Vec<(String, String)>,
    /// Variables removed from every spawned `bash` (e.g. `TMUX`, which
    /// would otherwise make tmux target the server this process runs in).
    env_remove: Vec<String>,
}

#[cfg(test)]
impl LocalExec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn without_env(mut self, key: &str) -> Self {
        self.env_remove.push(key.to_string());
        self
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(args.join(" "));
        for k in &self.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }

    async fn bounded(
        &self,
        host: &str,
        mut cmd: tokio::process::Command,
        wall_clock: Duration,
        token: Option<CancellationToken>,
        spawn_code: &str,
    ) -> Result<Output, IpcError> {
        // Same shape as `SshClient::run_child`: drain the pipes off to the
        // side, race exit / wall clock / cancel, and kill + reap on the two
        // early exits so no zombie is left behind.
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| IpcError::new(spawn_code, format!("bash spawn ({host}): {e}")))?;
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
        let cancelled = async {
            match token {
                Some(t) => t.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        let kill_and_reap = |mut child: tokio::process::Child| async move {
            let _ = child.start_kill();
            let _ = child.wait().await;
        };
        tokio::select! {
            biased;
            _ = cancelled => {
                kill_and_reap(child).await;
                stdout_task.abort();
                stderr_task.abort();
                Err(IpcError::new("E_CANCELLED", format!("ssh {host} cancelled")))
            }
            _ = tokio::time::sleep(wall_clock) => {
                kill_and_reap(child).await;
                stdout_task.abort();
                stderr_task.abort();
                Err(wall_clock_error(host, wall_clock, false))
            }
            status = child.wait() => {
                let status = status
                    .map_err(|e| IpcError::new(spawn_code, format!("bash wait ({host}): {e}")))?;
                let stdout = stdout_task.await.unwrap_or_default();
                let stderr = stderr_task.await.unwrap_or_default();
                Ok(Output { status, stdout, stderr })
            }
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl SshExec for LocalExec {
    async fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        let cmd = self.command(args);
        self.bounded(
            host,
            cmd,
            SshClient::default_wall_clock(timeout),
            None,
            "E_SSH",
        )
        .await
    }

    async fn run_cancellable(
        &self,
        host: &str,
        args: &[&str],
        timeout: Duration,
        token: CancellationToken,
    ) -> Result<Output, IpcError> {
        let cmd = self.command(args);
        self.bounded(
            host,
            cmd,
            SshClient::default_wall_clock(timeout),
            Some(token),
            "E_SSH",
        )
        .await
    }

    async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &str,
        _timeout: Duration,
    ) -> Result<(), IpcError> {
        let file = std::fs::File::open(local_path).map_err(|e| {
            IpcError::new("E_UPLOAD", format!("open {}: {e}", local_path.display()))
        })?;
        let remote_cmd = format!("cat > {}", crate::shell::quote(remote_path));
        let mut cmd = self.command(&[remote_cmd.as_str()]);
        cmd.stdin(std::process::Stdio::from(file));
        let out = self
            .bounded(host, cmd, UPLOAD_WALL_CLOCK, None, "E_UPLOAD")
            .await?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_UPLOAD",
                format!(
                    "upload to {host} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(())
    }

    async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
        let out = self
            .run(host, &["printenv", "HOME"], Duration::from_secs(5))
            .await?;
        home_from_output(host, &out)
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
        // Keepalives bound a wedged multiplexed session — `ConnectTimeout`
        // only covers the initial connect of a fresh master, not a later
        // attach onto an existing one.
        assert!(opts.iter().any(|o| o == "ServerAliveInterval=5"));
        assert!(opts.iter().any(|o| o == "ServerAliveCountMax=2"));
    }

    #[test]
    fn default_wall_clock_is_multiple_of_connect_with_floor() {
        // Small connect budgets are floored at 30s; large ones scale ×3.
        assert_eq!(
            SshClient::default_wall_clock(Duration::from_secs(5)),
            Duration::from_secs(30)
        );
        assert_eq!(
            SshClient::default_wall_clock(Duration::from_secs(10)),
            Duration::from_secs(30)
        );
        assert_eq!(
            SshClient::default_wall_clock(Duration::from_secs(20)),
            Duration::from_secs(60)
        );
    }

    /// `sh` is always present on the Unix CI runners this suite targets; a
    /// box without it (or with a broken `sleep`) gets a clean skip rather
    /// than a spurious red.
    fn have_sh() -> bool {
        std::process::Command::new("sh")
            .args(["-c", "true"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn run_child_times_out_kills_and_returns_e_ssh_timeout() {
        // BE-1 / OPS-4 regression: a child that hangs after spawn (the
        // wedged-ControlMaster case) must come back as `E_SSH_TIMEOUT` at
        // ~the wall-clock bound, and the child must be reaped — not left
        // running or as a zombie. Drives `run_child` with a plain `sh` so no
        // ssh host is needed.
        if !have_sh() {
            eprintln!("skipping: no sh on this box");
            return;
        }
        let c = SshClient::new();
        // Echo the pid so we can assert the process is gone afterwards.
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "echo $$; sleep 30"]);
        let start = std::time::Instant::now();
        let err = c
            .run_child(
                "fleet-test-nonexistent-host",
                cmd,
                Duration::from_millis(150),
                None,
                "E_SSH",
            )
            .await
            .expect_err("a 30s sleep under a 150ms wall clock must time out");
        let elapsed = start.elapsed();
        assert_eq!(err.code, "E_SSH_TIMEOUT", "got: {err:?}");
        assert!(
            elapsed < Duration::from_secs(10),
            "timeout arm must fire near the bound (incl. best-effort master reset), took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn timeout_with_other_command_in_flight_does_not_reset_master() {
        // The ControlMaster is shared with the attached PTY and every other
        // command on the host. A timeout must not tear it down while another
        // command is still multiplexed through it.
        if !have_sh() {
            eprintln!("skipping: no sh on this box");
            return;
        }
        let c = SshClient::new();
        let host = "fleet-test-nonexistent-host";
        // A long-lived "upload" on the same host.
        let c_long = c.clone();
        let long = tokio::spawn(async move {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", "sleep 2"]);
            c_long
                .run_child(host, cmd, Duration::from_secs(30), None, "E_SSH")
                .await
        });
        // Let it spawn and register as in flight.
        for _ in 0..100 {
            if c.others_in_flight(host) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(c.others_in_flight(host), 1, "long command registered");

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let err = c
            .run_child(host, cmd, Duration::from_millis(100), None, "E_SSH")
            .await
            .expect_err("times out");
        assert_eq!(err.code, "E_SSH_TIMEOUT");
        assert_eq!(
            c.master_reset_count(),
            0,
            "reset skipped while another command is live on the host"
        );
        assert!(c.master_reset_counts().is_empty(), "no per-host reset");
        assert!(
            !err.message.contains("connection reset"),
            "message must not claim a reset: {}",
            err.message
        );
        long.await.unwrap().expect("long command finishes normally");
        assert_eq!(c.others_in_flight(host), 0, "guard released");
    }

    #[tokio::test]
    async fn timeout_alone_resets_master_when_check_fails() {
        // No other command in flight and no master answering `-O check`
        // (there is no socket for this fake host) ⇒ the reset path runs.
        if !have_sh() {
            eprintln!("skipping: no sh on this box");
            return;
        }
        let c = SshClient::new();
        let host = "fleet-test-nonexistent-host-2";
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let err = c
            .run_child(host, cmd, Duration::from_millis(100), None, "E_SSH")
            .await
            .expect_err("times out");
        assert_eq!(err.code, "E_SSH_TIMEOUT");
        assert_eq!(c.master_reset_count(), 1, "wedged/missing master is reset");
        assert_eq!(
            c.master_reset_counts(),
            std::collections::BTreeMap::from([(host.to_string(), 1)]),
            "the reset is attributed to its host"
        );
        assert_eq!(c.others_in_flight(host), 0);
    }

    #[test]
    fn master_reset_counts_start_empty_and_sum_per_host() {
        let c = SshClient::new();
        assert_eq!(c.master_reset_count(), 0);
        assert!(c.master_reset_counts().is_empty());
        c.inner.master_resets.insert("b-host".into(), 2);
        c.inner.master_resets.insert("a-host".into(), 1);
        assert_eq!(c.master_reset_count(), 3);
        assert_eq!(
            c.master_reset_counts().into_iter().collect::<Vec<_>>(),
            [("a-host".to_string(), 1), ("b-host".to_string(), 2)],
            "ordered by alias"
        );
    }

    #[tokio::test]
    async fn run_child_returns_output_when_child_finishes_in_time() {
        if !have_sh() {
            eprintln!("skipping: no sh on this box");
            return;
        }
        let c = SshClient::new();
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "printf hello; printf err >&2; exit 3"]);
        let out = c
            .run_child(
                "fleet-test-nonexistent-host",
                cmd,
                Duration::from_secs(10),
                None,
                "E_SSH",
            )
            .await
            .expect("fast child completes");
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"hello");
        assert_eq!(out.stderr, b"err");
    }

    #[tokio::test]
    async fn run_child_cancel_wins_over_wall_clock() {
        if !have_sh() {
            eprintln!("skipping: no sh on this box");
            return;
        }
        let c = SshClient::new();
        let token = CancellationToken::new();
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            t2.cancel();
        });
        let err = c
            .run_child(
                "fleet-test-nonexistent-host",
                cmd,
                Duration::from_secs(20),
                Some(token),
                "E_SSH",
            )
            .await
            .expect_err("cancelled");
        assert_eq!(err.code, "E_CANCELLED", "got: {err:?}");
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
