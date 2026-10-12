//! The New session picker's per-project choices (project picker spec v2):
//! the transport-agnostic layer the Tauri commands and the hub tools share.
//! The rules live in `Store::set_project_pick`.

use crate::ipc_error::{lock, IpcError};
use crate::store::{now_unix, ProjectPickRow, Store};
use std::sync::Mutex;

/// One project's new picker choices — a full replace: send the current
/// value of what you are not changing. The empty state removes the row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetProjectPickArgs {
    /// The project's owner, as list_projects names it.
    pub owner: String,
    /// The project's repo, as list_projects names it.
    pub repo: String,
    /// Pinned to the top of the picker.
    #[serde(default)]
    pub pinned: bool,
    /// hide | keep; null = the rules decide.
    #[serde(default)]
    pub vis: Option<String>,
    /// The picker group (at most 40 characters); null or blank = automatic.
    #[serde(default)]
    pub grp: Option<String>,
}

pub fn list(store: &Mutex<Store>) -> Result<Vec<ProjectPickRow>, IpcError> {
    lock(store)?.list_project_picks()
}

pub fn set(store: &Mutex<Store>, args: &SetProjectPickArgs) -> Result<ProjectPickRow, IpcError> {
    lock(store)?.set_project_pick(
        &args.owner,
        &args.repo,
        args.pinned,
        args.vis.as_deref(),
        args.grp.as_deref(),
        now_unix(),
    )
}
