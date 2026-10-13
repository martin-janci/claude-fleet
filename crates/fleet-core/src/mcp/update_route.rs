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
    let req: fleet_update::wire::CheckRequest = match parse(&body) {
        Ok(r) => r,
        Err(e) => return refusal(e),
    };
    if req.refresh {
        update::refresh_asked(state.store(), crate::store::now_unix()).await;
    }
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

/// `POST /update/poke`: "a release was just published" — the release
/// workflow's ping, so a `dev` hub picks a build up within seconds instead
/// of at its next poll. Unauthenticated on purpose, and it can do nothing
/// but what the poll does: re-read the channel, which is signed, verified
/// against the compiled-in release key, and fetched from the hub's own
/// configured base URL — never from anything in the request. It takes no
/// body, answers 202 at once, and the re-read itself runs at most once a
/// minute across every caller (`refresh_asked`), so a flood costs nothing.
///
/// The channel is served through a CDN that may hold the old document for up
/// to five minutes after the push, so a poke reads it now and once more
/// [`POKE_REREAD_SECS`] later; one such second read is pending at a time.
pub async fn handle_poke(State(state): State<ReportState>) -> Response {
    use std::sync::atomic::Ordering::SeqCst;
    let store = state.store_arc();
    crate::rt::spawn(async move {
        read_and_decide(&store).await;
        if POKE_REREAD_PENDING.swap(true, SeqCst) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(POKE_REREAD_SECS)).await;
        POKE_REREAD_PENDING.store(false, SeqCst);
        read_and_decide(&store).await;
    });
    StatusCode::ACCEPTED.into_response()
}

/// Past `raw.githubusercontent.com`'s `max-age=300`.
pub const POKE_REREAD_SECS: u64 = 330;
static POKE_REREAD_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Re-read the channel, then push the decisions it moves now (waking the
/// hub's own updater when its target moved) rather than at the next beat.
async fn read_and_decide(store: &std::sync::Mutex<crate::store::Store>) {
    update::refresh_asked(store, crate::store::now_unix()).await;
    update::decisions_may_have_changed();
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
    let fetch = update::HttpsFetch::for_artifacts(Some(&update::channel_base_url()));
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
            is_fleet_admin: false,
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
