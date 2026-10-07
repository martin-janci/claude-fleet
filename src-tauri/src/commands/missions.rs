//! Tauri commands of the Work view's missions (orchestration O1,
//! `docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md`):
//! two reads of `work` and five writes of `work_link`, each routed by command
//! name on a paired desktop and served by `service::work::missions` here.
//! Thin wrappers, as `commands::work_view` is: the rules live in fleet-core.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::work::missions::{self, MissionDeleted, MissionDetail, MissionInput};
use fleet_core::service::work::{WorkArgs, WorkLinkArgs};
use fleet_core::store::{MissionRow, Store};
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
}
