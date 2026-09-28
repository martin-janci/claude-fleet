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
