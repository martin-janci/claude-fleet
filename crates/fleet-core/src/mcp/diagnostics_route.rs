//! `/agent-diagnostics`: the `fleet-diagnostics` MCP server a fleet agent's
//! run reads the fleet's own logs and state through
//! (`service::fleet_agents::diagnostics`).
//!
//! Outside `authorize`, like `/agent-records`: no hub token reaches it and
//! none is asked for. Its one credential is a run token, issued to a run
//! whose definition sets `run.diagnostics`; anything without a live one is
//! 401. Plain JSON-RPC over POST, no session, no stream.

use super::records_route::bearer;
use super::report_route::ReportState;
use crate::service::fleet_agents::diagnostics;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Largest request body: a tools/call with a few filters.
pub const BODY_MAX: usize = 64 * 1024;

/// `POST /agent-diagnostics`.
pub async fn handle_diagnostics(
    State(state): State<ReportState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let now = crate::store::now_unix();
    if bearer(&headers)
        .and_then(|t| diagnostics::grant_for(t, now))
        .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let req: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(v) if v.is_object() => v,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let store = state.store_arc();
    // A tool that reads a host awaits it here; every other one reads the
    // store and the log files, off the async workers.
    if req.get("method").and_then(serde_json::Value::as_str) == Some("tools/call") {
        if let Some(id) = req.get("id") {
            let params = req.get("params").unwrap_or(&serde_json::Value::Null);
            if let Some(result) = diagnostics::call_host(&store, state.ssh(), params, now).await {
                return Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                    .into_response();
            }
        }
    }
    let answer = tokio::task::spawn_blocking(move || {
        diagnostics::answer(&store, &diagnostics::Sources::live(), &req, now)
    })
    .await;
    match answer {
        Ok(Some(v)) => Json(v).into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
