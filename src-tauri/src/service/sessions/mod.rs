//! Session management service, split by concern (a pure move of the former
//! `sessions.rs`): `reconcile`, `paths`, `lifecycle`, `prompt`, `review` and
//! `targeting`. Every item is re-exported here, so `crate::service::sessions::X`
//! paths are unchanged. The submodules and the test modules see each other
//! through `use super::*`, exactly as in the single file.

use crate::cancel::{CancelGuard, CancellationRegistry};
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{HostReconcile, HostRow, ProjectRow, ReconcileSession, SessionRow, Store};
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

mod lifecycle;
mod paths;
mod prompt;
mod reconcile;
mod review;
mod targeting;

#[cfg(test)]
mod fill_session_name_tests;
#[cfg(test)]
mod ghost_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;

pub use self::lifecycle::*;
// `paths` has no `pub` item — its widest is `pub(crate)` — so the re-export
// is `pub(crate)` too (a `pub` glob would re-export nothing).
pub(crate) use self::paths::*;
pub use self::prompt::*;
pub use self::reconcile::*;
pub use self::review::*;
pub use self::targeting::*;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
