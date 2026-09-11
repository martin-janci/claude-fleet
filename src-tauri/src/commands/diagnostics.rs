//! Tauri IPC wrappers for Settings → Diagnostics. The report itself is built
//! in `service::diagnostics`; this file only gathers managed state. Not
//! exposed as MCP tools (so control-api-reference.md needs no regeneration).

use crate::ipc_error::IpcError;
use crate::mcp::McpRuntime;
use crate::service::diagnostics::{self, DiagnosticsBundle, DiagnosticsInputs};
use crate::service::tunnel::TunnelSupervisor;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Build the redacted plain-text diagnostics bundle (see
/// `service::diagnostics`). Reads cached state only; no network.
#[tauri::command]
pub fn collect_diagnostics(
    store: State<'_, Arc<Mutex<Store>>>,
    tunnels: State<'_, Arc<TunnelSupervisor>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<DiagnosticsBundle, IpcError> {
    let data_dir = crate::appdata_dir();
    let log_dir = crate::logging::log_dir_in(&data_dir);
    let (mcp_running, mcp_bind_error) = {
        let rt = runtime.lock().map_err(|_| IpcError::lock())?;
        (rt.is_running(), rt.last_error().map(str::to_string))
    };
    diagnostics::collect(
        &store,
        DiagnosticsInputs {
            data_dir: &data_dir,
            log_dir: &log_dir,
            tunnels: tunnels.snapshot(),
            mcp_running,
            mcp_bind_error,
        },
    )
}

/// Open the log folder in the OS file manager. Returns the folder path so
/// the UI can also show it (and offer a copy) when opening fails.
#[tauri::command]
pub fn open_log_folder(app: tauri::AppHandle) -> Result<String, IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let dir = crate::logging::log_dir_in(&crate::appdata_dir());
    std::fs::create_dir_all(&dir)?;
    let path = dir.display().to_string();
    app.opener()
        .open_path(path.clone(), None::<&str>)
        .map_err(|e| {
            IpcError::new(
                crate::ipc_error::codes::E_IO,
                format!("could not open {path}: {e}"),
            )
        })?;
    Ok(path)
}
