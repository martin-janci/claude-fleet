//! Tauri IPC wrappers for the composer's chip row. Logic (defaults,
//! validation, storage) lives in `service::quick_replies`; these only adapt
//! `tauri::State`.
//!
//! Both route in remote mode. The chips are fleet state, not this machine's
//! preference — that is the whole point of moving them off `localStorage` —
//! so a desktop paired to a hub must edit the hub's list and not a private
//! one that would silently disagree with the phone's.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::quick_replies::{self, QuickReply};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// The fleet's chips, in order. Built-in defaults when none were saved.
#[tauri::command]
pub async fn quick_replies(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<QuickReply>, IpcError> {
    routed::quick_replies(&backend, &store).await
}

/// Replace the whole list (`[]` restores the defaults). Returns the list as
/// stored, which is what every client will read next.
#[tauri::command]
pub async fn set_quick_replies(
    entries: Vec<QuickReply>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<QuickReply>, IpcError> {
    routed::set_quick_replies(&backend, entries, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn quick_replies(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<QuickReply>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.quick_replies().await,
            None => quick_replies::list(store),
        }
    }

    pub async fn set_quick_replies(
        backend: &FleetBackend,
        entries: Vec<QuickReply>,
        store: &Mutex<Store>,
    ) -> Result<Vec<QuickReply>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.set_quick_replies(entries).await,
            None => quick_replies::replace(store, entries),
        }
    }
}
