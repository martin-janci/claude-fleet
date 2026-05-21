//! Tauri IPC wrapper for the health-check command. The logic lives in
//! `service::health`; this file only adapts `tauri::State` to plain references.

use crate::service::health::{self, Health};
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn health_check(store: State<'_, Arc<Mutex<Store>>>) -> Health {
    health::health_check(&store)
}
