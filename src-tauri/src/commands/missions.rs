//! Tauri commands of the Work view's missions (orchestration O1,
//! `docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md`):
//! two reads of `work` and the writes of `work_link`, each routed by command
//! name on a paired desktop and served by `service::work::missions` here.
//! Thin wrappers, as `commands::work_view` is: the rules live in fleet-core.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::work::graph::{self, GraphChange};
use fleet_core::service::work::missions::{self, MissionDeleted, MissionDetail, MissionInput};
use fleet_core::service::work::orchestrate::{self, Deps, PlanOutcome, StartOutcome, StepResult};
use fleet_core::service::work::verify::{self, VerifyOutcome};
use fleet_core::service::work::{WorkArgs, WorkLinkArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::{CardRow, GrantRow, MissionRow, Store, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

/// `work_missions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkMissionsArgs {}

/// `work_mission`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkMissionArgs {
    pub mission_id: i64,
    /// Events older than this one.
    #[serde(default)]
    pub before_event: Option<i64>,
}

/// `save_mission`: without `mission_id` a new draft (rooted at `item_id`,
/// else at a new task of its name); with it, a change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SaveMissionArgs {
    #[serde(default)]
    pub mission_id: Option<i64>,
    #[serde(default)]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub expected_version: Option<i64>,
    pub mission: MissionInput,
}

/// `set_mission_state`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetMissionStateArgs {
    pub mission_id: i64,
    pub state: String,
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `set_mission_repo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetMissionRepoArgs {
    pub mission_id: i64,
    pub project_id: i64,
    #[serde(default)]
    pub role: Option<String>,
    pub on: bool,
}

/// `set_mission_item`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetMissionItemArgs {
    pub mission_id: i64,
    pub item_id: i64,
    pub on: bool,
}

/// `delete_mission`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteMissionArgs {
    pub mission_id: i64,
}

/// `set_work_dep`: `item_id` waits for `depends_on` (`on: false` erases).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetWorkDepArgs {
    pub item_id: i64,
    pub depends_on: i64,
    pub on: bool,
}

/// `set_work_hold`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetWorkHoldArgs {
    pub item_id: i64,
    pub on: bool,
}

/// `accept_work_proposals` and `undo_work_accept`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkProposalsArgs {
    pub item_ids: Vec<i64>,
}

/// `set_work_done_when`: an item's condition lines (`[]` clears them).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetWorkDoneWhenArgs {
    pub item_id: i64,
    pub done_when: Vec<String>,
}

/// `verify_work_item`: a person's check of one condition line.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerifyWorkItemArgs {
    pub item_id: i64,
    pub line: String,
    pub ok: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// `start_mission_wave`: take the mission's next steps, or the one `step`
/// names (orchestration O4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartMissionWaveArgs {
    pub mission_id: i64,
    #[serde(default)]
    pub step: Option<String>,
}

/// `retry_work_item`: another attempt at a mission's item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetryWorkItemArgs {
    pub item_id: i64,
    #[serde(default)]
    pub note: Option<String>,
}

/// `plan_mission`, `revoke_mission_grant`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissionIdArgs {
    pub mission_id: i64,
}

/// `decide_mission_card`: apply (`ok`) or dismiss a card; a question is
/// answered by applying it with `note`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecideMissionCardArgs {
    pub card_id: i64,
    pub ok: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// `grant_mission`: what the mission's loop may do by itself (O6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantMissionArgs {
    pub mission_id: i64,
    pub level: i64,
    #[serde(default)]
    pub hours: Option<u32>,
    #[serde(default)]
    pub budget_cents: Option<i64>,
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    #[serde(default)]
    pub max_parallel: Option<u32>,
}

/// `pause_all_missions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PauseAllMissionsArgs {}

/// The standalone desktop's reader: one person at the keyboard, as
/// `commands::work_view`'s. A paired desktop routes to the hub, which
/// builds the caller's own scope.
fn internal_view() -> fleet_core::service::view_scope::ViewScope {
    fleet_core::service::view_scope::ViewScope::internal()
}

#[tauri::command]
pub async fn work_missions(
    args: WorkMissionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<MissionRow>, IpcError> {
    routed::work_missions(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_mission(
    args: WorkMissionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<MissionDetail, IpcError> {
    routed::work_mission(&backend, args, &store).await
}

#[tauri::command]
pub async fn save_mission(
    args: SaveMissionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<MissionRow, IpcError> {
    routed::save_mission(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_mission_state(
    args: SetMissionStateArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<MissionRow, IpcError> {
    routed::set_mission_state(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_mission_repo(
    args: SetMissionRepoArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<MissionRow, IpcError> {
    routed::set_mission_repo(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_mission_item(
    args: SetMissionItemArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<MissionRow, IpcError> {
    routed::set_mission_item(&backend, args, &store).await
}

#[tauri::command]
pub async fn delete_mission(
    args: DeleteMissionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<MissionDeleted, IpcError> {
    routed::delete_mission(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_work_dep(
    args: SetWorkDepArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<GraphChange, IpcError> {
    routed::set_work_dep(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_work_hold(
    args: SetWorkHoldArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<GraphChange, IpcError> {
    routed::set_work_hold(&backend, args, &store).await
}

#[tauri::command]
pub async fn accept_work_proposals(
    args: WorkProposalsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorkItemRow>, IpcError> {
    routed::accept_work_proposals(&backend, args, &store).await
}

#[tauri::command]
pub async fn undo_work_accept(
    args: WorkProposalsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorkItemRow>, IpcError> {
    routed::undo_work_accept(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_work_done_when(
    args: SetWorkDoneWhenArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<VerifyOutcome, IpcError> {
    routed::set_work_done_when(&backend, args, &store).await
}

#[tauri::command]
pub async fn verify_work_item(
    args: VerifyWorkItemArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<VerifyOutcome, IpcError> {
    routed::verify_work_item(&backend, args, &store).await
}

#[tauri::command]
pub async fn start_mission_wave(
    args: StartMissionWaveArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<StartOutcome, IpcError> {
    routed::start_mission_wave(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn retry_work_item(
    args: RetryWorkItemArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<StepResult, IpcError> {
    routed::retry_work_item(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn plan_mission(
    args: MissionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<PlanOutcome, IpcError> {
    routed::plan_mission(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn decide_mission_card(
    args: DecideMissionCardArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<CardRow, IpcError> {
    routed::decide_mission_card(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn grant_mission(
    args: GrantMissionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<GrantRow, IpcError> {
    routed::grant_mission(&backend, args, &store).await
}

#[tauri::command]
pub async fn revoke_mission_grant(
    args: MissionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<usize, IpcError> {
    routed::revoke_mission_grant(&backend, args, &store).await
}

#[tauri::command]
pub async fn pause_all_missions(
    args: PauseAllMissionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<i64>, IpcError> {
    routed::pause_all_missions(&backend, args, &store).await
}

pub(crate) mod routed {
    use super::*;

    fn write(action: &str, mission_id: Option<i64>) -> WorkLinkArgs {
        WorkLinkArgs {
            action: action.into(),
            mission_id,
            ..Default::default()
        }
    }

    pub async fn work_missions(
        backend: &FleetBackend,
        _args: WorkMissionsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<MissionRow>, IpcError> {
        let wire = WorkArgs {
            action: Some("missions".into()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_missions", &wire).await,
            None => missions::missions(store, &internal_view()),
        }
    }

    pub async fn work_mission(
        backend: &FleetBackend,
        args: WorkMissionArgs,
        store: &Mutex<Store>,
    ) -> Result<MissionDetail, IpcError> {
        let wire = WorkArgs {
            action: Some("mission".into()),
            mission_id: Some(args.mission_id),
            before_event: args.before_event,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("work_mission", &wire).await,
            None => missions::mission(store, &internal_view(), args.mission_id, args.before_event),
        }
    }

    pub async fn save_mission(
        backend: &FleetBackend,
        args: SaveMissionArgs,
        store: &Mutex<Store>,
    ) -> Result<MissionRow, IpcError> {
        let wire = WorkLinkArgs {
            item_id: args.item_id,
            expected_version: args.expected_version,
            mission: Some(args.mission),
            ..write("mission_save", args.mission_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("save_mission", &wire).await,
            None => missions::save(&wire, store, &internal_view()),
        }
    }

    pub async fn set_mission_state(
        backend: &FleetBackend,
        args: SetMissionStateArgs,
        store: &Mutex<Store>,
    ) -> Result<MissionRow, IpcError> {
        let wire = WorkLinkArgs {
            status: Some(args.state),
            expected_version: args.expected_version,
            ..write("mission_state", Some(args.mission_id))
        };
        match backend.hub() {
            Some(hub) => hub.route("set_mission_state", &wire).await,
            None => missions::set_state(&wire, store, &internal_view()),
        }
    }

    pub async fn set_mission_repo(
        backend: &FleetBackend,
        args: SetMissionRepoArgs,
        store: &Mutex<Store>,
    ) -> Result<MissionRow, IpcError> {
        let wire = WorkLinkArgs {
            project_id: Some(args.project_id),
            role: args.role,
            on: Some(args.on),
            ..write("mission_repo", Some(args.mission_id))
        };
        match backend.hub() {
            Some(hub) => hub.route("set_mission_repo", &wire).await,
            None => missions::repo(&wire, store, &internal_view()),
        }
    }

    pub async fn set_mission_item(
        backend: &FleetBackend,
        args: SetMissionItemArgs,
        store: &Mutex<Store>,
    ) -> Result<MissionRow, IpcError> {
        let wire = WorkLinkArgs {
            item_id: Some(args.item_id),
            on: Some(args.on),
            ..write("mission_item", Some(args.mission_id))
        };
        match backend.hub() {
            Some(hub) => hub.route("set_mission_item", &wire).await,
            None => missions::item(&wire, store, &internal_view()),
        }
    }

    pub async fn delete_mission(
        backend: &FleetBackend,
        args: DeleteMissionArgs,
        store: &Mutex<Store>,
    ) -> Result<MissionDeleted, IpcError> {
        let wire = write("mission_delete", Some(args.mission_id));
        match backend.hub() {
            Some(hub) => hub.route("delete_mission", &wire).await,
            None => missions::delete(&wire, store, &internal_view()),
        }
    }

    pub async fn set_work_dep(
        backend: &FleetBackend,
        args: SetWorkDepArgs,
        store: &Mutex<Store>,
    ) -> Result<GraphChange, IpcError> {
        let wire = WorkLinkArgs {
            action: "dep".into(),
            item_id: Some(args.item_id),
            depends_on: Some(args.depends_on),
            on: Some(args.on),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("set_work_dep", &wire).await,
            None => graph::dep(&wire, store, &internal_view()),
        }
    }

    pub async fn set_work_hold(
        backend: &FleetBackend,
        args: SetWorkHoldArgs,
        store: &Mutex<Store>,
    ) -> Result<GraphChange, IpcError> {
        let wire = WorkLinkArgs {
            action: "hold".into(),
            item_id: Some(args.item_id),
            on: Some(args.on),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("set_work_hold", &wire).await,
            None => graph::hold(&wire, store, &internal_view()),
        }
    }

    pub async fn accept_work_proposals(
        backend: &FleetBackend,
        args: WorkProposalsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorkItemRow>, IpcError> {
        let wire = WorkLinkArgs {
            action: "accept_many".into(),
            item_ids: Some(args.item_ids),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("accept_work_proposals", &wire).await,
            None => graph::accept_many(&wire, store, &internal_view()),
        }
    }

    pub async fn undo_work_accept(
        backend: &FleetBackend,
        args: WorkProposalsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorkItemRow>, IpcError> {
        let wire = WorkLinkArgs {
            action: "undo_accept".into(),
            item_ids: Some(args.item_ids),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("undo_work_accept", &wire).await,
            None => graph::undo_accept(&wire, store, &internal_view()),
        }
    }

    pub async fn set_work_done_when(
        backend: &FleetBackend,
        args: SetWorkDoneWhenArgs,
        store: &Mutex<Store>,
    ) -> Result<VerifyOutcome, IpcError> {
        let wire = WorkLinkArgs {
            action: "done_when".into(),
            item_id: Some(args.item_id),
            done_when: Some(args.done_when),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("set_work_done_when", &wire).await,
            None => verify::set_done_when(&wire, store, &internal_view()),
        }
    }

    pub async fn verify_work_item(
        backend: &FleetBackend,
        args: VerifyWorkItemArgs,
        store: &Mutex<Store>,
    ) -> Result<VerifyOutcome, IpcError> {
        let wire = WorkLinkArgs {
            action: "verify".into(),
            item_id: Some(args.item_id),
            line: Some(args.line),
            ok: Some(args.ok),
            note: args.note,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("verify_work_item", &wire).await,
            None => verify::verify(&wire, store, &internal_view()),
        }
    }

    /// What the standalone desktop's loop steps need.
    fn deps(
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Deps {
        Deps {
            store: Arc::clone(store),
            ssh: Arc::clone(ssh),
            reg: Arc::clone(reg),
            net: fleet_core::service::trackers::default_net(),
        }
    }

    pub async fn start_mission_wave(
        backend: &FleetBackend,
        args: StartMissionWaveArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<StartOutcome, IpcError> {
        let wire = WorkLinkArgs {
            step: args.step,
            ..write("mission_start", Some(args.mission_id))
        };
        match backend.hub() {
            Some(hub) => hub.route("start_mission_wave", &wire).await,
            None => orchestrate::start(&wire, &deps(store, ssh, reg), &internal_view()).await,
        }
    }

    pub async fn retry_work_item(
        backend: &FleetBackend,
        args: RetryWorkItemArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<StepResult, IpcError> {
        let wire = WorkLinkArgs {
            item_id: Some(args.item_id),
            note: args.note,
            ..write("retry", None)
        };
        match backend.hub() {
            Some(hub) => hub.route("retry_work_item", &wire).await,
            None => orchestrate::retry(&wire, &deps(store, ssh, reg), &internal_view()).await,
        }
    }

    pub async fn plan_mission(
        backend: &FleetBackend,
        args: MissionIdArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<PlanOutcome, IpcError> {
        let wire = write("mission_plan", Some(args.mission_id));
        match backend.hub() {
            Some(hub) => hub.route("plan_mission", &wire).await,
            None => orchestrate::plan_now(&wire, &deps(store, ssh, reg), &internal_view()).await,
        }
    }

    pub async fn decide_mission_card(
        backend: &FleetBackend,
        args: DecideMissionCardArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<CardRow, IpcError> {
        let wire = WorkLinkArgs {
            card_id: Some(args.card_id),
            ok: Some(args.ok),
            note: args.note,
            ..write("card_decide", None)
        };
        match backend.hub() {
            Some(hub) => hub.route("decide_mission_card", &wire).await,
            None => orchestrate::decide_card(&wire, &deps(store, ssh, reg), &internal_view()).await,
        }
    }

    pub async fn grant_mission(
        backend: &FleetBackend,
        args: GrantMissionArgs,
        store: &Mutex<Store>,
    ) -> Result<GrantRow, IpcError> {
        let wire = WorkLinkArgs {
            level: Some(args.level),
            hours: args.hours,
            budget_cents: args.budget_cents,
            hosts: args.hosts,
            max_parallel: args.max_parallel,
            ..write("mission_grant", Some(args.mission_id))
        };
        match backend.hub() {
            Some(hub) => hub.route("grant_mission", &wire).await,
            None => orchestrate::grant(&wire, store, &internal_view()),
        }
    }

    pub async fn revoke_mission_grant(
        backend: &FleetBackend,
        args: MissionIdArgs,
        store: &Mutex<Store>,
    ) -> Result<usize, IpcError> {
        let wire = write("mission_revoke", Some(args.mission_id));
        match backend.hub() {
            Some(hub) => hub.route("revoke_mission_grant", &wire).await,
            None => orchestrate::revoke(&wire, store, &internal_view()),
        }
    }

    pub async fn pause_all_missions(
        backend: &FleetBackend,
        _args: PauseAllMissionsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<i64>, IpcError> {
        let wire = write("missions_pause_all", None);
        match backend.hub() {
            Some(hub) => hub.route("pause_all_missions", &wire).await,
            None => orchestrate::pause_all(store, &internal_view()),
        }
    }
}
