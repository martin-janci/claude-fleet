//! Tauri IPC wrapper for `move_session` (the "Move to host…" action).
//! Logic lives in `service::move_session`.
//!
//! Routes in remote mode: `MoveSessionArgs` is exactly the tool's parameter
//! set (`session_id`, `target_host_alias`, `keep_source`, `strict`,
//! `clean_target`, `dry_run`, `when`), and the tool answers the same
//! `MoveOutcome` (a `MoveReport` for a real move, a `MovePreview` for
//! `dry_run: true`, a `MoveWaiting` for `when: idle` on a busy source, a
//! `MoveWaitCancelled` for `when: cancel`).

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::move_session::{self, MoveOutcome, MoveSessionArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn move_session(
    args: MoveSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<MoveOutcome, IpcError> {
    routed::move_session(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn move_session(
        backend: &FleetBackend,
        args: MoveSessionArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<MoveOutcome, IpcError> {
        match backend.hub() {
            Some(hub) => {
                // A hub built before `dry_run` or `when` ignores whichever it
                // predates and MOVES the session — harmless for `when: idle`
                // (it just sees no such field and refuses a busy source as
                // today), but not for `when: cancel`: cancelling a wait would
                // instead perform the very move it was meant to stop. The
                // contract gate only refuses such a hub once its `ready`
                // frame has been judged, so any request an older hub could
                // misread this way waits for a positive in-range judgement
                // on this launch. A plain move (`when: now`, not a dry run)
                // stays ungated.
                if args.dry_run || args.when != move_session::When::Now {
                    hub.require_confirmed_contract(
                        "This transfer request",
                        "a hub older than this app would ignore what it asks and move the \
                         session at once",
                    )?;
                }
                hub.route("move_session", &args).await
            }
            None => move_session::move_session(args, store, ssh).await,
        }
    }
}
