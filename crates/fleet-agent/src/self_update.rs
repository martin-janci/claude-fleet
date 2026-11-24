//! The agent updates itself from inside `fleet-agent run`, wherever no
//! `fleet-agent-update.timer` does it (a container with no systemd, an agent
//! started by hand or by an entrypoint loop).
//!
//! The same pass as `fleet-agent update` ([`crate::update`]) — the hub
//! decides, the tarball must be the bytes the signed manifest names, the
//! release goes to `<root>/<version>/` and `<root>/current` points at it —
//! with one difference: there is no unit to restart, so "restart" is this
//! process `exec`ing the new build in place (same pid, same arguments, same
//! environment). The updater's state file carries the attempt across the
//! `exec`: the new build resumes it, waits to be connected through the soak,
//! and points `current` back and `exec`s the old build if it is not.
//!
//! The check runs over HTTP (`/update/check`), not the agent link, so an
//! agent the hub refuses on protocol still asks — and the hub answers a
//! refused agent with `update_required`, which installs whatever
//! `update.agent.mode` says. A refusal also wakes the pass at once instead
//! of waiting out the interval.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fleet_update::wire::Speaks;
use fleet_update::{Component, Version};
use fleet_updater::binary::{
    parse_version_output, BinState, BinaryConfig, BinaryUpdater, Health, Host, Outcome,
};
use fleet_updater::common::{trusted_keys, FileSequences};
#[cfg(test)]
use fleet_updater::net::GitFetch;
use fleet_updater::systemd::SystemdHost;

use crate::config::Config;
use crate::conn::LinkStatus;
use crate::update::{UpdatePaths, UPDATE_TIMER};

/// Set to `0`/`off`/`false` to leave updates to someone else.
pub const ENV_SWITCH: &str = "FLEET_AGENT_SELF_UPDATE";

/// How often the `update-now` poke is looked for.
const POKE_POLL: Duration = Duration::from_secs(30);

/// Everything the pass needs, settled in `main` before the runtime starts
/// (it sets `RUNTIME_DIRECTORY`, which must happen while single-threaded).
#[derive(Debug, Clone)]
pub struct SelfUpdate {
    pub paths: UpdatePaths,
    /// The path this process was started as: what `current` is linked
    /// behind, and what it `exec`s.
    pub link: PathBuf,
    /// The arguments after the program name, for the `exec`.
    pub args: Vec<OsString>,
    /// Where the hub's `update_now` drops `update-now`.
    pub poke_dir: PathBuf,
}

impl SelfUpdate {
    /// Run before the config is read (review w09-2): a candidate that dies
    /// before its pass ever resumes — one that refuses the config, say — is
    /// rolled back by the start count `early_rollback` keeps, and this
    /// process then `exec`s the previous build. Returns only when nothing
    /// had to be done, or when the `exec` failed (logged).
    pub fn guard_start(&self) {
        match fleet_updater::binary::early_rollback(&self.paths.root, &self.paths.state_dir) {
            Ok(None) => {}
            Ok(Some(why)) => {
                tracing::error!(%why, "[agent] the new build is rolled back before it starts");
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt as _;
                    let e = std::process::Command::new(&self.link)
                        .args(&self.args)
                        .exec();
                    tracing::error!(link = %self.link.display(), error = %e, "[agent] starting the previous build failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "[agent] the update state could not be read"),
        }
    }
}

/// Whether this process runs the pass itself, and with what. `None` when an
/// updater unit is installed (it does the job, and two would race over one
/// state file), when switched off, or when the program path is unknown.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    argv: &[OsString],
    home: Option<&Path>,
    config_home: Option<&Path>,
    root: bool,
    switch: Option<&str>,
    timer_installed: impl Fn(&Path) -> bool,
    path_var: Option<OsString>,
    cwd: Option<PathBuf>,
) -> Option<SelfUpdate> {
    if matches!(
        switch.map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "off" | "false" | "no")
    ) {
        return None;
    }
    let mut timers = vec![PathBuf::from("/etc/systemd/system").join(UPDATE_TIMER)];
    if let Some(c) = config_home {
        timers.push(c.join("systemd/user").join(UPDATE_TIMER));
    }
    if timers.iter().any(|t| timer_installed(t)) {
        return None;
    }
    let paths = if root {
        UpdatePaths::system()
    } else {
        UpdatePaths::user(home?)
    };
    let link = program_path(argv.first()?, path_var, cwd)?;
    Some(SelfUpdate {
        poke_dir: paths.state_dir.join("run"),
        paths,
        link,
        args: argv[1..].to_vec(),
    })
}

/// `argv[0]` as a path: as given when it names one, else found on `$PATH`.
/// Not canonicalised: through the update layout it is the symlink, and the
/// symlink is what must keep being run.
fn program_path(
    argv0: &OsString,
    path_var: Option<OsString>,
    cwd: Option<PathBuf>,
) -> Option<PathBuf> {
    let p = Path::new(argv0);
    if p.components().count() > 1 {
        return Some(if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd?.join(p)
        });
    }
    std::env::split_paths(&path_var?)
        .map(|d| d.join(p))
        .find(|c| c.is_file())
}

impl SelfUpdate {
    /// Check, install, validate — forever. Never returns an error: a failed
    /// pass is logged and the next one tries again.
    pub async fn run(self, config: &Config, link: Arc<LinkStatus>) {
        let _ = std::fs::create_dir_all(&self.poke_dir);
        let mut updater = match self.updater(config, Arc::clone(&link)) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("[agent] self-update is off: {e}");
                return;
            }
        };
        tracing::info!(
            link = %self.link.display(),
            "[agent] self-update on (no fleet-agent-update.timer here)"
        );
        loop {
            let (outcome, wait) = updater.tick().await;
            // A refusal wakes the pass at every redial (~60 s). After an
            // attempt that failed before anything ran (a download or an
            // unpacked build that will not run), that would download the
            // same tarball again every minute (review w09-4): such a pass
            // waits its normal interval, and only `update-now` cuts it short.
            let heed_refusals = !matches!(outcome, Outcome::Failed(_));
            let poke = self.poke_dir.join("update-now");
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let step = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .min(POKE_POLL);
                if step.is_zero() {
                    break;
                }
                tokio::select! {
                    () = tokio::time::sleep(step) => {}
                    () = link.refused.notified() => {
                        if heed_refusals {
                            tracing::info!("[agent] the hub refused this agent's protocol: checking for an update now");
                            break;
                        }
                    }
                }
                if std::fs::remove_file(&poke).is_ok() {
                    tracing::info!("[agent] update-now: checking for an update");
                    break;
                }
            }
        }
    }

    fn updater(
        &self,
        config: &Config,
        link: Arc<LinkStatus>,
    ) -> Result<BinaryUpdater<ExecHost>, String> {
        let state = BinState::load(&self.paths.state_dir)?;
        let (hub, git) = crate::update::update_clients(config, |k| std::env::var(k).ok())?;
        let channel = fleet_update::HubUpdateChannel::new(
            hub.clone(),
            trusted_keys(),
            Box::new(FileSequences::open(&self.paths.state_dir)),
        );
        let host = ExecHost {
            files: SystemdHost {
                unit: crate::install::UNIT_NAME.to_string(),
                user_scope: false,
                hub: None,
                link: self.link.clone(),
                git,
                mirror: Some(hub),
            },
            link: self.link.clone(),
            args: self.args.clone(),
            status: link,
            running: own_version()?,
        };
        let mut cfg = BinaryConfig::new(
            Component::Agent,
            "fleet-agent",
            &self.paths.root,
            &self.link,
            &self.paths.state_dir,
        );
        cfg.speaks = Speaks {
            contract_accepts: None,
            agent_proto: Some(fleet_proto::PROTO_VERSION),
        };
        cfg.name = "fleet-agent self-update".into();
        Ok(BinaryUpdater::new(host, Box::new(channel), cfg, state))
    }
}

fn own_version() -> Result<Version, String> {
    parse_version_output(env!("CARGO_PKG_VERSION"))
}

/// The machine, for a pass that runs inside the process it updates.
/// Downloads and unpacking are [`SystemdHost`]'s; restarting is an `exec`.
pub struct ExecHost {
    files: SystemdHost,
    link: PathBuf,
    args: Vec<OsString>,
    status: Arc<LinkStatus>,
    running: Version,
}

impl ExecHost {
    /// Whether `link` already resolves to the build this process runs. After
    /// an `exec` the new process resumes the attempt and "restarts" again on
    /// the way through: that must be a no-op, not an endless `exec`.
    /// `current_exe` rather than `/proc/self/exe`: macOS has no `/proc`, and
    /// there the check was always false, so a rollback `exec`'d forever.
    fn runs_link(&self) -> bool {
        self.runs_link_with(std::env::current_exe())
    }

    /// [`Self::runs_link`] against `exe`, the path this process was started
    /// from: injectable, so a test proves the check with a path that is not
    /// under `/proc` on Linux too (review 23b w19-4).
    fn runs_link_with(&self, exe: std::io::Result<PathBuf>) -> bool {
        match (
            std::fs::canonicalize(&self.link),
            exe.and_then(std::fs::canonicalize),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

#[async_trait]
impl Host for ExecHost {
    async fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        self.files.fetch(url, max_bytes).await
    }

    async fn fetch_mirror(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        self.files.fetch_mirror(path, max_bytes).await
    }

    fn unpack(&self, archive: &Path, into: &Path) -> Result<(), String> {
        self.files.unpack(archive, into)
    }

    fn version_of(&self, bin: &Path) -> Result<Version, String> {
        self.files.version_of(bin)
    }

    #[cfg(unix)]
    fn restart(&self) -> Result<(), String> {
        if self.runs_link() {
            return Ok(());
        }
        use std::os::unix::process::CommandExt as _;
        tracing::info!(link = %self.link.display(), "[agent] restarting into the new build");
        // Only returns on failure.
        let e = std::process::Command::new(&self.link)
            .args(&self.args)
            .exec();
        Err(format!("exec {}: {e}", self.link.display()))
    }

    #[cfg(not(unix))]
    fn restart(&self) -> Result<(), String> {
        Err("no exec on this platform".into())
    }

    /// Nothing to stop: the agent has no database to restore, and stopping
    /// would stop this very pass.
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    async fn health(&self) -> Health {
        let line = self.status.line();
        Health::Up {
            version: Some(self.running.clone()),
            ready: self.status.connected(),
            schema: None,
            restarts: 0,
            why: Some(line).filter(|l| !l.is_empty()),
        }
    }

    fn backup(&self, _prefix: &str) -> Result<Option<(PathBuf, Option<i64>)>, String> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn it_runs_where_no_updater_unit_does() {
        let a = argv(&["/home/u/.local/bin/fleet-agent", "run", "--config", "/c"]);
        let home = Path::new("/home/u");
        let cfg = Path::new("/home/u/.config");
        let none = |_: &Path| false;
        let p = plan(&a, Some(home), Some(cfg), false, None, none, None, None).unwrap();
        assert_eq!(p.link, Path::new("/home/u/.local/bin/fleet-agent"));
        assert_eq!(p.args, argv(&["run", "--config", "/c"]));
        assert_eq!(p.paths, UpdatePaths::user(home));
        assert!(p.poke_dir.starts_with(&p.paths.state_dir));

        let root = plan(&a, None, None, true, None, none, None, None).unwrap();
        assert_eq!(root.paths, UpdatePaths::system());

        // A user timer, or a system one, already updates this agent.
        let user_timer = |t: &Path| t.starts_with(cfg);
        assert!(plan(
            &a,
            Some(home),
            Some(cfg),
            false,
            None,
            user_timer,
            None,
            None
        )
        .is_none());
        let system_timer = |t: &Path| t.starts_with("/etc");
        assert!(plan(
            &a,
            Some(home),
            Some(cfg),
            false,
            None,
            system_timer,
            None,
            None
        )
        .is_none());
        // Switched off.
        for off in ["0", "off", "FALSE", "no"] {
            assert!(plan(
                &a,
                Some(home),
                Some(cfg),
                false,
                Some(off),
                none,
                None,
                None
            )
            .is_none());
        }
        assert!(plan(
            &a,
            Some(home),
            Some(cfg),
            false,
            Some("1"),
            none,
            None,
            None
        )
        .is_some());
        // A user agent with no $HOME has nowhere to keep releases.
        assert!(plan(&a, None, Some(cfg), false, None, none, None, None).is_none());
    }

    #[test]
    fn the_program_path_is_kept_as_started() {
        let rel = argv(&["bin/fleet-agent", "run"]);
        let p = plan(
            &rel,
            Some(Path::new("/h")),
            None,
            false,
            None,
            |_| false,
            None,
            Some(PathBuf::from("/srv")),
        )
        .unwrap();
        assert_eq!(p.link, Path::new("/srv/bin/fleet-agent"));

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fleet-agent"), "").unwrap();
        let bare = argv(&["fleet-agent", "run"]);
        let p = plan(
            &bare,
            Some(Path::new("/h")),
            None,
            false,
            None,
            |_| false,
            Some(dir.path().as_os_str().to_owned()),
            None,
        )
        .unwrap();
        assert_eq!(p.link, dir.path().join("fleet-agent"));
        assert!(plan(
            &bare,
            Some(Path::new("/h")),
            None,
            false,
            None,
            |_| false,
            None,
            None
        )
        .is_none());
    }

    #[test]
    fn its_own_version_parses() {
        assert!(own_version().is_ok());
    }

    #[tokio::test]
    async fn health_is_the_link_status_of_this_very_process() {
        let notifier = crate::conn::Notifier::at(None);
        let host = ExecHost {
            files: SystemdHost {
                unit: "x".into(),
                user_scope: false,
                hub: None,
                link: PathBuf::from("/nonexistent"),
                git: GitFetch::new(None).unwrap(),
                mirror: None,
            },
            link: PathBuf::from("/nonexistent"),
            args: vec![],
            status: notifier.link(),
            running: own_version().unwrap(),
        };
        notifier.status("connecting to wss://h/agent");
        assert!(matches!(
            host.health().await,
            Health::Up { ready: false, .. }
        ));
        notifier.status(&format!(
            "{} wss://h/agent since now",
            crate::conn::CONNECTED
        ));
        match host.health().await {
            Health::Up { ready, version, .. } => {
                assert!(ready);
                assert_eq!(version, Some(own_version().unwrap()));
            }
            other => panic!("{other:?}"),
        }
        // A link that is not this process's build is not "already running".
        assert!(!host.runs_link());
    }

    /// The rollback loop guard: a link that resolves to this very binary is
    /// "already running", on every OS (macOS has no `/proc/self/exe`).
    #[cfg(unix)]
    #[test]
    fn a_link_to_this_very_binary_is_already_running() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
        let host = ExecHost {
            files: SystemdHost {
                unit: "x".into(),
                user_scope: false,
                hub: None,
                link: link.clone(),
                git: GitFetch::new(None).unwrap(),
                mirror: None,
            },
            link,
            args: vec![],
            status: crate::conn::Notifier::at(None).link(),
            running: own_version().unwrap(),
        };
        assert!(host.runs_link());
        // So `restart` is a no-op rather than an `exec`.
        assert!(host.restart().is_ok());
    }

    /// Review 23b w19-4: the check holds for an exe path outside `/proc`,
    /// as on macOS, so a Linux run proves it too: a link to the binary the
    /// process was started from is "already running", a link to another
    /// is not.
    #[cfg(unix)]
    #[test]
    fn the_link_check_compares_the_started_binary_not_proc() {
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("fleet-agent-1.2.3");
        let other = dir.path().join("fleet-agent-1.2.2");
        std::fs::write(&started, b"new").unwrap();
        std::fs::write(&other, b"old").unwrap();
        let link = dir.path().join("current");
        std::os::unix::fs::symlink(&started, &link).unwrap();
        let host = ExecHost {
            files: SystemdHost {
                unit: "x".into(),
                user_scope: false,
                hub: None,
                link: link.clone(),
                git: GitFetch::new(None).unwrap(),
                mirror: None,
            },
            link,
            args: vec![],
            status: crate::conn::Notifier::at(None).link(),
            running: own_version().unwrap(),
        };
        assert!(host.runs_link_with(Ok(started.clone())));
        assert!(!host.runs_link_with(Ok(other)));
        assert!(!host.runs_link_with(Err(std::io::Error::other("no exe"))));
    }

    /// Review 23b w19-4: the update passes' hub client really dials the
    /// proxy (a `CONNECT` to the hub), and a host the environment's
    /// no-proxy list names goes direct.
    #[tokio::test]
    async fn the_update_clients_dial_the_proxy_and_skip_no_proxy_hosts() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        async fn read_head(s: &mut tokio::net::TcpStream) -> String {
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                head.push(b[0]);
            }
            String::from_utf8(head).unwrap()
        }
        const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        // The hub: answers any GET.
        let hub = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub_port = hub.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = hub.accept().await.unwrap();
                tokio::spawn(async move {
                    read_head(&mut s).await;
                    let _ = s.write_all(OK).await;
                });
            }
        });
        // The proxy: records each CONNECT, then tunnels to the target.
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_by_proxy = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (mut client, _) = proxy.accept().await.unwrap();
                let seen = Arc::clone(&seen_by_proxy);
                tokio::spawn(async move {
                    let head = read_head(&mut client).await;
                    let target = head
                        .strip_prefix("CONNECT ")
                        .and_then(|r| r.split_once(' '))
                        .map(|(t, _)| t.to_string())
                        .unwrap();
                    seen.lock().unwrap().push(target.clone());
                    let mut upstream = tokio::net::TcpStream::connect(&target).await.unwrap();
                    client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                });
            }
        });
        let config: Config = serde_json::from_value(serde_json::json!({
            "hub": format!("http://127.0.0.1:{hub_port}"),
            "token": "t",
            "insecure": true,
        }))
        .unwrap();
        let proxy_url = format!("http://127.0.0.1:{proxy_port}");
        let env = |no_proxy: Option<&'static str>| {
            let proxy_url = proxy_url.clone();
            move |k: &str| match k {
                "HTTPS_PROXY" => Some(proxy_url.clone()),
                "NO_PROXY" => no_proxy.map(str::to_string),
                _ => None,
            }
        };
        let (hub, _) = crate::update::update_clients(&config, env(None)).unwrap();
        assert_eq!(hub.get("/update/artifact/ab", 64).await.unwrap(), b"ok");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![format!("127.0.0.1:{hub_port}")],
            "the hub was reached through the proxy"
        );
        let (hub, _) = crate::update::update_clients(&config, env(Some("127.0.0.1"))).unwrap();
        assert_eq!(hub.get("/update/artifact/ab", 64).await.unwrap(), b"ok");
        assert_eq!(seen.lock().unwrap().len(), 1, "no-proxy goes direct");
    }
}
