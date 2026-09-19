//! End to end in one process: the hub a `fleet-hub` builds and the real
//! `fleet-agent`, talking over a loopback socket.
//!
//! The hub half is the real one: the `authorize` layer, the `/agent` route,
//! and `SshClient::with_agents` — the routed client `serve.rs` builds — so a
//! call enters through `SshExec` exactly as a service function makes it. The
//! agent half is `fleet_agent::conn::run_with`, the loop `fleet-agent run`
//! drives. Between them sits a tiny TCP proxy, so a test can cut the
//! connection the way a network does and watch the agent dial again.
//!
//! Nothing here waits on a clock. The hub's heartbeats are manual and never
//! fired; the agent's are real 30 s intervals that no test outlives; the
//! agent's backoff "sleep" is a yield. Every wait is on an observable
//! condition, bounded by [`PATIENCE`] only to turn a hang into a failure.

use super::registry::{AgentRegistry, ConnId};
use super::ws::AgentWsState;
use crate::ipc_error::codes;
use crate::ssh::{SshClient, SshExec};
use crate::store::Store;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// How long any wait may take before the test is called failed.
const PATIENCE: Duration = Duration::from_secs(10);

/// A wall clock no call here should come near. "Fast" is judged against it.
const LONG: Duration = Duration::from_secs(60);

const MASTER: &str = "master-token";
const LAPTOP_TOKEN: &str = "laptop-host-token";

/// The hub, as `fleet-hub serve` assembles it.
struct Hub {
    addr: SocketAddr,
    ssh: SshClient,
    registry: Arc<AgentRegistry>,
    /// Held so the manual beat source stays open; never fired.
    _beats: Arc<tokio::sync::watch::Sender<u64>>,
}

async fn hub() -> Hub {
    let store = Store::open_in_memory().unwrap();
    // `laptop` is an agent host with a token; `desk` is an agent host that
    // never gets an agent.
    for alias in ["laptop", "desk"] {
        store.insert_host(alias, Some(alias)).unwrap();
        store.set_host_transport(alias, "agent").unwrap();
    }
    store.upsert_host_token("laptop", LAPTOP_TOKEN).unwrap();
    let store = Arc::new(Mutex::new(store));

    let registry = AgentRegistry::new();
    let ssh = SshClient::with_agents(Arc::clone(&registry), Arc::clone(&store));
    let beats = Arc::new(tokio::sync::watch::Sender::new(0));
    // The endpoint registers on the registry the client routes through, as
    // `serve.rs` arranges; taken from `registry` rather than read back out of
    // the client, so a client that routes nothing fails the CALLS below, not
    // this fixture.
    let state = AgentWsState::new(Some((Arc::clone(&registry), Arc::clone(&store))))
        .with_manual_beats(Arc::clone(&beats));
    let app = crate::mcp::test_app(store, MASTER, state);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    crate::rt::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    Hub {
        addr,
        ssh,
        registry,
        _beats: beats,
    }
}

/// A TCP proxy in front of the hub. [`Proxy::cut`] drops the live
/// connection on both sides at once, the way a network failure does.
struct Proxy {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    live: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

impl Proxy {
    async fn to(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(Mutex::new(None::<tokio::task::AbortHandle>));
        let (count, slot) = (Arc::clone(&accepted), Arc::clone(&live));
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = TcpStream::connect(upstream).await else {
                    continue;
                };
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                let pipe = tokio::spawn(async move {
                    let (mut cr, mut cw) = client.into_split();
                    let (mut sr, mut sw) = server.into_split();
                    tokio::select! {
                        _ = pump(&mut cr, &mut sw) => {}
                        _ = pump(&mut sr, &mut cw) => {}
                    }
                });
                *slot.lock().unwrap() = Some(pipe.abort_handle());
                count.fetch_add(1, Ordering::SeqCst);
            }
        });
        Self {
            addr,
            accepted,
            live,
        }
    }

    /// Drop the live connection: both sockets close.
    fn cut(&self) {
        if let Some(pipe) = self.live.lock().unwrap().take() {
            pipe.abort();
        }
    }
}

async fn pump(from: &mut (impl AsyncReadExt + Unpin), to: &mut (impl AsyncWriteExt + Unpin)) {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if to.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// The real agent, dialling `proxy` with `laptop`'s token, serving `home`.
fn start_agent(proxy: SocketAddr, home: &Path) -> tokio::task::JoinHandle<()> {
    let endpoint = fleet_agent::conn::Endpoint::parse(&format!("http://{proxy}"), true).unwrap();
    let dialer = fleet_agent::conn::Dialer::new(endpoint, LAPTOP_TOKEN.into(), None).unwrap();
    let agent = fleet_agent::conn::Agent::new(Some(home.to_path_buf()), 8);
    tokio::spawn(async move {
        let notifier = fleet_agent::conn::Notifier::at(None);
        let _ = fleet_agent::conn::run_with(
            &dialer,
            &agent,
            &notifier,
            fleet_agent::conn::Beats::heartbeat,
            // The backoff is recorded nowhere and waited for never: a redial
            // happens as soon as the runtime gets to it.
            |_| tokio::task::yield_now(),
        )
        .await;
    })
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::task::yield_now().await;
    }
}

/// The live connection for `laptop`, once there is one other than `old`.
async fn connection_other_than(hub: &Hub, old: Option<ConnId>) -> ConnId {
    let mut now = None;
    wait_until("the agent to (re)register", || {
        now = hub
            .registry
            .live_conn("laptop")
            .filter(|id| Some(*id) != old);
        now.is_some()
    })
    .await;
    now.unwrap()
}

/// A call to a host with no agent fails with `E_AGENT_OFFLINE`, and FAST:
/// an earlier task found implementations that returned the right code only
/// after sleeping out the whole wall clock.
async fn assert_offline_fast(ssh: &SshClient, host: &str) {
    let started = Instant::now();
    let err = SshExec::run_bounded(ssh, host, &["echo", "never"], LONG, LONG)
        .await
        .unwrap_err();
    let took = started.elapsed();
    assert_eq!(err.code, codes::E_AGENT_OFFLINE, "{host}: {err:?}");
    assert!(
        took < Duration::from_secs(1),
        "{host}: E_AGENT_OFFLINE took {took:?} of a {LONG:?} wall clock"
    );
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_and_agent_talking_over_a_real_socket() {
    let hub = hub().await;
    let proxy = Proxy::to(hub.addr).await;
    let home = tempfile::tempdir().unwrap();

    // Before any agent: offline, immediately.
    assert_offline_fast(&hub.ssh, "laptop").await;

    let agent = start_agent(proxy.addr, home.path());
    let first = connection_other_than(&hub, None).await;

    // `echo` through SshExec::run, routed by the host row to the agent.
    let out = SshExec::run(&hub.ssh, "laptop", &["echo", "hello"], PATIENCE)
        .await
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, b"hello\n");
    // The service layer's shape, a quoted script (-c, not -lc: a login
    // profile on the machine running the test must not reach the output).
    let script = crate::shell::quote("echo \"$0 in $(pwd -P)\"; exit 3");
    let out = SshExec::run(&hub.ssh, "laptop", &["bash", "-c", &script], PATIENCE)
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let pwd = home.path().canonicalize().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("bash in {}\n", pwd.display()),
        "children start in the agent's home"
    );

    // An answer past tungstenite's 16 MiB default frame, both directions'
    // limits included: 17 MiB of stdout comes back whole.
    let out = SshExec::run(
        &hub.ssh,
        "laptop",
        &["head -c 17825792 /dev/zero"],
        PATIENCE,
    )
    .await
    .unwrap();
    assert_eq!(out.stdout.len(), 17 * 1024 * 1024);

    // upload_file writes with the right mode: a secret stays 0600, and a
    // world-writable local file arrives without group/other write.
    let local = tempfile::tempdir().unwrap();
    for (local_mode, remote_mode) in [(0o600, 0o600), (0o755, 0o755), (0o777, 0o755)] {
        let src = local.path().join(format!("src-{local_mode:o}"));
        std::fs::write(&src, format!("mode {local_mode:o}\n")).unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(local_mode)).unwrap();
        let dest = home.path().join(format!("uploads/dest-{local_mode:o}"));
        SshExec::upload_file(&hub.ssh, "laptop", &src, dest.to_str().unwrap(), PATIENCE)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&src).unwrap());
        assert_eq!(mode_of(&dest), remote_mode, "local {local_mode:o}");
    }

    // A killed connection: the agent dials again, the new connection
    // REPLACES the old one, and the very next call succeeds.
    proxy.cut();
    let second = connection_other_than(&hub, Some(first)).await;
    assert!(proxy.accepted.load(Ordering::SeqCst) >= 2, "it redialled");
    assert_ne!(first, second);
    let out = SshExec::run(&hub.ssh, "laptop", &["echo", "again"], PATIENCE)
        .await
        .unwrap();
    assert_eq!(out.stdout, b"again\n", "the first call after the reconnect");

    // A host whose agent is gone: offline, immediately. And a host that
    // never had one.
    agent.abort();
    let _ = agent.await;
    proxy.cut();
    wait_until("laptop to drop off the registry", || {
        hub.registry.live_conn("laptop").is_none()
    })
    .await;
    assert_offline_fast(&hub.ssh, "laptop").await;
    assert_offline_fast(&hub.ssh, "desk").await;
}
