//! The autofix loop: one pass per minute on the reconcile tick, detached.

use super::apply::{gather, live_agents, run_pass, scan_enabled, sync_scan, Config, SshFixExec};
use super::mcp_entry;
use crate::ipc_error::{lock, IpcError};
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

/// Detector 2's input: read the MCP entry of every reachable host that has
/// a stored token and is due ([`mcp_entry::PROBE_EVERY_SECS`]), one at a
/// time, before the pass scans. Skipped while the detector is off.
///
/// `force` (a person's "Check now"): every such host, due or not, all at
/// once, so an entry fixed by hand is read as it now is.
async fn probe_mcp_entries(store: &Mutex<Store>, ssh: &SshClient, now: i64, force: bool) {
    let due: Vec<String> = {
        let Ok(s) = lock(store) else { return };
        if !crate::service::settings::get_bool(
            &s,
            crate::service::settings::AUTOFIX_DETECTOR_MCP_ENTRY,
        ) {
            return;
        }
        let with_token: std::collections::BTreeSet<String> = s
            .list_host_tokens()
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.host_alias)
            .collect();
        let local = crate::service::hub::local_host_enabled();
        crate::service::hosts::active_hosts(s.list_hosts().unwrap_or_default(), local)
            .into_iter()
            .filter(|h| h.reachable && with_token.contains(&h.alias))
            .map(|h| h.alias)
            .filter(|a| force || mcp_entry::due(a, now))
            .collect()
    };
    let probe_one = |alias: String| async move {
        match mcp_entry::probe(ssh, &alias).await {
            Ok(p) => mcp_entry::remember(&alias, now, p),
            Err(e) => {
                tracing::debug!(host = %alias, error = %e, "[autofix] mcp entry probe failed");
                mcp_entry::remember(&alias, now, None);
            }
        }
    };
    if force {
        futures_util::future::join_all(due.into_iter().map(probe_one)).await;
    } else {
        for alias in due {
            probe_one(alias).await;
        }
    }
}

/// Check now (the `autofix` tool's `scan`, the desktop's `autofix_scan`):
/// read every host's MCP entry afresh, then scan and sync as a pass does
/// (an unreachable host's rows stay, a switched-off detector's are retired,
/// not "fixed"). Never applies a fix.
pub async fn scan_now(store: &Mutex<Store>, ssh: &SshClient, now: i64) -> Result<(), IpcError> {
    probe_mcp_entries(store, ssh, now, true).await;
    let live = live_agents(ssh);
    let s = lock(store)?;
    let cx = gather(&s, &live, now)?;
    let cfg = Config::read(&s);
    let found = scan_enabled(&cx, &cfg);
    sync_scan(&s, &cx, &cfg, &found, now)?;
    Ok(())
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
        probe_mcp_entries(&store, &ssh, now_unix(), false).await;
        let live = live_agents(&ssh);
        let exec = SshFixExec {
            store: &store,
            ssh: &ssh,
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
