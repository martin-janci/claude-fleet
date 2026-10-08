//! Presence on the desktop (redesign step 11.7b): who else has the open
//! session on screen. Paired, the hub's `session_presence` tool, where every
//! client reports; standalone there is nobody else to report, so the answer
//! is empty and the header shows no one.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::presence::{PresenceView, SessionPresenceArgs, HEARTBEAT_SECS};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn session_presence(
    args: SessionPresenceArgs,
    backend: State<'_, Arc<FleetBackend>>,
) -> Result<PresenceView, IpcError> {
    routed::session_presence(&backend, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn session_presence(
        backend: &FleetBackend,
        args: SessionPresenceArgs,
    ) -> Result<PresenceView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_presence", &args).await,
            None => Ok(PresenceView {
                session_id: args.session_id,
                viewers: Vec::new(),
                heartbeat_secs: HEARTBEAT_SECS,
            }),
        }
    }
}
