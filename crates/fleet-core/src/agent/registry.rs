//! Alias → live agent connection.
//!
//! The registry owns nothing about *how* a connection is carried: an entry is
//! a channel some owner drains into a socket, plus a table of requests waiting
//! for their answer. Task 5's `/agent` endpoint supplies both ends; these
//! tests supply a fake one.
//!
//! Two invariants earn their own generation counter ([`ConnId`]):
//!
//! - a second connection for an alias **replaces** the first, so a restarted
//!   agent recovers without waiting for a timeout;
//! - the replaced connection's reader and its eventual `disconnect` must not
//!   touch the replacement. Both are checked against the id they were given.

use crate::ipc_error::{codes, IpcError};
use dashmap::DashMap;
use fleet_proto::{AgentFrame, HubFrame};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Identifies one connection *attempt* for an alias. Monotonic per registry,
/// so a stale owner can always be told apart from the live one.
pub type ConnId = u64;

/// What an agent reported in its `hello` frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentHello {
    pub agent_version: String,
    pub host_name: String,
    pub os: String,
}

/// One row of [`AgentRegistry::snapshot`] — what `agent_status` (Task 8) and
/// the reconcile pass need to know about a connected agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentStatus {
    pub alias: String,
    /// Unix seconds when this connection was registered.
    pub connected_at: i64,
    pub agent_version: String,
    pub host_name: String,
    pub os: String,
}

struct Connection {
    id: ConnId,
    hello: AgentHello,
    connected_at: i64,
    /// Frames to write to the socket. Unbounded: the writer is a dedicated
    /// task and a blocked `send` here would block a service call.
    outbound: mpsc::UnboundedSender<HubFrame>,
    /// Request id → the caller waiting for its `result`/`pong`.
    pending: DashMap<String, oneshot::Sender<AgentFrame>>,
}

impl Connection {
    /// Wake every waiting caller with "the connection is gone". Dropping the
    /// senders is what the caller's `recv` sees; it maps to
    /// `E_AGENT_OFFLINE`, not a hang until the wall clock.
    fn abort_pending(&self) {
        self.pending.clear();
    }
}

/// Alias → the one live agent connection for that host.
pub struct AgentRegistry {
    conns: DashMap<String, Arc<Connection>>,
    next_id: AtomicU64,
}

impl AgentRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            conns: DashMap::new(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Register a live connection for `alias`, replacing (and aborting) any
    /// previous one. `outbound` is drained by the connection's owner.
    pub fn connect(
        &self,
        alias: &str,
        hello: AgentHello,
        outbound: mpsc::UnboundedSender<HubFrame>,
    ) -> ConnId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let conn = Arc::new(Connection {
            id,
            hello,
            connected_at: now_unix(),
            outbound,
            pending: DashMap::new(),
        });
        if let Some(previous) = self.conns.insert(alias.to_string(), conn) {
            previous.abort_pending();
        }
        id
    }

    /// Deregister `conn`, if it is still the live connection for `alias`. A
    /// late call from a connection that was already replaced does nothing.
    pub fn disconnect(&self, alias: &str, conn: ConnId) {
        let gone = self.conns.remove_if(alias, |_, live| live.id == conn);
        if let Some((_, live)) = gone {
            live.abort_pending();
        }
    }

    /// Is an agent connected for this host?
    pub fn connected(&self, alias: &str) -> bool {
        self.conns.contains_key(alias)
    }

    /// Every connected agent, ordered by alias.
    pub fn snapshot(&self) -> Vec<AgentStatus> {
        let mut rows: Vec<AgentStatus> = self
            .conns
            .iter()
            .map(|e| AgentStatus {
                alias: e.key().clone(),
                connected_at: e.connected_at,
                agent_version: e.hello.agent_version.clone(),
                host_name: e.hello.host_name.clone(),
                os: e.hello.os.clone(),
            })
            .collect();
        rows.sort_by(|a, b| a.alias.cmp(&b.alias));
        rows
    }

    /// Send `frame` and wait for the answer carrying the same id.
    ///
    /// With no live agent this returns **immediately** — an agent host that is
    /// not connected must report unreachable the way a down SSH host does, not
    /// spend the caller's whole wall clock finding out.
    pub async fn request(
        &self,
        alias: &str,
        frame: HubFrame,
        timeout: Duration,
    ) -> Result<AgentFrame, IpcError> {
        let id = frame_id(&frame).to_string();
        let conn = self.live(alias).ok_or_else(|| offline(alias))?;
        let (tx, rx) = oneshot::channel();
        // Claim the slot, or refuse. A plain `insert` evicted the sitting
        // tenant, and `PendingGuard::drop` then removed the *replacement's*
        // slot by id — so both callers were told `E_AGENT_OFFLINE` while the
        // agent was connected and answering, and the answer that did arrive
        // was dropped. `AgentTransport` cannot reach this (uuid v4 ids), but
        // `request` is public and the `/agent` endpoint drives it, so it
        // fails loudly instead of corrupting the caller that was here first.
        match conn.pending.entry(id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => {
                return Err(IpcError::new(
                    codes::E_AGENT_PROTOCOL,
                    format!("request id {id} is already in flight on {alias}"),
                ));
            }
            dashmap::mapref::entry::Entry::Vacant(slot) => {
                slot.insert(tx);
            }
        }
        // Only now: the guard must not remove a slot this call did not claim.
        let _guard = PendingGuard {
            conn: Arc::clone(&conn),
            id: id.clone(),
        };
        if conn.outbound.send(frame).is_err() {
            // The owner dropped the receiver: the socket is already gone.
            return Err(offline(alias));
        }
        // …and it could have been replaced between the lookup and the insert,
        // in which case nobody will ever answer this slot.
        if self.live(alias).map(|c| c.id) != Some(conn.id) {
            return Err(offline(alias));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => Ok(answer),
            // The sender was dropped: the connection was replaced or closed.
            Ok(Err(_)) => Err(offline(alias)),
            // `E_SSH_TIMEOUT`, not `E_TIMEOUT`, even though no ssh is
            // involved: it is the code EVERY `SshExec` returns for a blown
            // wall clock (`ssh::wall_clock_error`, shared by `SshClient`,
            // `LocalExec` and `ssh_fake`), and the service layer above the
            // seam branches on it. A code of its own looked harmless and was
            // not — `account_usage::classify_run` reads an unrecognised code
            // as "nothing ran on that host" and re-fires the Anthropic API
            // request against the next one, the exact double-billing that
            // classification exists to prevent. The message says agent, so
            // nothing in the text lies; only the identifier is shared.
            Err(_) => Err(IpcError::new(
                codes::E_SSH_TIMEOUT,
                format!(
                    "agent on {alias} did not answer within {}s",
                    timeout.as_secs_f32()
                ),
            )),
        }
    }

    /// Send a frame that has no answer (`cancel`). Fire and forget: the agent
    /// may already have finished, and an id it does not know is a no-op there.
    pub fn send(&self, alias: &str, frame: HubFrame) -> Result<(), IpcError> {
        let conn = self.live(alias).ok_or_else(|| offline(alias))?;
        conn.outbound.send(frame).map_err(|_| offline(alias))
    }

    /// Hand the registry a frame that arrived from `conn`. Returns whether it
    /// resolved a waiting request: an unknown id, a frame from a connection
    /// that has been replaced, and a `hello` are all dropped — never a panic,
    /// because every one of them is reachable from the network.
    pub fn deliver(&self, alias: &str, conn: ConnId, frame: AgentFrame) -> bool {
        let Some(live) = self.live(alias) else {
            return false;
        };
        if live.id != conn {
            return false;
        }
        let Some(id) = agent_frame_id(&frame).map(str::to_string) else {
            return false;
        };
        match live.pending.remove(&id) {
            Some((_, tx)) => tx.send(frame).is_ok(),
            None => false,
        }
    }

    fn live(&self, alias: &str) -> Option<Arc<Connection>> {
        self.conns.get(alias).map(|e| Arc::clone(e.value()))
    }

    /// How many requests are waiting on this host's connection. Test-only: it
    /// is how a leaked slot (a timed-out or abandoned call) becomes visible.
    /// Which connection is live for `alias`. Test-only: it is how an
    /// end-to-end test knows a reconnect has REPLACED the old connection,
    /// which `connected(alias)` cannot tell it.
    #[cfg(test)]
    pub(crate) fn live_conn(&self, alias: &str) -> Option<ConnId> {
        self.live(alias).map(|c| c.id)
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self, alias: &str) -> usize {
        self.live(alias).map(|c| c.pending.len()).unwrap_or(0)
    }
}

/// Removes a request's slot when its caller goes away.
struct PendingGuard {
    conn: Arc<Connection>,
    id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.conn.pending.remove(&self.id);
    }
}

fn offline(alias: &str) -> IpcError {
    IpcError::new(
        codes::E_AGENT_OFFLINE,
        format!("no fleet-agent is connected for host {alias}"),
    )
}

/// The request id a hub frame carries. Every variant has one.
pub(crate) fn frame_id(frame: &HubFrame) -> &str {
    match frame {
        HubFrame::Exec { id, .. }
        | HubFrame::Upload { id, .. }
        | HubFrame::Cancel { id }
        | HubFrame::Ping { id } => id,
    }
}

/// The request id an agent frame answers — `None` for `hello`, which answers
/// nothing.
fn agent_frame_id(frame: &AgentFrame) -> Option<&str> {
    match frame {
        AgentFrame::Result { id, .. } | AgentFrame::Pong { id } => Some(id),
        AgentFrame::Hello { .. } => None,
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::fake::{answer_exit, custom, hello, silent, FakeAgent};
    use crate::ipc_error::codes;
    use fleet_proto::{encode_b64, AgentFrame, HubFrame};
    use std::time::{Duration, Instant};

    fn exec(id: &str) -> HubFrame {
        HubFrame::Exec {
            id: id.into(),
            argv: vec!["true".into()],
            stdin: None,
            timeout_ms: 1_000,
            cap_bytes: None,
        }
    }

    fn ok_result(id: &str) -> AgentFrame {
        AgentFrame::Result {
            id: id.into(),
            exit_code: 0,
            stdout_b64: encode_b64(b""),
            stderr_b64: encode_b64(b""),
            truncated: false,
        }
    }

    // ── offline ─────────────────────────────────────────────────────────────

    /// The headline requirement: no live agent fails *now*, not after the
    /// timeout. A 60 s budget that returns in under a quarter of a second is
    /// the only way to tell "returned immediately" from "errored eventually".
    #[tokio::test]
    async fn a_request_with_no_connection_is_offline_immediately() {
        let reg = AgentRegistry::new();
        let started = Instant::now();
        let err = reg
            .request("ghost", exec("1"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "waited {:?} before reporting an offline agent",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn send_with_no_connection_is_offline() {
        let reg = AgentRegistry::new();
        let err = reg
            .send("ghost", HubFrame::Cancel { id: "1".into() })
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
    }

    // ── connect / snapshot ──────────────────────────────────────────────────

    #[tokio::test]
    async fn connect_records_the_hello_and_shows_up_in_the_snapshot() {
        let reg = AgentRegistry::new();
        assert!(!reg.connected("laptop"));
        let _agent = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        assert!(reg.connected("laptop"));

        let snap = reg.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].alias, "laptop");
        assert_eq!(snap[0].agent_version, hello().agent_version);
        assert_eq!(snap[0].host_name, hello().host_name);
        assert_eq!(snap[0].os, hello().os);
        assert!(snap[0].connected_at > 0, "connect time is recorded");
    }

    #[tokio::test]
    async fn the_snapshot_is_ordered_by_alias() {
        let reg = AgentRegistry::new();
        let _c = FakeAgent::connect(&reg, "charlie", answer_exit(0));
        let _a = FakeAgent::connect(&reg, "alpha", answer_exit(0));
        let _b = FakeAgent::connect(&reg, "bravo", answer_exit(0));
        let aliases: Vec<_> = reg.snapshot().into_iter().map(|s| s.alias).collect();
        assert_eq!(aliases, ["alpha", "bravo", "charlie"]);
    }

    #[tokio::test]
    async fn a_request_reaches_the_agent_and_comes_back() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", answer_exit(7));
        let answer = reg
            .request("laptop", exec("req-1"), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(answer, ok_result_with_code("req-1", 7));
        assert_eq!(agent.only_frame(), exec("req-1"));
    }

    fn ok_result_with_code(id: &str, code: i32) -> AgentFrame {
        match ok_result(id) {
            AgentFrame::Result {
                id,
                stdout_b64,
                stderr_b64,
                truncated,
                ..
            } => AgentFrame::Result {
                id,
                exit_code: code,
                stdout_b64,
                stderr_b64,
                truncated,
            },
            other => other,
        }
    }

    // ── reconnection ────────────────────────────────────────────────────────

    /// A restarted agent must take over without waiting for a timeout.
    #[tokio::test]
    async fn a_second_connection_replaces_the_first() {
        let reg = AgentRegistry::new();
        let first = FakeAgent::connect(&reg, "laptop", answer_exit(1));
        let second = FakeAgent::connect(&reg, "laptop", answer_exit(2));
        assert_ne!(first.conn_id, second.conn_id);

        let answer = reg
            .request("laptop", exec("r"), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            answer,
            ok_result_with_code("r", 2),
            "the new agent answered"
        );
        assert!(first.sent().is_empty(), "the replaced agent got nothing");
        assert_eq!(reg.snapshot().len(), 1, "one row per alias, not two");
    }

    /// The replaced connection's reader task may still be draining. Its
    /// frames must not resolve the *new* connection's requests.
    #[tokio::test]
    async fn deliver_from_a_replaced_connection_is_ignored() {
        let reg = AgentRegistry::new();
        let stale = FakeAgent::connect(&reg, "laptop", silent());
        let stale_id = stale.conn_id;
        let _fresh = FakeAgent::connect(&reg, "laptop", silent());

        let reg2 = Arc::clone(&reg);
        let call = tokio::spawn(async move {
            reg2.request("laptop", exec("r"), Duration::from_millis(100))
                .await
        });
        wait_pending(&reg, "laptop", 1).await;
        assert!(
            !reg.deliver("laptop", stale_id, ok_result("r")),
            "a frame from the replaced connection is dropped"
        );
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
    }

    /// Spin until `n` requests are waiting on `alias`'s connection. Yields
    /// instead of sleeping: the request is a task on this runtime, so this
    /// costs two polls rather than a slice of the real clock. Other tests in
    /// this binary are timing-sensitive; none of mine may hold the machine.
    async fn wait_pending(reg: &AgentRegistry, alias: &str, n: usize) {
        for _ in 0..100_000 {
            if reg.pending_len(alias) == n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("{alias} never had {n} request(s) pending");
    }

    /// A late `disconnect` from the connection that was replaced must not
    /// tear down the replacement — otherwise a reconnect would leave the
    /// host offline once the old socket finally noticed it was dead.
    #[tokio::test]
    async fn a_stale_disconnect_does_not_evict_the_replacement() {
        let reg = AgentRegistry::new();
        let first = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        let _second = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        first.disconnect();
        assert!(reg.connected("laptop"), "the replacement is still live");
    }

    #[tokio::test]
    async fn disconnect_removes_the_connection() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        agent.disconnect();
        assert!(!reg.connected("laptop"));
        assert!(reg.snapshot().is_empty());
        let err = reg
            .request("laptop", exec("r"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
    }

    /// The spec's "a call racing a disconnect": the caller must be told the
    /// agent is gone, not left waiting out the whole wall clock.
    #[tokio::test]
    async fn an_in_flight_request_fails_offline_when_the_connection_drops() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", silent());
        let reg2 = Arc::clone(&reg);
        let call = tokio::spawn(async move {
            reg2.request("laptop", exec("r"), Duration::from_secs(60))
                .await
        });
        wait_pending(&reg, "laptop", 1).await;
        let started = Instant::now();
        agent.disconnect();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "waited {:?} after the disconnect",
            started.elapsed()
        );
    }

    /// A second request under an id that is already in flight used to evict
    /// the first one's slot silently: **both** callers were then told
    /// `E_AGENT_OFFLINE` while the agent was connected and answering, and the
    /// answer that did arrive was dropped. `AgentTransport` cannot reach this
    /// (uuid v4 ids) but `AgentRegistry::request` is `pub` and the `/agent`
    /// endpoint drives it. The duplicate is refused; the original is
    /// untouched and still gets its answer.
    #[tokio::test]
    async fn a_duplicate_request_id_is_refused_and_leaves_the_first_alone() {
        let reg = AgentRegistry::new();
        let _agent = FakeAgent::connect(&reg, "laptop", silent());

        let reg2 = Arc::clone(&reg);
        let first = tokio::spawn(async move {
            reg2.request("laptop", exec("same"), Duration::from_secs(60))
                .await
        });
        wait_pending(&reg, "laptop", 1).await;

        let dup = reg
            .request("laptop", exec("same"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(dup.code, codes::E_AGENT_PROTOCOL);
        assert!(
            dup.message.contains("same"),
            "the message names the id: {}",
            dup.message
        );
        assert_eq!(
            reg.pending_len("laptop"),
            1,
            "the refusal must not disturb the slot it collided with"
        );

        assert!(
            reg.deliver("laptop", _agent.conn_id, ok_result("same")),
            "the original request is still waiting and takes the answer"
        );
        first.await.unwrap().expect("the first caller is answered");
    }

    // ── bookkeeping ─────────────────────────────────────────────────────────

    // `start_paused`: the deadline is the point, and virtual time reaches it
    // without this test occupying the machine for 60 real milliseconds.
    #[tokio::test(start_paused = true)]
    async fn a_timed_out_request_leaves_no_pending_entry_behind() {
        let reg = AgentRegistry::new();
        let _agent = FakeAgent::connect(&reg, "laptop", silent());
        let err = reg
            .request("laptop", exec("r"), Duration::from_millis(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
        assert_eq!(
            reg.pending_len("laptop"),
            0,
            "a timed-out request must not leak its slot"
        );
    }

    /// Dropping the future (what `tokio::select!` does when a cancellation
    /// token wins the race) must release the slot too.
    #[tokio::test]
    async fn an_abandoned_request_leaves_no_pending_entry_behind() {
        let reg = AgentRegistry::new();
        let _agent = FakeAgent::connect(&reg, "laptop", silent());
        {
            let call = reg.request("laptop", exec("r"), Duration::from_secs(60));
            tokio::pin!(call);
            // One poll dispatches the request and registers its slot; then the
            // future is dropped with the answer still outstanding.
            tokio::select! {
                biased;
                _ = &mut call => panic!("the silent agent cannot have answered"),
                _ = tokio::task::yield_now() => {}
            }
            assert_eq!(reg.pending_len("laptop"), 1, "the slot was taken");
        }
        assert_eq!(reg.pending_len("laptop"), 0);
    }

    #[tokio::test]
    async fn deliver_of_an_unknown_id_is_dropped_not_a_panic() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", silent());
        assert!(!reg.deliver("laptop", agent.conn_id, ok_result("nobody-waits")));
        assert!(!reg.deliver("ghost", agent.conn_id, ok_result("no-such-host")));
    }

    #[tokio::test]
    async fn two_requests_in_flight_get_their_own_answers() {
        let reg = AgentRegistry::new();
        // Echo the request id back in stdout so a crossed answer is visible.
        let _agent = FakeAgent::connect(
            &reg,
            "laptop",
            custom(|f: &HubFrame| {
                let id = match f {
                    HubFrame::Exec { id, .. } => id.clone(),
                    _ => return None,
                };
                Some(AgentFrame::Result {
                    id: id.clone(),
                    exit_code: 0,
                    stdout_b64: encode_b64(id.as_bytes()),
                    stderr_b64: encode_b64(b""),
                    truncated: false,
                })
            }),
        );
        let a = reg.request("laptop", exec("aaa"), Duration::from_secs(5));
        let b = reg.request("laptop", exec("bbb"), Duration::from_secs(5));
        let (a, b) = tokio::join!(a, b);
        assert_eq!(a.unwrap(), stdout_of("aaa"));
        assert_eq!(b.unwrap(), stdout_of("bbb"));
    }

    fn stdout_of(id: &str) -> AgentFrame {
        AgentFrame::Result {
            id: id.into(),
            exit_code: 0,
            stdout_b64: encode_b64(id.as_bytes()),
            stderr_b64: encode_b64(b""),
            truncated: false,
        }
    }
}
