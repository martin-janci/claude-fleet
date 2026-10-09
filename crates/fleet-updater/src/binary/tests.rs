//! The binary target's loop against a pretend machine: real directories and
//! symlinks in a temp dir, a fake systemd unit that runs whatever `current`
//! points at when it is restarted, and a fake hub.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fleet_update::manifest::ReleaseManifest;
use fleet_update::wire::{ChannelRef, DocRef, Reason, Target};
use fleet_update::{Mode, ReasonCode, Source, Track, UpdateError};

use super::*;

const BIN: &str = "fleet-agent";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Behaviour {
    Good,
    /// Never becomes ready (an agent that cannot reach its hub).
    NeverReady,
    /// Up, then restarted by systemd during the soak.
    Restarts,
}

struct Machine {
    /// version → behaviour, for what `current` runs after a restart.
    behaviour: HashMap<String, Behaviour>,
    running: Option<String>,
    restarts: u32,
    polls_since_restart: u32,
    /// The bytes each URL serves.
    files: HashMap<String, Vec<u8>>,
    mirror: HashMap<String, Vec<u8>>,
    fetched: Vec<String>,
    stopped: u32,
    /// `Some(schema)`: the hub, which backs up and reports a schema.
    hub_schema: HashMap<String, i64>,
    backup_dir: Option<PathBuf>,
    /// The hub's data dir: 0.5.4 migrates its database when it starts.
    migrates: Option<PathBuf>,
}

#[derive(Clone)]
struct FakeHost {
    m: Arc<Mutex<Machine>>,
    root: PathBuf,
}

/// A pretend binary is a text file that says its version.
fn fake_bin(path: &Path, version: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{BIN} {version}\n")).unwrap();
}

/// A pretend tarball: its bytes are the version it unpacks to.
fn tarball(version: &str) -> Vec<u8> {
    format!("tarball of {version}").into_bytes()
}

#[async_trait]
impl Host for FakeHost {
    async fn fetch(&self, url: &str, _max: u64) -> Result<Vec<u8>, String> {
        let mut m = self.m.lock().unwrap();
        m.fetched.push(url.to_string());
        m.files
            .get(url)
            .cloned()
            .ok_or_else(|| format!("{url}: 404"))
    }

    async fn fetch_mirror(&self, path: &str, _max: u64) -> Result<Vec<u8>, String> {
        let mut m = self.m.lock().unwrap();
        m.fetched.push(format!("mirror:{path}"));
        m.mirror
            .get(path)
            .cloned()
            .ok_or_else(|| format!("{path}: 404"))
    }

    fn unpack(&self, archive: &Path, into: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(archive).map_err(|e| e.to_string())?;
        let v = text.strip_prefix("tarball of ").ok_or("not a tarball")?;
        fake_bin(&into.join(format!("{BIN}-{v}-x86_64")).join(BIN), v);
        Ok(())
    }

    fn version_of(&self, bin: &Path) -> Result<Version, String> {
        let text = std::fs::read_to_string(bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        parse_version_output(&text)
    }

    fn restart(&self) -> Result<(), String> {
        let target = std::fs::read_link(self.root.join("current")).map_err(|e| e.to_string())?;
        let mut m = self.m.lock().unwrap();
        let v = target.to_string_lossy().into_owned();
        if let (Some(data), "0.5.4") = (&m.migrates, v.as_str()) {
            std::fs::write(data.join("state.db"), "db of 0.5.4 (migrated)").unwrap();
        }
        m.running = Some(v);
        m.polls_since_restart = 0;
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.running = None;
        m.stopped += 1;
        Ok(())
    }

    async fn health(&self) -> Health {
        let mut m = self.m.lock().unwrap();
        let Some(v) = m.running.clone() else {
            return Health::Down("inactive/dead".into());
        };
        m.polls_since_restart += 1;
        let b = m.behaviour.get(&v).copied().unwrap_or(Behaviour::Good);
        if b == Behaviour::Restarts && m.polls_since_restart > 2 {
            m.restarts += 1;
            m.polls_since_restart = 0;
        }
        Health::Up {
            version: Version::parse(&v).ok(),
            ready: b != Behaviour::NeverReady,
            schema: m.hub_schema.get(&v).copied(),
            restarts: m.restarts,
            why: (b == Behaviour::NeverReady).then(|| "not connected to its hub yet".into()),
        }
    }

    fn backup(&self, prefix: &str) -> Result<Option<(PathBuf, Option<i64>)>, String> {
        let m = self.m.lock().unwrap();
        let Some(dir) = &m.backup_dir else {
            return Ok(None);
        };
        let running = m.running.clone().unwrap_or_default();
        let p = dir.join(format!("{prefix}.db"));
        std::fs::write(&p, format!("db of {running}")).unwrap();
        Ok(Some((p, m.hub_schema.get(&running).copied())))
    }
}

#[derive(Clone)]
struct FakeChannel(Arc<Mutex<Hub>>);

struct Hub {
    outcome: Option<CheckOutcome>,
    reports: Vec<Report>,
}

#[async_trait]
impl UpdateChannel for FakeChannel {
    fn source(&self) -> Source {
        Source::Hub
    }

    async fn check(&self, _req: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        self.0
            .lock()
            .unwrap()
            .outcome
            .clone()
            .ok_or_else(|| UpdateError::Transport("no decision set".into()))
    }

    async fn report(&self, r: &Report) -> Result<(), UpdateError> {
        self.0.lock().unwrap().reports.push(r.clone());
        Ok(())
    }
}

const URL: &str = "https://github.com/x/releases/download/v0.5.4/fleet-agent-0.5.4-x86_64-unknown-linux-gnu.tar.gz";

fn outcome(
    component: Component,
    mode: Mode,
    code: ReasonCode,
    bytes: &[u8],
    schema_to: Option<i64>,
    mirror: Option<&str>,
) -> CheckOutcome {
    let mut m: ReleaseManifest = serde_json::from_str(&fleet_update::testkit::manifest_json(
        "0.5.4",
        1,
        [1, 1],
        1,
        [1, 1],
    ))
    .unwrap();
    m.compatibility.store = schema_to.map(|s| fleet_update::manifest::StoreCompat {
        schema_to: s,
        opens_down_to: s,
    });
    let artifact = Artifact::Tarball {
        target: "x86_64-unknown-linux-gnu".into(),
        name: "fleet-agent-0.5.4-x86_64-unknown-linux-gnu.tar.gz".into(),
        sha256: sha256_hex(bytes),
        size: bytes.len() as u64,
    };
    let v = Version::new(0, 5, 4);
    CheckOutcome {
        decision: Decision {
            update_proto: UPDATE_PROTO,
            component,
            status: Status::UpdateAvailable,
            source: Source::Hub,
            track: Track::Stable,
            mode,
            installed: Version::new(0, 5, 3),
            target: Some(Target {
                version: v.clone(),
                mandatory: false,
                deadline: None,
                manifest: DocRef {
                    url: "u".into(),
                    sha256: "s".into(),
                },
                channel: ChannelRef { sequence: 1 },
                artifact: artifact.clone(),
                url: Some(URL.into()),
                mirror: mirror.map(String::from),
                evidence: None,
            }),
            reason: Reason {
                code,
                text: "test".into(),
            },
            next_check_secs: 3600,
        },
        verified: Some(VerifiedTarget {
            component,
            version: v,
            artifact,
            url: Some(URL.into()),
            manifest: m,
            channel_sequence: 1,
        }),
    }
}

struct Rig {
    m: Arc<Mutex<Machine>>,
    hub: Arc<Mutex<Hub>>,
    u: BinaryUpdater<FakeHost>,
    root: PathBuf,
    link: PathBuf,
    _dir: tempfile::TempDir,
}

/// 0.5.3 runs from a plain file at `<dir>/bin/fleet-agent` (as `fleet-agent
/// install` left it); 0.5.4 behaves as `next`.
fn rig(next: Behaviour, component: Component) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("opt");
    let link = dir.path().join("bin").join(BIN);
    fake_bin(&link, "0.5.3");
    let mut behaviour = HashMap::new();
    behaviour.insert("0.5.4".to_string(), next);
    let m = Arc::new(Mutex::new(Machine {
        behaviour,
        running: Some("0.5.3".into()),
        restarts: 0,
        polls_since_restart: 0,
        files: HashMap::from([(URL.to_string(), tarball("0.5.4"))]),
        mirror: HashMap::new(),
        fetched: vec![],
        stopped: 0,
        hub_schema: HashMap::new(),
        backup_dir: None,
        migrates: None,
    }));
    let hub = Arc::new(Mutex::new(Hub {
        outcome: Some(outcome(
            component,
            Mode::Automatic,
            ReasonCode::NewerRecommended,
            &tarball("0.5.4"),
            None,
            None,
        )),
        reports: vec![],
    }));
    let mut cfg = BinaryConfig::new(component, BIN, &root, &link, &dir.path().join("state"));
    cfg.poll = Duration::from_millis(1);
    cfg.ready_timeout = Duration::from_millis(50);
    cfg.soak = Duration::from_millis(20);
    let host = FakeHost {
        m: m.clone(),
        root: root.clone(),
    };
    let u = BinaryUpdater::new(
        host,
        Box::new(FakeChannel(hub.clone())),
        cfg,
        BinState::default(),
    );
    Rig {
        m,
        hub,
        u,
        root,
        link,
        _dir: dir,
    }
}

fn phases(r: &Rig) -> Vec<&'static str> {
    r.hub
        .lock()
        .unwrap()
        .reports
        .iter()
        .map(|r| r.phase.as_str())
        .collect()
}

#[tokio::test]
async fn a_good_release_is_installed_through_the_versioned_layout() {
    let mut r = rig(Behaviour::Good, Component::Agent);
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(
        phases(&r),
        [
            "downloading",
            "verifying",
            "ready",
            "installing",
            "validating",
            "success"
        ]
    );
    // The old binary was adopted into the layout, and the unit's path now
    // runs through `current`.
    assert_eq!(
        std::fs::read_link(&r.link).unwrap(),
        r.root.join("current").join(BIN)
    );
    assert_eq!(
        std::fs::read_link(r.root.join("current")).unwrap(),
        Path::new("0.5.4")
    );
    assert!(r.root.join("0.5.3").join(BIN).is_file());
    assert_eq!(r.u.host.version_of(&r.link).unwrap(), Version::new(0, 5, 4));
    assert_eq!(r.u.state.phase, UpdatePhase::Idle);
    assert!(!r.root.join(".staging").exists());
}

#[tokio::test]
async fn one_that_never_connects_goes_back_to_the_previous_release() {
    let mut r = rig(Behaviour::NeverReady, Component::Agent);
    let (o, _) = r.u.tick().await;
    assert_eq!(
        o,
        Outcome::RolledBack {
            to: Version::new(0, 5, 3),
            data_restored: false
        }
    );
    assert_eq!(
        std::fs::read_link(r.root.join("current")).unwrap(),
        Path::new("0.5.3")
    );
    assert_eq!(r.m.lock().unwrap().running.as_deref(), Some("0.5.3"));
    assert!(phases(&r).ends_with(&["failed", "rolling_back", "recovered"]));
    let failed = r
        .hub
        .lock()
        .unwrap()
        .reports
        .iter()
        .find(|x| x.phase == UpdatePhase::Failed)
        .cloned()
        .unwrap();
    assert!(failed.error.unwrap().contains("not connected to its hub"));
    // Not retried by itself.
    assert!(r
        .u
        .state
        .bad
        .is_bad(&Version::new(0, 5, 4), Some(&sha256_hex(&tarball("0.5.4")))));
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Nothing);
}

#[tokio::test]
async fn a_restart_during_the_soak_fails_it() {
    let mut r = rig(Behaviour::Restarts, Component::Agent);
    r.u.cfg.soak = Duration::from_millis(50);
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::RolledBack { .. }), "{o:?}");
    let failed = r
        .hub
        .lock()
        .unwrap()
        .reports
        .iter()
        .find(|x| x.phase == UpdatePhase::Failed)
        .cloned()
        .unwrap();
    assert!(failed.error.unwrap().contains("restarted"));
}

#[tokio::test]
async fn bytes_the_manifest_did_not_sign_touch_nothing() {
    let mut r = rig(Behaviour::Good, Component::Agent);
    r.m.lock()
        .unwrap()
        .files
        .insert(URL.into(), b"tarball of 0.6.6".to_vec());
    let (o, _) = r.u.tick().await;
    let Outcome::Failed(why) = o else {
        panic!("{o:?}")
    };
    assert!(why.contains("the manifest signed"), "{why}");
    // The unit's binary is still the plain file it was; nothing restarted.
    assert!(std::fs::read_link(&r.link).is_err());
    assert_eq!(r.m.lock().unwrap().running.as_deref(), Some("0.5.3"));
    assert_eq!(phases(&r), ["downloading", "failed"]);
}

#[tokio::test]
async fn the_hubs_mirror_is_tried_first_and_github_after_it() {
    let mut r = rig(Behaviour::Good, Component::Agent);
    let path = format!("/update/artifact/{}", sha256_hex(&tarball("0.5.4")));
    r.hub.lock().unwrap().outcome = Some(outcome(
        Component::Agent,
        Mode::Automatic,
        ReasonCode::NewerRecommended,
        &tarball("0.5.4"),
        None,
        Some(&path),
    ));
    r.m.lock()
        .unwrap()
        .mirror
        .insert(path.clone(), tarball("0.5.4"));
    assert_eq!(r.u.tick().await.0, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(r.m.lock().unwrap().fetched, [format!("mirror:{path}")]);

    // A mirror that serves the wrong bytes is passed over for GitHub.
    let mut r = rig(Behaviour::Good, Component::Agent);
    r.hub.lock().unwrap().outcome = Some(outcome(
        Component::Agent,
        Mode::Automatic,
        ReasonCode::NewerRecommended,
        &tarball("0.5.4"),
        None,
        Some(&path),
    ));
    r.m.lock()
        .unwrap()
        .mirror
        .insert(path.clone(), b"junk".to_vec());
    assert_eq!(r.u.tick().await.0, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(
        r.m.lock().unwrap().fetched,
        [format!("mirror:{path}"), URL.to_string()]
    );
}

#[tokio::test]
async fn notify_offers_once_and_installs_nothing() {
    let mut r = rig(Behaviour::Good, Component::Agent);
    r.hub.lock().unwrap().outcome = Some(outcome(
        Component::Agent,
        Mode::Notify,
        ReasonCode::NewerRecommended,
        &tarball("0.5.4"),
        None,
        None,
    ));
    assert_eq!(r.u.tick().await.0, Outcome::Nothing);
    assert_eq!(r.u.tick().await.0, Outcome::Nothing);
    assert_eq!(phases(&r), ["available"]);
    assert!(r.m.lock().unwrap().fetched.is_empty());
    // A pin is a person saying yes.
    r.hub.lock().unwrap().outcome = Some(outcome(
        Component::Agent,
        Mode::Notify,
        ReasonCode::Pinned,
        &tarball("0.5.4"),
        None,
        None,
    ));
    assert_eq!(r.u.tick().await.0, Outcome::Updated(Version::new(0, 5, 4)));
}

#[tokio::test]
async fn a_bare_hub_that_migrated_comes_back_on_its_backup() {
    let mut r = rig(Behaviour::NeverReady, Component::Hub);
    let data = r._dir.path().join("data");
    std::fs::create_dir_all(data.join("backups")).unwrap();
    std::fs::write(data.join("state.db"), "db of 0.5.3").unwrap();
    {
        let mut m = r.m.lock().unwrap();
        m.backup_dir = Some(data.join("backups"));
        m.migrates = Some(data.clone());
        m.hub_schema.insert("0.5.3".into(), 140);
        m.hub_schema.insert("0.5.4".into(), 141);
    }
    r.u.cfg.data_dir = Some(data.clone());
    r.hub.lock().unwrap().outcome = Some(outcome(
        Component::Hub,
        Mode::Automatic,
        ReasonCode::NewerRecommended,
        &tarball("0.5.4"),
        Some(141),
        None,
    ));
    let (o, _) = r.u.tick().await;
    assert_eq!(
        o,
        Outcome::RolledBack {
            to: Version::new(0, 5, 3),
            data_restored: true
        }
    );
    // The backup 0.5.3 took is back; the migrated copy is kept aside.
    assert_eq!(
        std::fs::read_to_string(data.join("state.db")).unwrap(),
        "db of 0.5.3"
    );
    let aside: Vec<_> = std::fs::read_dir(&data)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("failed-0.5.4-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert!(
        r.m.lock().unwrap().stopped >= 1,
        "stopped before its database moved"
    );
    let recovered = r.hub.lock().unwrap().reports.last().cloned().unwrap();
    assert_eq!(recovered.phase, UpdatePhase::Recovered);
    assert_eq!(recovered.detail["data_restored"], true);
}

#[tokio::test]
async fn an_interrupted_install_resumes_its_validation() {
    let mut r = rig(Behaviour::Good, Component::Agent);
    // Everything up to the switch, then "the updater died".
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
    r.u.state.phase = UpdatePhase::Validating;
    r.u.state.candidate = Some(Candidate {
        version: Version::new(0, 5, 4),
        sha256: sha256_hex(&tarball("0.5.4")),
        schema_to: None,
    });
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
}

#[test]
fn old_releases_are_pruned_but_never_the_running_or_previous_one() {
    let mut r = rig(Behaviour::Good, Component::Agent);
    for v in ["0.5.0", "0.5.1", "0.5.2", "0.5.3", "0.5.4"] {
        fake_bin(&r.root.join(v).join(BIN), v);
    }
    r.u.state.previous = Some(Before {
        version: Version::new(0, 5, 0),
        schema: None,
        backup: None,
    });
    r.u.cfg.keep_versions = 1;
    r.u.prune_versions(&Version::new(0, 5, 4));
    let mut left: Vec<String> = std::fs::read_dir(&r.root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["0.5.0", "0.5.3", "0.5.4"]);
}

#[test]
fn version_lines_and_symlinks() {
    assert_eq!(
        parse_version_output("fleet-agent 0.5.4\n").unwrap(),
        Version::new(0, 5, 4)
    );
    assert_eq!(
        parse_version_output("fleet-hub 0.6.0-dev.3.gabc1234")
            .unwrap()
            .to_string(),
        "0.6.0-dev.3.gabc1234"
    );
    assert!(parse_version_output("nothing").is_err());
    let d = tempfile::tempdir().unwrap();
    let l = d.path().join("current");
    point(&l, Path::new("0.5.3")).unwrap();
    point(&l, Path::new("0.5.4")).unwrap();
    assert_eq!(std::fs::read_link(&l).unwrap(), Path::new("0.5.4"));
    let found = d.path().join("x/fleet-agent-1-y/fleet-agent");
    fake_bin(&found, "1.0.0");
    assert_eq!(find_binary(&d.path().join("x"), BIN), Some(found));
}

#[test]
fn a_restore_is_needed_unless_the_schema_did_not_move() {
    let prev = |s| Before {
        version: Version::new(0, 5, 3),
        schema: s,
        backup: None,
    };
    let cand = |s| Candidate {
        version: Version::new(0, 5, 4),
        sha256: String::new(),
        schema_to: s,
    };
    assert!(needs_restore(&prev(None), Some(&cand(Some(140))), None));
    assert!(!needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(140))),
        None
    ));
    assert!(needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(141))),
        None
    ));
    assert!(needs_restore(&prev(Some(140)), Some(&cand(None)), None));
    assert!(!needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(150))),
        Some(140)
    ));
}
