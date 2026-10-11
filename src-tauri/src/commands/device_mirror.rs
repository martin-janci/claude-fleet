//! The device mirror's desktop commands. They run on THIS machine in both
//! modes (scrcpy is a local window), and only the device lookup and its
//! claim go through `routed` (the hub's `debug_devices` tool when paired).

use super::debug_devices::{routed, ClaimDebugDeviceArgs, DebugDeviceArgs};
use crate::backend::FleetBackend;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::relay::ssh::SshDialer;
use fleet_core::service::debug_devices::{Asker, DebugDevice};
use fleet_core::service::device_mirror::{
    self as mirror, Lease, MirrorInfo, Mirrors, OwnedLease, Remote,
};
use fleet_core::ssh::{SshClient, SshExec};
use fleet_core::store::Store;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::State;

/// What Mirror answers, in the page's *Output* result shape.
#[derive(Debug, Clone, Serialize)]
pub struct MirrorStarted {
    pub title: String,
    pub output: String,
    pub exit_code: i32,
    pub truncated: bool,
}

/// The device's claim, taken and renewed through the routed claim command
/// (the hub's when paired), released when the mirror ends.
struct DesktopLease {
    backend: Arc<FleetBackend>,
    store: Arc<Mutex<Store>>,
    id: i64,
}

#[async_trait::async_trait]
impl Lease for DesktopLease {
    async fn renew(&self) -> Result<(), IpcError> {
        routed::claim_debug_device(
            &self.backend,
            &self.store,
            ClaimDebugDeviceArgs {
                id: self.id,
                note: Some("scrcpy mirror".into()),
                claim_s: Some(mirror::CLAIM_SECS),
            },
        )
        .await
        .map(|_| ())
    }

    async fn release(&self) {
        if let Err(e) = routed::release_debug_device(
            &self.backend,
            &self.store,
            DebugDeviceArgs { id: self.id },
        )
        .await
        {
            tracing::warn!(device = self.id, error = %e, "device mirror: releasing the claim failed");
        }
    }
}

/// The device as the debug devices inventory has it: this machine's
/// standalone, the hub's when paired. Only this lookup and the claim go
/// through `routed`; the mirror itself runs here in both modes.
async fn device(
    backend: &FleetBackend,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    id: i64,
) -> Result<DebugDevice, IpcError> {
    routed::list_debug_devices(backend, store, ssh)
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no debug device {id}")))
}

#[tauri::command]
pub async fn start_device_mirror(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    mirrors: State<'_, Arc<Mirrors>>,
) -> Result<MirrorStarted, IpcError> {
    let d = device(&backend, &store, &ssh, args.id).await?;
    mirror::validate(&d)?;
    // Everything that refuses without touching the host runs before the
    // claim, so a refusal costs no claim-and-release round trip.
    mirror::choose_route(&d.host, backend.hub().is_none())?;
    // This machine's ssh has no route to an agent host's adb server, and a
    // WSL distribution is reached through wsl.exe, not `ssh -W`.
    if ssh.routes_through_agent(&d.host) {
        return Err(IpcError::new(
            codes::E_NO_ROUTE,
            format!(
                "{} is an agent host; mirroring it needs the hub relay, which this version does not have yet",
                d.host
            ),
        ));
    }
    if fleet_core::wsl::is_wsl_host(&d.host) {
        return Err(IpcError::new(
            codes::E_NO_ROUTE,
            format!(
                "{} is a WSL host, which has no ssh route for the mirror's relays; mirroring it needs the hub relay, which this version does not have yet",
                d.host
            ),
        ));
    }
    // Before the claim: a second click must not re-claim a device whose
    // claim the running mirror holds.
    if mirrors.is_mirrored(d.id) {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!("{} is already mirrored", d.title),
        ));
    }
    let home = fleet_core::home::home_dir().unwrap_or_default();
    let tools = mirror::tools::find(&|k| std::env::var(k).ok(), &home)?;
    // The holder the claim command records for this desktop: the desktop
    // person standalone, the paired client on the hub (`Caller::label`).
    let holder = match backend.hub() {
        None => Asker::desktop().holder,
        Some(hub) => format!("client:{}", hub.config().client_name),
    };
    let lease = Arc::new(OwnedLease::new(
        Arc::new(DesktopLease {
            backend: backend.inner().clone(),
            store: store.inner().clone(),
            id: d.id,
        }),
        mirror::held_by(&d, &holder),
    ));
    lease.renew().await?; // takes the claim; E_CONFLICT names another holder
    let exec: Arc<dyn SshExec> = ssh.inner().clone();
    let remote = Remote {
        dialer: Arc::new(SshDialer::new(ssh.inner().as_ref().clone())),
        ssh: exec,
    };
    let info = mirrors
        .start(&d, backend.hub().is_none(), tools, remote, lease)
        .await?;
    Ok(MirrorStarted {
        title: format!("Mirroring {}", info.title),
        output: format!(
            "A scrcpy window shows {} from {} (route: {}). Close it, or use Stop mirror, to end.",
            info.title,
            info.host,
            match info.route {
                mirror::Route::Local => "local",
                mirror::Route::Ssh => "ssh",
            },
        ),
        exit_code: 0,
        truncated: false,
    })
}

#[tauri::command]
pub async fn stop_device_mirror(
    args: DebugDeviceArgs,
    mirrors: State<'_, Arc<Mirrors>>,
) -> Result<(), IpcError> {
    mirrors.stop(args.id).await
}

#[tauri::command]
pub async fn list_device_mirrors(
    mirrors: State<'_, Arc<Mirrors>>,
) -> Result<Vec<MirrorInfo>, IpcError> {
    Ok(mirrors.list())
}
