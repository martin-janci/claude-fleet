//! The plain read tools on the hub's read pool: they answer while the
//! writer's mutex is held, and they answer exactly what they answered on the
//! writer.

use super::*;
use crate::events::NoopEventBus;
use crate::store::{ReadPool, READ_POOL_SIZE};
use std::time::Duration;

/// A WAL file store with one host, project, worktree and session; `hosta`
/// is a fake host and `local` is off, so no call below can reach a real
/// tmux (the same precaution the `list_sessions` tests in `tests.rs` take).
fn seeded() -> (tempfile::TempDir, std::path::PathBuf, Arc<Mutex<Store>>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let s = Store::open_with_bus(&path, Arc::new(NoopEventBus)).unwrap();
    s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, "false")
        .unwrap();
    s.upsert_host("hosta").unwrap();
    let pid = s.upsert_project("o", "r", "/p").unwrap();
    let wt = s.upsert_worktree(pid, "feat", "/p/.wt/feat", None).unwrap();
    s.upsert_session("dev", "hosta", Some(pid), Some(wt), 1, 1, "running", None)
        .unwrap();
    (dir, path, Arc::new(Mutex::new(s)))
}

fn tools(store: &Arc<Mutex<Store>>, pool: Option<Arc<ReadPool>>) -> FleetTools {
    FleetTools::new(
        Arc::clone(store),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    )
    .with_read_pool(pool)
}

fn pool(path: &std::path::Path) -> Option<Arc<ReadPool>> {
    Some(Arc::new(
        ReadPool::open(path, READ_POOL_SIZE).unwrap().unwrap(),
    ))
}

fn host_caller() -> Caller {
    Caller {
        host_alias: Some("hosta".to_string()),
        client: None,
        mode: TokenMode::Full,
    }
}

async fn call(t: &FleetTools, tool: &str) -> Result<CallToolResult, McpError> {
    let empty = || serde_json::json!({});
    match tool {
        "list_hosts" => t.list_hosts().await,
        "whoami" => {
            t.whoami(
                Extension(host_caller()),
                Parameters(WhoamiParams {
                    tmux_name: "dev".to_string(),
                }),
            )
            .await
        }
        "list_worktrees" => {
            t.list_worktrees(Parameters(serde_json::from_value(empty()).unwrap()))
                .await
        }
        "list_projects" => {
            t.list_projects(Parameters(serde_json::from_value(empty()).unwrap()))
                .await
        }
        "fleet_health" => t.fleet_health(Extension(host_caller())).await,
        "list_sessions" => {
            t.list_sessions(
                Extension(host_caller()),
                Parameters(serde_json::from_value(empty()).unwrap()),
            )
            .await
        }
        other => panic!("no such read tool {other}"),
    }
}

fn json(r: &CallToolResult) -> serde_json::Value {
    let text = &r.content[0].as_text().expect("text").text;
    serde_json::from_str(text).expect("JSON result")
}

/// With the writer's mutex held (a reconcile pass's transaction, a hook),
/// each read tool still answers — bounded on an OS thread, since a tool
/// stuck in a std `Mutex::lock` would stall a tokio timer too.
///
/// `list_sessions` is left out here: whether it runs an inline reconcile
/// pass depends on the process-global reconcile gate other tests move.
#[test]
fn the_read_tools_answer_while_the_writer_is_held() {
    let (_dir, path, store) = seeded();
    let t = tools(&store, pool(&path));
    let (hold_tx, hold_rx) = std::sync::mpsc::channel::<()>();
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let writer = Arc::clone(&store);
    let holder = std::thread::spawn(move || {
        let _guard = writer.lock().unwrap();
        held_tx.send(()).unwrap();
        let _ = hold_rx.recv_timeout(Duration::from_secs(30));
    });
    held_rx.recv().unwrap();

    for tool in [
        "list_hosts",
        "whoami",
        "list_worktrees",
        "list_projects",
        "fleet_health",
    ] {
        let (tx, rx) = std::sync::mpsc::channel();
        let t = t.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = tx.send(rt.block_on(call(&t, tool)).map(|r| json(&r)));
        });
        let answered = rx.recv_timeout(Duration::from_secs(3));
        assert!(
            matches!(&answered, Ok(Ok(_))),
            "{tool} waited on the writer (or failed on the pool): {answered:?}"
        );
    }

    hold_tx.send(()).unwrap();
    holder.join().unwrap();
}

/// Same store, same caller: the pool answers exactly what the writer does,
/// org scoping included (a per-host caller throughout).
#[tokio::test]
async fn the_read_tools_answer_the_same_through_the_pool() {
    let (_dir, path, store) = seeded();
    let on_writer = tools(&store, None);
    let on_pool = tools(&store, pool(&path));
    for tool in [
        "list_hosts",
        "whoami",
        "list_worktrees",
        "list_projects",
        "fleet_health",
        "list_sessions",
    ] {
        let a = json(&call(&on_writer, tool).await.unwrap());
        let b = json(&call(&on_pool, tool).await.unwrap());
        assert_eq!(a, b, "{tool} answered differently through the pool");
    }
}

/// `db_ready: false` is how `fleet_health` reports a poisoned store mutex
/// (every write tool then fails `E_LOCK`). Reading through the pool must not
/// hide a poisoned WRITER: the pooled connections are fine, the writer is not.
#[tokio::test]
async fn fleet_health_reports_a_poisoned_writer_even_through_the_pool() {
    let (_dir, path, store) = seeded();
    let t = tools(&store, pool(&path));
    let healthy = json(&call(&t, "fleet_health").await.unwrap());
    assert_eq!(healthy["db_ready"], true, "{healthy}");

    let writer = Arc::clone(&store);
    let _ = std::thread::spawn(move || {
        let _guard = writer.lock().unwrap();
        panic!("poison the writer");
    })
    .join();
    assert!(store.is_poisoned());

    let sick = json(&call(&t, "fleet_health").await.unwrap());
    assert_eq!(
        sick["db_ready"], false,
        "a poisoned writer reported as ready: {sick}"
    );
}
