//! Hook service stub — real implementation in Task 2.

use crate::ipc_error::IpcError;
use crate::mcp::hooks::HookPayload;
use crate::store::Store;
use std::sync::{Arc, Mutex};

pub fn apply_hook(_store: &Arc<Mutex<Store>>, _payload: &HookPayload) -> Result<(), IpcError> {
    Ok(()) // stub — real implementation in Task 2
}
