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
use crate::ipc_error::{lock, IpcError};
use crate::ssh::SshClient;
use crate::store::{PeerLinkRow, Store, LINK_INCOMPATIBLE, LINK_REFUSED};
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
        let ctx = Ctx {
            store,
            ssh,
            transport,
            cancel,
        };
        let mut running: HashMap<i64, Running> = HashMap::new();
        loop {
            reap(&mut running).await;
            // The guard is dropped at the end of this statement, before any
            // `.await` below.
            let listed = lock(&ctx.store).and_then(|s| s.live_dialer_links());
            reconcile(&ctx, &mut running, listed).await;
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                _ = tokio::time::sleep(RESCAN) => {}
            }
        }
        for (_, r) in running {
            r.cancel.cancel();
            let _ = tokio::time::timeout(STOP_WAIT, r.handle).await;
        }
    })
}

/// What the supervisor starts loops with.
struct Ctx {
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    transport: Arc<dyn HubTransport>,
    cancel: CancellationToken,
}

/// One rescan: stop the loops whose link is gone, (re)start the rest.
async fn reconcile(
    ctx: &Ctx,
    running: &mut HashMap<i64, Running>,
    listed: Result<Vec<PeerLinkRow>, IpcError>,
) {
    // G7: a failed listing is not an empty one. Treating it as empty would
    // take every running link for gone and stop it; skip the pass instead,
    // and reconcile on the next one that can list.
    let links = match listed {
        Ok(links) => links,
        Err(e) => {
            tracing::warn!(error = %e.message, "[peer] cannot list the hub links; skipping this pass");
            return;
        }
    };
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
            // A re-pair moved new credentials onto this row: stop the old
            // loop, and start the new one only once it is gone.
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
        let child = ctx.cancel.child_token();
        let call = Arc::new(HttpPeerCall {
            url: url.clone(),
            token: token.clone(),
            transport: ctx.transport.clone(),
        });
        tracing::info!(link_id = l.id, "[peer] starting the exchange loop");
        let handle = crate::rt::spawn(run_link(
            ctx.store.clone(),
            ctx.ssh.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;
    use crate::service::peer::testkit::hub;
    use crate::service::peer::wire::{ExchangeRequest, ExchangeResponse, PROTO};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoTransport;

    #[async_trait::async_trait]
    impl HubTransport for NoTransport {
        async fn post_json(
            &self,
            _url: &str,
            _bearer: &str,
            _body: String,
        ) -> Result<crate::http_client::HubResponse, String> {
            Err("no transport in this test".into())
        }
    }

    /// A stand-in loop that runs until cancelled.
    fn running_loop(ctx: &Ctx) -> (CancellationToken, Running) {
        let child = ctx.cancel.child_token();
        let waits = child.clone();
        let handle = tokio::spawn(async move {
            waits.cancelled().await;
            LinkExit::Cancelled
        });
        let r = Running {
            cancel: child.clone(),
            handle,
            url: "https://b.example".into(),
            token: "t".into(),
        };
        (child, r)
    }

    /// G7: a pass whose listing failed says nothing about which links are
    /// gone, so it stops nothing — the running loops keep running and are
    /// reconciled on the next pass that can list.
    #[tokio::test]
    async fn a_failed_listing_skips_the_pass_and_stops_no_loop() {
        let (store, ssh) = hub("fleet-a");
        let ctx = Ctx {
            store,
            ssh,
            transport: Arc::new(NoTransport),
            cancel: CancellationToken::new(),
        };
        let mut running = HashMap::new();
        let (child, r) = running_loop(&ctx);
        running.insert(7, r);
        reconcile(
            &ctx,
            &mut running,
            Err(IpcError::new(codes::E_SQLITE, "database is locked")),
        )
        .await;
        assert!(!child.is_cancelled(), "a failed listing cancelled a loop");
        assert!(running.contains_key(&7));
        // The control: a listing that really lacks the link does stop it.
        reconcile(&ctx, &mut running, Ok(vec![])).await;
        assert!(child.is_cancelled());
    }

    /// A peer that answers the handshake at once, as `fleet-b` with nothing
    /// to carry, and then holds every parked long-poll until the caller
    /// drops it — what a listener does with a poll. A parked call is only
    /// ever dropped, so `dropped` counts the loop's cancellations as the
    /// peer sees them.
    #[derive(Default)]
    struct ParksThePoll {
        handshakes: AtomicUsize,
        parked: AtomicUsize,
        dropped: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl HubTransport for ParksThePoll {
        async fn post_json(
            &self,
            _url: &str,
            _bearer: &str,
            body: String,
        ) -> Result<crate::http_client::HubResponse, String> {
            let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
            let req: ExchangeRequest =
                serde_json::from_value(envelope["params"]["arguments"].clone()).unwrap();
            if req.wait_ms > 0 {
                struct Dropped<'a>(&'a ParksThePoll);
                impl Drop for Dropped<'_> {
                    fn drop(&mut self) {
                        self.0.dropped.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let _dropped = Dropped(self);
                self.parked.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<()>().await;
                unreachable!("a parked poll is only ever dropped");
            }
            self.handshakes.fetch_add(1, Ordering::SeqCst);
            let resp = ExchangeResponse {
                proto: PROTO,
                fleet_id: "fleet-b".into(),
                results: vec![],
                messages: vec![],
                more: false,
            };
            let answer = serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {
                    "content": [{"type": "text", "text": serde_json::to_string(&resp).unwrap()}],
                },
            });
            Ok(crate::http_client::HubResponse {
                status: 200,
                body: format!("event: message\ndata: {answer}\n\n"),
            })
        }
    }

    /// Waits for `cond` (bounded; a condition, not a window).
    async fn until(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..400 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    /// The supervisor's own stop of a revoked link. The loop is parked in
    /// the peer's long-poll, so it cannot have read the revocation itself:
    /// on this single-threaded runtime nothing runs between the revoke and
    /// the rescan that cancels it. The loop then ends `Cancelled` (the
    /// supervisor's doing — `Revoked` is the loop's own exit on its next
    /// read) within `STOP_WAIT`, the parked call is dropped, and the rescan
    /// after that reaps it and does not start it again.
    #[tokio::test]
    async fn a_revoked_link_is_stopped_by_the_rescan_and_not_restarted() {
        let (store, ssh) = hub("fleet-a");
        let link = store
            .lock()
            .unwrap()
            .insert_dialer_link("https://b.example", "t")
            .unwrap();
        let peer = Arc::new(ParksThePoll::default());
        let ctx = Ctx {
            store,
            ssh,
            transport: peer.clone(),
            cancel: CancellationToken::new(),
        };
        let listed = |ctx: &Ctx| lock(&ctx.store).and_then(|s| s.live_dialer_links());
        let mut running = HashMap::new();
        reconcile(&ctx, &mut running, listed(&ctx)).await;
        assert!(running.contains_key(&link), "started by the rescan");
        until("the loop to park", || {
            peer.parked.load(Ordering::SeqCst) == 1
        })
        .await;
        let row = ctx.store.lock().unwrap().peer_link(link).unwrap().unwrap();
        assert_eq!(row.state, "connected", "{:?}", row.last_error);
        assert_eq!(peer.handshakes.load(Ordering::SeqCst), 1);

        ctx.store.lock().unwrap().revoke_peer_link(link, 1).unwrap();
        reconcile(&ctx, &mut running, listed(&ctx)).await;
        let r = running.remove(&link).expect("tracked until reaped");
        assert!(r.cancel.is_cancelled(), "the rescan cancelled it");
        let exit = tokio::time::timeout(STOP_WAIT, r.handle)
            .await
            .expect("the loop stops within STOP_WAIT")
            .unwrap();
        assert_eq!(exit, LinkExit::Cancelled, "stopped by the supervisor");
        assert_eq!(
            peer.dropped.load(Ordering::SeqCst),
            1,
            "the parked poll was dropped"
        );

        // The rescan after: nothing to reap, and the revoked link is not
        // started again.
        reap(&mut running).await;
        reconcile(&ctx, &mut running, listed(&ctx)).await;
        assert!(running.is_empty(), "a revoked link is not restarted");
        assert_eq!(
            peer.handshakes.load(Ordering::SeqCst),
            1,
            "not dialled again"
        );
        assert_eq!(peer.parked.load(Ordering::SeqCst), 1);
    }
}
