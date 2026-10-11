//! Debug devices on the desktop (`docs/debug-devices.md`): the Debug devices
//! page lists the phones, emulators and simulators on the fleet's hosts,
//! and a person labels, shares, claims, releases, starts, stops and forgets
//! them, installs an app on one, reads its logs and takes a screenshot.
//! Standalone they run `service::debug_devices` here; paired, on the hub's
//! `debug_devices` tool, where the hub's own SSH reaches the hosts.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::debug_devices::{
    self as devices, scripts::RunOutput, Asker, DebugDevice, HostScan, InstallFrom,
};
use fleet_core::ssh::{SshClient, SshExec};
use fleet_core::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanDebugDevicesArgs {
    /// One host; every host when absent.
    #[serde(default)]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugDeviceArgs {
    pub id: i64,
}

/// Apply's changed fields: only those present are written.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateDebugDeviceArgs {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub shared: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaimDebugDeviceArgs {
    pub id: i64,
    /// What the claim is for.
    #[serde(default)]
    pub note: Option<String>,
    /// How long, in seconds; the service's default (30 min) when absent.
    #[serde(default)]
    pub claim_s: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallDebugDeviceArgs {
    pub id: i64,
    /// The .apk, .app or .ipa, on `host`.
    pub path: String,
    /// Where `path` is; the device's own host when absent or empty.
    #[serde(default)]
    pub host: Option<String>,
    /// M15 step G7.14: claim the device first (with `note`), so others see
    /// it in use while the app goes on. Absent: install only.
    #[serde(default)]
    pub claim: Option<bool>,
    /// What the claim is for.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugDeviceLogsArgs {
    pub id: i64,
    /// Only lines holding this.
    #[serde(default)]
    pub contains: Option<String>,
}

/// A screenshot as the page shows it: the image as base64, and its type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    pub caption: String,
    pub mime: String,
    pub data: String,
}

/// What `scan_debug_devices` answers: one entry per host scanned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanAnswer {
    pub hosts: Vec<HostScan>,
}

#[tauri::command]
pub async fn list_debug_devices(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<DebugDevice>, IpcError> {
    routed::list_debug_devices(&backend, &store, &ssh).await
}

#[tauri::command]
pub async fn scan_debug_devices(
    // Absent: every host (the Debug devices page's "Scan all hosts", a page
    // action, runs its command with no arguments).
    args: Option<ScanDebugDevicesArgs>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ScanAnswer, IpcError> {
    routed::scan_debug_devices(&backend, &store, &ssh, args.unwrap_or_default()).await
}

#[tauri::command]
pub async fn update_debug_device(
    args: UpdateDebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DebugDevice, IpcError> {
    routed::update_debug_device(&backend, &store, args).await
}

#[tauri::command]
pub async fn release_debug_device(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DebugDevice, IpcError> {
    routed::release_debug_device(&backend, &store, args).await
}

#[tauri::command]
pub async fn forget_debug_device(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::forget_debug_device(&backend, &store, args).await
}

#[tauri::command]
pub async fn boot_debug_device(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<serde_json::Value, IpcError> {
    routed::boot_debug_device(&backend, &store, &ssh, args).await
}

#[tauri::command]
pub async fn shutdown_debug_device(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<DebugDevice, IpcError> {
    routed::shutdown_debug_device(&backend, &store, &ssh, args).await
}

#[tauri::command]
pub async fn claim_debug_device(
    args: ClaimDebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DebugDevice, IpcError> {
    routed::claim_debug_device(&backend, &store, args).await
}

#[tauri::command]
pub async fn install_debug_device(
    args: InstallDebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RunOutput, IpcError> {
    routed::install_debug_device(&backend, &store, &ssh, args).await
}

#[tauri::command]
pub async fn debug_device_logs(
    args: DebugDeviceLogsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RunOutput, IpcError> {
    routed::debug_device_logs(&backend, &store, &ssh, args).await
}

#[tauri::command]
pub async fn debug_device_screenshot(
    args: DebugDeviceArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Screenshot, IpcError> {
    routed::debug_device_screenshot(&backend, &store, &ssh, args).await
}

/// Lines `debug_device_logs` asks for: the service's own default.
const LOG_LINES: u32 = 200;

pub(crate) mod routed {
    use super::*;
    use serde_json::json;

    pub async fn list_debug_devices(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<DebugDevice>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let list: devices::DeviceList = hub
                    .route("list_debug_devices", &json!({ "action": "list" }))
                    .await?;
                Ok(list.devices)
            }
            None => {
                let ssh: Arc<dyn SshExec> = ssh.clone();
                Ok(devices::list(store, ssh, &Asker::desktop(), false)
                    .await?
                    .devices)
            }
        }
    }

    pub async fn scan_debug_devices(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: ScanDebugDevicesArgs,
    ) -> Result<ScanAnswer, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "scan_debug_devices",
                    &json!({ "action": "scan", "host": args.host }),
                )
                .await
            }
            None => Ok(ScanAnswer {
                hosts: devices::scan(store, &**ssh, &Asker::desktop(), args.host.as_deref())
                    .await?,
            }),
        }
    }

    pub async fn update_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: UpdateDebugDeviceArgs,
    ) -> Result<DebugDevice, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "update_debug_device",
                    &json!({
                        "action": "configure",
                        "device": args.id.to_string(),
                        "label": args.label,
                        "shared": args.shared,
                    }),
                )
                .await
            }
            None => devices::configure(
                store,
                &Asker::desktop(),
                args.id,
                args.label.as_deref(),
                args.shared,
            ),
        }
    }

    pub async fn release_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: DebugDeviceArgs,
    ) -> Result<DebugDevice, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "release_debug_device",
                    &json!({ "action": "release", "device": args.id.to_string() }),
                )
                .await
            }
            None => devices::release(store, &Asker::desktop(), &args.id.to_string()),
        }
    }

    pub async fn forget_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: DebugDeviceArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "forget_debug_device",
                    &json!({ "action": "forget", "device": args.id.to_string() }),
                )
                .await
            }
            None => Ok(json!({
                "removed": devices::forget(store, &Asker::desktop(), args.id)?
            })),
        }
    }

    pub async fn boot_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: DebugDeviceArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "boot_debug_device",
                    &json!({ "action": "boot", "device": args.id.to_string() }),
                )
                .await
            }
            None => {
                let said = devices::boot(
                    store,
                    &**ssh,
                    &Asker::desktop(),
                    Some(&args.id.to_string()),
                    None,
                    None,
                )
                .await?;
                Ok(json!({ "state": said }))
            }
        }
    }

    pub async fn shutdown_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: DebugDeviceArgs,
    ) -> Result<DebugDevice, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "shutdown_debug_device",
                    &json!({ "action": "shutdown", "device": args.id.to_string() }),
                )
                .await
            }
            None => devices::shutdown(store, &**ssh, &Asker::desktop(), &args.id.to_string()).await,
        }
    }
    pub async fn claim_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: ClaimDebugDeviceArgs,
    ) -> Result<DebugDevice, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "claim_debug_device",
                    &json!({
                        "action": "claim",
                        "device": args.id.to_string(),
                        "note": args.note,
                        "claim_s": args.claim_s,
                    }),
                )
                .await
            }
            None => devices::claim(
                store,
                &Asker::desktop(),
                &args.id.to_string(),
                args.claim_s,
                args.note.as_deref(),
            ),
        }
    }

    pub async fn install_debug_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: InstallDebugDeviceArgs,
    ) -> Result<RunOutput, IpcError> {
        if args.claim == Some(true) {
            let note = args.note.clone().filter(|n| !n.trim().is_empty());
            claim_debug_device(
                backend,
                store,
                ClaimDebugDeviceArgs {
                    id: args.id,
                    note,
                    claim_s: None,
                },
            )
            .await?;
        }
        let host = args.host.filter(|h| !h.trim().is_empty());
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "install_debug_device",
                    &json!({
                        "action": "install",
                        "device": args.id.to_string(),
                        "path": args.path,
                        "host": host,
                    }),
                )
                .await
            }
            None => {
                let asker = Asker::desktop();
                let from_host = match host {
                    Some(h) => h,
                    None => {
                        let s = fleet_core::ipc_error::lock(store)?;
                        devices::resolve(&s, &asker, &args.id.to_string())?.host
                    }
                };
                devices::install(
                    store,
                    &**ssh,
                    &asker,
                    &args.id.to_string(),
                    InstallFrom {
                        host: &from_host,
                        path: &args.path,
                    },
                    false,
                )
                .await
            }
        }
    }

    pub async fn debug_device_logs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: DebugDeviceLogsArgs,
    ) -> Result<RunOutput, IpcError> {
        let contains = args.contains.filter(|c| !c.trim().is_empty());
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "debug_device_logs",
                    &json!({
                        "action": "logs",
                        "device": args.id.to_string(),
                        "lines": LOG_LINES,
                        "contains": contains,
                    }),
                )
                .await
            }
            None => {
                devices::logs(
                    store,
                    &**ssh,
                    &Asker::desktop(),
                    &args.id.to_string(),
                    Some(LOG_LINES),
                    None,
                    None,
                    contains.as_deref(),
                )
                .await
            }
        }
    }

    pub async fn debug_device_screenshot(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        args: DebugDeviceArgs,
    ) -> Result<Screenshot, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let image = hub
                    .route_image(
                        "debug_device_screenshot",
                        &json!({ "action": "screenshot", "device": args.id.to_string() }),
                    )
                    .await?;
                Ok(Screenshot {
                    caption: image.caption,
                    mime: image.mime,
                    data: image.data,
                })
            }
            None => {
                use base64::Engine as _;
                let (d, bytes, mime) =
                    devices::screenshot(store, &**ssh, &Asker::desktop(), &args.id.to_string())
                        .await?;
                Ok(Screenshot {
                    caption: format!("{} on {}", d.title, d.host),
                    mime: mime.to_string(),
                    data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                })
            }
        }
    }
}
