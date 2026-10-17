//! `POST /log`: a desktop app or phone asks whether its own log is wanted,
//! and sends it when it is (`service::client_logs`). Behind `authorize`,
//! like `/report`: any client token, `readonly` included, since sending a
//! log changes nothing about the fleet; a hub link is refused.

use super::auth::Caller;
use super::report_route::ReportState;
use crate::service::client_logs::{hub_logs, LogPost};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

/// Largest body: [`crate::service::client_logs::UPLOAD_MAX_LINES`] lines of
/// a redacted log, most far shorter than their cap.
pub const BODY_MAX: usize = 1024 * 1024;

/// `POST /log`.
pub async fn handle_log(
    State(_state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> Response {
    if crate::mcp::auth::refuses_peer(&caller).is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let post: LogPost = if body.iter().all(u8::is_ascii_whitespace) {
        LogPost::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(p) => p,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    };
    let Ok(mut logs) = hub_logs().lock() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    match logs.post(&caller.label(), post, crate::store::now_unix()) {
        Some(want) => Json(want).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}
