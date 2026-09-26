//! `authorize` with the hub's token cache on: the real layer over a real
//! socket, a file store in WAL and a read pool, as `fleet-hub serve` runs it.
//!
//! The security property under test: a token revoked (or rotated, or
//! narrowed) by ANY writer — the daemon's own connection or another process
//! opening `state.db` the way the `fleet-hub` CLI does — is refused on the
//! very next request, and a token minted or paired is accepted on it.

use super::*;
use crate::events::NoopEventBus;
use crate::store::{ReadPool, READ_POOL_SIZE};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MASTER_TOK: &str = "s3cret";
const HOST_TOK: &str = "host-tok";
const PHONE_TOK: &str = "phone-tok";

struct Hub {
    dir: tempfile::TempDir,
    store: Arc<Mutex<Store>>,
    addr: SocketAddr,
}

impl Hub {
    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("state.db")
    }

    /// A second writer on the same file, the way `fleet-hub peer remove` /
    /// `host-token-mode` / `agent-token --rotate` open it: another
    /// connection, which in production is another process.
    fn other_process(&self) -> Store {
        Store::open_with_bus(&self.path(), Arc::new(NoopEventBus)).unwrap()
    }

    /// `POST /mcp` with `token`; the stub answers the caller it was handed.
    async fn call(&self, token: &str) -> String {
        call_at(self.addr, token).await
    }

    async fn accepted(&self, token: &str) -> String {
        let r = self.call(token).await;
        assert!(
            r.contains("200 OK"),
            "expected {token} to be accepted:\n{r}"
        );
        r
    }

    async fn refused(&self, token: &str) {
        let r = self.call(token).await;
        assert!(r.contains("401"), "expected {token} to be refused:\n{r}");
    }
}

async fn call_at(addr: SocketAddr, token: &str) -> String {
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n{{}}"
    );
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

/// The real app over a WAL file store, with a pool and the token cache: one
/// host token and one paired client to start with.
async fn hub() -> Hub {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open_with_bus(&path, Arc::new(NoopEventBus)).unwrap();
    // The master token as `serve` leaves it: stored, and passed to the app.
    store.set_setting(SETTING_TOKEN, MASTER_TOK).unwrap();
    store.upsert_host("box").unwrap();
    store.upsert_host_token("box", HOST_TOK).unwrap();
    store
        .insert_client_token("phone", &auth::sha256_hex(PHONE_TOK), "full")
        .unwrap();
    let store = Arc::new(Mutex::new(store));
    let pool = Arc::new(ReadPool::open(&path, READ_POOL_SIZE).unwrap().unwrap());
    let tokens = Some(Arc::new(TokenCache::new(pool).unwrap()));
    let app = build_app(
        metrics::MetricsState {
            metrics: Arc::new(metrics::Metrics::new()),
            streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
        },
        axum::routing::any(|axum::Extension(c): axum::Extension<Caller>| async move {
            format!(
                "caller={} mode={:?} trusted={}",
                c.label(),
                c.mode,
                c.is_trusted_client()
            )
        }),
        None,
        hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        },
        AuthState {
            master: Arc::new(MASTER_TOK.to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(vec![]),
            tokens,
        },
        pairing::PairState::new(
            Arc::clone(&store),
            Arc::new(pairing::PendingPairings::new()),
            Arc::new(RateLimiter::new()),
            "http://127.0.0.1".to_string(),
        ),
        EventsState::disabled(),
        crate::agent::ws::AgentWsState::disabled(),
        report_route::ReportState::new(Arc::clone(&store)),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Hub { dir, store, addr }
}

#[tokio::test]
async fn a_revoked_client_token_is_refused_on_the_next_request() {
    let hub = hub().await;
    let r = hub.accepted(PHONE_TOK).await;
    assert!(r.contains("caller=client:phone"), "{r}");
    hub.store
        .lock()
        .unwrap()
        .revoke_client_token("phone")
        .unwrap();
    hub.refused(PHONE_TOK).await;
}

/// The `fleet-hub` CLI writes state.db from its own process; no in-process
/// invalidate can reach this server, and none is needed.
#[tokio::test]
async fn a_revoke_or_rotation_by_another_process_is_refused_on_the_next_request() {
    let hub = hub().await;
    hub.accepted(PHONE_TOK).await;
    let r = hub.accepted(HOST_TOK).await;
    assert!(r.contains("caller=host:box mode=Full"), "{r}");

    let cli = hub.other_process();
    cli.revoke_client_token("phone").unwrap();
    hub.refused(PHONE_TOK).await;

    // `host-token-mode box readonly`
    cli.set_host_token_mode("box", "readonly").unwrap();
    let r = hub.accepted(HOST_TOK).await;
    assert!(
        r.contains("mode=Readonly"),
        "a narrowed token kept full mode:\n{r}"
    );

    // `agent-token box --rotate`
    cli.upsert_host_token("box", "rotated-tok").unwrap();
    hub.refused(HOST_TOK).await;
    hub.accepted("rotated-tok").await;

    // A raw write, no fleet code at all: the trigger is in the file.
    let raw = rusqlite::Connection::open(hub.path()).unwrap();
    raw.busy_timeout(Duration::from_secs(5)).unwrap();
    raw.execute("DELETE FROM host_tokens WHERE host_alias = 'box'", [])
        .unwrap();
    hub.refused("rotated-tok").await;
}

/// `fleet-hub token regenerate` (and `init --regenerate-token`) writes
/// `mcp.token` from its own process. The master the daemon was started with
/// is refused on the next request, and the fresh one accepted — without a
/// restart.
#[tokio::test]
async fn a_master_token_regenerated_by_another_process_is_refused_on_the_next_request() {
    let hub = hub().await;
    let r = hub.accepted(MASTER_TOK).await;
    assert!(r.contains("caller=master"), "{r}");
    hub.other_process()
        .set_setting(SETTING_TOKEN, "fresh-master")
        .unwrap();
    hub.refused(MASTER_TOK).await;
    let r = hub.accepted("fresh-master").await;
    assert!(r.contains("caller=master mode=Full"), "{r}");
    // Rotated by the daemon itself, the same.
    hub.store
        .lock()
        .unwrap()
        .set_setting(SETTING_TOKEN, "fresher-master")
        .unwrap();
    hub.refused("fresh-master").await;
    hub.accepted("fresher-master").await;
}

#[tokio::test]
async fn a_newly_paired_or_minted_token_is_accepted_on_the_next_request() {
    let hub = hub().await;
    // Prime the cache, then write behind it.
    hub.accepted(PHONE_TOK).await;
    hub.refused("tablet-tok").await;
    hub.refused("box2-tok").await;
    {
        let s = hub.store.lock().unwrap();
        s.insert_client_token("tablet", &auth::sha256_hex("tablet-tok"), "readonly")
            .unwrap();
        s.upsert_host("box2").unwrap();
        s.upsert_host_token("box2", "box2-tok").unwrap();
    }
    let r = hub.accepted("tablet-tok").await;
    assert!(r.contains("caller=client:tablet mode=Readonly"), "{r}");
    hub.accepted("box2-tok").await;

    // Trust granted by another process reaches the next request too.
    hub.other_process()
        .set_client_trust("tablet", true)
        .unwrap();
    let r = hub.accepted("tablet-tok").await;
    assert!(r.contains("trusted=true"), "{r}");
}

/// With the writer's mutex held (a long reconcile transaction), a request
/// still authorizes: the token check reads the pool, and the liveness stamp
/// is skipped rather than waited for — then written by a later request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authorize_never_waits_on_the_writer() {
    let hub = hub().await;
    let (hold_tx, hold_rx) = std::sync::mpsc::channel::<()>();
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let writer = Arc::clone(&hub.store);
    let holder = std::thread::spawn(move || {
        let _guard = writer.lock().unwrap();
        held_tx.send(()).unwrap();
        let _ = hold_rx.recv_timeout(Duration::from_secs(30));
    });
    held_rx.recv().unwrap();

    for token in [HOST_TOK, PHONE_TOK] {
        // Timed on an OS thread, not a tokio timer: a request blocked in a
        // std `Mutex::lock` holds a runtime worker, and the bound must not
        // depend on the runtime staying responsive.
        let (tx, rx) = std::sync::mpsc::channel();
        let addr = hub.addr;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = tx.send(rt.block_on(call_at(addr, token)));
        });
        let answered = rx.recv_timeout(Duration::from_secs(3));
        assert!(
            matches!(&answered, Ok(r) if r.contains("200 OK")),
            "{token} waited on the writer's lock: {answered:?}"
        );
    }

    hold_tx.send(()).unwrap();
    holder.join().unwrap();
    // The stamp skipped while the writer was busy lands on the next request.
    hub.accepted(PHONE_TOK).await;
    let seen = hub
        .store
        .lock()
        .unwrap()
        .list_client_tokens(false)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "phone")
        .unwrap()
        .last_seen_at;
    assert!(
        seen.is_some(),
        "the retried liveness stamp was never written"
    );
}
