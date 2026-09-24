//! Runs one `run_link` per live dialer link. Rescans `peer_links` every 5 s,
//! so a CLI `peer add` / `peer remove` against `state.db` takes effect
//! without a restart.
//!
//! Never two loops on one link: two dialers on a link would supersede each
//! other's parked poll on the listener forever. `running` is keyed by link
//! id, and a loop is replaced (new url or token after a re-pair) only once
//! the old one has exited.

use super::dial::{run_link, HttpPeerCall, LinkExit};
use crate::http_client::HubTransport;
use crate::ipc_error::lock;
use crate::ssh::SshClient;
use crate::store::{Store, LINK_INCOMPATIBLE, LINK_REFUSED};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const RESCAN: Duration = Duration::from_secs(5);
/// How long a stop waits for one loop to exit.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// One running loop and the credentials it was started with. No `Debug`:
/// it holds the token.
struct Running {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<LinkExit>,
    url: String,
    token: String,
}

pub fn spawn_peer_supervisor(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    transport: Arc<dyn HubTransport>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(async move {
        let mut running: HashMap<i64, Running> = HashMap::new();
        loop {
            reap(&mut running).await;
            let links = lock(&store)
                .and_then(|s| s.live_dialer_links())
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e.message, "[peer] cannot list the hub links");
                    Vec::new()
                });
            // Gone (revoked, removed, or merged by a re-pair): stop it.
            for (id, r) in running.iter() {
                if !links.iter().any(|l| l.id == *id) {
                    r.cancel.cancel();
                }
            }
            for l in links {
                let (Some(url), Some(token)) = (l.url.clone(), l.token.clone()) else {
                    continue;
                };
                if let Some(r) = running.get(&l.id) {
                    if r.url == url && r.token == token {
                        continue;
                    }
                    // A re-pair moved new credentials onto this row: stop
                    // the old loop, and start the new one only once it is
                    // gone.
                    r.cancel.cancel();
                    let Some(mut old) = running.remove(&l.id) else {
                        continue;
                    };
                    if tokio::time::timeout(STOP_WAIT, &mut old.handle)
                        .await
                        .is_err()
                    {
                        running.insert(l.id, old);
                        continue;
                    }
                }
                if l.state == LINK_REFUSED || l.state == LINK_INCOMPATIBLE {
                    continue;
                }
                let child = cancel.child_token();
                let call = Arc::new(HttpPeerCall {
                    url: url.clone(),
                    token: token.clone(),
                    transport: transport.clone(),
                });
                tracing::info!(link_id = l.id, "[peer] starting the exchange loop");
                let handle = crate::rt::spawn(run_link(
                    store.clone(),
                    ssh.clone(),
                    l.id,
                    token.clone(),
                    call,
                    child.clone(),
                ));
                running.insert(
                    l.id,
                    Running {
                        cancel: child,
                        handle,
                        url,
                        token,
                    },
                );
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(RESCAN) => {}
            }
        }
        for (_, r) in running {
            r.cancel.cancel();
            let _ = tokio::time::timeout(STOP_WAIT, r.handle).await;
        }
    })
}

/// Drop the finished loops, logging how each ended. A `Rebound` loop's
/// surviving row is started by the same rescan.
async fn reap(running: &mut HashMap<i64, Running>) {
    let done: Vec<i64> = running
        .iter()
        .filter(|(_, r)| r.handle.is_finished())
        .map(|(id, _)| *id)
        .collect();
    for id in done {
        if let Some(r) = running.remove(&id) {
            match r.handle.await {
                Ok(exit) => {
                    tracing::info!(link_id = id, exit = ?exit, "[peer] exchange loop ended")
                }
                Err(e) => tracing::warn!(link_id = id, error = %e, "[peer] exchange loop panicked"),
            }
        }
    }
}
