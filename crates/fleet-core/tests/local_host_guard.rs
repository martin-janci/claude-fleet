//! `hub.local_host=false` guard, end to end. Its own test binary (its own
//! process) because `disable_local_host` flips a process-wide flag that must
//! never leak into the unit tests.

use fleet_core::ipc_error::codes;
use fleet_core::service::hub::{disable_local_host, local_host_enabled};
use fleet_core::ssh::SshClient;
use std::time::Duration;

const MESSAGE: &str = "host local is disabled on this hub (hub.local_host=false)";

#[tokio::test]
async fn a_disabled_local_host_refuses_explicit_local_targets() {
    disable_local_host();
    disable_local_host(); // idempotent
    assert!(!local_host_enabled());

    let e = fleet_core::validate::host_alias("local").unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert_eq!(e.message, MESSAGE);
    assert!(fleet_core::validate::host_alias("mefistos").is_ok());
    // Syntax errors still win over the guard for anything that is not `local`.
    assert_eq!(
        fleet_core::validate::host_alias("-oProxyCommand=x")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );

    // The local shell branch refuses before spawning: the script would create
    // `marker`, and it must not exist afterwards.
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let script = format!(
        "touch {}",
        fleet_core::shell::quote(&marker.to_string_lossy())
    );
    let ssh = SshClient::new();
    let e = fleet_core::ssh::run_shell(&ssh, "local", &script, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert_eq!(e.message, MESSAGE);
    let e = fleet_core::ssh::run_shell_bounded(
        &ssh,
        "local",
        &script,
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    // Give a (wrongly) spawned child time to run before checking.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "the local script must never run");

    // The local tmux executor refuses too, and its infallible methods come
    // back empty instead of spawning `claude` / `bash`.
    use fleet_core::tmux::{LocalTmux, TmuxExec};
    let e = LocalTmux.list_sessions().await.unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let e = LocalTmux.capture_pane("x").await.unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert!(LocalTmux.list_claude_agents().await.is_empty());
    assert!(LocalTmux
        .transcript_mtimes(&["00000000-0000-0000-0000-000000000000".to_string()])
        .await
        .is_none());

    // Defence in depth past the alias check: provisioning's local file I/O,
    // the catalog's local script runner and the local project scan.
    let e = fleet_core::service::provision::read_host_file(&ssh, "local", "~/.tmux.conf")
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let target = dir.path().join("written");
    let e = fleet_core::service::provision::write_host_file(
        &ssh,
        "local",
        &dir.path().to_string_lossy(),
        &target.to_string_lossy(),
        "x",
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert!(!target.exists());
    let shared = std::sync::Arc::new(SshClient::new());
    let e = fleet_core::service::catalog::inventory::run_host_script(&shared, "local", &script)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let store = std::sync::Mutex::new(
        fleet_core::store::Store::open_with_bus(
            &dir.path().join("state.db"),
            std::sync::Arc::new(fleet_core::events::NoopEventBus),
        )
        .unwrap(),
    );
    let e = fleet_core::service::projects::refresh_projects(&store)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "no local script may run");

    // A settings map keyed by `local` is data, not a target: still parses.
    let map = fleet_core::service::settings::parse_path_map(
        "projects.base_path",
        r#"{"local":"/srv","vps":"~/code"}"#,
    )
    .unwrap();
    assert_eq!(map.len(), 2);
}
