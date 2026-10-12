use super::*;
use crate::ipc_error::codes;
use crate::service::debug_devices::DebugDevice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn device(platform: &str, kind: &str, ready: bool, host: &str) -> DebugDevice {
    serde_json::from_value(serde_json::json!({
        "id": 7, "title": "bench pixel", "host": host, "platform": platform, "kind": kind,
        "key": "R5CT1", "serial": "R5CT1", "name": "Pixel 7",
        "state": if ready { "online" } else { "offline" }, "ready": ready, "shared": false,
        "first_seen_at": 0, "last_seen_at": 0
    }))
    .expect("DebugDevice fields changed: update this fixture")
}

// Only the unix-only `lifecycle` tests use it.
#[cfg(unix)]
#[derive(Default)]
struct CountingLease {
    renewed: AtomicUsize,
    released: AtomicUsize,
}
#[cfg(unix)]
#[async_trait::async_trait]
impl Lease for CountingLease {
    async fn renew(&self) -> Result<(), IpcError> {
        self.renewed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn release(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

/// A lease that records its calls and fails renewals with `fail` (an
/// error code) while it is set.
#[derive(Default)]
struct RecordingLease {
    renewed: AtomicUsize,
    released: AtomicUsize,
    fail: std::sync::Mutex<Option<&'static str>>,
}
impl RecordingLease {
    fn failing(&self, code: Option<&'static str>) {
        *self.fail.lock().unwrap() = code;
    }
    fn counts(&self) -> (usize, usize) {
        (
            self.renewed.load(Ordering::SeqCst),
            self.released.load(Ordering::SeqCst),
        )
    }
}
#[async_trait::async_trait]
impl Lease for RecordingLease {
    async fn renew(&self) -> Result<(), IpcError> {
        self.renewed.fetch_add(1, Ordering::SeqCst);
        let fail = *self.fail.lock().unwrap();
        match fail {
            Some(code) => Err(IpcError::new(code, "renewal refused")),
            None => Ok(()),
        }
    }
    async fn release(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

fn claimed(holder: Option<&str>) -> DebugDevice {
    let mut d = device("android", "physical", true, "mefistos");
    d.claimed_by = holder.map(str::to_string);
    d
}

#[test]
fn a_claim_is_pre_held_only_when_this_holder_has_it() {
    assert!(held_by(&claimed(Some("you (desktop)")), "you (desktop)"));
    assert!(held_by(&claimed(Some("client:laptop")), "client:laptop"));
    assert!(!held_by(
        &claimed(Some("host:mefistos#s1")),
        "you (desktop)"
    ));
    assert!(!held_by(&claimed(Some("client:phone")), "client:laptop"));
    assert!(!held_by(&claimed(None), "you (desktop)"));
}

#[tokio::test]
async fn a_pre_held_lease_never_renews_nor_releases() {
    let inner = Arc::new(RecordingLease::default());
    let lease = OwnedLease::new(inner.clone(), true);
    lease.renew().await.unwrap();
    lease.renew().await.unwrap();
    lease.release().await;
    assert_eq!(
        inner.counts(),
        (0, 0),
        "the claim held before the mirror stays as it was"
    );
}

#[tokio::test]
async fn a_normal_lease_renews_and_releases_once() {
    let inner = Arc::new(RecordingLease::default());
    let lease = OwnedLease::new(inner.clone(), false);
    lease.renew().await.unwrap();
    lease.renew().await.unwrap();
    lease.release().await;
    assert_eq!(inner.counts(), (2, 1));
}

#[tokio::test]
async fn the_first_renew_still_reports_another_holder() {
    let inner = Arc::new(RecordingLease::default());
    inner.failing(Some(codes::E_CONFLICT));
    let lease = OwnedLease::new(inner.clone(), false);
    assert_eq!(lease.renew().await.unwrap_err().code, codes::E_CONFLICT);
}

#[tokio::test]
async fn a_lost_claim_is_never_released_nor_retaken() {
    let inner = Arc::new(RecordingLease::default());
    let lease = OwnedLease::new(inner.clone(), false);
    lease.renew().await.unwrap();
    // The claim lapsed while the laptop slept; another session took it.
    inner.failing(Some(codes::E_CONFLICT));
    assert_eq!(lease.renew().await.unwrap_err().code, codes::E_CONFLICT);
    // Their claim lapses in turn: the mirror must not take it back.
    inner.failing(None);
    assert!(lease.renew().await.is_err());
    lease.release().await;
    assert_eq!(inner.counts(), (2, 0), "someone else's claim is left alone");
}

#[tokio::test]
async fn a_transient_renewal_failure_keeps_the_claim_ours() {
    let inner = Arc::new(RecordingLease::default());
    let lease = OwnedLease::new(inner.clone(), false);
    lease.renew().await.unwrap();
    inner.failing(Some(codes::E_HUB_UNREACHABLE));
    assert!(lease.renew().await.is_err());
    inner.failing(None);
    lease.renew().await.unwrap();
    lease.release().await;
    assert_eq!(inner.counts(), (3, 1));
}

#[test]
fn only_a_ready_android_device_with_a_serial_can_be_mirrored() {
    assert_eq!(
        validate(&device("android", "physical", true, "h")).unwrap(),
        "R5CT1"
    );
    assert_eq!(
        validate(&device("ios", "physical", true, "h"))
            .unwrap_err()
            .code,
        codes::E_UNSUPPORTED
    );
    assert_eq!(
        validate(&device("android", "emulator", false, "h"))
            .unwrap_err()
            .code,
        codes::E_INVALID_STATE
    );
}

#[test]
fn local_is_this_machine_only_when_standalone() {
    assert!(matches!(choose_route("local", true).unwrap(), Route::Local));
    assert!(matches!(
        choose_route("mefistos", true).unwrap(),
        Route::Ssh
    ));
    assert!(matches!(
        choose_route("mefistos", false).unwrap(),
        Route::Ssh
    ));
    let e = choose_route("local", false).unwrap_err();
    assert_eq!(e.code, codes::E_NO_ROUTE);
    assert!(e.message.contains("hub"), "{}", e.message);
}

#[test]
fn the_remote_scrcpy_command_points_adb_and_the_tunnel_at_the_relays() {
    let tools = tools::Tools {
        scrcpy: "/bin/scrcpy".into(),
        adb: "/sdk/adb".into(),
    };
    let cmd = scrcpy_command(
        &tools,
        "R5CT1",
        "bench pixel @ mefistos",
        Some(SpawnPorts {
            adb: 41000,
            forward: 27184,
            tunnel: 41001,
        }),
    );
    let std = cmd.as_std();
    let args: Vec<String> = std
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--serial",
            "R5CT1",
            "--window-title",
            "bench pixel @ mefistos",
            "--force-adb-forward",
            "--port",
            "27184",
            "--tunnel-host",
            "127.0.0.1",
            "--tunnel-port",
            "41001",
        ]
    );
    let env: Vec<(String, String)> = std
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                v?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    assert!(
        env.contains(&("ADB_SERVER_SOCKET".into(), "tcp:127.0.0.1:41000".into())),
        "{env:?}"
    );
    assert!(env.contains(&("ADB".into(), "/sdk/adb".into())), "{env:?}");
}

#[test]
fn the_local_scrcpy_command_has_no_tunnel() {
    let tools = tools::Tools {
        scrcpy: "/bin/scrcpy".into(),
        adb: "/sdk/adb".into(),
    };
    let cmd = scrcpy_command(&tools, "emulator-5554", "e @ local", None);
    let args: Vec<String> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        ["--serial", "emulator-5554", "--window-title", "e @ local"]
    );
    assert!(cmd
        .as_std()
        .get_envs()
        .all(|(k, _)| k != "ADB_SERVER_SOCKET"));
}

#[cfg(unix)]
mod lifecycle {
    use super::*;
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
    use std::time::Duration;

    /// A stub scrcpy: logs argv and env to `<dir>/scrcpy.log`, then sleeps
    /// (a running mirror) or exits with `code` at once.
    fn stub_scrcpy(dir: &std::path::Path, exit: Option<i32>) -> std::path::PathBuf {
        let log = crate::shell::quote(&dir.join("scrcpy.log").display().to_string());
        let tail = match exit {
            Some(c) => format!("echo 'ERROR: Could not open video stream' >&2\nexit {c}\n"),
            None => "exec sleep 600\n".to_string(),
        };
        write_exec(dir, "scrcpy", &format!(
            "#!/bin/sh\n{PROBE_GUARD}echo \"$@\" > {log}\necho \"ADB_SERVER_SOCKET=$ADB_SERVER_SOCKET\" >> {log}\n{tail}"
        ))
    }

    fn stub_adb(dir: &std::path::Path, version: u32) -> std::path::PathBuf {
        write_exec(
            dir,
            "adb",
            &format!("#!/bin/sh\n{PROBE_GUARD}echo 'Android Debug Bridge version 1.0.{version}'\n"),
        )
    }

    /// A fake adb server: answers host:version with `version`, list-forward
    /// with 27183 taken, killforward with OKAY; counts connections.
    async fn fake_adb_server(
        version: u32,
    ) -> (
        std::net::SocketAddr,
        Arc<AtomicUsize>,
        Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = l.local_addr().unwrap();
        let conns = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (c2, s2) = (conns.clone(), seen.clone());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                c2.fetch_add(1, Ordering::SeqCst);
                let seen = s2.clone();
                tokio::spawn(async move {
                    let mut len = [0u8; 4];
                    if s.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let n = usize::from_str_radix(std::str::from_utf8(&len).unwrap(), 16).unwrap();
                    let mut body = vec![0u8; n];
                    s.read_exact(&mut body).await.unwrap();
                    let req = String::from_utf8(body).unwrap();
                    seen.lock().unwrap().push(req.clone());
                    let reply = match req.as_str() {
                        "host:version" => format!("OKAY0004{version:04x}"),
                        "host:list-forward" => {
                            let p = "R5CT1 tcp:27183 localabstract:x\n";
                            format!("OKAY{:04x}{p}", p.len())
                        }
                        _ => "OKAY".to_string(),
                    };
                    s.write_all(reply.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, conns, seen)
    }

    /// The remote, and a clone of its fake ssh: the fake adb server
    /// answers, so a test asserts nothing ran over ssh (no start-server
    /// fallback).
    fn remote(addr: std::net::SocketAddr) -> (Remote, crate::ssh_fake::FakeSsh) {
        let ssh = crate::ssh_fake::FakeSsh::new();
        let r = Remote {
            dialer: Arc::new(crate::relay::tests::FixedDialer {
                addr,
                asked: Default::default(),
            }),
            ssh: Arc::new(ssh.clone()),
        };
        (r, ssh)
    }

    fn assert_no_ssh(ssh: &crate::ssh_fake::FakeSsh) {
        assert!(
            ssh.calls().is_empty(),
            "the adb server answered; nothing may run over ssh"
        );
    }

    #[tokio::test]
    async fn a_remote_mirror_runs_scrcpy_on_the_relays_and_cleans_up_on_stop() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), None),
            adb: stub_adb(dir.path(), 41),
        };
        let (addr, _conns, seen) = fake_adb_server(41).await;
        let lease = Arc::new(CountingLease::default());
        let mirrors = Mirrors::new();
        let (r, ssh) = remote(addr);
        let info = mirrors
            .start(
                &device("android", "physical", true, "mefistos"),
                true,
                tools,
                r,
                lease.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(info.route, Route::Ssh));
        assert_eq!(mirrors.list().len(), 1);
        let log = std::fs::read_to_string(dir.path().join("scrcpy.log")).unwrap();
        assert!(
            log.contains("--port 27184"),
            "27183 is taken on the host: {log}"
        );
        assert!(log.contains("ADB_SERVER_SOCKET=tcp:127.0.0.1:"), "{log}");

        mirrors.stop(7).await.unwrap();
        for _ in 0..50 {
            if mirrors.list().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(mirrors.list().is_empty());
        assert_eq!(lease.released.load(Ordering::SeqCst), 1);
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r == "host-serial:R5CT1:killforward:tcp:27184"));
        assert_no_ssh(&ssh);
    }

    #[tokio::test]
    async fn a_second_mirror_of_the_same_device_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), None),
            adb: stub_adb(dir.path(), 41),
        };
        let (addr, _, _) = fake_adb_server(41).await;
        let mirrors = Mirrors::new();
        let d = device("android", "physical", true, "mefistos");
        assert!(!mirrors.is_mirrored(7));
        let (r1, ssh1) = remote(addr);
        mirrors
            .start(
                &d,
                true,
                tools.clone(),
                r1,
                Arc::new(CountingLease::default()),
            )
            .await
            .unwrap();
        assert!(mirrors.is_mirrored(7));
        let lease = Arc::new(CountingLease::default());
        let (r2, ssh2) = remote(addr);
        let e = mirrors
            .start(&d, true, tools, r2, lease.clone())
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_EXISTS);
        assert_eq!(
            lease.released.load(Ordering::SeqCst),
            0,
            "claims are re-entrant per holder: the refused start's lease is the running mirror's claim"
        );
        assert_no_ssh(&ssh1);
        assert_no_ssh(&ssh2);
        mirrors.kill_all();
    }

    #[tokio::test]
    async fn a_version_mismatch_never_spawns_scrcpy() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), None),
            adb: stub_adb(dir.path(), 41),
        };
        let (addr, _, _) = fake_adb_server(40).await;
        let lease = Arc::new(CountingLease::default());
        let (r, ssh) = remote(addr);
        let e = Mirrors::new()
            .start(
                &device("android", "physical", true, "mefistos"),
                true,
                tools,
                r,
                lease.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_ADB_VERSION_MISMATCH);
        assert!(!dir.path().join("scrcpy.log").exists());
        assert_eq!(lease.released.load(Ordering::SeqCst), 1);
        assert_no_ssh(&ssh);
    }

    #[tokio::test]
    async fn scrcpy_failing_at_once_is_reported_with_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), Some(1)),
            adb: stub_adb(dir.path(), 41),
        };
        let (addr, _, _) = fake_adb_server(41).await;
        let lease = Arc::new(CountingLease::default());
        let mirrors = Mirrors::new();
        let (r, ssh) = remote(addr);
        let e = mirrors
            .start(
                &device("android", "physical", true, "mefistos"),
                true,
                tools,
                r,
                lease.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_MIRROR_FAILED);
        assert!(
            e.message.contains("Could not open video stream"),
            "{}",
            e.message
        );
        assert!(mirrors.list().is_empty());
        assert_eq!(lease.released.load(Ordering::SeqCst), 1);
        assert_no_ssh(&ssh);
    }

    #[tokio::test]
    async fn a_local_mirror_needs_no_relay_and_no_adb_server() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), None),
            adb: stub_adb(dir.path(), 41),
        };
        // A dead address: any dial would fail the test.
        let (dead, ssh) = remote("127.0.0.1:9".parse().unwrap());
        let mirrors = Mirrors::new();
        let info = mirrors
            .start(
                &device("android", "emulator", true, "local"),
                true,
                tools,
                dead,
                Arc::new(CountingLease::default()),
            )
            .await
            .unwrap();
        assert!(matches!(info.route, Route::Local));
        let log = std::fs::read_to_string(dir.path().join("scrcpy.log")).unwrap();
        assert!(!log.contains("--tunnel-port"), "{log}");
        mirrors.kill_all();
        assert_no_ssh(&ssh);
    }

    #[tokio::test]
    async fn a_pre_held_claim_survives_a_mirror_that_ran_and_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), None),
            adb: stub_adb(dir.path(), 41),
        };
        let (dead, _ssh) = remote("127.0.0.1:9".parse().unwrap());
        let inner = Arc::new(RecordingLease::default());
        let lease = Arc::new(OwnedLease::new(inner.clone(), true));
        lease.renew().await.unwrap();
        let mirrors = Mirrors::new();
        mirrors
            .start(
                &device("android", "emulator", true, "local"),
                true,
                tools,
                dead,
                lease,
            )
            .await
            .unwrap();
        mirrors.stop(7).await.unwrap();
        for _ in 0..50 {
            if mirrors.list().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(mirrors.list().is_empty());
        assert_eq!(inner.counts(), (0, 0));
    }

    #[tokio::test]
    async fn a_pre_held_claim_survives_a_failed_start() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy(dir.path(), Some(1)),
            adb: stub_adb(dir.path(), 41),
        };
        let (dead, _ssh) = remote("127.0.0.1:9".parse().unwrap());
        let inner = Arc::new(RecordingLease::default());
        let lease = Arc::new(OwnedLease::new(inner.clone(), true));
        lease.renew().await.unwrap();
        let e = Mirrors::new()
            .start(
                &device("android", "emulator", true, "local"),
                true,
                tools,
                dead,
                lease,
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_MIRROR_FAILED);
        assert_eq!(inner.counts(), (0, 0));
    }

    /// A stub scrcpy that records a SIGTERM in `<dir>/term` and exits.
    /// It waits on a background `sleep` (stopped in the trap) so the trap
    /// runs as soon as the signal lands.
    fn stub_scrcpy_recording_term(dir: &std::path::Path) -> std::path::PathBuf {
        let marker = crate::shell::quote(&dir.join("term").display().to_string());
        write_exec(
            dir,
            "scrcpy",
            &format!(
                "#!/bin/sh\n{PROBE_GUARD}sleep 600 &\ns=$!\ntrap \"kill $s; echo TERM > {marker}; exit 0\" TERM\nwait $s\n"
            ),
        )
    }

    fn pid_alive(pid: u32) -> bool {
        crate::proc::std_command("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn kill_all_kills_scrcpy_synchronously() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools::Tools {
            scrcpy: stub_scrcpy_recording_term(dir.path()),
            adb: stub_adb(dir.path(), 41),
        };
        let (dead, ssh) = remote("127.0.0.1:9".parse().unwrap());
        let mirrors = Mirrors::new();
        mirrors
            .start(
                &device("android", "emulator", true, "local"),
                true,
                tools,
                dead,
                Arc::new(CountingLease::default()),
            )
            .await
            .unwrap();
        let pid = {
            let g = mirrors.inner.lock().unwrap();
            g.get(&7)
                .and_then(|r| r.pid)
                .expect("a running mirror records its pid")
        };
        assert!(pid_alive(pid));

        mirrors.kill_all();
        // Block this (current-thread) runtime: the watcher cannot run, so
        // only `kill_all` itself can have delivered the signal, as on an
        // app exit through `std::process::exit`.
        let term = dir.path().join("term");
        for _ in 0..50 {
            if term.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            term.exists(),
            "kill_all did not signal scrcpy synchronously"
        );

        // Let the watcher reap it: the pid is then gone.
        for _ in 0..50 {
            if !pid_alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!pid_alive(pid), "scrcpy (pid {pid}) outlived kill_all");
        assert_no_ssh(&ssh);
    }
}

#[test]
fn a_hosts_live_forward_ports_are_excluded_from_the_next_pick() {
    let mirrors = Mirrors::new();
    let running = |id: i64, host: &str, forward: Option<u16>| Running {
        info: MirrorInfo {
            device_id: id,
            host: host.into(),
            serial: format!("s{id}"),
            title: format!("d{id}"),
            route: Route::Ssh,
            started_at: 0,
        },
        stop: None,
        pid: None,
        forward,
    };
    {
        let mut g = mirrors.inner.lock().unwrap();
        g.insert(1, running(1, "mefistos", Some(27183)));
        g.insert(2, running(2, "mefistos", Some(27184)));
        g.insert(3, running(3, "hetzner", Some(27185)));
        g.insert(4, running(4, "mefistos", None));
    }
    let mut taken = mirrors.forwards_on("mefistos");
    taken.sort_unstable();
    assert_eq!(taken, [27183, 27184]);
    assert_eq!(adb::free_forward_port(&taken), Some(27185));
}
