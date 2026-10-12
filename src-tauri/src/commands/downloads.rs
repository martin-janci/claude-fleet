//! Tauri IPC wrappers for file downloads (`service::downloads`): a file a
//! session sent from its host, kept on the machine that owns the fleet —
//! this one standalone, the hub when paired.
//!
//! `list_downloads`, `send_file`, `pause_download` and `remove_download` route to the hub
//! tools of the same name. `save_download` routes its check (the row,
//! through `list_downloads`) and then streams the bytes from the hub's
//! `GET /downloads/<id>`; standalone it copies them out of this machine's
//! data dir. The destination is always picked in this process's own save
//! dialog, never passed in by the page.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::downloads::{
    self, DownloadList, ListDownloadsArgs, PauseDownloadArgs, SendFileArgs,
};
use fleet_core::service::view_scope::ViewScope;
use fleet_core::ssh::{SshClient, SshExec};
use fleet_core::store::{DownloadRow, Store};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Files sent to this fleet's devices, newest first.
#[tauri::command]
pub async fn list_downloads(
    args: ListDownloadsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DownloadList, IpcError> {
    routed::list_downloads(&backend, args, &store).await
}

/// Copy a file from a session's host to the downloads (the file viewer's
/// "Send to downloads"). Answers the row in state `fetching`.
#[tauri::command]
pub async fn send_file(
    args: SendFileArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<DownloadRow, IpcError> {
    routed::send_file(&backend, args, &store, &ssh).await
}

/// Pause a copy in flight, or let it go on (gap plan G7.15). Answers the
/// row as `list_downloads` shows it.
#[tauri::command]
pub async fn pause_download(
    args: PauseDownloadArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<DownloadRow, IpcError> {
    routed::pause_download(&backend, args, &store).await
}

/// Remove a download and its copy.
#[tauri::command]
pub async fn remove_download(
    id: i64,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    routed::remove_download(&backend, id, &store).await
}

/// Save a ready download where the person picks (a save dialog opened at
/// their Downloads folder). The path written, or `None` when the dialog was
/// cancelled.
#[tauri::command]
pub async fn save_download(
    id: i64,
    app: tauri::AppHandle,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<String>, IpcError> {
    routed::save_download(&backend, id, &store, |name| pick_destination(&app, name)).await
}

/// The save dialog, at the Downloads folder with the file's own name.
async fn pick_destination(app: &tauri::AppHandle, name: String) -> Option<PathBuf> {
    use tauri::Manager;
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save download")
        .set_file_name(&name);
    if let Ok(dir) = app.path().download_dir() {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten().and_then(|p| p.into_path().ok())
}

pub(crate) mod routed {
    use super::*;

    pub async fn list_downloads(
        backend: &FleetBackend,
        args: ListDownloadsArgs,
        store: &Mutex<Store>,
    ) -> Result<DownloadList, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_downloads(&args).await,
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                downloads::list(&s, &ViewScope::internal(), &args)
            }
        }
    }

    pub async fn send_file(
        backend: &FleetBackend,
        args: SendFileArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<DownloadRow, IpcError> {
        if let Some(hub) = backend.hub() {
            return hub.send_file(&args).await;
        }
        let row = downloads::send(
            store,
            &**ssh,
            &ViewScope::internal(),
            downloads::SOURCE_PERSON,
            &args,
        )
        .await?;
        let exec: Arc<dyn SshExec> = ssh.clone();
        downloads::spawn_fetch(store.clone(), exec, row.clone());
        Ok(row)
    }

    pub async fn pause_download(
        backend: &FleetBackend,
        args: PauseDownloadArgs,
        store: &Mutex<Store>,
    ) -> Result<DownloadRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.pause_download(&args).await,
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                downloads::pause(&s, &ViewScope::internal(), &args)
            }
        }
    }

    pub async fn remove_download(
        backend: &FleetBackend,
        id: i64,
        store: &Mutex<Store>,
    ) -> Result<bool, IpcError> {
        match backend.hub() {
            Some(hub) => hub.remove_download(id).await,
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                downloads::remove(&s, &ViewScope::internal(), id)
            }
        }
    }

    /// `pick` asks where to write, given the file's name.
    pub async fn save_download<F, Fut>(
        backend: &FleetBackend,
        id: i64,
        store: &Mutex<Store>,
        pick: F,
    ) -> Result<Option<String>, IpcError>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Option<PathBuf>>,
    {
        match backend.hub() {
            Some(hub) => {
                let row = hub.ready_download(id).await?;
                let Some(dest) = pick(row.name.clone()).await else {
                    return Ok(None);
                };
                // A little over the row's size: the hub serves exactly that
                // many bytes, and anything past it is not this file.
                let max = (row.size.max(0) as u64).saturating_add(1);
                hub.fetch_download(id, &dest, max).await?;
                Ok(Some(dest.display().to_string()))
            }
            None => {
                let (row, src) = {
                    let s = fleet_core::ipc_error::lock(store)?;
                    downloads::open_ready(&s, &ViewScope::internal(), id)?
                };
                let Some(dest) = pick(row.name.clone()).await else {
                    return Ok(None);
                };
                tokio::fs::copy(&src, &dest).await.map_err(|e| {
                    IpcError::new(
                        codes::E_IO,
                        format!("could not save to {}: {e}", dest.display()),
                    )
                })?;
                Ok(Some(dest.display().to_string()))
            }
        }
    }
}
