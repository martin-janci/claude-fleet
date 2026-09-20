//! Tauri IPC wrappers for SSH host management. Logic lives in `service::hosts`;
//! this file only adapts `tauri::State` to plain references.
//!
//! Remote mode: `list_hosts`, `list_accounts` and `probe_host` have hub tools
//! and route there. The rest are **fleet administration**, which the hub
//! refuses to a paired client by design (`mcp::tools::support::enforce_admin`
//! makes `add_host`, `remove_host` and `hide_host` master-only), so they
//! refuse here with the reason rather than reaching the network to be told no —
//! or worse, editing this machine's own database behind the hub's back.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::hosts::{
    self, AddHostArgs, HideHostArgs, HostAliasArgs, ProbePreview, ProbeSshAliasArgs,
    SetAccountNicknameArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::ssh_config::SshHost;
use fleet_core::store::{AccountRow, HostRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn discover_hosts(backend: State<'_, Arc<FleetBackend>>) -> Result<Vec<SshHost>, IpcError> {
    // Reads *this machine's* ~/.ssh/config, which says nothing about the hub's
    // hosts — and it only ever feeds the Add-host dialog, which a client
    // cannot complete anyway.
    backend.refuse_local_only("discover_hosts")?;
    hosts::discover_hosts()
}

#[tauri::command]
pub async fn list_hosts(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostRow>, IpcError> {
    routed::list_hosts(&backend, &store).await
}

#[tauri::command]
pub async fn list_accounts(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AccountRow>, IpcError> {
    routed::list_accounts(&backend, &store).await
}

#[tauri::command]
pub async fn add_host(
    args: AddHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    backend.refuse_local_only("add_host")?;
    hosts::add_host(args, &store, &*ssh).await
}

#[tauri::command]
pub async fn probe_ssh_alias(
    args: ProbeSshAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<ProbePreview, IpcError> {
    backend.refuse_local_only("probe_ssh_alias")?;
    hosts::probe_ssh_alias(args, &*ssh, &reg).await
}

#[tauri::command]
pub async fn probe_host(
    args: HostAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<HostRow, IpcError> {
    routed::probe_host(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub fn remove_host(
    args: HostAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    backend.refuse_local_only("remove_host")?;
    hosts::remove_host(args, &store)
}

#[tauri::command]
pub fn hide_host(
    args: HideHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    backend.refuse_local_only("hide_host")?;
    hosts::hide_host(args, &store)
}

#[tauri::command]
pub fn set_account_nickname(
    args: SetAccountNicknameArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AccountRow, IpcError> {
    backend.refuse_local_only("set_account_nickname")?;
    hosts::set_account_nickname(args, &store)
}

/// The routing, away from `tauri::State` so the tests can drive it.
pub(crate) mod routed {
    use super::*;

    pub async fn list_hosts(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<HostRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_hosts().await,
            None => hosts::list_hosts(store),
        }
    }

    pub async fn list_accounts(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<AccountRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_accounts().await,
            None => hosts::list_accounts(store),
        }
    }

    /// Re-probing a host is a read of the fleet's state, not fleet
    /// administration, so a paired client may do it (`add_host` /
    /// `remove_host` / `hide_host` are the master-only ones).
    pub async fn probe_host(
        backend: &FleetBackend,
        args: HostAliasArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<HostRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("probe_host", &args).await,
            None => hosts::probe_host(args, store, &**ssh, reg).await,
        }
    }
}
