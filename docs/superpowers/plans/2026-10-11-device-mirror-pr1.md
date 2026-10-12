# Device mirror PR 1 (relay core, SSH + local routes) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A **Mirror** action on the Debug devices page opens a native `scrcpy` window for an Android device on this machine (standalone) or on any host this machine can SSH to — in standalone and in hub-client mode.

**Architecture:** A Tauri-free relay in fleet-core (`relay`) opens loopback TCP listeners whose connections are spliced to a `Dialer`; PR 1 ships the `SshDialer` (`ssh -W 127.0.0.1:<port> <alias>`). A Tauri-free `service::device_mirror` checks the device, speaks the adb wire protocol over the relay for a version check and a free forward port, spawns `scrcpy` pointed at the relayed sockets, holds an advisory claim while it runs, and tears everything down when scrcpy exits. Three desktop-local Tauri commands expose it; two `ActionSpec`s put it on the page.

**Tech Stack:** Rust (tokio, async-trait), Tauri 2, declarative pages (`pages/resources.rs`), Svelte 5 unchanged.

**Spec:** `docs/superpowers/specs/2026-10-11-scrcpy-device-mirror-design.md` (read §1, §4, §5, *Errors*, *Security notes*, *Testing*).

## Global Constraints

- Every child process via `fleet_core::proc::command` / `std_command`, never `Command::new` (`no_eprintln_tests::production_code_spawns_through_proc`).
- The relay dials only `127.0.0.1`, and only ports `5037` and `27183..=27199` (`relay::allowed_port`).
- A host alias reaches `ssh` only after `validate::host_alias`, and after a `--` end-of-options marker.
- Never hold a `std::sync::Mutex` guard across an `.await`.
- New error codes live in `ipc_error.rs::codes`: `E_TOOL_MISSING`, `E_ADB_VERSION_MISMATCH`, `E_NO_ROUTE`, `E_MIRROR_FAILED`. Reuse `E_INVALID`, `E_INVALID_STATE`, `E_UNSUPPORTED`, `E_LIMIT`, `E_EXISTS`, `E_NOTFOUND`, `E_CONFLICT`.
- The mirror commands are desktop-local in both modes: verdict `SameInBoth`, never routed to the hub.
- Tests that exec a fake binary write it with `tmux::fake_exec::write_exec` (carries `PROBE_GUARD`), never `std::fs::write` + chmod (ETXTBSY on Linux CI).
- Validation ladder (CLAUDE.md): `cargo fleet-fast-check` while working; `scripts/verify.sh` before each commit; `scripts/verify.sh full` before the PR. Never `cargo build` to check compilation; never `-p` in the inner loop.
- Git: run every git command as `/usr/bin/git` from the worktree root (`.claude/worktrees/scrcpy-device-mirror`); branch `feat/scrcpy-device-mirror`. No pull/push/rebase/stash in subagents.

## Deviations from the spec in this PR (decided while planning)

- **No settings rows.** `devices.mirror_route` only matters once the hub route exists (PR 2). `devices.scrcpy_path` would be a *fleet*-scoped setting (it lives on the hub in client mode), but it describes this machine. PR 1 finds tools on `PATH` plus the usual Homebrew/SDK folders (a GUI app on macOS does not inherit the shell's PATH), with `FLEET_SCRCPY` / `FLEET_ADB` env overrides.
- **No route badge or live event.** The page is declarative: **Mirror** returns an *Output* result ("Mirroring … via ssh"), and **Stop mirror** is a second action. Closing the scrcpy window is the usual way to stop.
- **The device's ready state is `online`** (the field `ready: bool`), not `device` as the spec says. `variant_by` on the page is `kind`, so the actions show on `physical` and `emulator` rows. An iOS physical device gets `E_UNSUPPORTED` from the command.
- **Local route only in standalone mode**, for the host `local` (`service::projects::LOCAL_HOST`). In hub-client mode, `local` is the hub's own machine. Mirroring it needs the hub relay (PR 2), and until then it returns `E_NO_ROUTE` saying so.
- **The claim is renewed** every 10 minutes while the mirror runs (claims lapse after 30), and the app quitting kills scrcpy without releasing. The claim then lapses on its own.

## File structure

| file | responsibility |
|------|----------------|
| `crates/fleet-core/src/ipc_error.rs` (modify) | four new codes |
| `crates/fleet-core/src/relay/mod.rs` (create) | allowlist, `Stream`/`BoxedStream`, `Dialer`, `RelayListener` |
| `crates/fleet-core/src/relay/ssh.rs` (create) | `SshDialer`, `ChildStream` |
| `crates/fleet-core/src/relay/tests.rs` (create) | relay + SSH dialer tests |
| `crates/fleet-core/src/lib.rs` (modify) | `pub mod relay;` |
| `crates/fleet-core/src/service/device_mirror/adb.rs` (create) | adb wire client, local adb version, forward-port pick |
| `crates/fleet-core/src/service/device_mirror/tools.rs` (create) | finding `scrcpy` / `adb` on this machine |
| `crates/fleet-core/src/service/device_mirror/mod.rs` (create) | validation, route, scrcpy argv, `Mirrors` registry, lifecycle |
| `crates/fleet-core/src/service/device_mirror/tests.rs` (create) | lifecycle tests with a stub scrcpy and a fake adb server |
| `crates/fleet-core/src/service/mod.rs` (modify) | `pub mod device_mirror;` |
| `crates/fleet-core/src/service/debug_devices/scripts.rs` (modify) | `start_server_script()` |
| `crates/fleet-core/src/service/debug_devices/mod.rs` (modify) | `start_adb_server()` |
| `crates/fleet-core/src/pages/resources.rs` (modify) | two `ActionSpec`s and the commands list |
| `src-tauri/src/commands/device_mirror.rs` (create) | three Tauri commands, `DesktopLease` |
| `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs` (modify) | register commands, manage `Mirrors`, kill on exit |
| `src-tauri/src/backend/verdicts.rs` (modify) | three `SameInBoth` rows |
| generated: `src/lib/hub_verdicts.generated.json`, `docs/hub.md` table, `docs/control-api-reference.md`, `src/lib/pages/registry.generated.json` + page docs | regenerated, never hand-edited |
| `docs/debug-devices.md`, `docs/status.md`, `docs/architecture.md` (modify) | docs |

Each file has one writer: tasks are ordered so no two tasks edit the same file.

---

### Task 1: Error codes + relay core (`relay/mod.rs`)

**Files:**
- Modify: `crates/fleet-core/src/ipc_error.rs` (inside `pub mod codes`, after `E_LOCAL_ONLY`)
- Modify: `crates/fleet-core/src/lib.rs` (add `pub mod relay;` in alphabetical order, after `pub mod projects;`)
- Create: `crates/fleet-core/src/relay/mod.rs`
- Create: `crates/fleet-core/src/relay/tests.rs`

**Interfaces:**
- Produces: `codes::{E_TOOL_MISSING, E_ADB_VERSION_MISMATCH, E_NO_ROUTE, E_MIRROR_FAILED}`; `relay::{ADB_SERVER_PORT, SCRCPY_PORTS, allowed_port, check_port, Stream, BoxedStream, Dialer, RelayListener}`.

- [ ] **Step 1: Add the codes**

In `ipc_error.rs`, inside `pub mod codes`, after `E_LOCAL_ONLY`:

```rust
    /// Device mirror: `scrcpy` or `adb` is not installed on this machine.
    /// The message names the tool and how to install it.
    pub const E_TOOL_MISSING: &str = "E_TOOL_MISSING";
    /// Device mirror: this machine's adb speaks another protocol version
    /// than the device host's adb server. Refused before any adb client
    /// runs, because a mismatched client `host:kill`s the server.
    pub const E_ADB_VERSION_MISMATCH: &str = "E_ADB_VERSION_MISMATCH";
    /// Device mirror: no way from this machine to the device's host.
    pub const E_NO_ROUTE: &str = "E_NO_ROUTE";
    /// Device mirror: `scrcpy` exited with an error right after starting;
    /// the message carries the tail of its output.
    pub const E_MIRROR_FAILED: &str = "E_MIRROR_FAILED";
```

- [ ] **Step 2: Write the failing tests** (`relay/tests.rs`)

```rust
use super::*;
use crate::ipc_error::codes;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// An echo server on loopback; returns its address.
pub(crate) async fn echo_server() -> SocketAddr {
    let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { break };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// Dials one fixed address whatever host/port it is asked for, and records
/// what it was asked.
pub(crate) struct FixedDialer {
    pub addr: SocketAddr,
    pub asked: std::sync::Mutex<Vec<(String, u16)>>,
}

#[async_trait::async_trait]
impl Dialer for FixedDialer {
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError> {
        self.asked.lock().unwrap().push((host.to_string(), port));
        Ok(Box::new(TcpStream::connect(self.addr).await?))
    }
}

struct FailDialer;

#[async_trait::async_trait]
impl Dialer for FailDialer {
    async fn dial(&self, _: &str, _: u16) -> Result<BoxedStream, IpcError> {
        Err(IpcError::new(codes::E_SSH, "no route"))
    }
}

#[test]
fn only_the_adb_server_and_scrcpy_ports_are_allowed() {
    for p in [5037, 27183, 27190, 27199] {
        assert!(allowed_port(p), "{p}");
    }
    for p in [0, 22, 5036, 5038, 27182, 27200, 65535] {
        assert!(!allowed_port(p), "{p}");
    }
    assert_eq!(check_port(22).unwrap_err().code, codes::E_INVALID);
}

#[tokio::test]
async fn a_connection_to_the_listener_is_spliced_to_the_dialed_stream() {
    let dialer = Arc::new(FixedDialer { addr: echo_server().await, asked: Default::default() });
    let relay = RelayListener::bind(dialer.clone(), "mefistos", 5037).await.unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", relay.local_port())).await.unwrap();
    c.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    c.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    assert_eq!(*dialer.asked.lock().unwrap(), vec![("mefistos".to_string(), 5037)]);
}

#[tokio::test]
async fn a_port_outside_the_allowlist_is_refused_at_bind() {
    let dialer = Arc::new(FixedDialer { addr: echo_server().await, asked: Default::default() });
    let err = RelayListener::bind(dialer, "h", 22).await.err().unwrap();
    assert_eq!(err.code, codes::E_INVALID);
}

#[tokio::test]
async fn a_failed_dial_closes_the_local_connection() {
    let relay = RelayListener::bind(Arc::new(FailDialer), "h", 5037).await.unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", relay.local_port())).await.unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0);
}

#[tokio::test]
async fn dropping_the_listener_ends_its_connections() {
    let dialer = Arc::new(FixedDialer { addr: echo_server().await, asked: Default::default() });
    let relay = RelayListener::bind(dialer, "h", 5037).await.unwrap();
    let port = relay.local_port();
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    c.write_all(b"x").await.unwrap();
    let mut b = [0u8; 1];
    c.read_exact(&mut b).await.unwrap();
    drop(relay);
    let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut b)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0, "the spliced connection must close");
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err(), "the listener must be gone");
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo fleet-test -- relay::tests`
Expected: compile error, `relay` module / `RelayListener` not found.

- [ ] **Step 4: Implement `relay/mod.rs`**

```rust
//! A loopback TCP listener whose connections are spliced to a [`Dialer`]:
//! how the device mirror reaches a device host's adb server and scrcpy
//! socket (`docs/debug-devices.md`, *Mirror*; spec
//! `docs/superpowers/specs/2026-10-11-scrcpy-device-mirror-design.md`).
//!
//! The target is always the host's own `127.0.0.1`, and only on
//! [`allowed_port`]s; nothing here dials a caller-supplied address.

pub mod ssh;

#[cfg(test)]
pub(crate) mod tests;

use crate::ipc_error::{codes, IpcError};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// The adb server's port on a device host.
pub const ADB_SERVER_PORT: u16 = 5037;
/// scrcpy's forward ports (`--port`'s default range).
pub const SCRCPY_PORTS: RangeInclusive<u16> = 27183..=27199;

/// May the relay reach `port` on a host's loopback?
pub fn allowed_port(port: u16) -> bool {
    port == ADB_SERVER_PORT || SCRCPY_PORTS.contains(&port)
}

/// [`allowed_port`] as a refusal.
pub fn check_port(port: u16) -> Result<(), IpcError> {
    if allowed_port(port) {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_INVALID,
        format!(
            "port {port} is not relayed; only the adb server ({ADB_SERVER_PORT}) and scrcpy's {}-{} are",
            SCRCPY_PORTS.start(),
            SCRCPY_PORTS.end()
        ),
    ))
}

/// A bidirectional byte stream.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
pub type BoxedStream = Box<dyn Stream>;

/// Opens one stream to `127.0.0.1:<port>` on `host`.
#[async_trait::async_trait]
pub trait Dialer: Send + Sync {
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError>;
}

/// A `127.0.0.1:<ephemeral>` listener: every connection to it is spliced to
/// `dialer.dial(host, port)`. Dropping it closes the listener and every
/// connection it carries.
pub struct RelayListener {
    local_port: u16,
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl RelayListener {
    pub async fn bind(dialer: Arc<dyn Dialer>, host: &str, port: u16) -> Result<Self, IpcError> {
        check_port(port)?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_port = listener.local_addr()?.port();
        let tasks: Arc<Mutex<Vec<AbortHandle>>> = Arc::default();
        let conns = Arc::clone(&tasks);
        let host = host.to_string();
        let accept = tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let dialer = Arc::clone(&dialer);
                let host = host.clone();
                let conn = tokio::spawn(async move {
                    match dialer.dial(&host, port).await {
                        Ok(mut remote) => {
                            let _ = tokio::io::copy_bidirectional(&mut sock, &mut remote).await;
                        }
                        Err(e) => tracing::warn!(host = %host, port, error = %e, "relay: dial failed"),
                    }
                });
                let mut g = conns.lock().unwrap_or_else(|p| p.into_inner());
                g.retain(|h| !h.is_finished());
                g.push(conn.abort_handle());
            }
        });
        tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(accept.abort_handle());
        Ok(Self { local_port, tasks })
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for RelayListener {
    fn drop(&mut self) {
        for h in self.tasks.lock().unwrap_or_else(|p| p.into_inner()).drain(..) {
            h.abort();
        }
    }
}
```

Create `relay/ssh.rs` with just `//! See Task 2.` for now, so the module compiles.

- [ ] **Step 5: Run the tests**

Run: `cargo fleet-test -- relay::tests`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/fleet-core/src/ipc_error.rs crates/fleet-core/src/lib.rs crates/fleet-core/src/relay/
/usr/bin/git commit -m "feat(relay): loopback relay listener and the mirror's error codes"
```

---

### Task 2: `SshDialer` (`relay/ssh.rs`)

**Files:**
- Modify: `crates/fleet-core/src/relay/ssh.rs` (replace the stub)
- Modify: `crates/fleet-core/src/relay/tests.rs` (append)

**Interfaces:**
- Consumes: `relay::{Dialer, BoxedStream, check_port}` (Task 1); `SshClient::{ssh_binary, mux_opts}`; `validate::host_alias`; `proc::command`.
- Produces: `relay::ssh::SshDialer::new(client: SshClient) -> SshDialer`, `SshDialer::argv(&self, host: &str, port: u16) -> Vec<String>`, `impl Dialer for SshDialer`; `pub const CONNECT_TIMEOUT: Duration = 5s`.

- [ ] **Step 1: Write the failing tests** (append to `relay/tests.rs`)

```rust
#[cfg(unix)]
mod ssh_dialer {
    use super::super::ssh::SshDialer;
    use super::*;
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};

    /// A fake `ssh` that records its argv and echoes stdin to stdout.
    fn fake_ssh(dir: &std::path::Path) -> std::path::PathBuf {
        let log = dir.join("argv");
        write_exec(
            dir,
            "ssh",
            &format!(
                "#!/bin/sh\n{PROBE_GUARD}printf '%s\\n' \"$@\" > {}\nexec cat\n",
                crate::shell::quote(&log.display().to_string())
            ),
        )
    }

    #[tokio::test]
    async fn it_runs_ssh_dash_w_to_the_hosts_loopback_and_splices_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let d = SshDialer::with_binary(fake_ssh(dir.path()));
        let mut s = d.dial("mefistos", 5037).await.unwrap();
        s.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        assert_eq!(argv[argv.len() - 4..], ["-W", "127.0.0.1:5037", "--", "mefistos"]);
    }

    #[tokio::test]
    async fn it_refuses_a_port_or_an_alias_that_could_escape() {
        let dir = tempfile::tempdir().unwrap();
        let d = SshDialer::with_binary(fake_ssh(dir.path()));
        assert_eq!(d.dial("mefistos", 22).await.err().unwrap().code, codes::E_INVALID);
        assert_eq!(
            d.dial("-oProxyCommand=touch /tmp/x", 5037).await.err().unwrap().code,
            codes::E_INVALID
        );
        assert!(!dir.path().join("argv").exists(), "ssh must not have run");
    }

    #[tokio::test]
    async fn the_production_argv_carries_batch_mode_and_a_connect_timeout() {
        let d = SshDialer::new(crate::ssh::SshClient::new());
        let argv = d.argv("mefistos", 27183);
        assert!(argv.iter().any(|a| a == "BatchMode=yes"), "{argv:?}");
        assert!(argv.iter().any(|a| a.starts_with("ConnectTimeout=")), "{argv:?}");
        assert_eq!(argv[argv.len() - 4..], ["-W", "127.0.0.1:27183", "--", "mefistos"]);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo fleet-test -- relay::tests::ssh_dialer`
Expected: compile error, `SshDialer` not found.

- [ ] **Step 3: Implement `relay/ssh.rs`**

```rust
//! [`Dialer`] over `ssh -W 127.0.0.1:<port> <alias>`: one ssh per stream,
//! multiplexed on the host's ControlMaster (`SshClient::mux_opts`), so a
//! mirror's many short adb connections share one handshake.

use super::{check_port, BoxedStream, Dialer};
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, Join, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// How long ssh may take to connect a stream.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Opts = Box<dyn Fn(&str) -> Vec<String> + Send + Sync>;

pub struct SshDialer {
    binary: PathBuf,
    opts: Opts,
}

impl SshDialer {
    pub fn new(client: SshClient) -> Self {
        let binary = client.ssh_binary().to_path_buf();
        Self {
            binary,
            opts: Box::new(move |host| client.mux_opts(host, CONNECT_TIMEOUT)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_binary(binary: PathBuf) -> Self {
        Self { binary, opts: Box::new(|_| Vec::new()) }
    }

    pub fn argv(&self, host: &str, port: u16) -> Vec<String> {
        let mut a = (self.opts)(host);
        a.extend(["-W".into(), format!("127.0.0.1:{port}"), "--".into(), host.to_string()]);
        a
    }
}

#[async_trait::async_trait]
impl Dialer for SshDialer {
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError> {
        check_port(port)?;
        crate::validate::host_alias(host)?;
        let mut child = crate::proc::command(&self.binary)
            .args(self.argv(host, port))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| IpcError::new(codes::E_SSH, format!("could not start ssh: {e}")))?;
        let (Some(out), Some(inp)) = (child.stdout.take(), child.stdin.take()) else {
            return Err(IpcError::new(codes::E_INTERNAL, "ssh started without pipes"));
        };
        Ok(Box::new(ChildStream { io: tokio::io::join(out, inp), _child: child }))
    }
}

/// The ssh child's stdio as one stream; dropping it kills the child.
struct ChildStream {
    io: Join<ChildStdout, ChildStdin>,
    _child: Child,
}

impl AsyncRead for ChildStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChildStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
```

If `validate::host_alias` accepts a leading `-`, the test catches it. Then also refuse `host.starts_with('-')` with `E_INVALID` in `dial`, before spawning.

- [ ] **Step 4: Run the tests**

Run: `cargo fleet-test -- relay::tests`
Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add crates/fleet-core/src/relay/
/usr/bin/git commit -m "feat(relay): SSH dialer over ssh -W"
```

---

### Task 3: adb wire client + local tool lookup (`device_mirror/adb.rs`, `tools.rs`)

**Files:**
- Create: `crates/fleet-core/src/service/device_mirror/adb.rs`
- Create: `crates/fleet-core/src/service/device_mirror/tools.rs`
- Create: `crates/fleet-core/src/service/device_mirror/mod.rs` (just `pub mod adb; pub mod tools;` for now)
- Modify: `crates/fleet-core/src/service/mod.rs` (`pub mod device_mirror;`, alphabetical, after `debug_devices`)

**Interfaces:**
- Consumes: `relay::{Stream, SCRCPY_PORTS}`; `codes::{E_TOOL_MISSING, E_ADB_VERSION_MISMATCH}`.
- Produces:
  - `adb::request(stream: &mut (impl Stream + ?Sized), service: &str) -> Result<Vec<u8>, IpcError>` — sends one host service and returns its length-prefixed payload.
  - `adb::server_version(stream) -> Result<u32, IpcError>`
  - `adb::forwarded_ports(stream) -> Result<Vec<u16>, IpcError>`
  - `adb::kill_forward(stream, serial: &str, port: u16) -> Result<(), IpcError>`
  - `adb::parse_local_version(text: &str) -> Option<u32>`
  - `adb::local_version(adb: &Path) -> Result<u32, IpcError>`
  - `adb::check_versions(local: u32, remote: u32) -> Result<(), IpcError>`
  - `adb::free_forward_port(used: &[u16]) -> Option<u16>`
  - `tools::Tools { scrcpy: PathBuf, adb: PathBuf }`, `tools::find(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Result<Tools, IpcError>`, `tools::candidates(name, env, home) -> Vec<PathBuf>`

**adb wire protocol** (host services, as the adb client speaks it):
- Request: 4 lowercase hex digits = byte length of the service, then the service (`000chost:version`).
- Reply: `OKAY` or `FAIL`. After `FAIL`: 4 hex length + message.
- `host:version` → `OKAY` + 4 hex length + payload; the payload is 4 hex digits of the version (`0029` = 41).
- `host:list-forward` → `OKAY` + 4 hex length + lines `<serial> tcp:<port> <remote>\n`.
- `host-serial:<serial>:killforward:tcp:<port>` → `OKAY` (sometimes followed by a second `OKAY`; ignore the rest).

- [ ] **Step 1: Write the failing tests** (a `#[cfg(test)] mod tests` at the bottom of `adb.rs` and of `tools.rs`)

`adb.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One end of a duplex pipe, after a fake server answered `reply` to
    /// whatever request arrives; returns (client end, the request the server read).
    async fn served(reply: impl Into<Vec<u8>>) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<String>) {
        let reply = reply.into();
        let (client, mut server) = tokio::io::duplex(4096);
        let req = tokio::spawn(async move {
            let mut len = [0u8; 4];
            server.read_exact(&mut len).await.unwrap();
            let n = usize::from_str_radix(std::str::from_utf8(&len).unwrap(), 16).unwrap();
            let mut body = vec![0u8; n];
            server.read_exact(&mut body).await.unwrap();
            server.write_all(&reply).await.unwrap();
            String::from_utf8(body).unwrap()
        });
        (client, req)
    }

    #[tokio::test]
    async fn server_version_reads_the_hex_payload() {
        let (mut c, req) = served(*b"OKAY00040029").await;
        assert_eq!(server_version(&mut c).await.unwrap(), 41);
        assert_eq!(req.await.unwrap(), "host:version");
    }

    #[tokio::test]
    async fn a_fail_reply_is_an_error_with_the_servers_message() {
        let (mut c, _req) = served(*b"FAIL0005nope!").await;
        let e = server_version(&mut c).await.unwrap_err();
        assert!(e.message.contains("nope!"), "{}", e.message);
    }

    #[tokio::test]
    async fn a_closed_stream_is_an_error_not_a_hang() {
        let (mut c, server) = tokio::io::duplex(64);
        drop(server);
        assert!(server_version(&mut c).await.is_err());
    }

    #[tokio::test]
    async fn forwarded_ports_lists_the_tcp_local_ports() {
        let list = "R5CT1 tcp:27183 localabstract:scrcpy_1\nemulator-5554 tcp:27185 tcp:8080\n";
        let (mut c, req) = served(format!("OKAY{:04x}{list}", list.len())).await;
        assert_eq!(forwarded_ports(&mut c).await.unwrap(), vec![27183, 27185]);
        assert_eq!(req.await.unwrap(), "host:list-forward");
    }

    #[tokio::test]
    async fn kill_forward_names_the_serial_and_port() {
        let (mut c, req) = served(*b"OKAYOKAY").await;
        kill_forward(&mut c, "R5CT1", 27183).await.unwrap();
        assert_eq!(req.await.unwrap(), "host-serial:R5CT1:killforward:tcp:27183");
    }

    #[test]
    fn the_local_version_is_the_last_component_of_adbs_banner() {
        let text = "Android Debug Bridge version 1.0.41\nVersion 35.0.2-12147458\nInstalled as /x/adb\n";
        assert_eq!(parse_local_version(text), Some(41));
        assert_eq!(parse_local_version("garbage"), None);
    }

    #[test]
    fn a_version_mismatch_is_refused_naming_both() {
        assert!(check_versions(41, 41).is_ok());
        let e = check_versions(41, 40).unwrap_err();
        assert_eq!(e.code, codes::E_ADB_VERSION_MISMATCH);
        assert!(e.message.contains("41") && e.message.contains("40"), "{}", e.message);
    }

    #[test]
    fn the_first_unused_scrcpy_port_is_picked() {
        assert_eq!(free_forward_port(&[]), Some(27183));
        assert_eq!(free_forward_port(&[27183, 27184, 8080]), Some(27185));
        let all: Vec<u16> = crate::relay::SCRCPY_PORTS.collect();
        assert_eq!(free_forward_port(&all), None);
    }
}
```

`tools.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn no_env(_: &str) -> Option<String> { None }

    #[test]
    fn candidates_cover_path_homebrew_and_the_sdk() {
        let home = Path::new("/home/u");
        let env = |k: &str| match k {
            "PATH" => Some("/a:/b".to_string()),
            "ANDROID_HOME" => Some("/sdk".to_string()),
            _ => None,
        };
        let adb = candidates("adb", &env, home);
        for want in ["/a/adb", "/b/adb", "/opt/homebrew/bin/adb", "/usr/local/bin/adb",
                     "/sdk/platform-tools/adb", "/home/u/Library/Android/sdk/platform-tools/adb",
                     "/home/u/Android/Sdk/platform-tools/adb"] {
            assert!(adb.iter().any(|p| p == Path::new(want)), "missing {want}: {adb:?}");
        }
        let scrcpy = candidates("scrcpy", &env, home);
        assert!(scrcpy.iter().all(|p| !p.to_string_lossy().contains("platform-tools")));
    }

    #[test]
    fn an_env_override_comes_first() {
        let env = |k: &str| (k == "FLEET_SCRCPY").then(|| "/opt/s/scrcpy".to_string());
        assert_eq!(candidates("scrcpy", &env, Path::new("/h"))[0], Path::new("/opt/s/scrcpy"));
    }

    #[test]
    fn a_missing_scrcpy_says_how_to_install_it() {
        let dir = tempfile::tempdir().unwrap();
        let e = find(&no_env, dir.path()).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_TOOL_MISSING);
        assert!(e.message.contains("scrcpy") && e.message.contains("brew install scrcpy"), "{}", e.message);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo fleet-test -- device_mirror::`
Expected: compile errors (functions missing).

- [ ] **Step 3: Implement `adb.rs`**

```rust
//! Just enough of the adb wire protocol to ask a device host's adb server
//! for its version and its forwards without running an adb client, so a
//! mismatched local client is never pointed at it (it would `host:kill`
//! the server).

use crate::ipc_error::{codes, IpcError};
use crate::relay::{Stream, SCRCPY_PORTS};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// The longest one host service may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn proto_err(what: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_PARSE, format!("adb server: {}", what.into()))
}

async fn read_hex4(s: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<usize, IpcError> {
    let mut b = [0u8; 4];
    s.read_exact(&mut b).await?;
    let t = std::str::from_utf8(&b).map_err(|_| proto_err("bad length"))?;
    usize::from_str_radix(t, 16).map_err(|_| proto_err(format!("bad length {t:?}")))
}

async fn status(s: &mut (impl Stream + ?Sized)) -> Result<(), IpcError> {
    let mut st = [0u8; 4];
    s.read_exact(&mut st).await?;
    match &st {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let n = read_hex4(s).await?;
            let mut msg = vec![0u8; n.min(4096)];
            s.read_exact(&mut msg).await?;
            Err(IpcError::new(codes::E_INVALID_STATE, format!("adb server refused: {}", String::from_utf8_lossy(&msg))))
        }
        other => Err(proto_err(format!("unexpected status {:?}", String::from_utf8_lossy(other)))),
    }
}

async fn send(s: &mut (impl Stream + ?Sized), service: &str) -> Result<(), IpcError> {
    s.write_all(format!("{:04x}{service}", service.len()).as_bytes()).await?;
    s.flush().await?;
    status(s).await
}

/// Send one host service and return its length-prefixed payload.
pub async fn request(s: &mut (impl Stream + ?Sized), service: &str) -> Result<Vec<u8>, IpcError> {
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        send(s, service).await?;
        let n = read_hex4(s).await?;
        let mut body = vec![0u8; n];
        s.read_exact(&mut body).await?;
        Ok(body)
    })
    .await
    .map_err(|_| IpcError::new(codes::E_TIMEOUT, format!("adb server did not answer {service}")))?
}

pub async fn server_version(s: &mut (impl Stream + ?Sized)) -> Result<u32, IpcError> {
    let body = request(s, "host:version").await?;
    let t = std::str::from_utf8(&body).map_err(|_| proto_err("bad version"))?;
    u32::from_str_radix(t.trim(), 16).map_err(|_| proto_err(format!("bad version {t:?}")))
}

pub async fn forwarded_ports(s: &mut (impl Stream + ?Sized)) -> Result<Vec<u16>, IpcError> {
    let body = request(s, "host:list-forward").await?;
    Ok(String::from_utf8_lossy(&body)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("tcp:")?.parse().ok())
        .collect())
}

pub async fn kill_forward(s: &mut (impl Stream + ?Sized), serial: &str, port: u16) -> Result<(), IpcError> {
    tokio::time::timeout(REQUEST_TIMEOUT, send(s, &format!("host-serial:{serial}:killforward:tcp:{port}")))
        .await
        .map_err(|_| IpcError::new(codes::E_TIMEOUT, "adb server did not answer killforward"))?
}

/// `Android Debug Bridge version 1.0.41` → 41.
pub fn parse_local_version(text: &str) -> Option<u32> {
    let line = text.lines().find(|l| l.starts_with("Android Debug Bridge version "))?;
    line.rsplit('.').next()?.trim().parse().ok()
}

pub async fn local_version(adb: &Path) -> Result<u32, IpcError> {
    let out = crate::proc::command(adb)
        .arg("version")
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TOOL_MISSING, format!("could not run {}: {e}", adb.display())))?;
    parse_local_version(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| proto_err(format!("{} version printed no version", adb.display())))
}

pub fn check_versions(local: u32, remote: u32) -> Result<(), IpcError> {
    if local == remote {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_ADB_VERSION_MISMATCH,
        format!(
            "this machine's adb speaks protocol {local}, the device host's adb server {remote}; \
             update platform-tools on the older side (a mismatched adb would restart the host's server)"
        ),
    ))
}

pub fn free_forward_port(used: &[u16]) -> Option<u16> {
    SCRCPY_PORTS.clone().find(|p| !used.contains(p))
}
```

In the tests, `served` hands a `DuplexStream`, which satisfies `Stream`. Read the payload length with `read_hex4` (4 hex digits, any case).

- [ ] **Step 4: Implement `tools.rs`**

```rust
//! Finding `scrcpy` and `adb` on THIS machine. A GUI app on macOS does not
//! inherit the login shell's PATH, so the usual Homebrew and Android SDK
//! folders are searched too; `FLEET_SCRCPY` / `FLEET_ADB` override.

use crate::ipc_error::{codes, IpcError};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Tools {
    pub scrcpy: PathBuf,
    pub adb: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Where to look for `name`, in order.
pub fn candidates(name: &str, env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Vec<PathBuf> {
    let file = exe(name);
    let mut out = Vec::new();
    if let Some(p) = env(&format!("FLEET_{}", name.to_uppercase())).filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if let Some(path) = env("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join(&file)));
    }
    out.push(Path::new("/opt/homebrew/bin").join(&file));
    out.push(Path::new("/usr/local/bin").join(&file));
    if name == "adb" {
        for k in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Some(d) = env(k).filter(|d| !d.is_empty()) {
                out.push(Path::new(&d).join("platform-tools").join(&file));
            }
        }
        for d in ["Library/Android/sdk", "Android/Sdk", "Android/sdk"] {
            out.push(home.join(d).join("platform-tools").join(&file));
        }
    }
    out
}

fn first_file(c: Vec<PathBuf>) -> Option<PathBuf> {
    c.into_iter().find(|p| p.is_file())
}

pub fn find(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Result<Tools, IpcError> {
    let scrcpy = first_file(candidates("scrcpy", env, home)).ok_or_else(|| {
        IpcError::new(
            codes::E_TOOL_MISSING,
            "scrcpy is not installed on this machine: `brew install scrcpy` (macOS) or your package manager; \
             or set FLEET_SCRCPY to its path",
        )
    })?;
    let adb = first_file(candidates("adb", env, home)).ok_or_else(|| {
        IpcError::new(
            codes::E_TOOL_MISSING,
            "adb is not installed on this machine: install Android platform-tools \
             (`brew install android-platform-tools`), or set FLEET_ADB to its path",
        )
    })?;
    Ok(Tools { scrcpy, adb })
}
```

`std::env::split_paths` on Unix splits on `:`, which matches the test's `/a:/b`.

- [ ] **Step 5: Run the tests**

Run: `cargo fleet-test -- device_mirror::`
Expected: all adb + tools tests pass (11).

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/fleet-core/src/service/device_mirror/ crates/fleet-core/src/service/mod.rs
/usr/bin/git commit -m "feat(device-mirror): adb wire preflight and local tool lookup"
```

---

### Task 4: Start a host's adb server (`debug_devices`)

**Files:**
- Modify: `crates/fleet-core/src/service/debug_devices/scripts.rs` (new fn next to `screenshot_script`, plus a test in its existing `mod tests`)
- Modify: `crates/fleet-core/src/service/debug_devices/mod.rs` (new pub fn next to `screenshot`)

**Interfaces:**
- Produces: `scripts::start_server_script() -> String`; `debug_devices::start_adb_server(ssh: &dyn SshExec, host: &str) -> Result<(), IpcError>`.

- [ ] **Step 1: Write the failing test** (in `scripts.rs`'s `mod tests`, modelled on `the_scan_and_run_scripts_work_against_a_stub_adb`)

```rust
    #[cfg(unix)]
    #[test]
    fn the_start_server_script_starts_adb_or_says_it_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = dir.path().join("log");
        crate::tmux::fake_exec::write_exec(
            &bin,
            "adb",
            &format!(
                "#!/bin/bash\n{}echo \"$@\" >> {}\nexit 0\n",
                crate::tmux::fake_exec::PROBE_GUARD,
                quote(&log.display().to_string())
            ),
        );
        let run = |path: String| {
            crate::proc::std_command("bash")
                .arg("-c")
                .arg(start_server_script())
                .env("PATH", path)
                .env("HOME", dir.path())
                .env_remove("ANDROID_HOME")
                .env_remove("ANDROID_SDK_ROOT")
                .output()
                .unwrap()
        };
        let out = run(format!("{}:/usr/bin:/bin", bin.display()));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "start-server");

        let out = run("/usr/bin:/bin".into());
        assert_eq!(out.status.code(), Some(5));
        assert!(failure(&out.stderr).unwrap().contains("adb"));
    }
```

(`failure` is the existing test helper that extracts the `__CF_DEV_FAILED__` reason; if its name differs, use the one `the_scan_and_run_scripts_work_against_a_stub_adb` uses.)

- [ ] **Step 2: Run it to see it fail**

Run: `cargo fleet-test -- debug_devices::scripts::tests::the_start_server_script`
Expected: compile error, `start_server_script` not found.

- [ ] **Step 3: Implement**

In `scripts.rs`, after `screenshot_script`:

```rust
/// Start the host's adb server (a no-op when it runs), for the device
/// mirror, which talks to the server over a relay and cannot start it.
pub fn start_server_script() -> String {
    format!(
        "# cf-devices:start-server\n{PREAMBLE}\
         [ -n \"$ADB\" ] || cf_fail 'adb is not installed on this host'\n\
         \"$ADB\" start-server >/dev/null 2>&1 || cf_fail 'adb start-server failed'\n"
    )
}
```

In `mod.rs`, after `screenshot`:

```rust
/// Start `host`'s adb server; the device mirror's preflight calls this when
/// the server does not answer.
pub async fn start_adb_server(ssh: &dyn SshExec, host: &str) -> Result<(), IpcError> {
    run_on(ssh, host, "adb start-server", &scripts::start_server_script(), Duration::from_secs(30))
        .await
        .map(|_| ())
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo fleet-test -- debug_devices::`
Expected: all pass, including the new one.

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add crates/fleet-core/src/service/debug_devices/
/usr/bin/git commit -m "feat(debug-devices): start a host's adb server on demand"
```

---

### Task 5: The mirror service (`device_mirror/mod.rs`)

**Files:**
- Modify: `crates/fleet-core/src/service/device_mirror/mod.rs` (replace the two-line module from Task 3; keep `pub mod adb; pub mod tools;`)
- Create: `crates/fleet-core/src/service/device_mirror/tests.rs`

**Interfaces:**
- Consumes: `relay::{Dialer, RelayListener, ADB_SERVER_PORT}`, `relay::ssh::SshDialer` (Task 2 — used by the caller, not here); `adb::*`, `tools::Tools` (Task 3); `debug_devices::{DebugDevice, start_adb_server}` (Task 4); `ssh::SshExec`; `projects::LOCAL_HOST`.
- Produces (used by Task 6):

```rust
pub enum Route { Local, Ssh }                       // serde: "local" | "ssh"
pub struct MirrorInfo { pub device_id: i64, pub host: String, pub serial: String,
                        pub title: String, pub route: Route, pub started_at: i64 }
#[async_trait] pub trait Lease: Send + Sync {
    async fn renew(&self) -> Result<(), IpcError>;
    async fn release(&self);
}
pub struct Remote { pub dialer: Arc<dyn Dialer>, pub ssh: Arc<dyn SshExec> }
pub fn validate(d: &DebugDevice) -> Result<String /* serial */, IpcError>
pub fn choose_route(host: &str, standalone: bool) -> Result<Route, IpcError>
pub fn scrcpy_command(tools: &Tools, serial: &str, title: &str, ports: Option<SpawnPorts>) -> tokio::process::Command
pub struct SpawnPorts { pub adb: u16, pub forward: u16, pub tunnel: u16 }
pub struct Mirrors { .. }   // Default + new()
impl Mirrors {
    pub async fn start(&self, d: &DebugDevice, standalone: bool, tools: Tools,
                       remote: Remote, lease: Arc<dyn Lease>) -> Result<MirrorInfo, IpcError>;
    pub async fn stop(&self, device_id: i64) -> Result<(), IpcError>;
    pub fn list(&self) -> Vec<MirrorInfo>;
    pub fn kill_all(&self);
}
pub const STARTUP_GRACE: Duration = 5s; pub const RENEW_EVERY: Duration = 600s;
pub const CLAIM_SECS: i64 = 1800;
```

**Lifecycle of `start`** (the order matters; each step's failure releases what earlier steps took):

1. `validate(d)` → serial; `choose_route(&d.host, standalone)`.
2. Refuse `E_EXISTS` when `device_id` is already mirrored.
3. Ssh route only:
   - Bind `RelayListener` A on `ADB_SERVER_PORT`.
   - Dial once through `remote.dialer` and run `adb::server_version`. If that fails, call `debug_devices::start_adb_server(remote.ssh, host)` (map its error to `E_NO_ROUTE`: "cannot reach <host> over ssh from this machine: <msg>") and retry once.
   - `adb::check_versions(local_version(tools.adb), remote)`.
   - On a fresh dial, `forwarded_ports`, then `free_forward_port` (or `E_LIMIT`).
   - Bind `RelayListener` B on that forward port.
4. Spawn `scrcpy_command(...)` with `stdout`/`stderr` piped and `kill_on_drop(true)`. Collect both outputs into a shared tail buffer (last 2 KiB).
5. Wait up to `STARTUP_GRACE` for an exit. A non-zero exit → `E_MIRROR_FAILED` with the tail, plus teardown (drop the listeners, release the lease). An exit with status 0 → `Ok`; it was closed already, so tear down and return the info anyway.
6. Register, and spawn the watcher task:
   - `select!` on `child.wait()`, a stop `oneshot`, and a `RENEW_EVERY` interval (`lease.renew()`; log its errors).
   - On stop: `start_kill()`, then `wait()`.
   - Then: drop the listeners. On the ssh route, best-effort `adb::kill_forward(serial, forward)` over a fresh dial. Call `lease.release()` and remove the entry.

The lease is taken by the CALLER (Task 6) before `start`. `start` releases it on any error it returns.

- [ ] **Step 1: Write the failing tests** (`device_mirror/tests.rs`; add `#[cfg(test)] mod tests;` in `mod.rs`)

```rust
use super::*;
use crate::ipc_error::codes;
use crate::service::debug_devices::DebugDevice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn device(platform: &str, kind: &str, ready: bool, host: &str) -> DebugDevice {
    serde_json::from_value(serde_json::json!({
        "id": 7, "title": "bench pixel", "host": host, "platform": platform, "kind": kind,
        "key": "R5CT1", "serial": "R5CT1", "name": "Pixel 7",
        "state": if ready { "online" } else { "offline" }, "ready": ready, "shared": false
    }))
    .expect("DebugDevice fields changed: update this fixture")
}

#[derive(Default)]
struct CountingLease { renewed: AtomicUsize, released: AtomicUsize }
#[async_trait::async_trait]
impl Lease for CountingLease {
    async fn renew(&self) -> Result<(), IpcError> { self.renewed.fetch_add(1, Ordering::SeqCst); Ok(()) }
    async fn release(&self) { self.released.fetch_add(1, Ordering::SeqCst); }
}

#[test]
fn only_a_ready_android_device_with_a_serial_can_be_mirrored() {
    assert_eq!(validate(&device("android", "physical", true, "h")).unwrap(), "R5CT1");
    assert_eq!(validate(&device("ios", "physical", true, "h")).unwrap_err().code, codes::E_UNSUPPORTED);
    assert_eq!(validate(&device("android", "emulator", false, "h")).unwrap_err().code, codes::E_INVALID_STATE);
}

#[test]
fn local_is_this_machine_only_when_standalone() {
    assert!(matches!(choose_route("local", true).unwrap(), Route::Local));
    assert!(matches!(choose_route("mefistos", true).unwrap(), Route::Ssh));
    assert!(matches!(choose_route("mefistos", false).unwrap(), Route::Ssh));
    let e = choose_route("local", false).unwrap_err();
    assert_eq!(e.code, codes::E_NO_ROUTE);
    assert!(e.message.contains("hub"), "{}", e.message);
}

#[test]
fn the_remote_scrcpy_command_points_adb_and_the_tunnel_at_the_relays() {
    let tools = tools::Tools { scrcpy: "/bin/scrcpy".into(), adb: "/sdk/adb".into() };
    let cmd = scrcpy_command(&tools, "R5CT1", "bench pixel @ mefistos",
        Some(SpawnPorts { adb: 41000, forward: 27184, tunnel: 41001 }));
    let std = cmd.as_std();
    let args: Vec<String> = std.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, [
        "--serial", "R5CT1", "--window-title", "bench pixel @ mefistos",
        "--force-adb-forward", "--port", "27184", "--tunnel-host", "127.0.0.1", "--tunnel-port", "41001",
    ]);
    let env: Vec<(String, String)> = std.get_envs()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
        .collect();
    assert!(env.contains(&("ADB_SERVER_SOCKET".into(), "tcp:127.0.0.1:41000".into())), "{env:?}");
    assert!(env.contains(&("ADB".into(), "/sdk/adb".into())), "{env:?}");
}

#[test]
fn the_local_scrcpy_command_has_no_tunnel() {
    let tools = tools::Tools { scrcpy: "/bin/scrcpy".into(), adb: "/sdk/adb".into() };
    let cmd = scrcpy_command(&tools, "emulator-5554", "e @ local", None);
    let args: Vec<String> = cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, ["--serial", "emulator-5554", "--window-title", "e @ local"]);
    assert!(cmd.as_std().get_envs().all(|(k, _)| k != "ADB_SERVER_SOCKET"));
}

#[cfg(unix)]
mod lifecycle {
    use super::*;
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};

    /// A stub scrcpy: logs argv and env to `<dir>/scrcpy.log`, then sleeps
    /// (a running mirror) or exits with `code` at once.
    fn stub_scrcpy(dir: &std::path::Path, exit: Option<i32>) -> std::path::PathBuf {
        let log = crate::shell::quote(&dir.join("scrcpy.log").display().to_string());
        let tail = match exit {
            Some(c) => format!("echo 'ERROR: Could not open video stream' >&2\nexit {c}\n"),
            None => "exec sleep 600\n".to_string(),
        };
        write_exec(dir, "scrcpy", &format!(
            "#!/bin/sh\n{PROBE_GUARD}echo \"$@\" > {log}\necho \"ADB_SERVER_SOCKET=$ADB_SERVER_SOCKET\" >> {log}\n{tail}"
        ))
    }

    fn stub_adb(dir: &std::path::Path, version: u32) -> std::path::PathBuf {
        write_exec(dir, "adb", &format!(
            "#!/bin/sh\n{PROBE_GUARD}echo 'Android Debug Bridge version 1.0.{version}'\n"
        ))
    }

    /// A fake adb server: answers host:version with `version`, list-forward
    /// with 27183 taken, killforward with OKAY; counts connections.
    async fn fake_adb_server(version: u32) -> (std::net::SocketAddr, Arc<AtomicUsize>, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = l.local_addr().unwrap();
        let conns = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (c2, s2) = (conns.clone(), seen.clone());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                c2.fetch_add(1, Ordering::SeqCst);
                let seen = s2.clone();
                tokio::spawn(async move {
                    let mut len = [0u8; 4];
                    if s.read_exact(&mut len).await.is_err() { return; }
                    let n = usize::from_str_radix(std::str::from_utf8(&len).unwrap(), 16).unwrap();
                    let mut body = vec![0u8; n];
                    s.read_exact(&mut body).await.unwrap();
                    let req = String::from_utf8(body).unwrap();
                    seen.lock().unwrap().push(req.clone());
                    let reply = match req.as_str() {
                        "host:version" => format!("OKAY0004{version:04x}"),
                        "host:list-forward" => {
                            let p = "R5CT1 tcp:27183 localabstract:x\n";
                            format!("OKAY{:04x}{p}", p.len())
                        }
                        _ => "OKAY".to_string(),
                    };
                    s.write_all(reply.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, conns, seen)
    }

    struct NoSsh;
    #[async_trait::async_trait]
    impl crate::ssh::SshExec for NoSsh { /* implement every required method with
        `unreachable!("the adb server answered; nothing may run over ssh")` — copy the
        method list from `ssh_fake.rs`'s fake, or reuse that fake if it has a constructor
        that records calls. */ }

    fn remote(addr: std::net::SocketAddr) -> Remote {
        Remote {
            dialer: Arc::new(crate::relay::tests::FixedDialer { addr, asked: Default::default() }),
            ssh: Arc::new(NoSsh),
        }
    }

    #[tokio::test]
    async fn a_remote_mirror_runs_scrcpy_on_the_relays_and_cleans_up_on_stop() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools { scrcpy: stub_scrcpy(dir.path(), None), adb: stub_adb(dir.path(), 41) };
        let (addr, _conns, seen) = fake_adb_server(41).await;
        let lease = Arc::new(CountingLease::default());
        let mirrors = Mirrors::new();
        let info = mirrors.start(&device("android", "physical", true, "mefistos"), true, tools,
                                 remote(addr), lease.clone()).await.unwrap();
        assert!(matches!(info.route, Route::Ssh));
        assert_eq!(mirrors.list().len(), 1);
        let log = std::fs::read_to_string(dir.path().join("scrcpy.log")).unwrap();
        assert!(log.contains("--port 27184"), "27183 is taken on the host: {log}");
        assert!(log.contains("ADB_SERVER_SOCKET=tcp:127.0.0.1:"), "{log}");

        mirrors.stop(7).await.unwrap();
        for _ in 0..50 {
            if mirrors.list().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(mirrors.list().is_empty());
        assert_eq!(lease.released.load(Ordering::SeqCst), 1);
        assert!(seen.lock().unwrap().iter().any(|r| r == "host-serial:R5CT1:killforward:tcp:27184"));
    }

    #[tokio::test]
    async fn a_second_mirror_of_the_same_device_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools { scrcpy: stub_scrcpy(dir.path(), None), adb: stub_adb(dir.path(), 41) };
        let (addr, _, _) = fake_adb_server(41).await;
        let mirrors = Mirrors::new();
        let d = device("android", "physical", true, "mefistos");
        mirrors.start(&d, true, tools.clone(), remote(addr), Arc::new(CountingLease::default())).await.unwrap();
        let lease = Arc::new(CountingLease::default());
        let e = mirrors.start(&d, true, tools, remote(addr), lease.clone()).await.unwrap_err();
        assert_eq!(e.code, codes::E_EXISTS);
        assert_eq!(lease.released.load(Ordering::SeqCst), 1, "a refused start releases its lease");
        mirrors.kill_all();
    }

    #[tokio::test]
    async fn a_version_mismatch_never_spawns_scrcpy() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools { scrcpy: stub_scrcpy(dir.path(), None), adb: stub_adb(dir.path(), 41) };
        let (addr, _, _) = fake_adb_server(40).await;
        let lease = Arc::new(CountingLease::default());
        let e = Mirrors::new().start(&device("android", "physical", true, "mefistos"), true, tools,
                                     remote(addr), lease.clone()).await.unwrap_err();
        assert_eq!(e.code, codes::E_ADB_VERSION_MISMATCH);
        assert!(!dir.path().join("scrcpy.log").exists());
        assert_eq!(lease.released.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scrcpy_failing_at_once_is_reported_with_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools { scrcpy: stub_scrcpy(dir.path(), Some(1)), adb: stub_adb(dir.path(), 41) };
        let (addr, _, _) = fake_adb_server(41).await;
        let lease = Arc::new(CountingLease::default());
        let mirrors = Mirrors::new();
        let e = mirrors.start(&device("android", "physical", true, "mefistos"), true, tools,
                              remote(addr), lease.clone()).await.unwrap_err();
        assert_eq!(e.code, codes::E_MIRROR_FAILED);
        assert!(e.message.contains("Could not open video stream"), "{}", e.message);
        assert!(mirrors.list().is_empty());
        assert_eq!(lease.released.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_local_mirror_needs_no_relay_and_no_adb_server() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools { scrcpy: stub_scrcpy(dir.path(), None), adb: stub_adb(dir.path(), 41) };
        // A dead address: any dial would fail the test.
        let dead = remote("127.0.0.1:9".parse().unwrap());
        let mirrors = Mirrors::new();
        let info = mirrors.start(&device("android", "emulator", true, "local"), true, tools, dead,
                                 Arc::new(CountingLease::default())).await.unwrap();
        assert!(matches!(info.route, Route::Local));
        let log = std::fs::read_to_string(dir.path().join("scrcpy.log")).unwrap();
        assert!(!log.contains("--tunnel-port"), "{log}");
        mirrors.kill_all();
    }
}
```

`relay::tests::FixedDialer` is visible here because Task 1 declared `#[cfg(test)] pub(crate) mod tests;` and made `FixedDialer` `pub(crate)`. Run Task 5 after Task 2.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo fleet-test -- device_mirror::tests`
Expected: compile errors (`Mirrors`, `validate`… missing).

- [ ] **Step 3: Implement `device_mirror/mod.rs`**

```rust
//! Device mirror: a native `scrcpy` window on this machine for an Android
//! device on any host it can reach (spec
//! `docs/superpowers/specs/2026-10-11-scrcpy-device-mirror-design.md`;
//! guide `docs/debug-devices.md`, *Mirror*). This PR: the `local` and
//! `ssh` routes; the hub relay comes next.

pub mod adb;
pub mod tools;

#[cfg(test)]
mod tests;

use crate::ipc_error::{codes, IpcError};
use crate::relay::{Dialer, RelayListener, ADB_SERVER_PORT};
use crate::service::debug_devices::{self, DebugDevice};
use crate::service::projects::LOCAL_HOST;
use crate::ssh::SshExec;
use serde::Serialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;
use tools::Tools;

pub const STARTUP_GRACE: Duration = Duration::from_secs(5);
pub const RENEW_EVERY: Duration = Duration::from_secs(600);
/// The claim a mirror takes (and renews every [`RENEW_EVERY`]).
pub const CLAIM_SECS: i64 = 1800;
const TAIL_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route { Local, Ssh }

#[derive(Debug, Clone, Serialize)]
pub struct MirrorInfo {
    pub device_id: i64,
    pub host: String,
    pub serial: String,
    pub title: String,
    pub route: Route,
    pub started_at: i64,
}

/// The device's claim, held while the mirror runs.
#[async_trait::async_trait]
pub trait Lease: Send + Sync {
    async fn renew(&self) -> Result<(), IpcError>;
    async fn release(&self);
}

/// How a remote mirror reaches its host.
pub struct Remote {
    pub dialer: Arc<dyn Dialer>,
    pub ssh: Arc<dyn SshExec>,
}

#[derive(Debug, Clone, Copy)]
pub struct SpawnPorts {
    /// Local relay to the host's adb server.
    pub adb: u16,
    /// scrcpy's forward port on the host.
    pub forward: u16,
    /// Local relay to that forward port.
    pub tunnel: u16,
}

pub fn validate(d: &DebugDevice) -> Result<String, IpcError> {
    if d.platform != "android" {
        return Err(IpcError::new(codes::E_UNSUPPORTED, "scrcpy mirrors Android devices only; use Screenshot for iOS"));
    }
    if !d.ready {
        return Err(IpcError::new(codes::E_INVALID_STATE, format!("{} is {}; start or reconnect it first", d.title, d.state)));
    }
    d.serial.clone().ok_or_else(|| IpcError::new(codes::E_INVALID_STATE, format!("{} has no adb serial yet; rescan its host", d.title)))
}

pub fn choose_route(host: &str, standalone: bool) -> Result<Route, IpcError> {
    match (host == LOCAL_HOST, standalone) {
        (true, true) => Ok(Route::Local),
        (true, false) => Err(IpcError::new(
            codes::E_NO_ROUTE,
            "this device is on the hub's own machine; mirroring it needs the hub relay, which this version does not have yet",
        )),
        (false, _) => Ok(Route::Ssh),
    }
}

pub fn scrcpy_command(tools: &Tools, serial: &str, title: &str, ports: Option<SpawnPorts>) -> tokio::process::Command {
    let mut c = crate::proc::command(&tools.scrcpy);
    c.args(["--serial", serial, "--window-title", title]);
    c.env("ADB", &tools.adb);
    if let Some(p) = ports {
        c.args(["--force-adb-forward", "--port", &p.forward.to_string(),
                "--tunnel-host", "127.0.0.1", "--tunnel-port", &p.tunnel.to_string()]);
        c.env("ADB_SERVER_SOCKET", format!("tcp:127.0.0.1:{}", p.adb));
    }
    c
}

/// What a running mirror holds; dropping the listeners ends its relays.
struct Running {
    info: MirrorInfo,
    stop: Option<oneshot::Sender<()>>,
}

#[derive(Default, Clone)]
pub struct Mirrors {
    inner: Arc<Mutex<HashMap<i64, Running>>>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The remote half of a start: relays bound, versions checked, a free
/// forward port picked.
struct Relays {
    adb: RelayListener,
    tunnel: RelayListener,
    forward: u16,
}

async fn prepare_remote(host: &str, tools: &Tools, remote: &Remote) -> Result<Relays, IpcError> {
    let version = match server_version(remote, host).await {
        Ok(v) => v,
        Err(_) => {
            debug_devices::start_adb_server(remote.ssh.as_ref(), host).await.map_err(|e| {
                IpcError::new(codes::E_NO_ROUTE, format!("cannot reach the adb server on {host} over ssh from this machine: {}", e.message))
            })?;
            server_version(remote, host).await.map_err(|e| {
                IpcError::new(codes::E_NO_ROUTE, format!("{host}'s adb server does not answer after start-server: {}", e.message))
            })?
        }
    };
    adb::check_versions(adb::local_version(&tools.adb).await?, version)?;
    let mut s = remote.dialer.dial(host, ADB_SERVER_PORT).await?;
    let used = adb::forwarded_ports(&mut s).await?;
    let forward = adb::free_forward_port(&used)
        .ok_or_else(|| IpcError::new(codes::E_LIMIT, format!("every scrcpy port on {host} is forwarded already")))?;
    Ok(Relays {
        adb: RelayListener::bind(Arc::clone(&remote.dialer), host, ADB_SERVER_PORT).await?,
        tunnel: RelayListener::bind(Arc::clone(&remote.dialer), host, forward).await?,
        forward,
    })
}

async fn server_version(remote: &Remote, host: &str) -> Result<u32, IpcError> {
    let mut s = remote.dialer.dial(host, ADB_SERVER_PORT).await?;
    adb::server_version(&mut s).await
}

/// Keep the last [`TAIL_BYTES`] of a child's output.
fn collect_tail(mut r: impl tokio::io::AsyncRead + Unpin + Send + 'static, tail: Arc<Mutex<Vec<u8>>>) {
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        while let Ok(n) = r.read(&mut buf).await {
            if n == 0 { break; }
            let mut t = tail.lock().unwrap_or_else(|p| p.into_inner());
            t.extend_from_slice(&buf[..n]);
            let cut = t.len().saturating_sub(TAIL_BYTES);
            t.drain(..cut);
        }
    });
}

impl Mirrors {
    pub fn new() -> Self { Self::default() }

    pub fn list(&self) -> Vec<MirrorInfo> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<_> = g.values().map(|r| r.info.clone()).collect();
        v.sort_by_key(|i| i.started_at);
        v
    }

    pub async fn stop(&self, device_id: i64) -> Result<(), IpcError> {
        let tx = {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            g.get_mut(&device_id).and_then(|r| r.stop.take())
        };
        match tx {
            Some(tx) => { let _ = tx.send(()); Ok(()) }
            None => Err(IpcError::new(codes::E_NOTFOUND, "that device is not being mirrored")),
        }
    }

    /// App exit: ask every watcher to kill its scrcpy. Claims lapse on
    /// their own (no await is possible here).
    pub fn kill_all(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for r in g.values_mut() {
            if let Some(tx) = r.stop.take() { let _ = tx.send(()); }
        }
    }

    pub async fn start(&self, d: &DebugDevice, standalone: bool, tools: Tools,
                       remote: Remote, lease: Arc<dyn Lease>) -> Result<MirrorInfo, IpcError> {
        match self.start_inner(d, standalone, tools, remote, Arc::clone(&lease)).await {
            Ok(info) => Ok(info),
            Err(e) => { lease.release().await; Err(e) }
        }
    }

    async fn start_inner(&self, d: &DebugDevice, standalone: bool, tools: Tools,
                         remote: Remote, lease: Arc<dyn Lease>) -> Result<MirrorInfo, IpcError> {
        let serial = validate(d)?;
        let route = choose_route(&d.host, standalone)?;
        if self.inner.lock().unwrap_or_else(|p| p.into_inner()).contains_key(&d.id) {
            return Err(IpcError::new(codes::E_EXISTS, format!("{} is already mirrored", d.title)));
        }
        let relays = match route {
            Route::Local => None,
            Route::Ssh => Some(prepare_remote(&d.host, &tools, &remote).await?),
        };
        let ports = relays.as_ref().map(|r| SpawnPorts { adb: r.adb.local_port(), forward: r.forward, tunnel: r.tunnel.local_port() });
        let title = format!("{} @ {}", d.title, d.host);
        let mut child = scrcpy_command(&tools, &serial, &title, ports)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| IpcError::new(codes::E_TOOL_MISSING, format!("could not start {}: {e}", tools.scrcpy.display())))?;
        let tail = Arc::new(Mutex::new(Vec::new()));
        if let Some(o) = child.stdout.take() { collect_tail(o, Arc::clone(&tail)); }
        if let Some(e) = child.stderr.take() { collect_tail(e, Arc::clone(&tail)); }

        let early = tokio::time::timeout(STARTUP_GRACE, child.wait()).await;
        if let Ok(status) = early {
            // Give the tail readers a moment to drain the pipes.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let status = status?;
            if !status.success() {
                let t = String::from_utf8_lossy(&tail.lock().unwrap_or_else(|p| p.into_inner())).into_owned();
                return Err(IpcError::new(codes::E_MIRROR_FAILED, format!("scrcpy exited ({status}): {}", t.trim())));
            }
        }

        let info = MirrorInfo { device_id: d.id, host: d.host.clone(), serial: serial.clone(), title: d.title.clone(), route, started_at: now() };
        let (tx, rx) = oneshot::channel();
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
            .insert(d.id, Running { info: info.clone(), stop: Some(tx) });
        let exited = early.is_ok();
        let this = self.clone();
        let (host, id) = (d.host.clone(), d.id);
        tokio::spawn(async move {
            if !exited {
                let mut rx = rx;
                let mut renew = tokio::time::interval(RENEW_EVERY);
                renew.tick().await; // the first tick is immediate
                // The kill happens after the select: inside it, `child.wait()`
                // already borrows `child`.
                let mut kill = false;
                loop {
                    tokio::select! {
                        _ = child.wait() => break,
                        _ = &mut rx => { kill = true; break; }
                        _ = renew.tick() => {
                            if let Err(e) = lease.renew().await {
                                tracing::warn!(device = id, error = %e, "device mirror: claim renewal failed");
                            }
                        }
                    }
                }
                if kill {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
            if let Some(r) = relays {
                let forward = r.forward;
                drop(r);
                if let Ok(mut s) = remote.dialer.dial(&host, ADB_SERVER_PORT).await {
                    let _ = adb::kill_forward(&mut s, &serial, forward).await;
                }
            }
            lease.release().await;
            this.inner.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
        });
        Ok(info)
    }
}
```

`Mirrors` derives `Clone` (an `Arc` inside), so the watcher holds its own handle. The Tauri layer stores `Arc<Mirrors>`.

- [ ] **Step 4: Run the tests**

Run: `cargo fleet-test -- device_mirror::`
Expected: all pass (the 11 from Task 3 + 9 here).

- [ ] **Step 5: Lint and commit**

Run: `scripts/verify.sh`
Expected: fmt, lint and the touched modules' tests are clean.

```bash
/usr/bin/git add crates/fleet-core/src/service/device_mirror/
/usr/bin/git commit -m "feat(device-mirror): mirror lifecycle over the relay"
```

---

### Task 6: Desktop commands, page actions, verdicts, regenerated docs

**Files:**
- Create: `src-tauri/src/commands/device_mirror.rs`
- Modify: `src-tauri/src/commands/mod.rs` (`pub mod device_mirror;`)
- Modify: `src-tauri/src/lib.rs`:
  - `.manage(Arc<Mirrors>)` beside the store's `app.manage`
  - three handlers in `generate_handler!` after `commands::debug_devices::debug_device_screenshot`
  - `mirrors_for_exit.kill_all()` in `OnDestroyed::ShutDown` beside `tunnels_for_exit.stop_all()`
- Modify: `src-tauri/src/backend/verdicts.rs` (three rows next to the `*_debug_device` rows)
- Modify: `crates/fleet-core/src/pages/resources.rs`:
  - two `ActionSpec`s after `debug_device.screenshot`
  - the sorted `commands()` list in its test: add `start_device_mirror`, `stop_device_mirror`
- Regenerate: hub verdicts, control-API reference, page docs.

**Interfaces:**
- Consumes: `device_mirror::{Mirrors, Lease, Remote, MirrorInfo, CLAIM_SECS, tools}` (Tasks 3, 5); `relay::ssh::SshDialer` (Task 2); `commands::debug_devices::{DebugDeviceArgs, ClaimDebugDeviceArgs, routed}`.
- Produces: Tauri commands `start_device_mirror({ id }) -> MirrorStarted { title, output, exit_code, truncated }` (the *Output* result shape), `stop_device_mirror({ id }) -> ()`, `list_device_mirrors() -> Vec<MirrorInfo>`.

- [ ] **Step 1: Write the failing page test**

In `crates/fleet-core/src/pages/resources.rs`'s test that asserts `commands()`, insert `"start_device_mirror"` and `"stop_device_mirror"` in sorted position.

Run: `cargo fleet-test -- pages::resources`
Expected: FAIL, the list lacks the two commands.

- [ ] **Step 2: Add the page actions** (after the `debug_device.screenshot` line)

```rust
        ActionSpec::new("debug_device.mirror", "Mirror", "start_device_mirror", &[DEBUG_DEVICE_ID])
            .variants(&["physical", "emulator"])
            .result(ResultView::Output),
        ActionSpec::new("debug_device.mirror_stop", "Stop mirror", "stop_device_mirror", &[DEBUG_DEVICE_ID])
            .variants(&["physical", "emulator"]),
```

Run: `cargo fleet-test -- pages::`
Expected: the commands test passes. If a page-doc test fails, regenerate it: `REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`. That run fails on purpose; run it again without the variable and it passes.

- [ ] **Step 3: Write `src-tauri/src/commands/device_mirror.rs`**

```rust
//! The device mirror's desktop commands. They run on THIS machine in both
//! modes (scrcpy is a local window), and only the device lookup and its
//! claim go through `routed` (the hub's `debug_devices` tool when paired).

use super::debug_devices::{routed, ClaimDebugDeviceArgs, DebugDeviceArgs};
use crate::backend::FleetBackend;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::relay::ssh::SshDialer;
use fleet_core::service::device_mirror::{self as mirror, Lease, MirrorInfo, Mirrors, Remote};
use fleet_core::ssh::{SshClient, SshExec};
use fleet_core::store::Store;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Debug, Clone, Serialize)]
pub struct MirrorStarted {
    pub title: String,
    pub output: String,
    pub exit_code: i32,
    pub truncated: bool,
}

struct DesktopLease {
    backend: Arc<FleetBackend>,
    store: Arc<Mutex<Store>>,
    id: i64,
}

#[async_trait::async_trait]
impl Lease for DesktopLease {
    async fn renew(&self) -> Result<(), IpcError> {
        routed::claim_debug_device(&self.backend, &self.store, ClaimDebugDeviceArgs {
            id: self.id,
            note: Some("scrcpy mirror".into()),
            claim_s: Some(mirror::CLAIM_SECS),
        })
        .await
        .map(|_| ())
    }
    async fn release(&self) {
        if let Err(e) = routed::release_debug_device(&self.backend, &self.store, DebugDeviceArgs { id: self.id }).await {
            tracing::warn!(device = self.id, error = %e, "device mirror: releasing the claim failed");
        }
    }
}

#[tauri::command]
pub async fn start_device_mirror(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    mirrors: State<'_, Arc<Mirrors>>,
) -> Result<MirrorStarted, IpcError> {
    let devices = routed::list_debug_devices(&backend, &store, &ssh).await?;
    let d = devices.into_iter().find(|d| d.id == args.id)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no debug device {}", args.id)))?;
    mirror::validate(&d)?;
    let home = fleet_core::home::home_dir().unwrap_or_default();
    let tools = mirror::tools::find(&|k| std::env::var(k).ok(), &home)?;
    let lease = Arc::new(DesktopLease { backend: backend.inner().clone(), store: store.inner().clone(), id: d.id });
    lease.renew().await?; // takes the claim; E_CONFLICT names another holder
    let exec: Arc<dyn SshExec> = ssh.inner().clone();
    let remote = Remote { dialer: Arc::new(SshDialer::new(ssh.inner().as_ref().clone())), ssh: exec };
    let info = mirrors.start(&d, backend.hub().is_none(), tools, remote, lease).await?;
    Ok(MirrorStarted {
        title: format!("Mirroring {}", info.title),
        output: format!(
            "A scrcpy window shows {} from {} (route: {}). Close it, or use Stop mirror, to end.",
            info.title,
            info.host,
            match info.route { mirror::Route::Local => "local", mirror::Route::Ssh => "ssh" },
        ),
        exit_code: 0,
        truncated: false,
    })
}

#[tauri::command]
pub async fn stop_device_mirror(args: DebugDeviceArgs, mirrors: State<'_, Arc<Mirrors>>) -> Result<(), IpcError> {
    mirrors.stop(args.id).await
}

#[tauri::command]
pub async fn list_device_mirrors(mirrors: State<'_, Arc<Mirrors>>) -> Result<Vec<MirrorInfo>, IpcError> {
    Ok(mirrors.list())
}
```

Adjust these to the real names, without changing behaviour:
- **Home dir:** use `fleet_core::home`'s actual helper for this machine's home directory (grep `pub fn` in `crates/fleet-core/src/home.rs`).
- **`FleetBackend`:** if it is not managed as `Arc<FleetBackend>`, take `State<'_, Arc<FleetBackend>>` exactly as `commands::debug_devices` does and clone the `Arc` the same way.
- **`routed` signatures:** if a function takes `&Mutex<Store>` rather than `&Arc<Mutex<Store>>`, pass `&self.store` (it derefs).

- [ ] **Step 4: Register, manage, kill on exit** (`src-tauri/src/lib.rs`)

- Add the three commands to `generate_handler!` after `commands::debug_devices::debug_device_screenshot`.
- Where the `Arc<Store>` is managed (`app.manage(std::sync::Arc::clone(&store))`, ~line 264), add `let mirrors = std::sync::Arc::new(fleet_core::service::device_mirror::Mirrors::new()); app.manage(std::sync::Arc::clone(&mirrors));`.
- Make a `mirrors_for_exit` clone next to `tunnels_for_exit`, wherever that one is created (it has to be created before `.on_window_event`; if `mirrors` is created inside `setup`, create it before the builder instead and move a clone into `setup`).
- In `OnDestroyed::ShutDown`, add `mirrors_for_exit.kill_all();` after `tunnels_for_exit.stop_all();`.

- [ ] **Step 5: Verdict rows** (`src-tauri/src/backend/verdicts.rs`, after the `debug_device_screenshot` row)

```rust
    (
        "start_device_mirror",
        Verdict::SameInBoth {
            why: "opens a scrcpy window on THIS machine; it looks the device up and claims it through \
                  the routed debug_devices commands, so a paired desktop uses the hub's inventory",
        },
    ),
    (
        "stop_device_mirror",
        Verdict::SameInBoth { why: "stops a scrcpy window this machine runs" },
    ),
    (
        "list_device_mirrors",
        Verdict::SameInBoth { why: "the scrcpy windows this machine runs" },
    ),
```

- [ ] **Step 6: Regenerate the generated files**

Run, each one twice (the `REGEN_*` run fails on purpose, and the second run without the variable must pass):

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen
cargo fleet-test -- verdict_gen
REGEN_DOCS=1 cargo fleet-test -- reference_is_current
cargo fleet-test -- reference_is_current
```

If a routing test (`backend::tests_routing`) demands a row for the new commands, add it the way the `SameInBoth` rows there are written.

- [ ] **Step 7: Verify**

Run: `cargo fleet-lint` then `TMPDIR=/dev/shm/fleet-tests cargo fleet-test -- backend::` (on macOS: `TMPDIR=/tmp`), then `pnpm exec vitest run src/lib/pages`.
Expected: clean. The frontend tests pick up the new page actions from `registry.generated.json`.

- [ ] **Step 8: Commit**

```bash
/usr/bin/git add src-tauri/src/commands/device_mirror.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs \
  src-tauri/src/backend/verdicts.rs crates/fleet-core/src/pages/resources.rs \
  src/lib/hub_verdicts.generated.json src/lib/pages/registry.generated.json docs/hub.md docs/control-api-reference.md
/usr/bin/git status --short   # anything else regenerated (page docs) is added too
/usr/bin/git commit -m "feat(debug-devices): Mirror action opens scrcpy over SSH or locally"
```

---

### Task 7: Docs + full verification + manual acceptance

**Files:**
- Modify: `docs/debug-devices.md`
  - A new `## Mirror` section after *The Debug devices page*.
  - Add **Mirror** / **Stop mirror** to that section's list.
  - *Not built yet*: replace the "Live screen streaming" bullet with what is left (the hub route and agent hosts, PR 2).
- Modify: `docs/status.md` (the Debug devices paragraph: the mirror over SSH/local is landed, the hub relay is not)
- Modify: `docs/architecture.md` (the Debug devices bullet: add `relay/`, `service/device_mirror/`, `commands/device_mirror.rs`)

- [ ] **Step 1: Write the `## Mirror` section**, covering exactly:
  - what it does: a native scrcpy window, Android only, physical devices and emulators;
  - needs scrcpy and adb on THIS machine (`brew install scrcpy android-platform-tools`), with `FLEET_SCRCPY` / `FLEET_ADB` overrides;
  - routes: `local` (standalone, host `local`) and `ssh` (this machine's `~/.ssh/config` must reach the host's alias — in hub-client mode the alias comes from the hub);
  - it claims the device while it runs (note `scrcpy mirror`), renewed every 10 minutes and released when the window closes;
  - the adb version check, and why (`E_ADB_VERSION_MISMATCH`);
  - it starts the host's adb server when it is not running;
  - forward ports 27183–27199 per host, and `E_LIMIT` when all are taken;
  - the security note from the spec: the relays bind loopback only, but another local user could connect while a mirror runs;
  - the error codes table (the four new ones plus `E_UNSUPPORTED`, `E_INVALID_STATE`, `E_EXISTS`, `E_CONFLICT`).

- [ ] **Step 2: Full verification**

Run: `scripts/verify.sh full`
Expected: green. Investigate any failure (re-run known flakes alone, per CLAUDE.md); never `--no-verify`.

- [ ] **Step 3: Manual acceptance** (record the results in the PR body; no hardware has been used by Debug devices before)

1. Standalone or paired, on the Mac: start an AVD (`emulator -avd <name>`) → rescan `local` → **Mirror** → a scrcpy window opens; typing works; closing it ends the mirror. The device's claim is gone after a rescan.
2. A host reachable over SSH that has `adb` and a device or emulator → **Mirror** → the window opens; the result says `route: ssh`; **Stop mirror** closes it; `adb forward --list` on the host shows no leftover `tcp:2718x`.
3. With mismatched adb versions (if available), the refusal names both versions, and the host's `adb devices` still works afterwards (its server was not killed).

- [ ] **Step 4: Commit**

```bash
/usr/bin/git add docs/debug-devices.md docs/status.md docs/architecture.md
/usr/bin/git commit -m "docs(debug-devices): the device mirror"
```

---

## Self-review (done while writing)

- **Spec coverage, PR 1:**
  - §1 relay core: Tasks 1–2.
  - §4 steps 1–6: route (Task 5), tools (Task 3), preflight version + forward port + start-server (Tasks 3–5), claim (Task 6 + `Lease`), spawn (Task 5), register (Task 5), teardown with killforward and release (Task 5), app quit (Task 6).
  - §5 UI: Task 6, with the deviations listed above.
  - *Errors*: Task 1 codes; uses in Tasks 3/5.
  - *Testing*: relay, preflight, lifecycle (stub scrcpy, fake adb server), docs regen.
  - Hub `/relay`, `HubDialer`, contract 18 and the agent frames are PR 2.
- **Types:** `Mirrors::start(&DebugDevice, bool, Tools, Remote, Arc<dyn Lease>)` is identical in Task 5's interface block, its implementation and Task 6's call. `SpawnPorts { adb, forward, tunnel }` matches between test and implementation. `FixedDialer { addr, asked }` matches between Task 1 and Task 5.
- **Known judgment calls for the implementer:**
  - The `NoSsh` fake (copy the method list from `ssh_fake.rs`).
  - The home-dir helper name.
  - Exactly where `tunnels_for_exit` is created in `lib.rs`.

  Each is flagged inline with what to look for.
