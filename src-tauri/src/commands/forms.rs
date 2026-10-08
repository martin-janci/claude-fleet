//! Chat forms on the desktop: the forms an agent asked, and the person's
//! answer or decline. Standalone they run here; paired, on the hub's `ask`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{lock, IpcError};
use fleet_core::service::forms::{self, FormView};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};
use tauri::State;

/// How an answer from this desktop reads to the agent.
const BY_DESKTOP: &str = "you (desktop)";

#[tauri::command]
pub async fn list_forms(
    session_id: Option<i64>,
    state: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<FormView>, IpcError> {
    routed::list_forms(&backend, &store, session_id, state).await
}

#[tauri::command]
pub async fn get_form(
    form_id: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<FormView, IpcError> {
    routed::get_form(&backend, &store, form_id).await
}

#[tauri::command]
pub async fn answer_form(
    form_id: String,
    values: Map<String, Value>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FormView, IpcError> {
    routed::answer_form(&backend, &store, &ssh, form_id, values).await
}

#[tauri::command]
pub async fn decline_form(
    form_id: String,
    note: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<FormView, IpcError> {
    routed::decline_form(&backend, &store, form_id, note).await
}

pub(crate) mod routed {
    use super::*;
    use serde_json::json;

    pub async fn list_forms(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        session_id: Option<i64>,
        state: Option<String>,
    ) -> Result<Vec<FormView>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "list_forms",
                    &json!({ "list": { "session_id": session_id, "state": state } }),
                )
                .await
            }
            None => Ok(lock(store)?
                .forms(session_id, state.as_deref())?
                .iter()
                .map(forms::view)
                .collect()),
        }
    }

    pub async fn get_form(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        form_id: String,
    ) -> Result<FormView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("get_form", &json!({ "get": form_id })).await,
            None => forms::get(store, &form_id),
        }
    }

    pub async fn answer_form(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        form_id: String,
        values: Map<String, Value>,
    ) -> Result<FormView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "answer_form",
                    &json!({ "answer": form_id, "values": values }),
                )
                .await
            }
            None => forms::answer(store, &**ssh, &form_id, &values, BY_DESKTOP).await,
        }
    }

    pub async fn decline_form(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        form_id: String,
        note: Option<String>,
    ) -> Result<FormView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("decline_form", &json!({ "decline": form_id, "note": note }))
                    .await
            }
            None => forms::decline(store, &form_id, note.as_deref(), BY_DESKTOP),
        }
    }
}
