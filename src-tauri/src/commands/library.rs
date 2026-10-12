//! Tauri IPC wrappers for Control's Library (Orbit Fleet 9.7,
//! `service::library`): the files a person put on a host, indexed on the
//! machine that owns the fleet (this one standalone, the hub when paired).
//!
//! All three route to the hub tool `library`, by action. The bytes never
//! pass through here: Upload puts them on the host with `upload_attachments`
//! over this machine's own ssh, then records them with `add_library_items`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::library::{self, AddArgs, LibraryList, ListArgs};
use fleet_core::service::view_scope::ViewScope;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// The Library's own files, newest first.
#[tauri::command]
pub async fn list_library(
    args: ListArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LibraryList, IpcError> {
    routed::list_library(&backend, args, &store).await
}

/// Record files already put beside a session.
#[tauri::command]
pub async fn add_library_items(
    args: AddArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LibraryList, IpcError> {
    routed::add_library_items(&backend, args, &store).await
}

/// Drop a row from the Library (the file stays on the host).
#[tauri::command]
pub async fn remove_library_item(
    id: i64,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    routed::remove_library_item(&backend, id, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn list_library(
        backend: &FleetBackend,
        args: ListArgs,
        store: &Mutex<Store>,
    ) -> Result<LibraryList, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_library(&args).await,
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                library::list(&s, &ViewScope::internal(), &args)
            }
        }
    }

    pub async fn add_library_items(
        backend: &FleetBackend,
        args: AddArgs,
        store: &Mutex<Store>,
    ) -> Result<LibraryList, IpcError> {
        match backend.hub() {
            Some(hub) => hub.add_library_items(&args).await,
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                let items = library::add(&s, &ViewScope::internal(), &args)?;
                Ok(LibraryList { items })
            }
        }
    }

    pub async fn remove_library_item(
        backend: &FleetBackend,
        id: i64,
        store: &Mutex<Store>,
    ) -> Result<bool, IpcError> {
        match backend.hub() {
            Some(hub) => hub.remove_library_item(id).await,
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                library::remove(&s, &ViewScope::internal(), id)
            }
        }
    }
}
