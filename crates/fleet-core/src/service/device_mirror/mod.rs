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
/// How long the watcher's best-effort `killforward` may take.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Local,
    Ssh,
}

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
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "scrcpy mirrors Android devices only; use Screenshot for iOS",
        ));
    }
    if !d.ready {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{} is {}; start or reconnect it first", d.title, d.state),
        ));
    }
    d.serial.clone().ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!("{} has no adb serial yet; rescan its host", d.title),
        )
    })
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

pub fn scrcpy_command(
    tools: &Tools,
    serial: &str,
    title: &str,
    ports: Option<SpawnPorts>,
) -> tokio::process::Command {
    let mut c = crate::proc::command(&tools.scrcpy);
    c.args(["--serial", serial, "--window-title", title]);
    c.env("ADB", &tools.adb);
    if let Some(p) = ports {
        c.args([
            "--force-adb-forward",
            "--port",
            &p.forward.to_string(),
            "--tunnel-host",
            "127.0.0.1",
            "--tunnel-port",
            &p.tunnel.to_string(),
        ]);
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
            debug_devices::start_adb_server(remote.ssh.as_ref(), host)
                .await
                .map_err(|e| {
                    IpcError::new(
                        codes::E_NO_ROUTE,
                        format!(
                            "cannot reach the adb server on {host} over ssh from this machine: {}",
                            e.message
                        ),
                    )
                })?;
            server_version(remote, host).await.map_err(|e| {
                IpcError::new(
                    codes::E_NO_ROUTE,
                    format!(
                        "{host}'s adb server does not answer after start-server: {}",
                        e.message
                    ),
                )
            })?
        }
    };
    adb::check_versions(adb::local_version(&tools.adb).await?, version)?;
    let mut s = remote.dialer.dial(host, ADB_SERVER_PORT).await?;
    let used = adb::forwarded_ports(&mut s).await?;
    let forward = adb::free_forward_port(&used).ok_or_else(|| {
        IpcError::new(
            codes::E_LIMIT,
            format!("every scrcpy port on {host} is forwarded already"),
        )
    })?;
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
fn collect_tail(
    mut r: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    tail: Arc<Mutex<Vec<u8>>>,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        while let Ok(n) = r.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let mut t = tail.lock().unwrap_or_else(|p| p.into_inner());
            t.extend_from_slice(&buf[..n]);
            let cut = t.len().saturating_sub(TAIL_BYTES);
            t.drain(..cut);
        }
    });
}

impl Mirrors {
    pub fn new() -> Self {
        Self::default()
    }

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
            Some(tx) => {
                let _ = tx.send(());
                Ok(())
            }
            None => Err(IpcError::new(
                codes::E_NOTFOUND,
                "that device is not being mirrored",
            )),
        }
    }

    /// App exit: ask every watcher to kill its scrcpy. Claims lapse on
    /// their own (no await is possible here).
    pub fn kill_all(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for r in g.values_mut() {
            if let Some(tx) = r.stop.take() {
                let _ = tx.send(());
            }
        }
    }

    pub async fn start(
        &self,
        d: &DebugDevice,
        standalone: bool,
        tools: Tools,
        remote: Remote,
        lease: Arc<dyn Lease>,
    ) -> Result<MirrorInfo, IpcError> {
        match self
            .start_inner(d, standalone, tools, remote, Arc::clone(&lease))
            .await
        {
            Ok(info) => Ok(info),
            Err(e) => {
                lease.release().await;
                Err(e)
            }
        }
    }

    async fn start_inner(
        &self,
        d: &DebugDevice,
        standalone: bool,
        tools: Tools,
        remote: Remote,
        lease: Arc<dyn Lease>,
    ) -> Result<MirrorInfo, IpcError> {
        let serial = validate(d)?;
        let route = choose_route(&d.host, standalone)?;
        if self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&d.id)
        {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("{} is already mirrored", d.title),
            ));
        }
        let relays = match route {
            Route::Local => None,
            Route::Ssh => Some(prepare_remote(&d.host, &tools, &remote).await?),
        };
        let ports = relays.as_ref().map(|r| SpawnPorts {
            adb: r.adb.local_port(),
            forward: r.forward,
            tunnel: r.tunnel.local_port(),
        });
        let title = format!("{} @ {}", d.title, d.host);
        let mut child = scrcpy_command(&tools, &serial, &title, ports)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                IpcError::new(
                    codes::E_TOOL_MISSING,
                    format!("could not start {}: {e}", tools.scrcpy.display()),
                )
            })?;
        let tail = Arc::new(Mutex::new(Vec::new()));
        if let Some(o) = child.stdout.take() {
            collect_tail(o, Arc::clone(&tail));
        }
        if let Some(e) = child.stderr.take() {
            collect_tail(e, Arc::clone(&tail));
        }

        let early = tokio::time::timeout(STARTUP_GRACE, child.wait()).await;
        let exited = early.is_ok();
        if let Ok(status) = early {
            // Give the tail readers a moment to drain the pipes.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let status = status?;
            if !status.success() {
                let t = String::from_utf8_lossy(&tail.lock().unwrap_or_else(|p| p.into_inner()))
                    .into_owned();
                return Err(IpcError::new(
                    codes::E_MIRROR_FAILED,
                    format!("scrcpy exited ({status}): {}", t.trim()),
                ));
            }
        }

        let info = MirrorInfo {
            device_id: d.id,
            host: d.host.clone(),
            serial: serial.clone(),
            title: d.title.clone(),
            route,
            started_at: now(),
        };
        let (tx, rx) = oneshot::channel();
        {
            // Re-checked under the same guard as the insert: a concurrent
            // start of this device may have registered during the grace
            // wait. Overwriting it would drop its stop sender (killing its
            // scrcpy) and let its watcher remove this entry.
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            match g.entry(d.id) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("{} is already mirrored", d.title),
                    ));
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(Running {
                        info: info.clone(),
                        stop: Some(tx),
                    });
                }
            }
        }
        let this = self.clone();
        let (host, id) = (d.host.clone(), d.id);
        tokio::spawn(async move {
            if !exited {
                let mut rx = rx;
                let mut renew = tokio::time::interval(RENEW_EVERY);
                // The first tick is immediate.
                renew.tick().await;
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
                // Best effort, and bounded: a host that stopped answering
                // must not hold the lease release behind it.
                let _ = tokio::time::timeout(CLEANUP_TIMEOUT, async {
                    if let Ok(mut s) = remote.dialer.dial(&host, ADB_SERVER_PORT).await {
                        let _ = adb::kill_forward(&mut s, &serial, forward).await;
                    }
                })
                .await;
            }
            lease.release().await;
            this.inner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
        });
        Ok(info)
    }
}
