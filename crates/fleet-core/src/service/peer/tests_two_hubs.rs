//! Two hubs in one process: A dials B through a fake `PeerCall` that calls
//! B's `listen::exchange` directly and can drop, fail or refuse a call. One
//! test per row of the spec's crash table, plus the refusals.
//!
//! Faults are chosen by CONTENT, not by call order: `DropWhenSending` and
//! `DropWhenCarrying` lose the answer of the first call that carries
//! something, after B committed; `Transport` and `Refuse` apply to the next
//! call STARTED after the test arms them, and a test arms them only while a
//! parked long-poll is in flight, so the call they hit is the one the test
//! triggers. Every test that arms a fault asserts it fired.

use super::dial::*;
use super::testkit::*;
use super::wire::*;
use crate::ssh::SshClient;
use crate::store::{PeerLinkRow, SessionMessage, Store};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    /// Drop the answer of the first call whose `send` is non-empty, after
    /// the listener committed it.
    DropWhenSending,
    /// Drop the first answer whose `messages` is non-empty.
    DropWhenCarrying,
    /// Fail the next call started, before it reaches the listener.
    Transport,
    /// Answer the next call started with this listener error code.
    Refuse(&'static str),
}

#[derive(Default)]
struct Armed {
    drop_when_sending: bool,
    drop_when_carrying: bool,
    next: Option<Fault>,
}

struct Loopback {
    b: Arc<Mutex<Store>>,
    b_ssh: Arc<SshClient>,
    client_id: i64,
    armed: Mutex<Armed>,
    /// Every fault that fired, in order.
    fired: Mutex<Vec<Fault>>,
    /// While set, every call fails as a transport error: B is down.
    down: AtomicBool,
    down_hits: AtomicUsize,
    calls: AtomicUsize,
    /// Calls parked in B's long-poll right now.
    parked: AtomicUsize,
    /// Calls dropped by the dialer before they answered.
    dropped: AtomicUsize,
}

/// Counts a call as parked while it is, and as dropped if the dialer drops
/// it before it answers.
struct CallGuard<'a> {
    lb: &'a Loopback,
    parked: bool,
    done: bool,
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        if self.parked {
            self.lb.parked.fetch_sub(1, Ordering::SeqCst);
        }
        if !self.done {
            self.lb.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Loopback {
    fn arm(&self, f: Fault) {
        let mut a = self.armed.lock().unwrap();
        match f {
            Fault::DropWhenSending => a.drop_when_sending = true,
            Fault::DropWhenCarrying => a.drop_when_carrying = true,
            Fault::Transport | Fault::Refuse(_) => a.next = Some(f),
        }
    }
    fn fired(&self) -> Vec<Fault> {
        self.fired.lock().unwrap().clone()
    }
    fn fire(&self, f: Fault) {
        self.fired.lock().unwrap().push(f);
    }
}

#[async_trait::async_trait]
impl PeerCall for Loopback {
    async fn exchange(
        &self,
        req: &ExchangeRequest,
        timeout: Duration,
    ) -> Result<ExchangeResponse, CallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.armed.lock().unwrap().next.take();
        if self.down.load(Ordering::SeqCst) {
            self.down_hits.fetch_add(1, Ordering::SeqCst);
            return Err(CallError::Transport("peer is down".into()));
        }
        match next {
            Some(Fault::Transport) => {
                self.fire(Fault::Transport);
                return Err(CallError::Transport("connect refused".into()));
            }
            // As the listener would answer it, through the same classifier
            // as `HttpPeerCall`.
            Some(f @ Fault::Refuse(code)) => {
                self.fire(f);
                return Err(classify(code, "no".into()));
            }
            _ => {}
        }
        let parked = req.wait_ms > 0 && req.send.is_empty() && req.results.is_empty();
        if parked {
            self.parked.fetch_add(1, Ordering::SeqCst);
        }
        let mut guard = CallGuard {
            lb: self,
            parked,
            done: false,
        };
        let got = tokio::time::timeout(
            timeout,
            super::listen::exchange(&self.b, &self.b_ssh, self.client_id, req.clone()),
        )
        .await;
        guard.done = true;
        drop(guard);
        let got = got
            .map_err(|_| CallError::Transport("timeout".into()))?
            .map_err(|e| classify(&e.code, e.message))?;
        {
            let mut a = self.armed.lock().unwrap();
            if a.drop_when_sending && !req.send.is_empty() {
                a.drop_when_sending = false;
                drop(a);
                self.fire(Fault::DropWhenSending);
                return Err(CallError::Transport(
                    "connection reset after the peer committed".into(),
                ));
            }
            if a.drop_when_carrying && !got.messages.is_empty() {
                a.drop_when_carrying = false;
                drop(a);
                self.fire(Fault::DropWhenCarrying);
                return Err(CallError::Transport(
                    "connection reset after the peer answered".into(),
                ));
            }
        }
        Ok(got)
    }
}

struct Pair {
    a: Arc<Mutex<Store>>,
    a_ssh: Arc<SshClient>,
    a1: i64,
    b: Arc<Mutex<Store>>,
    b_ssh: Arc<SshClient>,
    b1: i64,
    link: i64,
    call: Arc<Loopback>,
    cancel: CancellationToken,
}

fn pair_of(a_fleet: &str, b_fleet: &str) -> Pair {
    let (a, a_ssh) = hub(a_fleet);
    let (b, b_ssh) = hub(b_fleet);
    let a1 = session(&a, "a1");
    let b1 = session(&b, "b1");
    let client_id = peer_client(&b, "hub-a");
    let link = a
        .lock()
        .unwrap()
        .insert_dialer_link("https://b.example", "t")
        .unwrap();
    let call = Arc::new(Loopback {
        b: b.clone(),
        b_ssh: b_ssh.clone(),
        client_id,
        armed: Mutex::new(Armed::default()),
        fired: Mutex::new(vec![]),
        down: AtomicBool::new(false),
        down_hits: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        parked: AtomicUsize::new(0),
        dropped: AtomicUsize::new(0),
    });
    Pair {
        a,
        a_ssh,
        a1,
        b,
        b_ssh,
        b1,
        link,
        call,
        cancel: CancellationToken::new(),
    }
}

fn pair() -> Pair {
    pair_of("fleet-a", "fleet-b")
}

fn args(from: i64, to_addr: &str, body: &str) -> crate::service::messages::SendMessageArgs {
    crate::service::messages::SendMessageArgs {
        from_session_id: from,
        to_session_id: 0,
        to_addr: Some(to_addr.into()),
        body: body.into(),
        kind: None,
        deliver: false,
        submit: true,
        reply_to: None,
        wake: false,
    }
}

impl Pair {
    fn start(&self) -> tokio::task::JoinHandle<LinkExit> {
        self.start_link(self.link)
    }
    fn start_link(&self, link: i64) -> tokio::task::JoinHandle<LinkExit> {
        tokio::spawn(run_link(
            self.a.clone(),
            self.a_ssh.clone(),
            link,
            self.call.clone(),
            self.cancel.clone(),
        ))
    }
    async fn send_a_to_b(&self, body: &str) -> i64 {
        let m = args(self.a1, "fleet-b/session/local/b1", body);
        crate::service::messages::send_message(m, &self.a, &self.a_ssh)
            .await
            .unwrap()
            .id
    }
    async fn send_b_to_a(&self, body: &str) -> i64 {
        let m = args(self.b1, "fleet-a/session/local/a1", body);
        crate::service::messages::send_message(m, &self.b, &self.b_ssh)
            .await
            .unwrap()
            .id
    }
    async fn until<F: Fn() -> bool>(&self, what: &str, f: F) {
        for _ in 0..100 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for: {what}");
    }
    fn row(&self) -> PeerLinkRow {
        self.a
            .lock()
            .unwrap()
            .peer_link(self.link)
            .unwrap()
            .unwrap()
    }
    async fn handshake(&self) {
        self.until("handshake", || self.row().fleet_id.is_some())
            .await;
    }
    /// A long-poll is parked on B: the next call the dialer starts is the
    /// one a test triggers.
    async fn parked(&self) {
        self.until("a parked long-poll", || {
            self.call.parked.load(Ordering::SeqCst) > 0
        })
        .await;
    }
    fn a_inbox(&self) -> Vec<SessionMessage> {
        self.a
            .lock()
            .unwrap()
            .list_inbox(self.a1, false, 50)
            .unwrap()
    }
    fn b_inbox(&self) -> Vec<SessionMessage> {
        self.b
            .lock()
            .unwrap()
            .list_inbox(self.b1, false, 50)
            .unwrap()
    }
    fn a_pending(&self) -> usize {
        self.a
            .lock()
            .unwrap()
            .pending_outbox(self.link, 0, 50)
            .unwrap()
            .len()
    }
    /// B's pending outbox on its listener link. Takes the id: the caller
    /// must not hold B's lock (it is not re-entrant).
    fn b_pending(&self, b_link: i64) -> usize {
        self.b
            .lock()
            .unwrap()
            .pending_outbox(b_link, 0, 50)
            .unwrap()
            .len()
    }
    fn b_listener(&self) -> PeerLinkRow {
        self.b
            .lock()
            .unwrap()
            .live_peer_link_for_fleet("fleet-a")
            .unwrap()
            .unwrap()
    }
    fn events(store: &Mutex<Store>, session: i64) -> Vec<crate::store::SessionEvent> {
        store
            .lock()
            .unwrap()
            .list_session_events(session, 50)
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_crosses_fast_and_a_reply_threads_back() {
    let p = pair();
    let h = p.start();
    // Handshake first so the address resolves on A.
    p.handshake().await;
    p.parked().await;
    let t0 = std::time::Instant::now();
    let sent = p.send_a_to_b("ping").await;
    p.until("B has it", || p.b_inbox().len() == 1).await;
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "A->B took {:?}",
        t0.elapsed()
    );
    assert!(
        p.call.dropped.load(Ordering::SeqCst) >= 1,
        "the parked poll was dropped to send"
    );
    let got = p.b_inbox().remove(0);
    assert_eq!(got.from_addr.as_deref(), Some("fleet-a/session/local/a1"));
    // B replies by address with reply_to.
    p.parked().await;
    let mut reply = args(p.b1, got.from_addr.as_deref().unwrap(), "pong");
    reply.reply_to = Some(got.id);
    let t1 = std::time::Instant::now();
    crate::service::messages::send_message(reply, &p.b, &p.b_ssh)
        .await
        .unwrap();
    p.until("A has the reply", || {
        p.a_inbox().iter().any(|m| m.body.ends_with("pong"))
    })
    .await;
    assert!(
        t1.elapsed() < Duration::from_secs(2),
        "B->A took {:?}",
        t1.elapsed()
    );
    let back = p.a_inbox().remove(0);
    assert_eq!(
        back.reply_to,
        Some(sent),
        "the thread maps back to A's own id"
    );
    assert_eq!(p.row().state, "connected");
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
}

/// Crash row 1: the listener inserted A's message and the response was lost.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_response_after_the_peer_committed_inserts_once() {
    let p = pair();
    p.call.arm(Fault::DropWhenSending);
    let h = p.start();
    p.handshake().await;
    p.send_a_to_b("once").await;
    p.until("accepted on A", || p.a_pending() == 0).await;
    assert_eq!(p.call.fired(), vec![Fault::DropWhenSending]);
    assert_eq!(p.b_inbox().len(), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.b_inbox().len(), 1);
    p.cancel.cancel();
    h.await.unwrap();
}

/// Crash row 2, first half: B's batch never reached A. `after` did not
/// move, so B hands the batch over again.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_that_lost_bs_batch_gets_it_again_and_stores_it_once() {
    let p = pair();
    p.call.arm(Fault::DropWhenCarrying);
    let h = p.start();
    p.handshake().await;
    p.send_b_to_a("to a").await;
    p.until("A has it", || !p.a_inbox().is_empty()).await;
    assert_eq!(p.call.fired(), vec![Fault::DropWhenCarrying]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.a_inbox().len(), 1);
    p.cancel.cancel();
    h.await.unwrap();
}

/// Crash row 2, second half: A stored B's batch and crashed before its
/// watermark reached B. B hands the batch over again; A dedupes on the key.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_that_crashed_before_its_watermark_stores_the_batch_once() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    let m = p.send_b_to_a("to a").await;
    p.until("A has it", || p.a_inbox().len() == 1).await;
    let b_link = p.b_listener().id;
    p.until("B handed it over", || p.b_pending(b_link) == 0)
        .await;
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
    // The crash: A's watermark never committed, so B never saw it.
    p.a.lock()
        .unwrap()
        .set_peer_link_progress(p.link, 0, None, 0)
        .unwrap();
    p.b.lock()
        .unwrap()
        .conn_ref()
        .execute(
            "UPDATE session_messages SET peer_state = 'pending' WHERE id = ?1",
            [m],
        )
        .unwrap();
    let calls = p.call.calls.load(Ordering::SeqCst);
    let restart = CancellationToken::new();
    let h = tokio::spawn(run_link(
        p.a.clone(),
        p.a_ssh.clone(),
        p.link,
        p.call.clone(),
        restart.clone(),
    ));
    assert_eq!(p.b_pending(b_link), 1);
    p.until("B handed it over again", || p.b_pending(b_link) == 0)
        .await;
    assert!(p.call.calls.load(Ordering::SeqCst) > calls);
    assert!(p.row().after >= m);
    assert_eq!(p.a_inbox().len(), 1, "the resend is a duplicate");
    restart.cancel();
    h.await.unwrap();
}

/// Crash row 3: the dialer drops its parked call to send. Nothing is lost
/// either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_that_drops_its_parked_call_to_send_loses_nothing() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    let dropped = p.call.dropped.load(Ordering::SeqCst);
    p.send_a_to_b("a1").await;
    p.send_b_to_a("b1").await;
    p.send_a_to_b("a2").await;
    p.until("both sides have theirs", || {
        p.b_inbox().len() == 2 && p.a_inbox().len() == 1
    })
    .await;
    assert!(
        p.call.dropped.load(Ordering::SeqCst) > dropped,
        "a parked call was dropped"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!((p.b_inbox().len(), p.a_inbox().len()), (2, 1));
    assert_eq!(p.a_pending(), 0);
    p.cancel.cancel();
    h.await.unwrap();
}

/// Crash row 4: either hub restarts; the loop resumes from the rows.
#[tokio::test(flavor = "multi_thread")]
async fn either_hub_restarting_resumes_from_the_rows() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    // B goes down: the dialer retries with backoff.
    p.call.down.store(true, Ordering::SeqCst);
    p.send_a_to_b("while b is down").await;
    p.until("retrying", || p.row().state == "retrying").await;
    assert!(p.call.down_hits.load(Ordering::SeqCst) >= 1);
    assert_eq!(p.row().last_error.as_deref(), Some("peer is down"));
    p.call.down.store(false, Ordering::SeqCst);
    p.until("B back, has it", || p.b_inbox().len() == 1).await;
    // A goes down: its loop stops; both sides queue meanwhile.
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
    p.send_a_to_b("while a is down").await;
    p.send_b_to_a("to a while a is down").await;
    let restart = CancellationToken::new();
    let h = tokio::spawn(run_link(
        p.a.clone(),
        p.a_ssh.clone(),
        p.link,
        p.call.clone(),
        restart.clone(),
    ));
    p.until("resumed both ways", || {
        p.b_inbox().len() == 2 && p.a_inbox().len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!((p.b_inbox().len(), p.a_inbox().len()), (2, 1));
    restart.cancel();
    h.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transport_failure_retries_and_a_refusal_stops_the_link() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    p.call.arm(Fault::Transport);
    p.send_a_to_b("x").await; // drops the parked poll; the next call fails
    p.until("retrying", || p.row().state == "retrying").await;
    p.until("recovered", || {
        p.row().state == "connected" && p.b_inbox().len() == 1
    })
    .await;
    assert_eq!(p.call.fired(), vec![Fault::Transport]);
    p.parked().await;
    p.call.arm(Fault::Refuse("E_UNAUTHORIZED"));
    p.send_a_to_b("y").await; // wakes the parked poll; the next call is refused
    let exit = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Refused);
    assert_eq!(
        p.call.fired(),
        vec![Fault::Transport, Fault::Refuse("E_UNAUTHORIZED")]
    );
    let row = p.row();
    assert_eq!(row.state, "refused");
    assert!(
        row.last_error
            .as_deref()
            .unwrap_or("")
            .starts_with("E_UNAUTHORIZED"),
        "{:?}",
        row.last_error
    );
    assert_eq!(p.a_pending(), 1, "kept for a re-pair");
}

/// Every non-terminal code from the peer is a transport failure: back off
/// and retry, never stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limited_peer_is_retried_not_refused() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    p.call.arm(Fault::Refuse("E_RATE_LIMITED"));
    p.send_a_to_b("x").await;
    p.until("retrying", || p.row().state == "retrying").await;
    assert!(p
        .row()
        .last_error
        .as_deref()
        .unwrap_or("")
        .starts_with("E_RATE_LIMITED"));
    p.until("delivered", || p.b_inbox().len() == 1).await;
    assert_eq!(p.call.fired(), vec![Fault::Refuse("E_RATE_LIMITED")]);
    assert!(!h.is_finished());
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_proto_is_incompatible() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    p.call.arm(Fault::Refuse("E_UNSUPPORTED"));
    p.send_a_to_b("x").await;
    let exit = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Incompatible);
    assert_eq!(p.call.fired(), vec![Fault::Refuse("E_UNSUPPORTED")]);
    assert_eq!(p.row().state, "incompatible");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_rejection_becomes_undeliverable_for_the_sender() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    let m = args(p.a1, "fleet-b/session/local/nobody", "lost");
    crate::service::messages::send_message(m, &p.a, &p.a_ssh)
        .await
        .unwrap();
    p.until("undeliverable", || {
        Pair::events(&p.a, p.a1).iter().any(|e| {
            e.kind == "message_undeliverable"
                && e.detail
                    .as_deref()
                    .unwrap_or("")
                    .contains("E_PARTICIPANT_UNKNOWN")
        })
    })
    .await;
    assert_eq!(p.a_pending(), 0);
    p.cancel.cancel();
    h.await.unwrap();
}

/// A peer that claims a third fleet's sender is rejected back to it: A's
/// rejection rides the next request and B's sender learns it.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_claiming_a_third_fleet_is_rejected_back_to_its_sender() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    {
        let s = p.b.lock().unwrap();
        let link = s.live_peer_link_for_fleet("fleet-a").unwrap().unwrap();
        let to = s
            .ensure_remote_participant(link.id, "fleet-a/session/local/a1")
            .unwrap();
        s.insert_outbound_remote(
            p.b1,
            "fleet-c/session/local/c1",
            to,
            "spoofed",
            "message",
            None,
            false,
        )
        .unwrap();
    }
    p.until("B's sender learns", || {
        Pair::events(&p.b, p.b1).iter().any(|e| {
            e.kind == "message_undeliverable"
                && e.detail.as_deref().unwrap_or("").contains("E_FORBIDDEN")
        })
    })
    .await;
    assert!(p.a_inbox().is_empty());
    p.until("rejections cleared", || p.row().pending_rejects.is_none())
        .await;
    p.cancel.cancel();
    h.await.unwrap();
}

/// B revoked the link: its listener refuses, and A stops.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_the_peer_revoked_is_refused() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    let listener = p.b_listener().id;
    p.b.lock().unwrap().revoke_peer_link(listener, 1).unwrap();
    p.send_a_to_b("x").await;
    let exit = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Refused);
    let row = p.row();
    assert_eq!(row.state, "refused");
    assert!(
        row.last_error
            .as_deref()
            .unwrap_or("")
            .starts_with("E_FORBIDDEN"),
        "{:?}",
        row.last_error
    );
    assert_eq!(p.a_pending(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_self_link_is_refused() {
    let p = pair_of("fleet-a", "fleet-a");
    let exit = tokio::time::timeout(Duration::from_secs(5), p.start())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Refused);
    assert_eq!(p.row().state, "refused");
    assert!(p.row().fleet_id.is_none());
}

/// The link is pinned to one fleet; a peer answering as another is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_answering_as_another_fleet_is_refused() {
    let p = pair();
    p.a.lock()
        .unwrap()
        .adopt_dialer_fleet(p.link, "fleet-x")
        .unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(5), p.start())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Refused);
    let row = p.row();
    assert_eq!(row.state, "refused");
    assert_eq!(row.fleet_id.as_deref(), Some("fleet-x"));
}

/// A re-pair of a fleet that already has a dialer row: the handshake moves
/// the new url and token onto the old row, drops the new one, and the old
/// row's pending messages go out over it.
#[tokio::test(flavor = "multi_thread")]
async fn a_re_pair_rebinds_onto_the_older_row_and_its_pending_rows_go_out() {
    let p = pair();
    let old = {
        let s = p.a.lock().unwrap();
        let old = s.insert_dialer_link("https://old.example", "old").unwrap();
        s.adopt_dialer_fleet(old, "fleet-b").unwrap();
        s.set_peer_link_state(old, "refused", Some("E_UNAUTHORIZED: gone"), 0)
            .unwrap();
        old
    };
    // Queued on the refused row before the re-pair.
    p.send_a_to_b("kept").await;
    assert_eq!(
        p.a.lock()
            .unwrap()
            .pending_outbox(old, 0, 50)
            .unwrap()
            .len(),
        1
    );
    let exit = tokio::time::timeout(Duration::from_secs(5), p.start())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Rebound(old));
    let (gone, kept) = {
        let s = p.a.lock().unwrap();
        (
            s.peer_link(p.link).unwrap(),
            s.peer_link(old).unwrap().unwrap(),
        )
    };
    assert!(gone.is_none(), "the temporary row is dropped");
    assert_eq!(kept.url.as_deref(), Some("https://b.example"));
    assert_eq!(kept.state, "retrying");
    let h = p.start_link(old);
    p.until("B has it", || p.b_inbox().len() == 1).await;
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
}

// ---- the supervisor -----------------------------------------------------------

struct Unreachable {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::http_client::HubTransport for Unreachable {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        _body: String,
    ) -> Result<crate::http_client::HubResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("connection refused".into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_supervisor_runs_a_dialer_link_and_stops_it_once_revoked() {
    let (a, a_ssh) = hub("fleet-a");
    let link = a
        .lock()
        .unwrap()
        .insert_dialer_link("https://b.example", "t")
        .unwrap();
    let transport = Arc::new(Unreachable {
        calls: AtomicUsize::new(0),
    });
    let cancel = CancellationToken::new();
    let h = super::supervisor::spawn_peer_supervisor(
        a.clone(),
        a_ssh,
        transport.clone(),
        cancel.clone(),
    );
    let row = || a.lock().unwrap().peer_link(link).unwrap().unwrap();
    let t0 = std::time::Instant::now();
    loop {
        let r = row();
        if r.last_error.is_some() {
            assert_eq!(r.state, "retrying");
            assert!(
                r.last_error
                    .as_deref()
                    .unwrap()
                    .contains("connection refused"),
                "{:?}",
                r.last_error
            );
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(1), "no attempt in 1 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    a.lock().unwrap().revoke_peer_link(link, 1).unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    let settled = transport.calls.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        settled,
        "a revoked link is no longer dialled"
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(6), h)
        .await
        .unwrap()
        .unwrap();
}
