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
use crate::store::{Adopted, PeerLinkRow, SessionMessage, Store};
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
    /// Hold the next call started until the test releases it, then answer
    /// it with this listener error code: a call in flight across a re-pair.
    HoldThenRefuse(&'static str),
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
    /// A `HoldThenRefuse` call is waiting for `release`.
    held: AtomicBool,
    release: tokio::sync::Notify,
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
            Fault::Transport | Fault::Refuse(_) | Fault::HoldThenRefuse(_) => a.next = Some(f),
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
            Some(f @ Fault::HoldThenRefuse(code)) => {
                self.held.store(true, Ordering::SeqCst);
                self.release.notified().await;
                self.fire(f);
                return Err(classify(code, "no".into()));
            }
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
    let call = loopback(&b, &b_ssh, client_id);
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

/// A fake transport onto hub `b`'s listener, as the client `client_id`.
fn loopback(b: &Arc<Mutex<Store>>, b_ssh: &Arc<SshClient>, client_id: i64) -> Arc<Loopback> {
    Arc::new(Loopback {
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
        held: AtomicBool::new(false),
        release: tokio::sync::Notify::new(),
    })
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
    /// A loop on `link` with the credentials the row carries now, as the
    /// supervisor would start it.
    fn start_link(&self, link: i64) -> tokio::task::JoinHandle<LinkExit> {
        let token = self.token_of(link);
        tokio::spawn(run_link(
            self.a.clone(),
            self.a_ssh.clone(),
            link,
            token,
            self.call.clone(),
            self.cancel.clone(),
        ))
    }
    fn token_of(&self, link: i64) -> String {
        self.a
            .lock()
            .unwrap()
            .peer_link(link)
            .unwrap()
            .unwrap()
            .token
            .unwrap()
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
    let token = p.token_of(p.link);
    p.a.lock()
        .unwrap()
        .set_dialer_link_progress(p.link, &token, 0, None, 0)
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
        p.token_of(p.link),
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
    // B sends only once the dialer has dropped its parked call to carry
    // a1: sent sooner, b1 answers that call and a1 rides the next one, so
    // nothing is ever dropped.
    p.until("the parked call was dropped to carry a1", || {
        p.call.dropped.load(Ordering::SeqCst) > dropped
    })
    .await;
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
        p.token_of(p.link),
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

/// C1: a newly paired hub C whose handshake claims fleet-b — which A
/// already has a connected link to — cannot take that link over. C's own
/// row is refused the fleet: it waits, naming the live link, and never
/// merges while that link works; the live link keeps its url, token and
/// state, and A's messages still go to B.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_peer_claiming_a_connected_links_fleet_is_refused() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.until("connected", || p.row().state == "connected").await;
    // Hub C answers as fleet-b.
    let (c, c_ssh) = hub("fleet-b");
    let c_client = peer_client(&c, "hub-a");
    let claimant =
        p.a.lock()
            .unwrap()
            .insert_dialer_link("https://c.example", "t-c")
            .unwrap();
    let c_call = loopback(&c, &c_ssh, c_client);
    let claim_loop = tokio::spawn(run_link(
        p.a.clone(),
        p.a_ssh.clone(),
        claimant,
        "t-c".into(),
        c_call.clone(),
        p.cancel.clone(),
    ));
    let claim_row = || p.a.lock().unwrap().peer_link(claimant).unwrap();
    p.until("the claim waits", || {
        claim_row().is_some_and(|r| r.last_error.is_some())
    })
    .await;
    // C has a message for A's session: it must not land while C waits.
    let c1 = session(&c, "c1");
    crate::service::messages::send_message(
        args(c1, "fleet-a/session/local/a1", "from c"),
        &c,
        &c_ssh,
    )
    .await
    .unwrap();
    // More handshakes: the claim is re-checked, and refused again.
    p.until("the claim was checked again", || {
        c_call.calls.load(Ordering::SeqCst) >= 3
    })
    .await;
    p.send_a_to_b("still to b").await;
    p.until("B has it", || p.b_inbox().len() == 1).await;
    let live = p.row();
    let claim = claim_row().expect("the claimant row stays");
    assert_eq!(live.url.as_deref(), Some("https://b.example"));
    assert_eq!(live.token.as_deref(), Some("t"));
    assert_eq!(live.state, "connected");
    assert_eq!(claim.state, "retrying");
    assert!(claim.fleet_id.is_none());
    assert_eq!(claim.token.as_deref(), Some("t-c"));
    let why = claim.last_error.unwrap_or_default();
    assert!(
        why.contains(&format!(
            "fleet fleet-b is still linked (link {}); waiting for it to stop",
            p.link
        )) && !why.contains("peer remove"),
        "{why}"
    );
    assert!(!claim_loop.is_finished());
    assert!(p.a_inbox().is_empty(), "C's message landed while it waits");
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
    assert_eq!(claim_loop.await.unwrap(), LinkExit::Cancelled);
}

/// The documented re-pair, as it races: B revokes A's old peer client and
/// A adds the new code while A's old loop still has a call in flight on the
/// old token, so the old row still reads `connected`. The new row's
/// handshake reaches B (which rebinds to the new token) but must not strand
/// the link: it waits `retrying`, delivering and accepting nothing, until
/// the old loop is refused, then merges — and the link ends `connected` on
/// the new token with every waiting message, both ways, delivered once.
#[tokio::test(flavor = "multi_thread")]
async fn a_re_pair_racing_the_old_loop_waits_for_it_and_then_merges() {
    let p = pair();
    let old_loop = p.start();
    p.handshake().await;
    p.parked().await;
    // B: the old peer client is revoked and a new one paired.
    p.b.lock().unwrap().revoke_client_token("hub-a").unwrap();
    let c2 = peer_client(&p.b, "hub-a-2");
    let new_call = loopback(&p.b, &p.b_ssh, c2);
    // A's old loop: its next call is in flight across the re-pair, and B's
    // auth layer answers it 401 once released (the old token is revoked).
    p.call.arm(Fault::HoldThenRefuse("E_UNAUTHORIZED"));
    p.send_a_to_b("waiting on the old link").await;
    p.until("the old call is in flight", || {
        p.call.held.load(Ordering::SeqCst)
    })
    .await;
    assert_eq!(p.row().state, "connected");
    // A: `peer add` with the new code.
    let tmp =
        p.a.lock()
            .unwrap()
            .insert_dialer_link("https://b.example", "t2-new-token")
            .unwrap();
    let new_loop = tokio::spawn(run_link(
        p.a.clone(),
        p.a_ssh.clone(),
        tmp,
        "t2-new-token".into(),
        new_call.clone(),
        p.cancel.clone(),
    ));
    let tmp_row = || p.a.lock().unwrap().peer_link(tmp).unwrap();
    let waiting = format!(
        "fleet fleet-b is still linked (link {}); waiting for it to stop",
        p.link
    );
    p.until("the new row waits", || {
        tmp_row().is_some_and(|r| {
            r.state == "retrying" && r.last_error.as_deref().unwrap_or("").contains(&waiting)
        })
    })
    .await;
    // While it waits, B has a message for A: the new row must not take it.
    p.send_b_to_a("waiting on B").await;
    p.until("the new row asked again", || {
        new_call.calls.load(Ordering::SeqCst) >= 2
    })
    .await;
    assert!(p.a_inbox().is_empty(), "accepted before the merge");
    assert!(p.b_inbox().is_empty(), "delivered before the merge");
    let r = tmp_row().expect("the new row stays until the merge");
    assert!(r.fleet_id.is_none());
    assert!(
        !r.last_error.unwrap_or_default().contains("peer remove"),
        "never tell the operator to fail the waiting messages"
    );
    assert!(!new_loop.is_finished());
    let old = p.row();
    assert_eq!(
        (old.state.as_str(), old.token.as_deref()),
        ("connected", Some("t"))
    );
    // The old call comes back refused.
    p.call.release.notify_one();
    let exit = tokio::time::timeout(Duration::from_secs(5), old_loop)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, LinkExit::Refused);
    let exit = tokio::time::timeout(Duration::from_secs(10), new_loop)
        .await
        .expect("the new row merges once the old one stopped")
        .unwrap();
    assert_eq!(exit, LinkExit::Rebound(p.link));
    assert!(tmp_row().is_none(), "the temporary row is dropped");
    let kept = p.row();
    assert_eq!(kept.token.as_deref(), Some("t2-new-token"));
    assert_eq!(kept.state, "retrying");
    // The supervisor starts the kept row on its new credentials.
    let h = tokio::spawn(run_link(
        p.a.clone(),
        p.a_ssh.clone(),
        p.link,
        "t2-new-token".into(),
        new_call.clone(),
        p.cancel.clone(),
    ));
    p.until("connected on the new token, both ways", || {
        p.row().state == "connected"
            && p.a_pending() == 0
            && p.b_inbox().len() == 1
            && p.a_inbox().len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.b_inbox().len(), 1);
    assert_eq!(p.a_inbox().len(), 1);
    assert!(p.b_inbox()[0].body.ends_with("waiting on the old link"));
    assert!(p.a_inbox()[0].body.ends_with("waiting on B"));
    assert_eq!(p.row().token.as_deref(), Some("t2-new-token"));
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
}

// ---- fix round 1 --------------------------------------------------------------

/// I1: a loop started with credentials that a re-pair has since replaced
/// cannot write over the row. Its call is in flight across the re-pair and
/// comes back refused (the old token is revoked); the row keeps the new
/// credentials and stays `retrying`, and a loop on them connects.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_loops_refusal_cannot_clobber_a_re_paired_row() {
    let p = pair();
    let stale = p.start();
    p.handshake().await;
    p.parked().await;
    p.call.arm(Fault::HoldThenRefuse("E_UNAUTHORIZED"));
    p.send_a_to_b("queued across the re-pair").await;
    p.until("the stale call is in flight", || {
        p.call.held.load(Ordering::SeqCst)
    })
    .await;
    {
        let s = p.a.lock().unwrap();
        // A re-pair merges only into a stopped row (C1): the row reads
        // `refused` while the stale loop's call is still in flight.
        s.set_peer_link_state(p.link, "refused", Some("E_UNAUTHORIZED: gone"), 0)
            .unwrap();
        let tmp = s
            .insert_dialer_link("https://b2.example", "t2-new-token")
            .unwrap();
        assert_eq!(
            s.adopt_dialer_fleet(tmp, "fleet-b").unwrap(),
            Adopted::Link(p.link)
        );
    }
    p.call.release.notify_one();
    let exit = tokio::time::timeout(Duration::from_secs(5), stale)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        p.call.fired(),
        vec![Fault::HoldThenRefuse("E_UNAUTHORIZED")]
    );
    let row = p.row();
    assert_eq!(row.state, "retrying", "{:?}", row.last_error);
    assert!(row.last_error.is_none(), "{:?}", row.last_error);
    assert_eq!(row.token.as_deref(), Some("t2-new-token"));
    assert_eq!(exit, LinkExit::Superseded);
    let h = p.start();
    p.until("connected on the new credentials", || {
        p.row().state == "connected" && p.b_inbox().len() == 1
    })
    .await;
    p.cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
}

fn inject_store_fault(s: &Mutex<Store>) {
    s.lock()
        .unwrap()
        .conn_ref()
        .execute_batch(
            "CREATE TEMP TRIGGER boom BEFORE INSERT ON session_messages \
             WHEN NEW.body LIKE '%boom%' \
             BEGIN SELECT RAISE(ABORT, 'injected store fault'); END;",
        )
        .unwrap();
}

fn heal_store_fault(s: &Mutex<Store>) {
    s.lock()
        .unwrap()
        .conn_ref()
        .execute_batch("DROP TRIGGER temp.boom;")
        .unwrap();
}

fn undeliverable(store: &Mutex<Store>, session: i64) -> usize {
    Pair::events(store, session)
        .iter()
        .filter(|e| e.kind == "message_undeliverable")
        .count()
}

/// A store fault on the listener while it applies A's item is not A's
/// fault: the exchange fails (E_INTERNAL, retried), nothing is rejected,
/// and the retry delivers the item once.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_fault_on_the_listener_is_retried_not_rejected() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    inject_store_fault(&p.b);
    p.send_a_to_b("boom").await;
    p.until("retrying on E_INTERNAL", || {
        let r = p.row();
        r.state == "retrying"
            && r.last_error
                .as_deref()
                .unwrap_or("")
                .starts_with("E_INTERNAL")
    })
    .await;
    assert_eq!(p.a_pending(), 1);
    assert_eq!(undeliverable(&p.a, p.a1), 0);
    heal_store_fault(&p.b);
    p.until("delivered", || p.b_inbox().len() == 1).await;
    p.until("accepted", || p.a_pending() == 0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.b_inbox().len(), 1);
    assert_eq!(undeliverable(&p.a, p.a1), 0);
    p.cancel.cancel();
    h.await.unwrap();
}

/// The dialer's side: a store fault while applying B's item leaves `after`
/// where it was and rejects nothing; the next exchange stores it once.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_fault_on_the_dialer_is_retried_not_rejected() {
    let p = pair();
    let h = p.start();
    p.handshake().await;
    p.parked().await;
    inject_store_fault(&p.a);
    let m = p.send_b_to_a("boom").await;
    p.until("retrying", || {
        let r = p.row();
        r.state == "retrying" && r.last_error.is_some()
    })
    .await;
    let row = p.row();
    assert!(row.after < m, "after moved past an unstored item");
    assert!(row.pending_rejects.is_none(), "{:?}", row.pending_rejects);
    assert_eq!(undeliverable(&p.b, p.b1), 0);
    heal_store_fault(&p.a);
    p.until("stored", || p.a_inbox().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.a_inbox().len(), 1);
    assert_eq!(undeliverable(&p.b, p.b1), 0);
    p.cancel.cancel();
    h.await.unwrap();
}

/// M3: a fleet this hub already listens for cannot also be dialled; that
/// does not heal by retrying, so the link is refused and says why.
#[tokio::test(flavor = "multi_thread")]
async fn a_fleet_already_linked_the_other_way_is_refused_not_retried() {
    let p = pair();
    let c = peer_client(&p.a, "hub-b");
    p.a.lock()
        .unwrap()
        .ensure_listener_link(c, "fleet-b")
        .unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(5), p.start())
        .await
        .expect("the loop stops")
        .unwrap();
    assert_eq!(exit, LinkExit::Refused);
    let row = p.row();
    assert_eq!(row.state, "refused");
    assert!(
        row.last_error
            .as_deref()
            .unwrap_or("")
            .contains("already linked the other way"),
        "{:?}",
        row.last_error
    );
}

/// M4: a link whose state cannot be read says so on the row while it
/// retries, and recovers once the read works.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_cannot_be_read_says_so_and_recovers() {
    let p = pair();
    p.a.lock()
        .unwrap()
        .conn_ref()
        .execute_batch("ALTER TABLE participants RENAME TO participants_away;")
        .unwrap();
    let h = p.start();
    p.until("the read error is on the row", || {
        p.row().last_error.is_some()
    })
    .await;
    let row = p.row();
    assert_eq!(row.state, "retrying");
    assert!(
        row.last_error.as_deref().unwrap().contains("participants"),
        "{:?}",
        row.last_error
    );
    p.a.lock()
        .unwrap()
        .conn_ref()
        .execute_batch("ALTER TABLE participants_away RENAME TO participants;")
        .unwrap();
    p.until("connected", || p.row().state == "connected").await;
    p.cancel.cancel();
    h.await.unwrap();
}

// ---- review fixes (dialer) ------------------------------------------------------

/// A peer that answers every call at once, as `fleet-b` speaking `proto`,
/// with nothing in it. Records each call's `wait_ms`.
struct AnswersEmpty {
    proto: u32,
    waits: Mutex<Vec<u64>>,
}

impl AnswersEmpty {
    fn new(proto: u32) -> Arc<Self> {
        Arc::new(Self {
            proto,
            waits: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> usize {
        self.waits.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl PeerCall for AnswersEmpty {
    async fn exchange(
        &self,
        req: &ExchangeRequest,
        _timeout: Duration,
    ) -> Result<ExchangeResponse, CallError> {
        self.waits.lock().unwrap().push(req.wait_ms);
        Ok(ExchangeResponse {
            proto: self.proto,
            fleet_id: "fleet-b".into(),
            results: vec![],
            messages: vec![],
            more: false,
        })
    }
}

/// G3: a peer (or a proxy, or a second dialer on the same link releasing
/// this one's parked call) that answers every parked long-poll at once with
/// an empty page must not spin the dialer: an empty parked answer faster
/// than the floor is followed by the backoff.
// multi_thread: a hot loop must not starve the test's own clock.
#[tokio::test(flavor = "multi_thread")]
async fn an_always_empty_instant_answer_does_not_spin_the_dialer() {
    let (a, a_ssh) = hub("fleet-a");
    let link = a
        .lock()
        .unwrap()
        .insert_dialer_link("https://b.example", "t")
        .unwrap();
    let peer = AnswersEmpty::new(PROTO);
    let cancel = CancellationToken::new();
    let h = tokio::spawn(run_link(
        a.clone(),
        a_ssh,
        link,
        "t".into(),
        peer.clone(),
        cancel.clone(),
    ));
    tokio::time::sleep(Duration::from_secs(3)).await;
    let calls = peer.calls();
    let parked = peer
        .waits
        .lock()
        .unwrap()
        .iter()
        .filter(|w| **w > 0)
        .count();
    // Stopped BEFORE any assertion: a spinning loop never yields, and a
    // panic here would leave the runtime's drop waiting on it forever.
    cancel.cancel();
    assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
    assert!(parked >= 1, "the loop parked at least once: {calls} calls");
    assert!(
        calls <= 6,
        "{calls} calls in 3 s against an always-empty peer"
    );
    let row = a.lock().unwrap().peer_link(link).unwrap().unwrap();
    assert_eq!(row.state, "connected", "{:?}", row.last_error);
}

/// G26(d): a peer that answers in a proto this hub does not speak (the
/// response itself, not a refusal code) ends the link `incompatible`, says
/// which proto on the row, and pins no fleet.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_answering_in_another_proto_is_incompatible() {
    let (a, a_ssh) = hub("fleet-a");
    let link = a
        .lock()
        .unwrap()
        .insert_dialer_link("https://b.example", "t")
        .unwrap();
    let peer = AnswersEmpty::new(PROTO + 1);
    let exit = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::spawn(run_link(
            a.clone(),
            a_ssh,
            link,
            "t".into(),
            peer.clone(),
            CancellationToken::new(),
        )),
    )
    .await
    .expect("the loop stops")
    .unwrap();
    assert_eq!(exit, LinkExit::Incompatible);
    assert_eq!(peer.calls(), 1, "not retried");
    let row = a.lock().unwrap().peer_link(link).unwrap().unwrap();
    assert_eq!(row.state, "incompatible");
    assert!(row.fleet_id.is_none(), "{:?}", row.fleet_id);
    assert_eq!(
        row.last_error.as_deref(),
        Some(format!("the peer speaks proto {}, not {PROTO}", PROTO + 1).as_str())
    );
}

/// G10, the dialer's half over a real listener: B already dials A, so A's
/// handshake at B conflicts on B's side. B answers E_FORBIDDEN, and A's link
/// ends `refused` instead of retrying forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_already_dialling_us_refuses_the_link_terminally() {
    let p = pair();
    {
        let s = p.b.lock().unwrap();
        let b_dials_a = s.insert_dialer_link("https://a.example", "tb").unwrap();
        s.adopt_dialer_fleet(b_dials_a, "fleet-a").unwrap();
    }
    let exit = tokio::time::timeout(Duration::from_secs(5), p.start())
        .await
        .expect("the loop stops")
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
    assert_eq!(p.call.calls.load(Ordering::SeqCst), 1, "not retried");
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

/// B's `/mcp` as a `HubTransport`: the bearer names a peer client token on
/// B (401 once revoked, as the auth layer answers), the body is the
/// `tools/call` envelope `HttpPeerCall` sends, and the answer is framed as
/// rmcp frames it. Records which bearer each call carried.
struct HubOverLoopback {
    b: Arc<Mutex<Store>>,
    b_ssh: Arc<SshClient>,
    clients: Mutex<Vec<(String, i64)>>,
    seen: Mutex<Vec<String>>,
}

impl HubOverLoopback {
    fn calls_with(&self, bearer: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.as_str() == bearer)
            .count()
    }
}

#[async_trait::async_trait]
impl crate::http_client::HubTransport for HubOverLoopback {
    async fn post_json(
        &self,
        _url: &str,
        bearer: &str,
        body: String,
    ) -> Result<crate::http_client::HubResponse, String> {
        self.seen.lock().unwrap().push(bearer.to_string());
        let client = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .find(|(t, _)| t == bearer)
            .map(|(_, id)| *id);
        let live = client
            .map(|id| self.b.lock().unwrap().client_token_is_live(id).unwrap())
            .unwrap_or(false);
        let (Some(client), true) = (client, live) else {
            return Ok(crate::http_client::HubResponse {
                status: 401,
                body: String::new(),
            });
        };
        let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
        let req: ExchangeRequest =
            serde_json::from_value(envelope["params"]["arguments"].clone()).unwrap();
        let result = match super::listen::exchange(&self.b, &self.b_ssh, client, req).await {
            Ok(resp) => serde_json::json!({
                "content": [{"type": "text", "text": serde_json::to_string(&resp).unwrap()}],
            }),
            Err(e) => serde_json::json!({
                "isError": true,
                "content": [{"type": "text", "text": format!("{}: {}", e.code, e.message)}],
                "structuredContent": {"code": e.code, "message": e.message, "details": null},
            }),
        };
        let answer = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": result});
        Ok(crate::http_client::HubResponse {
            status: 200,
            body: format!("event: message\ndata: {answer}\n\n"),
        })
    }
}

/// I1, the supervisor half: a re-pair moves new credentials onto a row
/// whose loop is running. The old loop is stopped and a new one runs with
/// the new token — and only that one dials from then on.
#[tokio::test(flavor = "multi_thread")]
async fn the_supervisor_restarts_a_running_link_on_new_credentials() {
    const OLD: &str = "old-token-0000000000000000";
    const NEW: &str = "new-token-1111111111111111";
    let (a, a_ssh) = hub("fleet-a");
    let (b, b_ssh) = hub("fleet-b");
    let a1 = session(&a, "a1");
    let b1 = session(&b, "b1");
    let c_old = peer_client(&b, "hub-a");
    let link = a
        .lock()
        .unwrap()
        .insert_dialer_link("https://b.example", OLD)
        .unwrap();
    let transport = Arc::new(HubOverLoopback {
        b: b.clone(),
        b_ssh,
        clients: Mutex::new(vec![(OLD.to_string(), c_old)]),
        seen: Mutex::new(vec![]),
    });
    let cancel = CancellationToken::new();
    let h = super::supervisor::spawn_peer_supervisor(
        a.clone(),
        a_ssh.clone(),
        transport.clone(),
        cancel.clone(),
    );
    let row = || a.lock().unwrap().peer_link(link).unwrap().unwrap();
    let wait = |what: &'static str, f: &dyn Fn() -> bool| {
        let ok = (0..200).any(|_| {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
            false
        });
        assert!(ok, "timed out waiting for: {what}");
    };
    tokio::task::block_in_place(|| {
        wait("connected on the old token", &|| row().state == "connected")
    });
    // The re-pair: B pairs a new token and revokes the old one; A's
    // handshake on the new token moves it onto the running row.
    let c_new = peer_client(&b, "hub-a-2");
    transport
        .clients
        .lock()
        .unwrap()
        .push((NEW.to_string(), c_new));
    b.lock().unwrap().revoke_client_token("hub-a").unwrap();
    {
        let s = a.lock().unwrap();
        // A re-pair merges only into a stopped row (C1).
        s.set_peer_link_state(link, "refused", Some("E_UNAUTHORIZED: gone"), 0)
            .unwrap();
        let tmp = s.insert_dialer_link("https://b.example", NEW).unwrap();
        assert_eq!(
            s.adopt_dialer_fleet(tmp, "fleet-b").unwrap(),
            Adopted::Link(link)
        );
    }
    tokio::task::block_in_place(|| {
        wait("a loop on the new token", &|| {
            transport.calls_with(NEW) > 0 && row().state == "connected"
        })
    });
    let old_calls = transport.calls_with(OLD);
    let m = crate::service::messages::send_message(
        args(a1, "fleet-b/session/local/b1", "after the re-pair"),
        &a,
        &a_ssh,
    )
    .await
    .unwrap();
    tokio::task::block_in_place(|| {
        wait("delivered on the new token", &|| {
            b.lock().unwrap().list_inbox(b1, false, 10).unwrap().len() == 1
        })
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(transport.calls_with(OLD), old_calls, "the old loop is gone");
    let r = row();
    assert_eq!(r.state, "connected");
    assert_eq!(r.token.as_deref(), Some(NEW));
    assert!(a
        .lock()
        .unwrap()
        .pending_outbox(link, 0, 10)
        .unwrap()
        .is_empty());
    let _ = m;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(6), h)
        .await
        .unwrap()
        .unwrap();
}
