//! `POST /update/check` and `POST /update/report` (update-channel design §6):
//! the frozen update wire, `update_proto: 1`. Behind `authorize` like
//! `/report`: the credential says who is asking (`service::update::identity`),
//! never the body. Outside MCP on purpose — the one surface a client whose
//! contract revision the hub no longer matches can still reach (U4).

use super::auth::{Caller, TokenMode};
use super::report_route::ReportState;
use crate::ipc_error::{codes, IpcError};
use crate::service::update;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

/// Requests are a few hundred bytes; this is generous.
pub const BODY_MAX: usize = 64 * 1024;

fn refusal(e: IpcError) -> Response {
    let status = match e.code.as_str() {
        codes::E_FORBIDDEN => StatusCode::FORBIDDEN,
        codes::E_INVALID | codes::E_UNSUPPORTED => StatusCode::BAD_REQUEST,
        _ => {
            tracing::error!(code = %e.code, error = %e.message, "[update] failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        Json(serde_json::json!({"code": e.code, "message": e.message})),
    )
        .into_response()
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, IpcError> {
    serde_json::from_slice(body)
        .map_err(|e| IpcError::new(codes::E_INVALID, format!("unreadable update request: {e}")))
}

fn refuses_peer(caller: &Caller) -> Option<Response> {
    // A hub link's only door is `peer_exchange`; the updater's is this one.
    (caller.mode == TokenMode::Peer).then(|| {
        refusal(IpcError::new(
            codes::E_FORBIDDEN,
            "a hub link may call peer_exchange only",
        ))
    })
}

pub async fn handle_check(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(r) = refuses_peer(&caller) {
        return r;
    }
    let req = match parse(&body) {
        Ok(r) => r,
        Err(e) => return refusal(e),
    };
    let keys = update::trusted_keys();
    match update::check(
        state.store(),
        &caller,
        &req,
        &keys,
        crate::store::now_unix(),
    ) {
        Ok(d) => Json(d).into_response(),
        Err(e) => refusal(e),
    }
}

pub async fn handle_report(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(r) = refuses_peer(&caller) {
        return r;
    }
    let report = match parse(&body) {
        Ok(r) => r,
        Err(e) => return refusal(e),
    };
    match update::report(state.store(), &caller, &report, crate::store::now_unix()) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => refusal(e),
    }
}
