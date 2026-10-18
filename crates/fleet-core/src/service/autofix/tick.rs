//! The autofix loop: one pass per minute on the reconcile tick, detached.

use super::apply::{live_agents, run_pass, SshFixExec};
use crate::ipc_error::lock;
use crate::ssh::SshClient;
use crate::store::{now_unix, Store};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const PERIOD: Duration = Duration::from_secs(60);
static RUNNING: AtomicBool = AtomicBool::new(false);
static LAST: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

struct InFlight(&'static AtomicBool);
impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Tick entry point: start one pass in the background when it is due, the
/// loop is not paused, and no earlier pass is still going.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    {
        let Ok(mut last) = LAST.lock() else { return };
        if last.is_some_and(|t| t.elapsed() < PERIOD) {
            return;
        }
        let Ok(s) = lock(store) else { return };
        if !crate::service::loops::gate_in("autofix", &s, Some(PERIOD)) {
            return;
        }
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        *last = Some(Instant::now());
    }
    let guard = InFlight(&RUNNING);
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    tokio::spawn(async move {
        let _guard = guard;
        let live = live_agents(&ssh);
        let exec = SshFixExec {
            store: &store,
            ssh: &*ssh,
        };
        let res = run_pass(&store, &exec, &live, now_unix()).await;
        if let Ok(o) = &res {
            if !o.applied.is_empty() || !o.sync.opened.is_empty() {
                tracing::info!(
                    opened = o.sync.opened.len(),
                    applied = o.applied.len(),
                    "[autofix] pass"
                );
            }
        }
        crate::service::loops::report(
            "autofix",
            res.map(|_| ()).map_err(|e| e.message),
            Some(PERIOD),
        );
    });
}
