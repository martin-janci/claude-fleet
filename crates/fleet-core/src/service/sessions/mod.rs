//! Session management service, split by concern (a pure move of the former
//! `sessions.rs`): `reconcile`, `paths`, `lifecycle`, `prompt`, `review` and
//! `targeting`. Every item is re-exported here, so `crate::service::sessions::X`
//! paths are unchanged. The submodules and the test modules see each other
//! through `use super::*`, exactly as in the single file.

use crate::cancel::{CancelGuard, CancellationRegistry};
use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{
    HostReconcile, HostRow, ProjectRow, ReconcileSession, SessionRow, Store, StoredIdentity,
};
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

mod activity;
mod adopt;
mod claim;
pub mod deferred;
mod discover;
mod lifecycle;
mod lost_found;
mod paths;
#[cfg(feature = "nl-detect")]
pub(crate) use paths::project_for_local_path;
mod prompt;
mod reconcile;
mod restore;
mod review;
pub mod seed;
mod sharing;
mod start_progress;
mod targeting;
pub mod terminals;

#[cfg(test)]
mod fill_session_name_tests;
#[cfg(test)]
mod ghost_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;

pub use self::activity::*;
pub use self::adopt::*;
pub use self::claim::*;
pub use self::deferred::*;
pub use self::discover::*;
pub use self::lifecycle::*;
pub use self::lost_found::*;
// `paths` has no `pub` item — its widest is `pub(crate)` — so the re-export
// is `pub(crate)` too (a `pub` glob would re-export nothing).
pub(crate) use self::paths::*;
pub use self::prompt::*;
pub use self::reconcile::*;
pub(crate) use self::start_progress::StartReporter;
pub use self::start_progress::{validate_start_token, START_TOKEN_MAX};
// The two probe timeouts by name: a `pub` glob does not carry
// `pub(crate)` items, and `store::reconcile` derives its kill-memory
// window from them.
pub(crate) use self::reconcile::{HOST_PROBE_TIMEOUT, PR_PROBE_TIMEOUT};
pub use self::restore::*;
pub use self::review::*;
pub use self::sharing::*;
pub use self::targeting::*;
pub use self::terminals::*;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
