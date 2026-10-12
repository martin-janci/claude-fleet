//! `GET /downloads/<id>`: the bytes of a ready file download
//! (`service::downloads`), as a plain HTTP body. A tool result could not
//! carry them: `/mcp` answers through axum's 2 MB default body and the
//! phone caps an RPC answer at 8 MiB, while a download may be 100 MB.
//!
//! Behind `authorize` like `/events`; the caller sees what `list_downloads`
//! shows it (its org's files for a bound client), and a host's own token,
//! a hub link and an updater are refused — none of them is a person
//! fetching a file.
//!
//! **Multi-user M1.** This is the SECOND route M1 fences (`/events` was the
//! first), and the only one that serves file BYTES. The gate is
//! `service::downloads::visible`, through `open_ready`, at the `own` tier:
//! the bytes are an unconstrained absolute-path read of the owner's host, so
//! a `watch` or `drive` grantee is refused them exactly as it is refused
//! `send_file`. The early `host_alias` refusal below is belt and braces —
//! `visible` refuses a per-host token on its own, which is what
//! `downloads_tests::only_the_owner_reaches_a_private_sessions_download`
//! holds, so deleting this early return would not open the route.

use super::auth::{refuses_peer, Caller};
use super::report_route::ReportState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};

pub async fn handle_download(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(refused) = refuses_peer(&caller) {
        return refused;
    }
    if caller.host_alias.is_some() {
        return (
            StatusCode::FORBIDDEN,
            "a host's token sends files; a person's device fetches them\n",
        )
            .into_response();
    }
    let opened = {
        let Ok(s) = state.store().lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        // Multi-user M1: the PERSON scope, not the org one. The bytes of a
        // file taken out of a session are that session's content (spec
        // §4.3), so this route is fenced exactly as `list_downloads` is —
        // `downloads::visible` asks `sees_session_row` on the session the
        // file came from.
        caller
            .view_scope(&s)
            .and_then(|scope| crate::service::downloads::open_ready(&s, &scope, id))
    };
    let (row, path) = match opened {
        Ok(v) => v,
        Err(e) if e.code == crate::ipc_error::codes::E_NOTFOUND => {
            return (StatusCode::NOT_FOUND, "no such download\n").into_response()
        }
        Err(e) => {
            tracing::warn!(id, code = %e.code, error = %e.message, "[downloads] open failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(id, error = %e, "[downloads] the copy is gone");
            return (StatusCode::NOT_FOUND, "no such download\n").into_response();
        }
    };
    let len = match file.metadata().await {
        Ok(m) => m.len(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    tracing::info!(id, size = len, "[downloads] served");
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(crate::service::downloads::content_type(&row.name)),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if let Ok(v) = HeaderValue::from_str(&crate::service::downloads::content_disposition(&row.name))
    {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Some(sha) = row
        .sha256
        .as_deref()
        .and_then(|s| HeaderValue::from_str(s).ok())
    {
        h.insert("x-fleet-sha256", sha);
    }
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    resp
}
