//! `/agent-records`: the `fleet-records` MCP server a fleet agent's run
//! reads its fleet stores through (`service::fleet_agents::records`).
//!
//! Outside `authorize`, like `/pair`: no hub token reaches it and none is
//! asked for. Its one credential is a run token, which opens the
//! namespaces of one rule for one run and nothing else; anything without a
//! live one is 401. Plain JSON-RPC over POST (the streamable-HTTP
//! transport's JSON answer), no session, no stream.

use super::report_route::ReportState;
use crate::service::fleet_agents::records;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Largest request body: a tools/call with a key and a text.
pub const BODY_MAX: usize = 64 * 1024;

pub(super) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// How long an authorised caller has to send its (small) body.
pub(super) const BODY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The body of a request whose credential has already been checked: at most
/// `max` bytes within [`BODY_DEADLINE`]. These routes sit outside
/// `authorize`, and an extractor would have read (and waited for) the body
/// of any caller before the token was looked at: a connection that sent a
/// `Content-Length` and then nothing held its slot under the hub's
/// connection cap for good.
pub(super) async fn body_after_auth(
    body: axum::body::Body,
    max: usize,
) -> Result<axum::body::Bytes, StatusCode> {
    match tokio::time::timeout(BODY_DEADLINE, axum::body::to_bytes(body, max)).await {
        Ok(Ok(b)) => Ok(b),
        Ok(Err(_)) => Err(StatusCode::PAYLOAD_TOO_LARGE),
        Err(_) => Err(StatusCode::REQUEST_TIMEOUT),
    }
}

/// `POST /agent-records`.
pub async fn handle_records(
    State(state): State<ReportState>,
    request: axum::extract::Request,
) -> Response {
    let now = crate::store::now_unix();
    let Some(grant) = bearer(request.headers()).and_then(|t| records::grant_for(t, now)) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let body = match body_after_auth(request.into_body(), BODY_MAX).await {
        Ok(b) => b,
        Err(code) => return code.into_response(),
    };
    let req: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(v) if v.is_object() => v,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    match records::answer(state.store(), &grant, &req, now) {
        Some(v) => Json(v).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}
