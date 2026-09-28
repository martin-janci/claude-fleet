//! Declarative pages (design
//! `docs/superpowers/specs/2026-09-28-declarative-pages-design.md`, P3): the
//! compiled-in page specs and the data sources they name. Logic lives in
//! `fleet_core::pages`; the settings a page edits go through the existing
//! `describe_fleet_settings` / `set_fleet_setting`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{lock, IpcError};
use fleet_core::pages::{self, resources, sources, Page};
use fleet_core::store::Store;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Every page spec and the declared shape of every data source, so the
/// renderer can lay a page out and format a column before any data arrives.
#[derive(Serialize)]
pub struct PagesBundle {
    pub pages: &'static [Page],
    pub sources: &'static [sources::SourceSpec],
    pub resources: &'static [resources::ResourceType],
}

/// The page specs this build ships. Compiled into the binary, so the answer
/// is the same whether or not the app is paired with a hub.
#[tauri::command]
pub fn list_pages() -> PagesBundle {
    PagesBundle {
        pages: pages::all(),
        sources: sources::SOURCES,
        resources: resources::RESOURCES,
    }
}

/// Run one data source with a page's literal parameters. `E_INVALID` for an
/// unknown source or a parameter it does not take.
#[tauri::command]
pub fn fetch_page_source(
    id: String,
    params: Option<serde_json::Map<String, serde_json::Value>>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    backend.refuse_local_only("fetch_page_source")?;
    let s = lock(&store)?;
    sources::fetch(
        &s,
        &id,
        &params.unwrap_or_default(),
        fleet_core::store::now_unix(),
    )
}

// ── flows (declarative pages P4b, layout L3) ─────────────────────────────────
//
// The backend decides every step (`fleet_core::pages::flows`); these only
// carry the step and the values. Local-only: the one flow today adds a
// tracker, fleet administration a paired desktop does not own.

/// Open `flow`, prefilled from `prefill`.
#[tauri::command]
pub fn flow_start(
    flow: String,
    prefill: Option<std::collections::BTreeMap<String, String>>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::pages::flows::Step, IpcError> {
    backend.refuse_local_only("flow_start")?;
    fleet_core::pages::flows::start(&store, &flow, &prefill.unwrap_or_default())
}

/// Submit the open step's values: the next step, the same one with an
/// error, or done. A secret in `values` is used here and kept nowhere.
#[tauri::command]
pub async fn flow_submit(
    flow_id: String,
    values: std::collections::BTreeMap<String, String>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::pages::flows::Outcome, IpcError> {
    backend.refuse_local_only("flow_submit")?;
    fleet_core::pages::flows::submit(
        &store,
        &fleet_core::service::trackers::default_net(),
        &flow_id,
        &values,
    )
    .await
}

/// Back to the first step, what was entered kept.
#[tauri::command]
pub fn flow_back(
    flow_id: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::pages::flows::Step, IpcError> {
    backend.refuse_local_only("flow_back")?;
    fleet_core::pages::flows::back(&store, &flow_id)
}

/// Close a flow nobody will finish.
#[tauri::command]
pub fn flow_cancel(flow_id: String, backend: State<'_, Arc<FleetBackend>>) -> Result<(), IpcError> {
    backend.refuse_local_only("flow_cancel")?;
    fleet_core::pages::flows::cancel(&flow_id);
    Ok(())
}
