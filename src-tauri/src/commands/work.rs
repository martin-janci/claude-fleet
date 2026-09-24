//! Tauri commands for a session's work links (roadmap M1b.2): read them, and
//! link / reject / unlink. Thin wrappers over `service::work`.
//!
//! Four commands map onto two hub tools: `session_work_links` → `work`, and
//! the three decisions → `work_link` with the action filled in here, so a
//! paired desktop decides on the hub exactly as a local one decides here.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::work::{self, WorkArgs, WorkLinkArgs};
use fleet_core::store::{SessionRow, Store, WorkLinkRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Link a session to a key or an item; it becomes the session's primary work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinkSessionWorkArgs {
    pub session_id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub item_id: Option<i64>,
}

/// Say a session does NOT work on a key or item (sticky).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RejectSessionWorkArgs {
    pub session_id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub item_id: Option<i64>,
}

/// Remove one live link of a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnlinkSessionWorkArgs {
    pub session_id: i64,
    pub link_id: i64,
}

/// A session's live links (`session_id`), or the ended links to a key (`key`).
#[tauri::command]
pub async fn session_work_links(
    args: WorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorkLinkRow>, IpcError> {
    routed::session_work_links(&backend, args, &store).await
}

/// A person links the session from the desktop: `source` is `manual`.
#[tauri::command]
pub async fn link_session_work(
    args: LinkSessionWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::link_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn reject_session_work(
    args: RejectSessionWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::reject_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn unlink_session_work(
    args: UnlinkSessionWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::unlink_session_work(&backend, args, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn session_work_links(
        backend: &FleetBackend,
        args: WorkArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorkLinkRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_work_links", &args).await,
            None => work::work(&args, store),
        }
    }

    pub async fn link_session_work(
        backend: &FleetBackend,
        args: LinkSessionWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            session_id: args.session_id,
            action: "link".into(),
            key: args.key,
            item_id: args.item_id,
            link_id: None,
            source: Some("manual".into()),
        };
        match backend.hub() {
            Some(hub) => hub.route("link_session_work", &args).await,
            None => work::work_link(&args, store),
        }
    }

    pub async fn reject_session_work(
        backend: &FleetBackend,
        args: RejectSessionWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            session_id: args.session_id,
            action: "reject".into(),
            key: args.key,
            item_id: args.item_id,
            link_id: None,
            source: None,
        };
        match backend.hub() {
            Some(hub) => hub.route("reject_session_work", &args).await,
            None => work::work_link(&args, store),
        }
    }

    pub async fn unlink_session_work(
        backend: &FleetBackend,
        args: UnlinkSessionWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            session_id: args.session_id,
            action: "unlink".into(),
            key: None,
            item_id: None,
            link_id: Some(args.link_id),
            source: None,
        };
        match backend.hub() {
            Some(hub) => hub.route("unlink_session_work", &args).await,
            None => work::work_link(&args, store),
        }
    }
}
