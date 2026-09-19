//! A fake agent connection for tests.
//!
//! It is a *real* registry entry — the same `mpsc` sender Task 5's WebSocket
//! writer will hold — drained by a task that records every frame the hub sent
//! and answers it according to a policy. So these tests exercise the actual
//! registry and transport code paths, not a stand-in for them.

use super::registry::{frame_id, AgentHello, AgentRegistry, ConnId};
use fleet_proto::{encode_b64, AgentFrame, HubFrame};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How a fake agent answers one hub frame. `None` = say nothing at all.
pub type Policy = Arc<dyn Fn(&HubFrame) -> Option<AgentFrame> + Send + Sync>;

/// A connected fake agent. Dropping it aborts the responder task; call
/// [`FakeAgent::disconnect`] to deregister it from the registry as well.
pub struct FakeAgent {
    pub alias: String,
    pub conn_id: ConnId,
    registry: Arc<AgentRegistry>,
    sent: Arc<Mutex<Vec<HubFrame>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeAgent {
    /// Register a fake agent for `alias`, answering with `policy`.
    pub fn connect(registry: &Arc<AgentRegistry>, alias: &str, policy: Policy) -> Self {
        Self::connect_as(registry, alias, hello(), policy)
    }

    /// [`FakeAgent::connect`] with an explicit `hello`.
    pub fn connect_as(
        registry: &Arc<AgentRegistry>,
        alias: &str,
        hello: AgentHello,
        policy: Policy,
    ) -> Self {
        Self::start(registry, alias, hello, None, policy)
    }

    /// A fake agent that authenticated with `token`, as the `/agent`
    /// endpoint registers a real one, so revoking the token can reach it.
    pub fn connect_with_token(
        registry: &Arc<AgentRegistry>,
        alias: &str,
        token: &str,
        policy: Policy,
    ) -> Self {
        let credential = crate::mcp::auth::sha256_hex(token);
        Self::start(registry, alias, hello(), Some(credential), policy)
    }

    fn start(
        registry: &Arc<AgentRegistry>,
        alias: &str,
        hello: AgentHello,
        credential: Option<String>,
        policy: Policy,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let conn_id = match credential {
            Some(c) => registry.connect_bound(alias, hello, tx, c),
            None => registry.connect(alias, hello, tx),
        };
        let sent = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn({
            let registry = Arc::clone(registry);
            let alias = alias.to_string();
            let sent = Arc::clone(&sent);
            async move {
                while let Some(frame) = rx.recv().await {
                    sent.lock().unwrap().push(frame.clone());
                    if let Some(answer) = policy(&frame) {
                        registry.deliver(&alias, conn_id, answer);
                    }
                }
            }
        });
        Self {
            alias: alias.to_string(),
            conn_id,
            registry: Arc::clone(registry),
            sent,
            task,
        }
    }

    /// Every frame the hub has sent this agent, in order.
    pub fn sent(&self) -> Vec<HubFrame> {
        self.sent.lock().unwrap().clone()
    }

    /// The one frame the hub sent. Panics unless there is exactly one.
    pub fn only_frame(&self) -> HubFrame {
        let sent = self.sent();
        assert_eq!(sent.len(), 1, "expected exactly one frame, got {sent:?}");
        sent.into_iter().next().unwrap()
    }

    /// Wait (bounded) until the hub has sent `n` frames. Needed for a
    /// fire-and-forget frame like `cancel`, which no reply orders against.
    ///
    /// Yields rather than sleeping: the responder is a task on this same
    /// runtime, so a yield is all it needs, and the whole suite stays off the
    /// real clock — timing-sensitive tests elsewhere in this binary have to
    /// share the machine with it.
    pub async fn wait_until_sent(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.sent.lock().unwrap().len() >= n {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "only {} frame(s) arrived, wanted {n}",
                self.sent.lock().unwrap().len()
            );
            tokio::task::yield_now().await;
        }
    }

    /// Deregister this connection, as the endpoint does when the socket dies.
    pub fn disconnect(&self) {
        self.registry.disconnect(&self.alias, self.conn_id);
    }
}

/// The `hello` a fake agent reports.
pub fn hello() -> AgentHello {
    AgentHello {
        agent_version: "9.9.9".into(),
        host_name: "fake-host".into(),
        os: "linux".into(),
    }
}

/// Answer every request with `exit_code` and empty streams; `ping` with `pong`.
pub fn answer_exit(exit_code: i32) -> Policy {
    answer_with(exit_code, b"", b"")
}

/// Answer every request with this exit code and these streams.
pub fn answer_with(exit_code: i32, stdout: &'static [u8], stderr: &'static [u8]) -> Policy {
    Arc::new(move |frame: &HubFrame| match frame {
        HubFrame::Ping { id } => Some(AgentFrame::Pong { id: id.clone() }),
        HubFrame::Cancel { .. } => None,
        other => Some(AgentFrame::Result {
            id: frame_id(other).to_string(),
            exit_code,
            stdout_b64: encode_b64(stdout),
            stderr_b64: encode_b64(stderr),
            truncated: false,
        }),
    })
}

/// Take every frame and never answer.
pub fn silent() -> Policy {
    Arc::new(|_: &HubFrame| None)
}

/// Answer with whatever the closure builds.
pub fn custom(f: impl Fn(&HubFrame) -> Option<AgentFrame> + Send + Sync + 'static) -> Policy {
    Arc::new(f)
}
