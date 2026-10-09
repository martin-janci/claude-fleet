//! The update loop against a simulated Docker daemon and hub (design §12,
//! S6's acceptance: a good image, one that crashes on start, and one that
//! migrates and then never becomes ready — each ending in the right state,
//! the last with the backup restored).
//!
//! The simulated database is `state.db` holding its schema number. A build
//! migrates it up on start, and — like the real downgrade guard — refuses
//! (crashes) on a database newer than it knows.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fleet_update::manifest::ReleaseManifest;
use fleet_update::wire::{ChannelRef, DocRef, Reason, Target};
use fleet_update::{Source, Track, UpdateError};
use serde_json::json;

use super::*;
use crate::docker::{DockerError, ExecOutput};

const IMAGE: &str = "ghcr.io/test/fleet-hub";
const NAME: &str = "hub-fleet-hub-1";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Behaviour {
    Good,
    /// Exits at once; the restart policy brings it back, again and again.
    CrashOnStart,
    /// Migrates the database, then never reports ready.
    MigratesThenUnready,
}

#[derive(Clone, Debug)]
struct ImageDef {
    id: String,
    version: String,
    schema: i64,
    behaviour: Behaviour,
    /// What `RepoDigests` says (`image@digest`).
    repo_digest: String,
    /// Starts after which it behaves as `CrashOnStart`.
    breaks_after_starts: Option<u32>,
    starts: u32,
}

#[derive(Clone, Debug)]
struct Ctr {
    id: String,
    image_id: String,
    running: bool,
    restarts: i64,
    ready: bool,
    body: Value,
}

struct World {
    images: Vec<ImageDef>,
    containers: HashMap<String, Ctr>,
    data: PathBuf,
    next: u32,
    calls: Vec<String>,
    /// Pull answers with an image whose RepoDigests do not carry the digest.
    pull_wrong_digest: bool,
}

impl World {
    fn db_schema(&self) -> i64 {
        std::fs::read_to_string(self.data.join("state.db"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn image(&self, reference: &str) -> Option<&ImageDef> {
        self.images
            .iter()
            .find(|i| i.id == reference || i.repo_digest == reference)
    }

    fn start(&mut self, name: &str) {
        let image_id = self.containers[name].image_id.clone();
        let db = self.db_schema();
        let img = self.images.iter_mut().find(|i| i.id == image_id).unwrap();
        img.starts += 1;
        let broken = img.breaks_after_starts.is_some_and(|n| img.starts > n);
        let behaviour = if broken {
            Behaviour::CrashOnStart
        } else {
            img.behaviour
        };
        let (schema, data) = (img.schema, self.data.clone());
        let c = self.containers.get_mut(name).unwrap();
        // The downgrade guard: a database newer than this build is refused.
        if db > schema || behaviour == Behaviour::CrashOnStart {
            c.running = false;
            c.restarts = 3;
            c.ready = false;
            return;
        }
        if schema > db {
            std::fs::write(data.join("state.db"), schema.to_string()).unwrap();
        }
        c.running = true;
        c.restarts = 0;
        c.ready = behaviour == Behaviour::Good;
    }
}

#[derive(Clone)]
struct FakeDocker(Arc<Mutex<World>>);

fn err(s: &str) -> DockerError {
    DockerError(s.into())
}

#[async_trait]
impl Docker for FakeDocker {
    async fn inspect_container(&self, name: &str) -> Result<Option<Value>, DockerError> {
        let w = self.0.lock().unwrap();
        Ok(w.containers.get(name).map(|c| {
            json!({
                "Id": c.id, "Image": c.image_id, "RestartCount": c.restarts,
                "State": {"Running": c.running, "ExitCode": if c.running { 0 } else { 1 }},
                "Config": c.body.clone(),
                "HostConfig": {"RestartPolicy": {"Name": "unless-stopped"}},
                "NetworkSettings": {"Networks": {"hub_default": {"Aliases": ["fleet-hub", &c.id[..12]]}}}
            })
        }))
    }

    async fn inspect_image(&self, reference: &str) -> Result<Option<Value>, DockerError> {
        let w = self.0.lock().unwrap();
        Ok(w.image(reference).map(|i| {
            let digests = if w.pull_wrong_digest {
                vec![format!("{IMAGE}@sha256:somethingelse")]
            } else {
                vec![i.repo_digest.clone()]
            };
            json!({"Id": i.id, "RepoDigests": digests, "Config": {"Env": ["PATH=/usr/bin"], "Cmd": ["serve"]}})
        }))
    }

    async fn list_containers(&self, _labels: &[String]) -> Result<Vec<Value>, DockerError> {
        Ok(vec![])
    }

    async fn pull(&self, image: &str, digest: &str) -> Result<(), DockerError> {
        let mut w = self.0.lock().unwrap();
        w.calls.push(format!("pull {image}@{digest}"));
        w.image(&format!("{image}@{digest}"))
            .map(|_| ())
            .ok_or_else(|| err("manifest unknown"))
    }

    async fn create(&self, name: &str, body: &Value) -> Result<String, DockerError> {
        let mut w = self.0.lock().unwrap();
        if w.containers.contains_key(name) {
            return Err(err("Conflict. The container name is already in use"));
        }
        let reference = body["Image"].as_str().unwrap().to_string();
        let image_id = w
            .image(&reference)
            .ok_or_else(|| err("No such image"))?
            .id
            .clone();
        w.next += 1;
        let id = format!("{:064}", w.next);
        w.calls.push(format!("create {reference}"));
        w.containers.insert(
            name.into(),
            Ctr {
                id: id.clone(),
                image_id,
                running: false,
                restarts: 0,
                ready: false,
                body: body.clone(),
            },
        );
        Ok(id)
    }

    async fn start(&self, id: &str) -> Result<(), DockerError> {
        let mut w = self.0.lock().unwrap();
        let name = w
            .containers
            .iter()
            .find(|(_, c)| c.id == id)
            .map(|(n, _)| n.clone())
            .ok_or_else(|| err("no such container"))?;
        w.calls.push("start".into());
        w.start(&name);
        Ok(())
    }

    async fn stop(&self, id: &str, _t: u64) -> Result<(), DockerError> {
        let mut w = self.0.lock().unwrap();
        for c in w.containers.values_mut().filter(|c| c.id == id) {
            c.running = false;
            c.ready = false;
        }
        Ok(())
    }

    async fn remove(&self, id: &str) -> Result<(), DockerError> {
        let mut w = self.0.lock().unwrap();
        w.containers.retain(|_, c| c.id != id);
        Ok(())
    }

    async fn exec(&self, id: &str, cmd: &[String]) -> Result<ExecOutput, DockerError> {
        let mut w = self.0.lock().unwrap();
        let c = w
            .containers
            .values()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| err("no such container"))?;
        if !c.running {
            return Err(err("container is not running"));
        }
        let img = w.image(&c.image_id).unwrap().clone();
        let out = |code: i64, stdout: String| ExecOutput {
            exit_code: code,
            stdout,
            stderr: String::new(),
        };
        match cmd.get(1).map(String::as_str) {
            Some("healthcheck") => {
                let report = json!({
                    "ready": c.ready, "live": true, "fresh": true,
                    "hub": {"ready": c.ready, "pid": 7, "version": img.version, "commit": format!("c{}", img.version),
                            "build_id": "test", "contract": 1, "agent_proto": [1, 1], "peer_proto": 1,
                            "schema": w.db_schema(), "started_at": 0, "heartbeat_at": 0,
                            "checks": {"store": "ok", "listener": "ok",
                                       "first_reconcile": if c.ready { "ok" } else { "failed" }, "reconcile_failures": 0}},
                    "error": if c.ready { Value::Null } else { json!("first reconcile failed") }
                });
                Ok(out(if c.ready { 0 } else { 1 }, report.to_string()))
            }
            Some("backup") => {
                let prefix = &cmd[3];
                w.next += 1;
                let name = format!("{prefix}-{}.db", w.next);
                let dir = w.data.join("backups");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::copy(w.data.join("state.db"), dir.join(&name)).unwrap();
                w.calls.push(format!("backup {name}"));
                Ok(out(
                    0,
                    json!({"path": format!("/var/lib/fleet-hub/backups/{name}"), "schema": w.db_schema(), "bytes": 3})
                        .to_string(),
                ))
            }
            _ => Ok(out(127, String::new())),
        }
    }

    async fn logs(&self, _id: &str, _tail: u32) -> Result<String, DockerError> {
        Ok("panic: the candidate fell over\n".into())
    }
}

/// The hub, as far as the updater sees it: one decision, and the reports.
#[derive(Clone)]
struct FakeChannel(Arc<Mutex<Hub>>);

struct Hub {
    outcome: Option<CheckOutcome>,
    reports: Vec<Report>,
    down: bool,
    /// Checks get through, reports do not (the hub restarting under them).
    reports_down: bool,
}

#[async_trait]
impl UpdateChannel for FakeChannel {
    fn source(&self) -> Source {
        Source::Hub
    }

    async fn check(&self, _req: &CheckRequest) -> Result<CheckOutcome, fleet_update::UpdateError> {
        let h = self.0.lock().unwrap();
        if h.down {
            return Err(UpdateError::Transport("connection refused".into()));
        }
        h.outcome
            .clone()
            .ok_or_else(|| UpdateError::Transport("no decision set".into()))
    }

    async fn report(&self, r: &Report) -> Result<(), fleet_update::UpdateError> {
        let mut h = self.0.lock().unwrap();
        if h.down || h.reports_down {
            return Err(UpdateError::Transport("connection refused".into()));
        }
        h.reports.push(r.clone());
        Ok(())
    }
}

fn manifest(version: &str, schema_to: Option<i64>) -> ReleaseManifest {
    let mut m: ReleaseManifest = serde_json::from_str(&fleet_update::testkit::manifest_json(
        version,
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
    m
}

fn outcome(
    status: Status,
    mode: Mode,
    code: ReasonCode,
    version: &str,
    schema_to: Option<i64>,
) -> CheckOutcome {
    let m = manifest(version, schema_to);
    let artifact = m
        .artifact_for(Component::Hub, &Platform::new("linux", "x86_64", "oci"))
        .unwrap()
        .clone();
    let v = Version::parse(version).unwrap();
    CheckOutcome {
        decision: Decision {
            update_proto: UPDATE_PROTO,
            component: Component::Hub,
            status,
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
                url: None,
                evidence: None,
            }),
            reason: Reason {
                code,
                text: "test".into(),
            },
            next_check_secs: 3600,
        },
        verified: Some(VerifiedTarget {
            component: Component::Hub,
            version: v,
            artifact,
            url: None,
            manifest: m,
            channel_sequence: 1,
        }),
    }
}

struct Rig {
    world: Arc<Mutex<World>>,
    hub: Arc<Mutex<Hub>>,
    u: Updater<FakeDocker>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// 0.5.3 runs on schema 140; `next` is the 0.5.4 image.
fn rig(next: Behaviour, next_schema: i64) -> Rig {
    let data = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("state.db"), "140").unwrap();
    let images = vec![
        ImageDef {
            id: "sha256:old".into(),
            version: "0.5.3".into(),
            schema: 140,
            behaviour: Behaviour::Good,
            repo_digest: format!("{IMAGE}@sha256:hub-0.5.3"),
            breaks_after_starts: None,
            starts: 0,
        },
        ImageDef {
            id: "sha256:new".into(),
            version: "0.5.4".into(),
            schema: next_schema,
            behaviour: next,
            repo_digest: format!("{IMAGE}@sha256:hub-0.5.4"),
            breaks_after_starts: None,
            starts: 0,
        },
    ];
    let mut w = World {
        images,
        containers: HashMap::new(),
        data: data.path().to_path_buf(),
        next: 0,
        calls: vec![],
        pull_wrong_digest: false,
    };
    w.containers.insert(
        NAME.into(),
        Ctr {
            id: format!("{:064}", 999),
            image_id: "sha256:old".into(),
            running: false,
            restarts: 0,
            ready: false,
            body: json!({"Image": format!("{IMAGE}:0.5.3"), "Env": ["PATH=/usr/bin", "FLEET_HUB_BIND=0.0.0.0"], "Cmd": ["serve"]}),
        },
    );
    w.start(NAME);
    let world = Arc::new(Mutex::new(w));
    let hub = Arc::new(Mutex::new(Hub {
        outcome: None,
        reports: vec![],
        down: false,
        reports_down: false,
    }));
    let mut cfg = Config::defaults(NAME, data.path(), state.path());
    cfg.platform = Platform::new("linux", "x86_64", "oci");
    cfg.poll = Duration::from_secs(1);
    let u = Updater::new(
        FakeDocker(world.clone()),
        Box::new(FakeChannel(hub.clone())),
        cfg,
        State::load(state.path()).unwrap(),
    );
    Rig {
        world,
        hub,
        u,
        _dirs: (data, state),
    }
}

impl Rig {
    fn decide(&self, o: CheckOutcome) {
        self.hub.lock().unwrap().outcome = Some(o);
    }

    fn phases(&self) -> Vec<&'static str> {
        self.hub
            .lock()
            .unwrap()
            .reports
            .iter()
            .map(|r| r.phase.as_str())
            .collect()
    }

    fn running_image(&self) -> String {
        let w = self.world.lock().unwrap();
        let c = &w.containers[NAME];
        assert!(c.running, "the hub container is not running");
        c.image_id.clone()
    }

    fn db(&self) -> i64 {
        self.world.lock().unwrap().db_schema()
    }
}

fn automatic(version: &str, schema_to: Option<i64>) -> CheckOutcome {
    outcome(
        Status::UpdateAvailable,
        Mode::Automatic,
        ReasonCode::NewerRecommended,
        version,
        schema_to,
    )
}

#[tokio::test(start_paused = true)]
async fn a_good_image_is_installed_validated_and_kept() {
    let mut r = rig(Behaviour::Good, 141);
    r.decide(automatic("0.5.4", Some(141)));
    let (o, wait) = r.u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(wait, Duration::from_secs(3600));
    assert_eq!(r.running_image(), "sha256:new");
    assert_eq!(r.db(), 141);
    assert_eq!(
        r.phases(),
        [
            "downloading",
            "verifying",
            "ready",
            "installing",
            "validating",
            "success"
        ]
    );
    let s = &r.u.state;
    assert_eq!(s.phase, UpdatePhase::Idle);
    assert_eq!(s.current.as_ref().unwrap().version, Version::new(0, 5, 4));
    assert_eq!(s.previous.as_ref().unwrap().version, Version::new(0, 5, 3));
    assert!(s.pending_reports.is_empty());
    // Pulled by digest, created by digest; the container keeps its own env and
    // drops the image's.
    let w = r.world.lock().unwrap();
    assert!(w.calls.contains(&format!("pull {IMAGE}@sha256:hub-0.5.4")));
    assert!(w
        .calls
        .contains(&format!("create {IMAGE}@sha256:hub-0.5.4")));
    assert_eq!(
        w.containers[NAME].body["Env"],
        json!(["FLEET_HUB_BIND=0.0.0.0"])
    );
    // The pre-update backup exists, of the old schema.
    let backup = s.previous.as_ref().unwrap().backup.clone().unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "140");
    // The success report says what runs now.
    let last = r.hub.lock().unwrap().reports.last().cloned().unwrap();
    assert_eq!(last.installed.version, Version::new(0, 5, 4));
    assert_eq!(last.from, Some(Version::new(0, 5, 3)));
    assert_eq!(last.to, Some(Version::new(0, 5, 4)));
    let attempts: std::collections::BTreeSet<_> = r
        .hub
        .lock()
        .unwrap()
        .reports
        .iter()
        .map(|r| r.attempt.clone())
        .collect();
    assert_eq!(attempts.len(), 1, "one attempt id for the whole run");
}

#[tokio::test(start_paused = true)]
async fn an_image_that_crashes_on_start_is_rolled_back_without_touching_the_data() {
    let mut r = rig(Behaviour::CrashOnStart, 140);
    r.decide(automatic("0.5.4", Some(140)));
    let (o, _) = r.u.tick().await;
    assert_eq!(
        o,
        Outcome::RolledBack {
            to: Version::new(0, 5, 3),
            data_restored: false
        }
    );
    assert_eq!(r.running_image(), "sha256:old");
    assert_eq!(r.db(), 140);
    assert_eq!(
        r.phases(),
        [
            "downloading",
            "verifying",
            "ready",
            "installing",
            "validating",
            "failed",
            "rolling_back",
            "recovered"
        ]
    );
    assert!(r
        .u
        .state
        .bad
        .is_bad(&Version::new(0, 5, 4), Some("sha256:hub-0.5.4")));
    assert_eq!(r.u.state.phase, UpdatePhase::Idle);
    // The failed candidate's log was kept.
    let logs: Vec<_> = std::fs::read_dir(&r.u.cfg.state_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("failed-0.5.4-"))
        .collect();
    assert_eq!(logs.len(), 1);

    // The loop breaker: the same version is not tried again.
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Nothing);
    assert_eq!(r.phases().len(), 8);
}

#[tokio::test(start_paused = true)]
async fn an_image_that_migrates_and_never_gets_ready_is_rolled_back_with_the_backup() {
    let mut r = rig(Behaviour::MigratesThenUnready, 141);
    r.decide(automatic("0.5.4", Some(141)));
    let (o, _) = r.u.tick().await;
    assert_eq!(
        o,
        Outcome::RolledBack {
            to: Version::new(0, 5, 3),
            data_restored: true
        }
    );
    // 0.5.3 would refuse a schema-141 database (the downgrade guard): it runs,
    // so the backup is in place.
    assert_eq!(r.running_image(), "sha256:old");
    assert_eq!(r.db(), 140);
    // The migrated database was moved aside, never deleted.
    let data = r.world.lock().unwrap().data.clone();
    let aside: Vec<_> = std::fs::read_dir(&data)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("failed-0.5.4-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(
        std::fs::read_to_string(aside[0].path().join("state.db")).unwrap(),
        "141"
    );
    let recovered = r.hub.lock().unwrap().reports.last().cloned().unwrap();
    assert_eq!(recovered.phase, UpdatePhase::Recovered);
    assert_eq!(recovered.detail["data_restored"], json!(true));
    assert!(r.phases().contains(&"failed"));
}

#[tokio::test(start_paused = true)]
async fn when_the_previous_build_does_not_come_back_either_it_stops_for_an_operator() {
    let mut r = rig(Behaviour::CrashOnStart, 140);
    r.world.lock().unwrap().images[0].breaks_after_starts = Some(1);
    r.decide(automatic("0.5.4", Some(140)));
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::RollbackFailed(_)), "{o:?}");
    assert_eq!(r.u.state.phase, UpdatePhase::RollbackFailed);
    assert_eq!(r.phases().last(), Some(&"rollback_failed"));

    // It never loops: nothing happens until an operator clears it.
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Blocked);
    r.u.clear();
    assert_eq!(r.u.state.phase, UpdatePhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn notify_offers_once_and_installs_nothing() {
    let mut r = rig(Behaviour::Good, 141);
    r.decide(outcome(
        Status::UpdateAvailable,
        Mode::Notify,
        ReasonCode::NewerRecommended,
        "0.5.4",
        Some(141),
    ));
    assert_eq!(r.u.tick().await.0, Outcome::Nothing);
    assert_eq!(r.u.tick().await.0, Outcome::Nothing);
    assert_eq!(r.phases(), ["available"]);
    assert_eq!(r.running_image(), "sha256:old");

    // An operator's pin is the person saying so.
    r.decide(outcome(
        Status::UpdateAvailable,
        Mode::Notify,
        ReasonCode::Pinned,
        "0.5.4",
        Some(141),
    ));
    assert_eq!(r.u.tick().await.0, Outcome::Updated(Version::new(0, 5, 4)));
}

#[tokio::test(start_paused = true)]
async fn a_digest_the_image_does_not_carry_stops_before_anything_is_touched() {
    let mut r = rig(Behaviour::Good, 141);
    r.world.lock().unwrap().pull_wrong_digest = true;
    r.decide(automatic("0.5.4", Some(141)));
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::Failed(_)), "{o:?}");
    assert_eq!(r.running_image(), "sha256:old");
    assert_eq!(r.phases(), ["downloading", "verifying", "failed"]);
    assert_eq!(r.u.state.phase, UpdatePhase::Idle);
    assert!(r.u.state.previous.is_none());
    assert!(!r
        .world
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|c| c.starts_with("backup")));
}

#[tokio::test(start_paused = true)]
async fn reports_the_hub_misses_are_replayed_in_order() {
    let mut r = rig(Behaviour::Good, 141);
    r.decide(automatic("0.5.4", Some(141)));
    // The check gets through; the reports do not.
    r.hub.lock().unwrap().reports_down = true;
    assert_eq!(r.u.tick().await.0, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(r.u.state.pending_reports.len(), 6);
    assert!(r.phases().is_empty());
    // They survive a restart of the updater: they are in the state file.
    assert_eq!(
        State::load(&r.u.cfg.state_dir)
            .unwrap()
            .pending_reports
            .len(),
        6
    );

    r.hub.lock().unwrap().reports_down = false;
    r.u.flush_reports().await;
    assert!(r.u.state.pending_reports.is_empty());
    assert_eq!(
        r.phases(),
        [
            "downloading",
            "verifying",
            "ready",
            "installing",
            "validating",
            "success"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_validation_is_finished_after_a_restart() {
    let mut r = rig(Behaviour::Good, 141);
    r.decide(automatic("0.5.4", Some(141)));
    r.u.tick().await;
    // Pretend the updater died while validating the install it just made.
    let state_dir = r.u.cfg.state_dir.clone();
    let mut s = r.u.state.clone();
    s.phase = UpdatePhase::Validating;
    s.current = s.previous.as_ref().map(|p| Build {
        version: p.version.clone(),
        image: None,
        digest: None,
        image_id: Some(p.image_id.clone()),
        commit: None,
        build_id: None,
        schema_to: None,
    });
    s.save(&state_dir).unwrap();
    let cfg = r.u.cfg.clone();
    let mut u = Updater::new(
        FakeDocker(r.world.clone()),
        Box::new(FakeChannel(r.hub.clone())),
        cfg,
        State::load(&state_dir).unwrap(),
    );
    let (o, _) = u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(u.state.phase, UpdatePhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_hub_changes_nothing() {
    let mut r = rig(Behaviour::Good, 141);
    r.hub.lock().unwrap().down = true;
    let (o, wait) = r.u.tick().await;
    assert_eq!(o, Outcome::Nothing);
    assert!(wait <= Duration::from_secs(900));
    assert_eq!(r.running_image(), "sha256:old");
    assert!(r
        .u
        .state
        .last_error
        .as_deref()
        .unwrap()
        .contains("connection refused"));
}

#[test]
fn what_installs() {
    let d = |status, mode, code| outcome(status, mode, code, "0.5.4", None).decision;
    use ReasonCode::*;
    assert!(wants_install(&d(
        Status::UpdateRequired,
        Mode::Notify,
        BelowSignedMinimum
    )));
    assert!(wants_install(&d(Status::Rollback, Mode::Notify, Pinned)));
    assert!(wants_install(&d(
        Status::UpdateAvailable,
        Mode::Automatic,
        NewerRecommended
    )));
    assert!(wants_install(&d(
        Status::UpdateAvailable,
        Mode::Notify,
        Pinned
    )));
    assert!(!wants_install(&d(
        Status::UpdateAvailable,
        Mode::Notify,
        NewerRecommended
    )));
    assert!(!wants_install(&d(
        Status::Hold,
        Mode::Automatic,
        ManualMode
    )));
    assert!(!wants_install(&d(
        Status::UpToDate,
        Mode::Automatic,
        UpToDate
    )));
    assert!(!wants_install(&d(
        Status::ClientTooNew,
        Mode::Automatic,
        ClientAhead
    )));
}

#[test]
fn when_a_rollback_restores_the_database() {
    let prev = |schema| Previous {
        version: Version::new(0, 5, 3),
        image_id: "i".into(),
        spec: Value::Null,
        schema,
        backup: None,
    };
    let cand = |schema_to| Build {
        version: Version::new(0, 5, 4),
        image: None,
        digest: None,
        image_id: None,
        commit: None,
        build_id: None,
        schema_to,
    };
    // What the candidate reported wins over what its manifest promised.
    assert!(needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(140))),
        Some(141)
    ));
    assert!(!needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(141))),
        Some(140)
    ));
    // It never reported: its manifest decides.
    assert!(needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(141))),
        None
    ));
    assert!(!needs_restore(
        &prev(Some(140)),
        Some(&cand(Some(140))),
        None
    ));
    // Nothing known: restore (the backup is seconds old).
    assert!(needs_restore(&prev(Some(140)), Some(&cand(None)), None));
    assert!(needs_restore(
        &prev(None),
        Some(&cand(Some(140))),
        Some(140)
    ));
}

#[test]
fn restore_moves_the_database_aside_and_copies_the_backup_in() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("state.db"), "141").unwrap();
    std::fs::write(d.path().join("state.db-wal"), "wal").unwrap();
    std::fs::create_dir(d.path().join("backups")).unwrap();
    let b = d.path().join("backups/pre-0.5.4-1.db");
    std::fs::write(&b, "140").unwrap();
    restore_db(d.path(), &b, "0.5.4").unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join("state.db")).unwrap(),
        "140"
    );
    assert!(!d.path().join("state.db-wal").exists());
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "140",
        "the backup itself stays"
    );
    let aside = std::fs::read_dir(d.path())
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("failed-0.5.4-"))
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read_to_string(aside.join("state.db")).unwrap(),
        "141"
    );
    assert_eq!(
        std::fs::read_to_string(aside.join("state.db-wal")).unwrap(),
        "wal"
    );
}

#[test]
fn backups_are_pruned_to_the_newest() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..5 {
        let p = d.path().join(format!("pre-0.5.{i}-x.db"));
        std::fs::write(&p, "x").unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + i);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
    std::fs::write(d.path().join("manual.db"), "x").unwrap();
    prune_backups(d.path(), 3, &d.path().join("pre-0.5.4-x.db"));
    let mut left: Vec<String> = std::fs::read_dir(d.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "manual.db",
            "pre-0.5.2-x.db",
            "pre-0.5.3-x.db",
            "pre-0.5.4-x.db"
        ]
    );
}
