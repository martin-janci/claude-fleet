//! Tauri commands for a session's work links (roadmap M1b.2): read them, and
//! link / reject / unlink. Thin wrappers over `service::work`.
//!
//! Seven commands map onto two hub tools: `session_work_links`,
//! `work_resume_plan` and `work_purge_impact` → `work`, and the three
//! decisions plus `resume_work` → `work_link`, with the action filled in
//! here, so a paired desktop decides and resumes on the hub exactly as a
//! local one does here (work graph M2.4 added the resume half).
//!
//! Work graph M7.2 adds the lifecycle: `work_tidy` / `work_reopened` →
//! `work`, and archive / unarchive / snooze / never / tidy_apply / dismiss →
//! `work_link`.
//!
//! Work graph M11.1 adds local work ("Name this work…"):
//! `list_local_work_items` → `work { local_items }`, and `name_session_work`
//! / `rename_work_item` → `work_link { name }`.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::work::card::TicketCard;
use fleet_core::service::work::local::LocalWorkItem;
use fleet_core::service::work::resume::ResumePlan;
use fleet_core::service::work::summary::SummaryOutcome;
use fleet_core::service::work::tidy::{TidyApplyItem, TidyApplyReport, TidyReport};
use fleet_core::service::work::today::Today;
use fleet_core::service::work::{self, Dismissed, PurgeImpact, WorkArgs, WorkLinkArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::{ReopenedWork, SessionRow, Store, WorkItemRow, WorkLinkRow};
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
    /// Link work of another org anyway (work graph M5): a person saw the
    /// refusal and meant it.
    #[serde(default)]
    pub force_cross_org: bool,
}

/// Say a session does NOT work on a key or item (sticky) — or, with
/// `link_id` alone, on one detected suggestion ("Not this", work graph M4.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RejectSessionWorkArgs {
    pub session_id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub link_id: Option<i64>,
}

/// Confirm one detected suggestion: it becomes the session's primary work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfirmSessionWorkArgs {
    pub session_id: i64,
    pub link_id: i64,
    /// Confirm a suggestion of another org anyway (work graph M5).
    #[serde(default)]
    pub force_cross_org: bool,
}

/// Trust (or stop trusting) branch keys in a project (rule R3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetWorkProjectTrustArgs {
    pub project_id: i64,
    pub on: bool,
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
    /// Resume another org's work here anyway (work graph M5).
    #[serde(default)]
    pub force_cross_org: bool,
}

/// The work keys a purge of `project_id` on `host_aliases` strands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkPurgeImpactArgs {
    pub project_id: i64,
    #[serde(default)]
    pub host_aliases: Vec<String>,
}

/// Archive a live session from the UI (it collapses into its work group's
/// Done; tmux keeps running), or un-archive it — which an attach also does.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionLifecycleArgs {
    pub session_id: i64,
}

/// Snooze tidy-up for a session's work (`days`, default 7), or never
/// suggest it (`days` ignored).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TidyFlagArgs {
    pub session_id: i64,
    #[serde(default)]
    pub link_id: Option<i64>,
    #[serde(default)]
    pub days: Option<u32>,
}

/// The Tidy-up sheet's choices, one per session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TidyApplyArgs {
    pub items: Vec<TidyApplyItem>,
}

/// Dismiss a reopened item's Attention entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DismissReopenedArgs {
    pub item_id: i64,
}

/// "Name this work…" (work graph M11.1): new local work with a title (and
/// an optional key), linked to the session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NameSessionWorkArgs {
    pub session_id: i64,
    pub title: String,
    #[serde(default)]
    pub key: Option<String>,
}

/// Rename a local work item (a tracker's ticket is refused).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenameWorkItemArgs {
    pub item_id: i64,
    pub title: String,
}

/// The local work items (work graph M11.1).
#[tauri::command]
pub async fn list_local_work_items(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<LocalWorkItem>, IpcError> {
    routed::list_local_work_items(&backend, &store).await
}

/// A person names the session's work from the desktop.
#[tauri::command]
pub async fn name_session_work(
    args: NameSessionWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::name_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn rename_work_item(
    args: RenameWorkItemArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WorkItemRow, IpcError> {
    routed::rename_work_item(&backend, args, &store).await
}

/// The tidy-up candidates (work graph M7).
#[tauri::command]
pub async fn work_tidy(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TidyReport, IpcError> {
    routed::work_tidy(&backend, &store).await
}

/// Work open again that has past sessions (work graph M7).
#[tauri::command]
pub async fn work_reopened(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ReopenedWork>, IpcError> {
    routed::work_reopened(&backend, &store).await
}

#[tauri::command]
pub async fn archive_session_work(
    args: SessionLifecycleArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::archive_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn unarchive_session_work(
    args: SessionLifecycleArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::unarchive_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn snooze_tidy(
    args: TidyFlagArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::snooze_tidy(&backend, args, &store).await
}

#[tauri::command]
pub async fn never_tidy(
    args: TidyFlagArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::never_tidy(&backend, args, &store).await
}

#[tauri::command]
pub async fn tidy_apply(
    args: TidyApplyArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<TidyApplyReport, IpcError> {
    routed::tidy_apply(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn dismiss_reopened(
    args: DismissReopenedArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Dismissed, IpcError> {
    routed::dismiss_reopened(&backend, args, &store).await
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

/// Ask a live session to write its hand-off (work graph M9.3, on demand).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestWorkHandoverArgs {
    pub session_id: i64,
}

#[tauri::command]
pub async fn request_work_handover(
    args: RequestWorkHandoverArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::request_work_handover(&backend, args, &store, &ssh).await
}

/// A Claude-written summary of past work (work graph M13.1, on demand):
/// ended link `link_id` of `key`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SummarizePastWorkArgs {
    pub key: String,
    pub link_id: i64,
}

#[tauri::command]
pub async fn summarize_past_work(
    args: SummarizePastWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SummaryOutcome, IpcError> {
    routed::summarize_past_work(&backend, args, &store, &ssh).await
}

/// The Today view's digest (work graph M9.1). `since` is the viewer's local
/// midnight: the hub does not know the desktop's timezone.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkTodayArgs {
    #[serde(default)]
    pub since: Option<i64>,
}

#[tauri::command]
pub async fn work_today(
    args: WorkTodayArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Today, IpcError> {
    routed::work_today(&backend, args, &store).await
}

/// A ticket's context card from the hub's cache (work graph M9.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkTicketCardArgs {
    pub key: String,
}

#[tauri::command]
pub async fn work_ticket_card(
    args: WorkTicketCardArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TicketCard, IpcError> {
    routed::work_ticket_card(&backend, args, &store).await
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
pub async fn confirm_session_work(
    args: ConfirmSessionWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::confirm_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_work_project_trust(
    args: SetWorkProjectTrustArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<work::ProjectTrust, IpcError> {
    routed::set_work_project_trust(&backend, args, &store).await
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

    fn now() -> i64 {
        fleet_core::service::catalog::now_secs()
    }

    pub async fn work_tidy(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<TidyReport, IpcError> {
        let args = WorkArgs {
            action: Some("tidy".into()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_tidy", &args).await,
            None => work::tidy::work_tidy(store, &fleet_core::service::orgs::OrgScope::All, now()),
        }
    }

    pub async fn work_reopened(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ReopenedWork>, IpcError> {
        let args = WorkArgs {
            action: Some("reopened".into()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_reopened", &args).await,
            None => fleet_core::ipc_error::lock(store)?.reopened_work(),
        }
    }

    fn lifecycle(action: &str, session_id: i64) -> WorkLinkArgs {
        WorkLinkArgs {
            session_id: Some(session_id),
            action: action.into(),
            ..Default::default()
        }
    }

    pub async fn archive_session_work(
        backend: &FleetBackend,
        args: SessionLifecycleArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = lifecycle("archive", args.session_id);
        match backend.hub() {
            Some(hub) => hub.route("archive_session_work", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn unarchive_session_work(
        backend: &FleetBackend,
        args: SessionLifecycleArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = lifecycle("unarchive", args.session_id);
        match backend.hub() {
            Some(hub) => hub.route("unarchive_session_work", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn snooze_tidy(
        backend: &FleetBackend,
        args: TidyFlagArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            link_id: args.link_id,
            days: args.days,
            ..lifecycle("snooze", args.session_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("snooze_tidy", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn never_tidy(
        backend: &FleetBackend,
        args: TidyFlagArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            link_id: args.link_id,
            ..lifecycle("never", args.session_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("never_tidy", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn tidy_apply(
        backend: &FleetBackend,
        args: TidyApplyArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<TidyApplyReport, IpcError> {
        let wire = WorkLinkArgs {
            action: "tidy_apply".into(),
            items: Some(args.items),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("tidy_apply", &wire).await,
            None => {
                let exec = fleet_core::service::gc::RealGcExec {
                    store: Arc::clone(store),
                    ssh: Arc::clone(ssh),
                };
                work::tidy::tidy_apply(
                    store,
                    &exec,
                    wire.items.as_deref().unwrap_or_default(),
                    &fleet_core::service::orgs::OrgScope::All,
                    now(),
                )
                .await
            }
        }
    }

    pub async fn dismiss_reopened(
        backend: &FleetBackend,
        args: DismissReopenedArgs,
        store: &Mutex<Store>,
    ) -> Result<Dismissed, IpcError> {
        let args = WorkLinkArgs {
            action: "dismiss".into(),
            item_id: Some(args.item_id),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("dismiss_reopened", &args).await,
            None => work::dismiss_reopened(&args, store),
        }
    }

    pub async fn list_local_work_items(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<LocalWorkItem>, IpcError> {
        let args = WorkArgs {
            action: Some("local_items".into()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("list_local_work_items", &args).await,
            None => work::local::local_items(store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn name_session_work(
        backend: &FleetBackend,
        args: NameSessionWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            session_id: Some(args.session_id),
            action: "name".into(),
            key: args.key,
            title: Some(args.title),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("name_session_work", &args).await,
            None => work::local::name_session_work(
                &args,
                store,
                &fleet_core::service::orgs::OrgScope::All,
            ),
        }
    }

    pub async fn rename_work_item(
        backend: &FleetBackend,
        args: RenameWorkItemArgs,
        store: &Mutex<Store>,
    ) -> Result<WorkItemRow, IpcError> {
        let args = WorkLinkArgs {
            item_id: Some(args.item_id),
            action: "name".into(),
            title: Some(args.title),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("rename_work_item", &args).await,
            None => work::local::rename_local_item(
                &args,
                store,
                &fleet_core::service::orgs::OrgScope::All,
            ),
        }
    }

    pub async fn session_work_links(
        backend: &FleetBackend,
        args: WorkArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorkLinkRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("session_work_links", &args).await,
            None => work::work(&args, store, &fleet_core::service::orgs::OrgScope::All),
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
            force_cross_org: args.force_cross_org.then_some(true),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("link_session_work", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
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
            link_id: args.link_id,
            source: None,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("reject_session_work", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn confirm_session_work(
        backend: &FleetBackend,
        args: ConfirmSessionWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let args = WorkLinkArgs {
            session_id: Some(args.session_id),
            action: "confirm".into(),
            link_id: Some(args.link_id),
            force_cross_org: args.force_cross_org.then_some(true),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("confirm_session_work", &args).await,
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn set_work_project_trust(
        backend: &FleetBackend,
        args: SetWorkProjectTrustArgs,
        store: &Mutex<Store>,
    ) -> Result<work::ProjectTrust, IpcError> {
        let args = WorkLinkArgs {
            action: "trust_project".into(),
            project_id: Some(args.project_id),
            on: Some(args.on),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("set_work_project_trust", &args).await,
            None => work::trust_project(&args, store),
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
            None => work::work_link(&args, store, &fleet_core::service::orgs::OrgScope::All),
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
            None => {
                work::work_resume_plan(&args, store, ssh, &fleet_core::service::orgs::OrgScope::All)
                    .await
            }
        }
    }

    pub async fn request_work_handover(
        backend: &FleetBackend,
        args: RequestWorkHandoverArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        let wire = WorkLinkArgs {
            action: "handover".into(),
            session_id: Some(args.session_id),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("request_work_handover", &wire).await,
            None => {
                work::agent_handover::request(
                    store,
                    ssh,
                    args.session_id,
                    &fleet_core::service::orgs::OrgScope::All,
                )
                .await
            }
        }
    }

    pub async fn summarize_past_work(
        backend: &FleetBackend,
        args: SummarizePastWorkArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SummaryOutcome, IpcError> {
        let wire = WorkLinkArgs {
            action: "summarize".into(),
            key: Some(args.key.clone()),
            link_id: Some(args.link_id),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("summarize_past_work", &wire).await,
            None => {
                work::summary::summarize(
                    store,
                    ssh.as_ref(),
                    &args.key,
                    args.link_id,
                    &fleet_core::service::orgs::OrgScope::All,
                )
                .await
            }
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
            force_cross_org: args.force_cross_org.then_some(true),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("resume_work", &args).await,
            None => {
                work::work_resume(
                    &args,
                    store,
                    ssh,
                    reg,
                    &fleet_core::service::orgs::OrgScope::All,
                )
                .await
            }
        }
    }

    pub async fn work_today(
        backend: &FleetBackend,
        args: WorkTodayArgs,
        store: &Mutex<Store>,
    ) -> Result<Today, IpcError> {
        let wire = WorkArgs {
            action: Some("today".into()),
            since: args.since,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_today", &wire).await,
            None => {
                work::today::today(store, args.since, &fleet_core::service::orgs::OrgScope::All)
            }
        }
    }

    pub async fn work_ticket_card(
        backend: &FleetBackend,
        args: WorkTicketCardArgs,
        store: &Mutex<Store>,
    ) -> Result<TicketCard, IpcError> {
        let wire = WorkArgs {
            action: Some("card".into()),
            key: Some(args.key.clone()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_ticket_card", &wire).await,
            None => work::card::card(store, &args.key, &fleet_core::service::orgs::OrgScope::All),
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
            None => {
                work::work_purge_impact(&args, store, &fleet_core::service::orgs::OrgScope::All)
            }
        }
    }
}
