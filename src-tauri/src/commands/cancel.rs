//! Cancel an in-flight IPC call by its `call_id` (see `cancel.rs`).

use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn cancel_command(
    call_id: u64,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<(), IpcError> {
    reg.cancel(call_id);
    Ok(())
}
