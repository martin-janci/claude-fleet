//! Debug devices on the desktop (`docs/debug-devices.md`): the Debug devices
//! page lists the phones, emulators and simulators on the fleet's hosts,
//! and a person labels, shares, releases, starts, stops and forgets them.
//! Standalone they run `service::debug_devices` here; paired, on the hub's
//! `debug_devices` tool, where the hub's own SSH reaches the hosts.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::debug_devices::{self as devices, Asker, DebugDevice, HostScan};
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
    args: ScanDebugDevicesArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ScanAnswer, IpcError> {
    routed::scan_debug_devices(&backend, &store, &ssh, args).await
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
}
