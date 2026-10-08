//! Tauri commands of the Work view's missions (orchestration O1,
//! `docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md`):
//! two reads of `work` and five writes of `work_link`, each routed by command
//! name on a paired desktop and served by `service::work::missions` here.
//! Thin wrappers, as `commands::work_view` is: the rules live in fleet-core.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::work::graph::{self, GraphChange};
use fleet_core::service::work::missions::{self, MissionDeleted, MissionDetail, MissionInput};
use fleet_core::service::work::{WorkArgs, WorkLinkArgs};
use fleet_core::store::{MissionRow, Store, WorkItemRow};
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
}
