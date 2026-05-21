//! Tauri IPC wrappers for SSH host management. Logic lives in `service::hosts`;
//! this file only adapts `tauri::State` to plain references.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::service::hosts::{
    self, AddHostArgs, HideHostArgs, HostAliasArgs, ProbePreview, ProbeSshAliasArgs,
};
use crate::ssh::SshClient;
use crate::ssh_config::SshHost;
use crate::store::{AccountRow, HostRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn discover_hosts() -> Result<Vec<SshHost>, IpcError> {
    hosts::discover_hosts()
}

#[tauri::command]
pub fn list_hosts(store: State<'_, Arc<Mutex<Store>>>) -> Result<Vec<HostRow>, IpcError> {
    hosts::list_hosts(&store)
}

#[tauri::command]
pub fn list_accounts(store: State<'_, Arc<Mutex<Store>>>) -> Result<Vec<AccountRow>, IpcError> {
    hosts::list_accounts(&store)
}

#[tauri::command]
pub async fn add_host(
    args: AddHostArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    hosts::add_host(args, &store, &ssh).await
}

#[tauri::command]
pub async fn probe_ssh_alias(
    args: ProbeSshAliasArgs,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<ProbePreview, IpcError> {
    hosts::probe_ssh_alias(args, &ssh, &reg).await
}

#[tauri::command]
pub async fn probe_host(
    args: HostAliasArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<HostRow, IpcError> {
    hosts::probe_host(args, &store, &ssh, &reg).await
}

#[tauri::command]
pub fn remove_host(
    args: HostAliasArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    hosts::remove_host(args, &store)
}

#[tauri::command]
pub fn hide_host(
    args: HideHostArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    hosts::hide_host(args, &store)
}
