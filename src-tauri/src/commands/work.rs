//! Tauri commands for a session's work links (roadmap M1b.2): read them, and
//! link / reject / unlink. Thin wrappers over `service::work`.
//!
//! Seven commands map onto two hub tools: `session_work_links`,
//! `work_resume_plan` and `work_purge_impact` → `work`, and the three
//! decisions plus `resume_work` → `work_link`, with the action filled in
//! here, so a paired desktop decides and resumes on the hub exactly as a
//! local one does here (work graph M2.4 added the resume half).

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::work::resume::ResumePlan;
use fleet_core::service::work::{self, PurgeImpact, WorkArgs, WorkLinkArgs};
use fleet_core::ssh::SshClient;
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

/// What a resume of a work key would do, and which modes are possible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkResumePlanArgs {
    pub key: String,
    #[serde(default)]
    pub link_id: Option<i64>,
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Include the handover brief (one git probe on the landing host).
    #[serde(default)]
    pub with_brief: bool,
}

/// Resume past work: `mode` `last` | `brief` | `fresh`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeWorkArgs {
    pub key: String,
    pub mode: String,
    #[serde(default)]
    pub link_id: Option<i64>,
    #[serde(default)]
    pub host_alias: Option<String>,
    /// The brief as the person edited it in the preview.
    #[serde(default)]
    pub brief: Option<String>,
}

/// The work keys a purge of `project_id` on `host_aliases` strands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkPurgeImpactArgs {
    pub project_id: i64,
    #[serde(default)]
    pub host_aliases: Vec<String>,
}

#[tauri::command]
pub async fn work_resume_plan(
    args: WorkResumePlanArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ResumePlan, IpcError> {
    routed::work_resume_plan(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn resume_work(
    args: ResumeWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    routed::resume_work(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn work_purge_impact(
    args: WorkPurgeImpactArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<PurgeImpact, IpcError> {
    routed::work_purge_impact(&backend, args, &store).await
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
            session_id: Some(args.session_id),
            action: "link".into(),
            key: args.key,
            item_id: args.item_id,
            link_id: None,
            source: Some("manual".into()),
            ..Default::default()
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
            session_id: Some(args.session_id),
            action: "reject".into(),
            key: args.key,
            item_id: args.item_id,
            link_id: None,
            source: None,
            ..Default::default()
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
            session_id: Some(args.session_id),
            action: "unlink".into(),
            key: None,
            item_id: None,
            link_id: Some(args.link_id),
            source: None,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("unlink_session_work", &args).await,
            None => work::work_link(&args, store),
        }
    }

    pub async fn work_resume_plan(
        backend: &FleetBackend,
        args: WorkResumePlanArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ResumePlan, IpcError> {
        let args = WorkArgs {
            key: Some(args.key),
            action: Some("resume_plan".into()),
            link_id: args.link_id,
            host_alias: args.host_alias,
            with_brief: Some(args.with_brief),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_resume_plan", &args).await,
            None => work::work_resume_plan(&args, store, ssh).await,
        }
    }

    pub async fn resume_work(
        backend: &FleetBackend,
        args: ResumeWorkArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            action: "resume".into(),
            key: Some(args.key),
            link_id: args.link_id,
            mode: Some(args.mode),
            host_alias: args.host_alias,
            brief: args.brief,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("resume_work", &args).await,
            None => work::work_resume(&args, store, ssh, reg).await,
        }
    }

    pub async fn work_purge_impact(
        backend: &FleetBackend,
        args: WorkPurgeImpactArgs,
        store: &Mutex<Store>,
    ) -> Result<PurgeImpact, IpcError> {
        let args = WorkArgs {
            action: Some("purge_impact".into()),
            project_id: Some(args.project_id),
            host_aliases: Some(args.host_aliases),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_purge_impact", &args).await,
            None => work::work_purge_impact(&args, store),
        }
    }
}
