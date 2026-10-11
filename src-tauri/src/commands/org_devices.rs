//! Tauri commands for the company's paired devices and people (org
//! administration phase B): Settings → Devices, Settings → People and the
//! org page's *Devices*. Each routes to the hub's `org_admin` — the hub
//! lets its owner's own device list them and a trusted `full` one change
//! them, and never lets a device lock itself out. Standalone, they run
//! `service::org_admin` on this desktop's store, where a pairing code
//! cannot be minted (`pair_device` says so).

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::org_admin::{DeviceSummary, OrgAdminArgs, PersonSummary};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

use super::orgs::routed::local;
use fleet_core::store::Store;

fn decode<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, IpcError> {
    serde_json::from_value(v)
        .map_err(|e| IpcError::new(fleet_core::ipc_error::codes::E_SERIALIZE, e.to_string()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairDeviceArgs {
    pub device: String,
    /// `full` (default) or `readonly`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub trusted: Option<bool>,
    /// Bind it to this org (by name, or `org_id`).
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub org_id: Option<i64>,
    /// Whose device; the hub's owner when absent.
    #[serde(default)]
    pub person: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceArgs {
    pub device: String,
}

/// Apply's changed fields on Settings → Devices: only those present are
/// written. `name` is the device's new name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateDeviceArgs {
    pub device: String,
    #[serde(default)]
    pub name: Option<String>,
    /// `full` or `readonly`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub trusted: Option<bool>,
    /// M15 step G7.14: the org it is bound to, picked in the edit form; ""
    /// unbinds.
    #[serde(default)]
    pub org: Option<String>,
    /// G7.14: whose device it is, picked in the edit form.
    #[serde(default)]
    pub person: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceTrustArgs {
    pub device: String,
    pub trusted: bool,
}

/// No org (`org` and `org_id` both absent or empty) unbinds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BindDeviceArgs {
    pub device: String,
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub org_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevicePersonArgs {
    pub device: String,
    pub person: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceCatalogArgs {
    pub device: String,
    pub catalog: String,
    pub on: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenamePersonArgs {
    pub person_id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// M15 step G7.14: "+ Person".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddPersonArgs {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersonIdArgs {
    pub person_id: i64,
}

/// What `pair_device` answers: a one-time code, the URL a phone opens, and
/// the URL's QR as rows of `1` (dark) / `0`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub url: String,
    pub code: String,
    pub expires_in_s: u64,
    pub name: String,
    pub mode: String,
    pub trusted: bool,
    #[serde(default)]
    pub org_id: Option<i64>,
    #[serde(default)]
    pub person: Option<String>,
    #[serde(default)]
    pub qr: Vec<String>,
}

#[tauri::command]
pub async fn list_devices(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<DeviceSummary>, IpcError> {
    decode(routed::list_devices(&backend, &store, OrgAdminArgs::new("list_devices")).await?)
}

#[tauri::command]
pub async fn pair_device(
    backend: State<'_, Arc<FleetBackend>>,
    args: PairDeviceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Pairing, IpcError> {
    decode(
        routed::pair_device(
            &backend,
            &store,
            OrgAdminArgs {
                device: Some(args.device),
                mode: args.mode,
                trusted: args.trusted,
                org: args.org,
                org_id: args.org_id,
                person: args.person,
                ..OrgAdminArgs::new("pair_device")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn revoke_device(
    backend: State<'_, Arc<FleetBackend>>,
    args: DeviceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::revoke_device(
        &backend,
        &store,
        OrgAdminArgs {
            device: Some(args.device),
            ..OrgAdminArgs::new("revoke_device")
        },
    )
    .await
}

#[tauri::command]
pub async fn set_device_trust(
    backend: State<'_, Arc<FleetBackend>>,
    args: DeviceTrustArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DeviceSummary, IpcError> {
    decode(
        routed::set_device_trust(
            &backend,
            &store,
            OrgAdminArgs {
                device: Some(args.device),
                trusted: Some(args.trusted),
                ..OrgAdminArgs::new("set_device_trust")
            },
        )
        .await?,
    )
}

/// Trust, then mode, then the org and the person (M15 step G7.14), then the
/// name, each through its own `org_admin` action, so a hub that predates
/// `rename_device` / `set_device_mode` still takes a trust change. The
/// rename goes last: the others address the device by the name it has now.
#[tauri::command]
pub async fn update_device(
    backend: State<'_, Arc<FleetBackend>>,
    args: UpdateDeviceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DeviceSummary, IpcError> {
    decode(routed::update_device(&backend, &store, args).await?)
}

#[tauri::command]
pub async fn bind_device_org(
    backend: State<'_, Arc<FleetBackend>>,
    args: BindDeviceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DeviceSummary, IpcError> {
    decode(
        routed::bind_device_org(
            &backend,
            &store,
            OrgAdminArgs {
                device: Some(args.device),
                org: args.org.filter(|o| !o.trim().is_empty()),
                org_id: args.org_id,
                ..OrgAdminArgs::new("bind_device")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn set_device_person(
    backend: State<'_, Arc<FleetBackend>>,
    args: DevicePersonArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DeviceSummary, IpcError> {
    decode(
        routed::set_device_person(
            &backend,
            &store,
            OrgAdminArgs {
                device: Some(args.device),
                person: Some(args.person),
                ..OrgAdminArgs::new("set_device_person")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn grant_device_catalog(
    backend: State<'_, Arc<FleetBackend>>,
    args: DeviceCatalogArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DeviceSummary, IpcError> {
    decode(
        routed::grant_device_catalog(
            &backend,
            &store,
            OrgAdminArgs {
                device: Some(args.device),
                catalog: Some(args.catalog),
                on: Some(args.on),
                ..OrgAdminArgs::new("grant_catalog")
            },
        )
        .await?,
    )
}

#[tauri::command]
pub async fn list_people(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<PersonSummary>, IpcError> {
    decode(routed::list_people(&backend, &store, OrgAdminArgs::new("list_people")).await?)
}

#[tauri::command]
pub async fn add_person(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddPersonArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::add_person(
        &backend,
        &store,
        OrgAdminArgs {
            name: Some(args.name),
            display_name: args.display_name,
            ..OrgAdminArgs::new("add_person")
        },
    )
    .await
}

#[tauri::command]
pub async fn rename_person(
    backend: State<'_, Arc<FleetBackend>>,
    args: RenamePersonArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::rename_person(
        &backend,
        &store,
        OrgAdminArgs {
            person_id: Some(args.person_id),
            name: args.name,
            display_name: args.display_name,
            ..OrgAdminArgs::new("rename_person")
        },
    )
    .await
}

#[tauri::command]
pub async fn disable_person(
    backend: State<'_, Arc<FleetBackend>>,
    args: PersonIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::disable_person(
        &backend,
        &store,
        OrgAdminArgs {
            person_id: Some(args.person_id),
            ..OrgAdminArgs::new("disable_person")
        },
    )
    .await
}

pub(crate) mod routed {
    use super::*;

    pub async fn list_devices(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_devices", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn pair_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("pair_device", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn revoke_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("revoke_device", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn set_device_trust(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_device_trust", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn update_device(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: UpdateDeviceArgs,
    ) -> Result<serde_json::Value, IpcError> {
        let mut steps: Vec<OrgAdminArgs> = Vec::new();
        if let Some(trusted) = args.trusted {
            steps.push(OrgAdminArgs {
                device: Some(args.device.clone()),
                trusted: Some(trusted),
                ..OrgAdminArgs::new("set_device_trust")
            });
        }
        if let Some(mode) = args.mode {
            steps.push(OrgAdminArgs {
                device: Some(args.device.clone()),
                mode: Some(mode),
                ..OrgAdminArgs::new("set_device_mode")
            });
        }
        if let Some(org) = args.org {
            let org = org.trim();
            steps.push(OrgAdminArgs {
                device: Some(args.device.clone()),
                org: (!org.is_empty()).then(|| org.to_string()),
                ..OrgAdminArgs::new("bind_device")
            });
        }
        if let Some(person) = args.person.filter(|p| !p.trim().is_empty()) {
            steps.push(OrgAdminArgs {
                device: Some(args.device.clone()),
                person: Some(person.trim().to_string()),
                ..OrgAdminArgs::new("set_device_person")
            });
        }
        if let Some(name) = args.name.filter(|n| n.trim() != args.device.trim()) {
            steps.push(OrgAdminArgs {
                device: Some(args.device.clone()),
                name: Some(name),
                ..OrgAdminArgs::new("rename_device")
            });
        }
        if steps.is_empty() {
            return Err(IpcError::new(
                fleet_core::ipc_error::codes::E_INVALID,
                "update_device needs name, mode, trusted, org or person",
            ));
        }
        let mut last = serde_json::Value::Null;
        for step in steps {
            last = match backend.hub() {
                Some(hub) => hub.route("update_device", &step).await?,
                None => local(&step, store)?,
            };
        }
        Ok(last)
    }

    pub async fn bind_device_org(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("bind_device_org", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn set_device_person(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("set_device_person", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn grant_device_catalog(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("grant_device_catalog", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn list_people(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_people", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn add_person(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("add_person", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn rename_person(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("rename_person", &args).await,
            None => local(&args, store),
        }
    }

    pub async fn disable_person(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: OrgAdminArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("disable_person", &args).await,
            None => local(&args, store),
        }
    }
}
