//! Declarative pages (design
//! `docs/superpowers/specs/2026-09-28-declarative-pages-design.md`, P3): the
//! compiled-in page specs and the data sources they name, and the fleet's
//! settings they edit (`describe_fleet_settings` / `set_fleet_setting`, the
//! proposals and their history). Logic lives in `fleet_core::pages` and
//! `fleet_core::service::settings{,_review}`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{lock, IpcError};
use fleet_core::pages::{self, sources};
use fleet_core::service::{guides, settings_review};
use fleet_core::store::Store;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tauri::State;

/// The page specs this build ships. Compiled into the binary, so the answer
/// is the same whether or not the app is paired with a hub.
#[tauri::command]
pub fn list_pages() -> pages::PagesBundle {
    pages::bundle()
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

// ── the fleet's settings (P1, P5; routed to the hub since P6) ───────────────
//
// Typed key/value settings behind the generated pages (the registry in
// `service::settings` owns keys, defaults and validation). On a paired
// desktop they are the hub's: every command routes to the hub's tool, which
// answers the master and a person's own paired device (a client bound to no
// org); writing and deciding also need the operator to trust the device.

/// Every registered operator setting with its effective value.
#[tauri::command]
pub async fn get_fleet_settings(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<BTreeMap<String, String>, IpcError> {
    routed::get_fleet_settings(&backend, &store).await
}

/// Every registered operator setting with its metadata (label, help, kind,
/// bounds, unit, danger, restart, ai policy) and effective value, in display
/// order: what a generated settings page renders. A JSON value on both paths,
/// so the hub's answer passes through as it came.
#[tauri::command]
pub async fn describe_fleet_settings(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::describe_fleet_settings(&backend, &store).await
}

/// Validate and persist one operator setting as the person at this desktop.
/// `E_INVALID` for an unknown key or a value of the wrong shape. Returns the
/// full effective map so the page can re-render from one source of truth.
#[tauri::command]
pub async fn set_fleet_setting(
    key: String,
    value: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<BTreeMap<String, String>, IpcError> {
    routed::set_fleet_setting(&backend, &store, key, value).await
}

/// Every pending settings proposal, with each key's value now, and whether
/// this desktop may decide them (always, standalone).
#[tauri::command]
pub async fn setting_proposals(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<settings_review::Pending, IpcError> {
    routed::setting_proposals(&backend, &store).await
}

/// A person's review: apply `accept`, reject `reject`. Each is decided on
/// its own; what could not be is in `failed`.
#[tauri::command]
pub async fn decide_setting_proposals(
    accept: Vec<i64>,
    reject: Vec<i64>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<settings_review::Decided, IpcError> {
    routed::decide_setting_proposals(&backend, &store, accept, reject).await
}

/// One setting's writes, newest first: who, when, before → after.
#[tauri::command]
pub async fn setting_history(
    key: String,
    limit: Option<i64>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<fleet_core::store::SettingAuditRow>, IpcError> {
    routed::setting_history(&backend, &store, key, limit).await
}

// ── guides (declarative pages, layout L9) ───────────────────────────────────
//
// A guide an agent proposed (`guide { propose }` on the control API) joins
// the pages once a person approves it. On a paired desktop the guides are
// the hub's, and the hub decides whether this device may approve.

/// The live guides, the proposals waiting, and whether this desktop may
/// decide them (always, standalone).
#[tauri::command]
pub async fn list_guides(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<guides::GuidesView, IpcError> {
    routed::list_guides(&backend, &store).await
}

/// Approve (or reject) one guide proposal, as the person at this desktop.
#[tauri::command]
pub async fn decide_guide(
    id: i64,
    approve: bool,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<guides::GuidesView, IpcError> {
    routed::decide_guide(&backend, &store, id, approve).await
}

/// Take a live guide off the pages.
#[tauri::command]
pub async fn remove_guide(
    page_id: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<guides::GuidesView, IpcError> {
    routed::remove_guide(&backend, &store, page_id).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::service::settings::{self, Actor};
    use serde_json::json;

    pub async fn get_fleet_settings(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<BTreeMap<String, String>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("get_fleet_settings", &json!({})).await,
            None => Ok(settings::read_all(&*lock(store)?)),
        }
    }

    pub async fn describe_fleet_settings(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("describe_fleet_settings", &json!({ "describe": true }))
                    .await
            }
            None => serde_json::to_value(settings::describe(&*lock(store)?)).map_err(|e| {
                IpcError::new(fleet_core::ipc_error::codes::E_SERIALIZE, e.to_string())
            }),
        }
    }

    pub async fn set_fleet_setting(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        key: String,
        value: String,
    ) -> Result<BTreeMap<String, String>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("set_fleet_setting", &json!({ "key": key, "value": value }))
                    .await
            }
            None => {
                let s = lock(store)?;
                settings::set_by(&s, &key, &value, Actor::Person, None)?;
                Ok(settings::read_all(&s))
            }
        }
    }

    pub async fn setting_proposals(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<settings_review::Pending, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("setting_proposals", &json!({})).await,
            None => Ok(settings_review::Pending {
                can_write: true,
                proposals: settings_review::pending(&*lock(store)?)?,
            }),
        }
    }

    pub async fn decide_setting_proposals(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        accept: Vec<i64>,
        reject: Vec<i64>,
    ) -> Result<settings_review::Decided, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "decide_setting_proposals",
                    &json!({ "accept": accept, "reject": reject }),
                )
                .await
            }
            None => settings_review::decide(&*lock(store)?, &accept, &reject),
        }
    }

    pub async fn list_guides(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<guides::GuidesView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_guides", &json!({ "action": "list" })).await,
            None => guides::view(&*lock(store)?, true),
        }
    }

    pub async fn decide_guide(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        id: i64,
        approve: bool,
    ) -> Result<guides::GuidesView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "decide_guide",
                    &json!({ "action": "decide", "id": id, "approve": approve }),
                )
                .await
            }
            None => guides::decide(&*lock(store)?, id, approve, Actor::Person),
        }
    }

    pub async fn remove_guide(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        page_id: String,
    ) -> Result<guides::GuidesView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "remove_guide",
                    &json!({ "action": "remove", "page_id": page_id }),
                )
                .await
            }
            None => guides::remove(&*lock(store)?, &page_id, Actor::Person),
        }
    }

    pub async fn setting_history(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        key: String,
        limit: Option<i64>,
    ) -> Result<Vec<fleet_core::store::SettingAuditRow>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("setting_history", &json!({ "key": key, "limit": limit }))
                    .await
            }
            None => settings_review::history(&*lock(store)?, &key, limit),
        }
    }
}
