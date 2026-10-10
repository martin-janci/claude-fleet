//! `POST /update/check` and `POST /update/report` (update-channel design §6):
//! the frozen update wire, `update_proto: 1`. Behind `authorize` like
//! `/report`: the credential says who is asking (`service::update::identity`),
//! never the body; a hub link's token is refused there (`E_FORBIDDEN`, 403),
//! since its only door is `peer_exchange`. Outside MCP on purpose — the one surface a client whose
//! contract revision the hub no longer matches can still reach (U4).

use super::auth::Caller;
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

pub async fn handle_check(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> Response {
    // Who is asking comes before what it asks: a hub link is refused (403)
    // whatever its body says.
    if let Err(e) = update::identity(&caller) {
        return refusal(e);
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

/// `GET /update/artifact/<sha256>`: a release file from the hub's mirror
/// (`update.mirror`, S9), for a target that cannot reach GitHub. Any
/// credential that may ask `/update/check` may fetch; a hub link may not.
/// 404 for a sha no verified manifest lists, or with the mirror off.
pub async fn handle_artifact(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    axum::extract::Path(sha256): axum::extract::Path<String>,
) -> Response {
    if let Err(e) = update::identity(&caller) {
        return refusal(e);
    }
    let fetch = update::HttpsFetch::new(Some(&update::channel_base_url()));
    let path = match update::mirror::local_copy(
        state.store(),
        &fetch,
        &update::trusted_keys(),
        &sha256,
        crate::store::now_unix(),
    )
    .await
    {
        Ok(p) => p,
        Err(e) if e.code == codes::E_NOTFOUND => {
            return (StatusCode::NOT_FOUND, "no such artifact\n").into_response()
        }
        Err(e) => {
            tracing::warn!(code = %e.code, error = %e.message, "[update] mirror fetch failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"code": e.code, "message": e.message})),
            )
                .into_response();
        }
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return (StatusCode::NOT_FOUND, "no such artifact\n").into_response(),
    };
    let len = match file.metadata().await {
        Ok(m) => m.len(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut resp = Response::new(axum::body::Body::from_stream(
        tokio_util::io::ReaderStream::new(file),
    ));
    let h = resp.headers_mut();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(
        axum::http::header::CONTENT_LENGTH,
        axum::http::HeaderValue::from(len),
    );
    if let Ok(v) = axum::http::HeaderValue::from_str(&sha256) {
        h.insert("x-fleet-sha256", v);
    }
    resp
}

pub async fn handle_report(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> Response {
    // Who is asking comes before what it asks: a hub link is refused (403)
    // whatever its body says.
    if let Err(e) = update::identity(&caller) {
        return refusal(e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::auth::{ClientRef, TokenMode};

    #[test]
    fn a_hub_link_is_refused_with_403() {
        let e = update::identity(&peer()).unwrap_err();
        assert_eq!(refusal(e).status(), StatusCode::FORBIDDEN);
    }

    fn peer() -> Caller {
        Caller {
            api: None,
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "hub-b".into(),
                trusted: false,
                org_id: None,
                person_id: None,
            }),
            mode: TokenMode::Peer,
            pane: None,
            is_personal_owner: false,
        }
    }

    /// The refusal comes before the body is read: a hub link that sends
    /// garbage or an unsupported `update_proto` is still refused, not
    /// answered with a parse or proto error.
    #[tokio::test]
    async fn a_hub_link_is_refused_before_its_body_is_read() {
        use std::sync::{Arc, Mutex};
        let store = Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
        let state = || ReportState::new(Arc::clone(&store));
        for body in ["{", r#"{"update_proto":0}"#, r#"{"update_proto":99}"#] {
            let bytes = axum::body::Bytes::from_static(body.as_bytes());
            let r = handle_check(State(state()), Extension(peer()), bytes.clone()).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "check {body:?}");
            let r = handle_report(State(state()), Extension(peer()), bytes).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "report {body:?}");
        }
    }
}
