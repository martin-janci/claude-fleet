//! End-to-end tests that drive the REAL reconcile core and the REAL
//! `RemoteTmux` command builder over a non-ssh `SshExec` (W4 F6 / OPS-8):
//!
//! - `FakeSsh` for the multi-host pass with unreachable and hanging hosts;
//! - `LocalExec` (`bash -c`) + a private tmux server for the opt-in
//!   `tmux_roundtrip`, which needs `tmux` on PATH:
//!
//!   ```text
//!   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored tmux_roundtrip --nocapture
//!   ```

use crate::service::sessions::{reconcile_sessions_with, ReconcileDeps};
use crate::ssh::SshExec;
use crate::ssh_fake::{FakeSsh, Match, Reply};
use crate::store::Store;
use crate::tmux::{RemoteTmux, TmuxExec};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Reconcile deps whose every host — `local` included — is a `RemoteTmux`
/// over `fake`, so the exact scripts the production builder emits are what
/// the fake sees.
fn deps_over(fake: FakeSsh, probe_timeout: Duration) -> Arc<ReconcileDeps> {
    ReconcileDeps::fake(
        move |alias| {
            Box::new(RemoteTmux {
                client: fake.clone(),
                host: alias.to_string(),
            })
        },
        probe_timeout,
    )
}

const LIST_SCRIPT: &str = "tmux list-sessions -F '#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}' 2>&1";

#[tokio::test]
async fn reconcile_pass_updates_reachable_hosts_and_keeps_unreachable_ones() {
    // Fleet: `local` (no tmux server), `alpha` (one live session, one
    // stale row), `beta` (ssh cannot connect; one stored session), `gamma`
    // (ssh black-holes; wall clock is the only way out).
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.insert_host("alpha", Some("alpha")).unwrap();
        s.upsert_session("alpha-stale", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_host("beta").unwrap();
        s.upsert_session("beta-old", "beta", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_host("gamma").unwrap();
        s.upsert_session("gamma-old", "gamma", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let fake = FakeSsh::new();
    fake.set_wall_clock(Duration::from_millis(200));
    // `2>&1` in the list script: tmux's "no server" message arrives on
    // stdout with a non-zero exit, which RemoteTmux maps to "no sessions".
    fake.on_host(
        "local",
        Match::script(LIST_SCRIPT),
        Reply::Exit {
            code: 1,
            stdout: b"no server running on /tmp/tmux-1000/default\n".to_vec(),
            stderr: Vec::new(),
        },
    );
    fake.on_host(
        "alpha",
        Match::script(LIST_SCRIPT),
        Reply::ok("alpha-live|1700000000|1700000100|0|/home/x/projects/github.com/o/r\n"),
    )
    .on_host(
        "alpha",
        Match::script_contains("claude agents --json"),
        Reply::ok("[]\n"),
    )
    .on_host(
        "alpha",
        Match::script_contains("tmux capture-pane"),
        Reply::ok("Some tool output\n❯ \n"),
    );
    fake.unreachable("beta");
    fake.hanging("gamma");

    let deps = deps_over(fake.clone(), Duration::from_secs(5));
    let start = std::time::Instant::now();
    reconcile_sessions_with(&store, &deps)
        .await
        .expect("the pass completes");
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "one dead + one hanging host must not stall the pass: {:?}",
        start.elapsed()
    );

    let s = store.lock().unwrap();
    let hosts = s.list_hosts().unwrap();
    let reachable = |a: &str| hosts.iter().find(|h| h.alias == a).unwrap().reachable;
    assert!(reachable("local"));
    assert!(reachable("alpha"), "alpha answered → reachable");
    assert!(!reachable("beta"), "beta refused → unreachable");
    assert!(!reachable("gamma"), "gamma hung → unreachable");

    // alpha: the live session is new, the stale row is ghosted.
    let live = s.get_session("alpha-live", "alpha").unwrap().expect("row");
    assert_eq!(live.status, "running");
    assert_eq!(live.created_at, 1700000000);
    assert_eq!(live.worktree_key.as_deref(), Some("main"));
    assert!(live.lost_at.is_none());
    let stale = s
        .get_session("alpha-stale", "alpha")
        .unwrap()
        .expect("kept");
    assert_eq!(stale.status, "ghost");
    // beta / gamma: last-known sessions are untouched, not ghosted.
    for (name, host) in [("beta-old", "beta"), ("gamma-old", "gamma")] {
        let row = s.get_session(name, host).unwrap().expect("row kept");
        assert_eq!(row.status, "running", "{host}'s rows survive its outage");
        assert!(row.lost_at.is_none());
    }

    // What actually crossed the wire for alpha: list → agents → one pane
    // capture per live session, each as a single quoted `bash -lc` word.
    let scripts: Vec<String> = fake
        .calls_for("alpha")
        .iter()
        .map(|c| {
            assert_eq!(&c.args[..2], ["bash", "-lc"]);
            assert!(c.args[2].starts_with('\'') && c.args[2].ends_with('\''));
            c.script().unwrap()
        })
        .collect();
    assert_eq!(
        scripts,
        vec![
            LIST_SCRIPT.to_string(),
            "claude agents --json 2>/dev/null || echo '[]'".to_string(),
            "tmux capture-pane -t 'alpha-live' -S '-8' -p".to_string(),
        ]
    );
    // beta and gamma: the list and the (unconditional) agents probe, and
    // nothing more — a failed list skips the per-session pane captures.
    for host in ["beta", "gamma"] {
        let scripts: Vec<String> = fake
            .calls_for(host)
            .iter()
            .map(|c| c.script().unwrap())
            .collect();
        assert_eq!(
            scripts,
            vec![
                LIST_SCRIPT.to_string(),
                "claude agents --json 2>/dev/null || echo '[]'".to_string(),
            ],
            "{host}"
        );
    }
}

#[tokio::test]
async fn reconcile_recovers_a_host_once_it_answers_again() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    store.lock().unwrap().upsert_host("beta").unwrap();
    let fake = FakeSsh::new();
    fake.on_host(
        "local",
        Match::script(LIST_SCRIPT),
        Reply::fail(1, "no server running on /tmp/tmux-1000/default"),
    );
    fake.unreachable("beta");
    let deps = deps_over(fake.clone(), Duration::from_secs(5));
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert!(
        !store
            .lock()
            .unwrap()
            .list_hosts()
            .unwrap()
            .iter()
            .find(|h| h.alias == "beta")
            .unwrap()
            .reachable
    );

    // Later rules win: beta comes back with one session.
    fake.on_host(
        "beta",
        Match::Any,
        Reply::fail(1, "no such command for anything but the list"),
    )
    .on_host(
        "beta",
        Match::script(LIST_SCRIPT),
        Reply::ok("beta-live|1|2|1|/tmp\n"),
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let s = store.lock().unwrap();
    assert!(
        s.list_hosts()
            .unwrap()
            .iter()
            .find(|h| h.alias == "beta")
            .unwrap()
            .reachable
    );
    let row = s.get_session("beta-live", "beta").unwrap().expect("row");
    assert_eq!(row.status, "running");
}

/// Opt-in: drives the real `RemoteTmux` scripts through `LocalExec`
/// (`bash -c`) against a private tmux server, then the real reconcile over
/// that executor. Skipped by default; run with
///
/// ```text
/// cargo test --manifest-path src-tauri/Cargo.toml -- --ignored tmux_roundtrip --nocapture
/// ```
///
/// The server is private via `TMUX_TMPDIR` (tmux puts its socket under it),
/// which isolates it exactly like `tmux -L <name>` would without having to
/// thread `-L` through every script the production builder emits. `TMUX` /
/// `TMUX_PANE` are cleared so running the test from inside a tmux session
/// cannot make it target that outer server.
#[tokio::test]
#[ignore = "needs tmux on PATH; run: cargo test -- --ignored tmux_roundtrip --nocapture"]
async fn tmux_roundtrip() {
    use crate::ssh::LocalExec;

    let tmux_bin = std::process::Command::new("tmux").arg("-V").output();
    assert!(
        tmux_bin.is_ok_and(|o| o.status.success()),
        "tmux must be on PATH for this test"
    );
    let dir = tempfile::tempdir().unwrap();
    let exec = LocalExec::new()
        .with_env("TMUX_TMPDIR", &dir.path().to_string_lossy())
        .without_env("TMUX")
        .without_env("TMUX_PANE");
    let tmux = RemoteTmux {
        client: exec.clone(),
        host: "local".to_string(),
    };
    let name = format!("fleet-test-{}", std::process::id());

    // Fresh server: nothing listed.
    let initial = tmux.list_sessions().await.unwrap();
    assert!(
        initial.is_empty(),
        "private server must start empty: {initial:?}"
    );

    // Create through the production script builder.
    let cwd = dir.path().to_path_buf();
    tmux.new_session(&name, &cwd, "sleep 300")
        .await
        .expect("tmux new-session");
    let live = tmux.list_sessions().await.unwrap();
    eprintln!("[tmux_roundtrip] live after create: {live:?}");
    assert!(live.iter().any(|s| s.name == name));
    let pane = tmux.capture_pane_scrollback(&name, 8).await.unwrap();
    eprintln!("[tmux_roundtrip] pane tail: {pane:?}");

    // Real reconcile over the same executor (every alias → this server).
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let exec_for_deps = exec.clone();
    let deps = ReconcileDeps::fake(
        move |_alias| {
            Box::new(RemoteTmux {
                client: exec_for_deps.clone(),
                host: "local".to_string(),
            })
        },
        Duration::from_secs(30),
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s
            .get_session(&name, "local")
            .unwrap()
            .expect("reconciled row");
        eprintln!(
            "[tmux_roundtrip] reconciled: {} status={}",
            row.tmux_name, row.status
        );
        assert_eq!(row.status, "running");
    }

    // Kill, reconcile again: ghosted.
    tmux.kill_session(&name).await.expect("tmux kill-session");
    assert!(tmux
        .list_sessions()
        .await
        .unwrap()
        .iter()
        .all(|s| s.name != name));
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s.get_session(&name, "local").unwrap().expect("row kept");
        eprintln!("[tmux_roundtrip] after kill: status={}", row.status);
        assert_eq!(row.status, "ghost");
    }

    // Shut the private server down (it usually exits with its last
    // session; ignore the "no server" exit).
    let _ = exec
        .run("local", &["tmux", "kill-server"], Duration::from_secs(5))
        .await;
}
