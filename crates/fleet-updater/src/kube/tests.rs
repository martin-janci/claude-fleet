//! The update loop against a simulated Kubernetes API: a StatefulSet
//! controller that rolls `fleet-hub-0` when the template changes, a kubelet
//! that starts it (pulling, migrating the database, crashing, never getting
//! ready), and `pods/exec` into the hub. The same acceptance as the Docker
//! loop's (`engine::tests`): a good image, one that crashes, one that
//! migrates and never gets ready (restored from the backup), a digest the
//! pod does not run, a pull that never succeeds, and a previous build that
//! does not come back.
//!
//! The simulated database is `state.db` holding its schema number, as in
//! `engine::tests`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use fleet_update::manifest::Artifact;
use fleet_update::{CheckOutcome, Platform, UpdatePhase, Version};
use serde_json::{json, Value};

use super::*;
use crate::engine::tests::{automatic, FakeChannel, Hub};
use crate::engine::{Config, Outcome, Updater};
use crate::state::State;

const REPO: &str = "ghcr.io/test/fleet-hub";
const NS: &str = "papayapos-fleet";

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Behaviour {
    Good,
    CrashOnStart,
    MigratesThenUnready,
    /// The kubelet never gets the image.
    PullFails,
}

#[derive(Clone, Debug)]
struct Img {
    digest: String,
    tag: String,
    version: String,
    schema: i64,
    behaviour: Behaviour,
    breaks_after_starts: Option<u32>,
    starts: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum PodState {
    Creating,
    PullBackOff,
    Running { ready: bool },
    Crashing,
}

#[derive(Clone, Debug)]
struct Pod {
    revision: String,
    containers: Vec<(String, String)>,
    inits: Vec<(String, String)>,
    state: PodState,
    /// What the kubelet reports in the hub container's imageID.
    digest: Option<String>,
    restarts: i64,
    deleting: bool,
    version: String,
}

struct Cluster {
    images: Vec<Img>,
    data: PathBuf,
    generation: i64,
    observed: i64,
    replicas: i64,
    strategy: &'static str,
    containers: Vec<(String, String)>,
    inits: Vec<(String, String)>,
    /// Template image sets, by revision index.
    revisions: Vec<String>,
    current_revision: String,
    /// What `status.currentRevision` says instead (a rollout someone else began).
    force_current: Option<String>,
    pod: Option<Pod>,
    calls: Vec<String>,
    /// The kubelet reports a digest other than the one asked for (a
    /// mutating webhook rewriting images, say).
    rewrite_digest: bool,
}

impl Cluster {
    fn db_schema(&self) -> i64 {
        std::fs::read_to_string(self.data.join("state.db"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn revision(&mut self) -> String {
        let key = serde_json::to_string(&(&self.containers, &self.inits)).unwrap();
        let i = match self.revisions.iter().position(|r| *r == key) {
            Some(i) => i,
            None => {
                self.revisions.push(key);
                self.revisions.len() - 1
            }
        };
        format!("fleet-hub-rev{i}")
    }

    fn resolve(&self, image: &str) -> Option<usize> {
        if let Some((repo, d)) = image.split_once('@') {
            return (repo == REPO)
                .then(|| self.images.iter().position(|i| i.digest == d))
                .flatten();
        }
        let (repo, tag) = image.rsplit_once(':')?;
        (repo == REPO)
            .then(|| self.images.iter().position(|i| i.tag == tag))
            .flatten()
    }

    /// The controller and the kubelet, one step.
    fn tick(&mut self) {
        self.observed = self.generation;
        let update = self.revision();
        let replicas = self.replicas;
        let strategy = self.strategy;
        match self.pod.as_mut() {
            Some(p) if p.deleting => self.pod = None,
            Some(p) if replicas == 0 => p.deleting = true,
            Some(p) if p.revision != update && strategy == "RollingUpdate" => p.deleting = true,
            Some(_) => self.advance(),
            None if replicas >= 1 => {
                self.pod = Some(Pod {
                    revision: update.clone(),
                    containers: self.containers.clone(),
                    inits: self.inits.clone(),
                    state: PodState::Creating,
                    digest: None,
                    restarts: 0,
                    deleting: false,
                    version: String::new(),
                });
            }
            None => {}
        }
        // As the controller does: only a rolling update ever completes.
        if let Some(p) = &self.pod {
            if strategy == "RollingUpdate"
                && p.revision == update
                && matches!(p.state, PodState::Running { ready: true })
            {
                self.current_revision = update;
            }
        }
    }

    /// Start a pod that is being created: pull, guard, migrate.
    fn advance(&mut self) {
        let p = self.pod.as_ref().unwrap();
        if p.state != PodState::Creating {
            return;
        }
        let hub_image = p.containers[0].1.clone();
        let db = self.db_schema();
        let rewrite = self.rewrite_digest;
        let Some(i) = self.resolve(&hub_image) else {
            self.pod.as_mut().unwrap().state = PodState::PullBackOff;
            return;
        };
        let img = &mut self.images[i];
        if img.behaviour == Behaviour::PullFails {
            self.pod.as_mut().unwrap().state = PodState::PullBackOff;
            return;
        }
        img.starts += 1;
        let broken = img.breaks_after_starts.is_some_and(|n| img.starts > n);
        let behaviour = if broken {
            Behaviour::CrashOnStart
        } else {
            img.behaviour
        };
        let (schema, version) = (img.schema, img.version.clone());
        let running_digest = if rewrite && hub_image.contains('@') {
            digest('f')
        } else {
            img.digest.clone()
        };
        let data = self.data.clone();
        let p = self.pod.as_mut().unwrap();
        p.digest = Some(running_digest);
        p.version = version;
        if db > schema || behaviour == Behaviour::CrashOnStart {
            p.state = PodState::Crashing;
            p.restarts = 3;
            return;
        }
        if schema > db {
            std::fs::write(data.join("state.db"), schema.to_string()).unwrap();
        }
        p.state = PodState::Running {
            ready: behaviour == Behaviour::Good,
        };
    }

    fn sts_json(&self) -> Value {
        let list = |v: &[(String, String)], env: bool| -> Vec<Value> {
            v.iter()
                .map(|(n, i)| {
                    let mut c = json!({ "name": n, "image": i });
                    if env && n == "fleet-hub" {
                        c["env"] = json!([{ "name": "FLEET_HUB_BIND", "value": "0.0.0.0" }]);
                    }
                    c
                })
                .collect()
        };
        let mut s = json!({
            "metadata": { "name": "fleet-hub", "namespace": NS, "generation": self.generation },
            "spec": {
                "replicas": self.replicas,
                "updateStrategy": { "type": self.strategy },
                "template": { "spec": {
                    "containers": list(&self.containers, true),
                    "initContainers": list(&self.inits, false),
                } }
            },
            "status": {
                "observedGeneration": self.observed,
                "currentRevision": self.force_current.clone().unwrap_or_else(|| self.current_revision.clone()),
            }
        });
        let key = serde_json::to_string(&(&self.containers, &self.inits)).unwrap();
        if let Some(i) = self.revisions.iter().position(|r| *r == key) {
            s["status"]["updateRevision"] = json!(format!("fleet-hub-rev{i}"));
        }
        s
    }

    fn pod_json(&self) -> Option<Value> {
        let p = self.pod.as_ref()?;
        let image_id = |img: &str| -> String {
            match self.resolve(img) {
                Some(i) => format!("{REPO}@{}", self.images[i].digest),
                None => format!("docker.io/library/{}@{}", img, digest('c')),
            }
        };
        let started = !matches!(p.state, PodState::Creating | PodState::PullBackOff);
        let hub_id = match (&p.digest, started) {
            (Some(d), true) => format!("{REPO}@{d}"),
            _ => String::new(),
        };
        let hub_state = match p.state {
            PodState::Creating => json!({ "waiting": { "reason": "PodInitializing" } }),
            PodState::PullBackOff => json!({ "waiting": {
                "reason": "ImagePullBackOff",
                "message": "Back-off pulling image"
            } }),
            PodState::Running { .. } => {
                json!({ "running": { "startedAt": "2026-10-15T00:00:00Z" } })
            }
            PodState::Crashing => {
                json!({ "waiting": { "reason": "CrashLoopBackOff", "message": "back-off 40s" } })
            }
        };
        let ready = matches!(p.state, PodState::Running { ready: true });
        let mut statuses = vec![json!({
            "name": "fleet-hub", "image": p.containers[0].1, "imageID": hub_id,
            "restartCount": p.restarts, "ready": ready, "state": hub_state,
        })];
        for (n, i) in p.containers.iter().skip(1) {
            statuses.push(json!({
                "name": n, "image": i, "imageID": if started { image_id(i) } else { String::new() },
                "restartCount": 0, "ready": started, "state": { "running": {} },
            }));
        }
        let inits: Vec<Value> = p
            .inits
            .iter()
            .map(|(n, i)| {
                let id = if started {
                    // The init container ran the same image as the hub.
                    if i == &p.containers[0].1 {
                        hub_id.clone()
                    } else {
                        image_id(i)
                    }
                } else {
                    String::new()
                };
                json!({
                    "name": n, "image": i, "imageID": id, "restartCount": 0,
                    "state": if started { json!({ "terminated": { "exitCode": 0, "reason": "Completed" } }) }
                             else { json!({ "running": {} }) },
                })
            })
            .collect();
        let mut v = json!({
            "metadata": { "name": "fleet-hub-0", "namespace": NS,
                          "labels": { "controller-revision-hash": p.revision } },
            "spec": {
                "containers": p.containers.iter().map(|(n, i)| json!({ "name": n, "image": i })).collect::<Vec<_>>(),
                "initContainers": p.inits.iter().map(|(n, i)| json!({ "name": n, "image": i })).collect::<Vec<_>>(),
            },
            "status": {
                "phase": if started { "Running" } else { "Pending" },
                "containerStatuses": statuses,
                "initContainerStatuses": inits,
            }
        });
        if p.deleting {
            v["metadata"]["deletionTimestamp"] = json!("2026-10-15T00:00:00Z");
        }
        Some(v)
    }

    fn patch(&mut self, body: &Value) {
        let mut changed = false;
        if let Some(r) = body.pointer("/spec/replicas").and_then(Value::as_i64) {
            changed |= r != self.replicas;
            self.replicas = r;
        }
        let mut images = vec![];
        for (key, list) in [
            ("containers", &mut self.containers),
            ("initContainers", &mut self.inits),
        ] {
            for c in body
                .pointer(&format!("/spec/template/spec/{key}"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = c["name"].as_str().unwrap();
                let image = c["image"].as_str().unwrap();
                let slot = list
                    .iter_mut()
                    .find(|(n, _)| n == name)
                    .unwrap_or_else(|| panic!("the patch adds a container {name}"));
                changed |= slot.1 != image;
                slot.1 = image.to_string();
                images.push(format!("{name}={image}"));
            }
        }
        if changed {
            self.generation += 1;
        }
        self.calls.push(format!(
            "patch replicas={} {}",
            self.replicas,
            images.join(" ")
        ));
    }
}

#[derive(Clone)]
struct FakeKube(Arc<Mutex<Cluster>>);

const STS: &str = "/apis/apps/v1/namespaces/papayapos-fleet/statefulsets/fleet-hub";
const POD: &str = "/api/v1/namespaces/papayapos-fleet/pods/fleet-hub-0";

#[async_trait]
impl KubeApi for FakeKube {
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &Value)>,
    ) -> Result<Option<Value>, String> {
        let mut c = self.0.lock().unwrap();
        match (method, path) {
            ("GET", STS) => {
                c.tick();
                Ok(Some(c.sts_json()))
            }
            ("GET", POD) => {
                c.tick();
                Ok(c.pod_json())
            }
            ("PATCH", STS) => {
                let (ct, b) = body.expect("a patch has a body");
                assert_eq!(ct, STRATEGIC);
                c.patch(b);
                Ok(Some(c.sts_json()))
            }
            ("DELETE", POD) => {
                c.calls.push("delete pod".into());
                Ok(c.pod.as_mut().map(|p| {
                    p.deleting = true;
                    json!({})
                }))
            }
            _ => Err(format!("the fake API has no {method} {path}")),
        }
    }

    async fn text(&self, path: &str) -> Result<String, String> {
        assert!(path.starts_with(&format!("{POD}/log?container=fleet-hub&tailLines=500")));
        Ok("panic: the candidate fell over\n".into())
    }

    async fn exec(&self, path: &str) -> Result<ExecOutput, String> {
        let (base, query) = path.split_once('?').unwrap();
        assert_eq!(base, format!("{POD}/exec"));
        assert!(query.starts_with("container=fleet-hub&stdout=true&stderr=true"));
        let cmd: Vec<&str> = query
            .split('&')
            .filter_map(|kv| kv.strip_prefix("command="))
            .collect();
        let mut c = self.0.lock().unwrap();
        let p = c.pod.clone().ok_or("pods \"fleet-hub-0\" not found")?;
        let PodState::Running { ready } = p.state else {
            return Err("exec refused: HTTP 400: container fleet-hub is not running".into());
        };
        let out = |code: i64, stdout: String| ExecOutput {
            exit_code: code,
            stdout,
            stderr: String::new(),
        };
        match cmd.get(1).copied() {
            Some("healthcheck") => {
                let report = json!({
                    "ready": ready, "live": true, "fresh": true,
                    "hub": {"ready": ready, "pid": 7, "version": p.version, "commit": format!("c{}", p.version),
                            "build_id": "test", "contract": 1, "agent_proto": [1, 1], "peer_proto": 1,
                            "schema": c.db_schema(), "started_at": 0, "heartbeat_at": 0,
                            "checks": {"store": "ok", "listener": "ok",
                                       "first_reconcile": if ready { "ok" } else { "failed" }, "reconcile_failures": 0}},
                    "error": if ready { Value::Null } else { json!("first reconcile failed") }
                });
                Ok(out(if ready { 0 } else { 1 }, report.to_string()))
            }
            Some("backup") => {
                let prefix = cmd[3];
                let name = format!("{prefix}-{}.db", c.calls.len());
                let dir = c.data.join("backups");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::copy(c.data.join("state.db"), dir.join(&name)).unwrap();
                c.calls.push(format!("backup {name}"));
                Ok(out(
                    0,
                    json!({"path": format!("/var/lib/fleet-hub/backups/{name}"), "schema": c.db_schema(), "bytes": 3})
                        .to_string(),
                ))
            }
            _ => Ok(out(127, String::new())),
        }
    }
}

struct Rig {
    cluster: Arc<Mutex<Cluster>>,
    hub: Arc<Mutex<Hub>>,
    u: Updater<KubeRuntime<FakeKube>>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// 0.5.3 runs on schema 140 from the tag `:0.5.3`; `next` is 0.5.4.
fn rig(next: Behaviour, next_schema: i64) -> Rig {
    let data = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("state.db"), "140").unwrap();
    let images = vec![
        Img {
            digest: digest('a'),
            tag: "0.5.3".into(),
            version: "0.5.3".into(),
            schema: 140,
            behaviour: Behaviour::Good,
            breaks_after_starts: None,
            starts: 0,
        },
        Img {
            digest: digest('b'),
            tag: "0.5.4".into(),
            version: "0.5.4".into(),
            schema: next_schema,
            behaviour: next,
            breaks_after_starts: None,
            starts: 0,
        },
    ];
    let mut c = Cluster {
        images,
        data: data.path().to_path_buf(),
        generation: 1,
        observed: 0,
        replicas: 1,
        strategy: "RollingUpdate",
        containers: vec![
            ("fleet-hub".into(), format!("{REPO}:0.5.3")),
            ("log-shipper".into(), "busybox:1.36".into()),
        ],
        inits: vec![("init".into(), format!("{REPO}:0.5.3"))],
        revisions: vec![],
        current_revision: String::new(),
        force_current: None,
        pod: None,
        calls: vec![],
        rewrite_digest: false,
    };
    // Bring 0.5.3 up.
    for _ in 0..3 {
        c.tick();
    }
    assert!(matches!(
        c.pod.as_ref().unwrap().state,
        PodState::Running { ready: true }
    ));
    let cluster = Arc::new(Mutex::new(c));
    let hub = Arc::new(Mutex::new(Hub {
        outcome: None,
        reports: vec![],
        down: false,
        reports_down: false,
    }));
    let mut cfg = Config::defaults(data.path(), state.path());
    cfg.platform = Platform::new("linux", "x86_64", "oci");
    cfg.poll = Duration::from_secs(1);
    let mut k = KubeConfig::new(NS, "fleet-hub");
    k.poll = Duration::from_secs(1);
    let rt = KubeRuntime::new(FakeKube(cluster.clone()), k).unwrap();
    let u = Updater::new(
        rt,
        Box::new(FakeChannel(hub.clone())),
        cfg,
        State::load(state.path()).unwrap(),
    );
    Rig {
        cluster,
        hub,
        u,
        _dirs: (data, state),
    }
}

/// testkit's manifests carry `sha256:hub-<version>`; a real one a hex digest.
fn with_digest(mut o: CheckOutcome, d: &str) -> CheckOutcome {
    let set = |a: &mut Artifact| {
        if let Artifact::Oci { digest, .. } = a {
            *digest = d.to_string();
        }
    };
    if let Some(t) = o.decision.target.as_mut() {
        set(&mut t.artifact);
    }
    if let Some(v) = o.verified.as_mut() {
        set(&mut v.artifact);
    }
    o
}

fn update_to_054(schema_to: i64) -> CheckOutcome {
    with_digest(automatic("0.5.4", Some(schema_to)), &digest('b'))
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

    /// The digest the hub pod runs; it must be running and ready.
    fn running(&self) -> String {
        let c = self.cluster.lock().unwrap();
        let p = c.pod.as_ref().expect("no hub pod");
        assert!(
            matches!(p.state, PodState::Running { ready: true }),
            "the hub pod is {:?}",
            p.state
        );
        p.digest.clone().unwrap()
    }

    fn template(&self) -> (Images, Images) {
        let c = self.cluster.lock().unwrap();
        (c.containers.clone(), c.inits.clone())
    }

    fn db(&self) -> i64 {
        self.cluster.lock().unwrap().db_schema()
    }

    fn calls(&self) -> Vec<String> {
        self.cluster.lock().unwrap().calls.clone()
    }
}

fn pair(n: &str, i: &str) -> (String, String) {
    (n.to_string(), i.to_string())
}

const FULL_RUN: [&str; 6] = [
    "downloading",
    "verifying",
    "ready",
    "installing",
    "validating",
    "success",
];

#[tokio::test(start_paused = true)]
async fn a_good_image_is_patched_in_by_digest_rolled_out_and_kept() {
    let mut r = rig(Behaviour::Good, 141);
    r.decide(update_to_054(141));
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(r.phases(), FULL_RUN);
    assert_eq!(r.running(), digest('b'));
    assert_eq!(r.db(), 141);
    // The hub container and the init container on the hub's repository
    // moved to the signed digest; the sidecar on another image did not.
    let pinned = format!("{REPO}@{}", digest('b'));
    assert_eq!(
        r.template(),
        (
            vec![
                pair("fleet-hub", &pinned),
                pair("log-shipper", "busybox:1.36")
            ],
            vec![pair("init", &pinned)]
        )
    );
    let s = &r.u.state;
    assert_eq!(s.phase, UpdatePhase::Idle);
    assert_eq!(s.current.as_ref().unwrap().version, Version::new(0, 5, 4));
    let prev = s.previous.as_ref().unwrap();
    assert_eq!(prev.version, Version::new(0, 5, 3));
    assert_eq!(prev.image_id, digest('a'));
    assert_eq!(
        prev.spec["containers"]["fleet-hub"],
        json!(format!("{REPO}:0.5.3"))
    );
    assert_eq!(
        prev.spec["initContainers"]["init"],
        json!(format!("{REPO}:0.5.3"))
    );
    assert!(prev.spec["containers"].get("log-shipper").is_none());
    assert_eq!(prev.spec["replicas"], json!(1));
    assert_eq!(prev.spec["revision"], json!("fleet-hub-rev0"));
    let backup = prev.backup.clone().unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "140");
    // The backup came before the patch; the patch named the digest.
    let calls = r.calls();
    let b = calls.iter().position(|c| c.starts_with("backup ")).unwrap();
    let p = calls.iter().position(|c| c.starts_with("patch ")).unwrap();
    assert!(b < p, "{calls:?}");
    assert_eq!(
        calls[p],
        format!("patch replicas=1 fleet-hub={pinned} init={pinned}")
    );
    assert_eq!(calls.iter().filter(|c| c.starts_with("patch ")).count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_crashing_image_is_rolled_back_to_the_old_template_without_touching_the_data() {
    let mut r = rig(Behaviour::CrashOnStart, 140);
    r.decide(update_to_054(140));
    let (o, _) = r.u.tick().await;
    assert_eq!(
        o,
        Outcome::RolledBack {
            to: Version::new(0, 5, 3),
            data_restored: false
        }
    );
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
    assert_eq!(r.running(), digest('a'));
    assert_eq!(r.db(), 140);
    // Exactly what it ran before: the tag, not a digest.
    let tag = format!("{REPO}:0.5.3");
    assert_eq!(
        r.template(),
        (
            vec![pair("fleet-hub", &tag), pair("log-shipper", "busybox:1.36")],
            vec![pair("init", &tag)]
        )
    );
    assert_eq!(r.cluster.lock().unwrap().replicas, 1);
    // Scaled to zero before the old images went back.
    let calls = r.calls();
    let zero = calls.iter().position(|c| c == "patch replicas=0 ").unwrap();
    let back = calls
        .iter()
        .position(|c| c == &format!("patch replicas=1 fleet-hub={tag} init={tag}"))
        .unwrap();
    assert!(zero < back, "{calls:?}");
    assert!(r
        .u
        .state
        .bad
        .is_bad(&Version::new(0, 5, 4), Some(&digest('b'))));
    let logs: Vec<_> = std::fs::read_dir(&r.u.cfg.state_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("failed-0.5.4-"))
        .collect();
    assert_eq!(logs.len(), 1);
    let log = std::fs::read_to_string(logs[0].path()).unwrap();
    assert!(
        log.contains("the previous container") && log.contains("fell over"),
        "{log}"
    );

    // Not retried by itself.
    assert_eq!(r.u.tick().await.0, Outcome::Nothing);
}

#[tokio::test(start_paused = true)]
async fn an_image_that_migrates_and_never_gets_ready_is_rolled_back_with_the_backup() {
    let mut r = rig(Behaviour::MigratesThenUnready, 141);
    r.decide(update_to_054(141));
    let (o, _) = r.u.tick().await;
    assert_eq!(
        o,
        Outcome::RolledBack {
            to: Version::new(0, 5, 3),
            data_restored: true
        }
    );
    // 0.5.3 refuses a schema-141 database: it runs, so the backup is in place.
    assert_eq!(r.running(), digest('a'));
    assert_eq!(r.db(), 140);
    let data = r.cluster.lock().unwrap().data.clone();
    let aside: Vec<_> = std::fs::read_dir(&data)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("failed-0.5.4-"))
        .collect();
    assert_eq!(aside.len(), 1, "the migrated database is moved aside");
    assert_eq!(
        std::fs::read_to_string(aside[0].path().join("state.db")).unwrap(),
        "141"
    );
    let recovered = r.hub.lock().unwrap().reports.last().cloned().unwrap();
    assert_eq!(recovered.phase, UpdatePhase::Recovered);
    assert_eq!(recovered.detail["data_restored"], json!(true));
}

#[tokio::test(start_paused = true)]
async fn a_pod_running_another_digest_fails_the_gate() {
    let mut r = rig(Behaviour::Good, 140);
    r.cluster.lock().unwrap().rewrite_digest = true;
    r.decide(update_to_054(140));
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::RolledBack { .. }), "{o:?}");
    let failed = r
        .hub
        .lock()
        .unwrap()
        .reports
        .iter()
        .find(|r| r.phase == UpdatePhase::Failed)
        .cloned()
        .unwrap();
    let why = failed.error.unwrap();
    assert!(
        why.contains(&digest('f')) && why.contains(&digest('b')),
        "{why}"
    );
    assert_eq!(r.running(), digest('a'));
}

#[tokio::test(start_paused = true)]
async fn an_image_the_kubelet_never_pulls_is_rolled_back_after_the_ready_timeout() {
    let mut r = rig(Behaviour::PullFails, 141);
    r.decide(update_to_054(141));
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::RolledBack { .. }), "{o:?}");
    let why = r.u.state.last_error.clone().unwrap();
    assert!(why.contains("ImagePullBackOff"), "{why}");
    assert_eq!(r.running(), digest('a'));
    assert_eq!(r.db(), 140);
}

#[tokio::test(start_paused = true)]
async fn on_delete_statefulsets_get_their_pod_deleted() {
    let mut r = rig(Behaviour::Good, 141);
    r.cluster.lock().unwrap().strategy = "OnDelete";
    r.decide(update_to_054(141));
    let (o, _) = r.u.tick().await;
    assert_eq!(o, Outcome::Updated(Version::new(0, 5, 4)));
    assert_eq!(r.running(), digest('b'));
    assert_eq!(r.calls().iter().filter(|c| *c == "delete pod").count(), 1);
    // currentRevision never moves under OnDelete; the next update is not
    // mistaken for one mid-rollout.
    let snap = r.u.runtime.snapshot().await.unwrap();
    assert_eq!(snap.image_id, digest('b'));
    assert_eq!(snap.spec["strategy"], json!("OnDelete"));
}

#[tokio::test(start_paused = true)]
async fn when_the_previous_build_does_not_come_back_it_stays_scaled_down_for_an_operator() {
    let mut r = rig(Behaviour::CrashOnStart, 140);
    r.cluster.lock().unwrap().images[0].breaks_after_starts = Some(1);
    r.decide(update_to_054(140));
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::RollbackFailed(_)), "{o:?}");
    assert_eq!(r.u.state.phase, UpdatePhase::RollbackFailed);
    assert_eq!(r.phases().last(), Some(&"rollback_failed"));
    assert_eq!(r.cluster.lock().unwrap().replicas, 0);
    // It never loops.
    assert_eq!(r.u.tick().await.0, Outcome::Blocked);
    assert_eq!(r.cluster.lock().unwrap().replicas, 0);
}

#[tokio::test(start_paused = true)]
async fn a_statefulset_mid_rollout_is_not_touched() {
    let mut r = rig(Behaviour::Good, 141);
    r.cluster.lock().unwrap().force_current = Some("fleet-hub-somethingelse".into());
    r.decide(update_to_054(141));
    let (o, _) = r.u.tick().await;
    match o {
        Outcome::Failed(why) => assert!(why.contains("mid-rollout"), "{why}"),
        o => panic!("{o:?}"),
    }
    assert_eq!(r.phases(), ["downloading", "verifying", "ready", "failed"]);
    assert!(r.calls().is_empty(), "no backup, no patch: {:?}", r.calls());
    assert!(r.u.state.previous.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_digest_that_is_not_one_stops_before_anything_is_touched() {
    let mut r = rig(Behaviour::Good, 141);
    // testkit's own `sha256:hub-0.5.4`.
    r.decide(automatic("0.5.4", Some(141)));
    let (o, _) = r.u.tick().await;
    assert!(matches!(o, Outcome::Failed(_)), "{o:?}");
    assert_eq!(r.phases(), ["downloading", "verifying", "failed"]);
    assert!(r.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_validation_is_finished_after_a_restart() {
    let mut r = rig(Behaviour::Good, 141);
    r.decide(update_to_054(141));
    r.u.tick().await;
    let state_dir = r.u.cfg.state_dir.clone();
    let mut s = r.u.state.clone();
    s.phase = UpdatePhase::Validating;
    s.save(&state_dir).unwrap();
    let mut k = KubeConfig::new(NS, "fleet-hub");
    k.poll = Duration::from_secs(1);
    let mut u = Updater::new(
        KubeRuntime::new(FakeKube(r.cluster.clone()), k).unwrap(),
        Box::new(FakeChannel(r.hub.clone())),
        r.u.cfg.clone(),
        State::load(&state_dir).unwrap(),
    );
    assert_eq!(u.tick().await.0, Outcome::Updated(Version::new(0, 5, 4)));
}

// ── the pieces ──

#[test]
fn repositories_and_digests() {
    assert_eq!(
        repo_of("ghcr.io/x/fleet-hub:0.6.3-dev.419"),
        "ghcr.io/x/fleet-hub"
    );
    assert_eq!(
        repo_of("ghcr.io/x/fleet-hub@sha256:ab"),
        "ghcr.io/x/fleet-hub"
    );
    assert_eq!(
        repo_of("ghcr.io/x/fleet-hub:1@sha256:ab"),
        "ghcr.io/x/fleet-hub"
    );
    assert_eq!(
        repo_of("registry:5000/fleet-hub"),
        "registry:5000/fleet-hub"
    );
    assert_eq!(
        repo_of("registry:5000/fleet-hub:1"),
        "registry:5000/fleet-hub"
    );
    assert_eq!(repo_of("busybox"), "busybox");
    assert_eq!(
        digest_of("ghcr.io/x/fleet-hub@sha256:AB").as_deref(),
        Some("sha256:ab")
    );
    assert_eq!(
        digest_of("docker-pullable://ghcr.io/x/hub@sha256:ab").as_deref(),
        Some("sha256:ab")
    );
    assert_eq!(digest_of(""), None);
}

#[test]
fn which_containers_switch() {
    let hub = format!("{REPO}:0.5.3");
    let main = vec![pair("fleet-hub", &hub), pair("log-shipper", "busybox:1.36")];
    let init = vec![pair("init", &hub), pair("chown", "busybox:1.36")];
    let (m, i) = select(&main, &init, &hub, "fleet-hub", None).unwrap();
    assert_eq!(m, vec![pair("fleet-hub", &hub)]);
    assert_eq!(i, vec![pair("init", &hub)]);

    let named = vec!["fleet-hub".to_string()];
    let (m, i) = select(&main, &init, &hub, "fleet-hub", Some(&named)).unwrap();
    assert_eq!((m.len(), i.len()), (1, 0));

    let typo = vec!["fleet-hub".to_string(), "inti".to_string()];
    assert!(select(&main, &init, &hub, "fleet-hub", Some(&typo))
        .unwrap_err()
        .contains("inti"));
    let without_hub = vec!["init".to_string()];
    assert!(select(&main, &init, &hub, "fleet-hub", Some(&without_hub)).is_err());
}

#[test]
fn what_a_pod_says() {
    let pod = |hub_state: Value, inits: Value| {
        json!({
            "metadata": { "name": "fleet-hub-0" },
            "spec": { "containers": [{ "name": "fleet-hub", "image": "r:1" }],
                      "initContainers": [{ "name": "init", "image": "r:1" }] },
            "status": { "phase": "Running", "initContainerStatuses": inits,
                        "containerStatuses": [{ "name": "fleet-hub", "imageID": "r@sha256:aa",
                                                "restartCount": 0, "ready": true, "state": hub_state }] }
        })
    };
    let done = json!([{ "name": "init", "imageID": "r@sha256:aa", "restartCount": 0,
                        "state": { "terminated": { "exitCode": 0 } } }]);
    let s = seen_of(&pod(json!({ "running": {} }), done.clone()), "fleet-hub");
    assert_eq!(s.run, Run::Running);
    assert_eq!(s.image_id.as_deref(), Some("sha256:aa"));
    assert_eq!(s.health.as_deref(), Some("healthy"));

    let s = seen_of(
        &pod(
            json!({ "waiting": { "reason": "ContainerCreating" } }),
            done.clone(),
        ),
        "fleet-hub",
    );
    assert!(matches!(s.run, Run::Starting(_)));
    let s = seen_of(
        &pod(
            json!({ "waiting": { "reason": "CreateContainerConfigError", "message": "secret x not found" } }),
            done.clone(),
        ),
        "fleet-hub",
    );
    assert_eq!(
        s.run,
        Run::Down("CreateContainerConfigError secret x not found".into())
    );
    let s = seen_of(
        &pod(
            json!({ "terminated": { "exitCode": 2, "reason": "Error" } }),
            done,
        ),
        "fleet-hub",
    );
    assert_eq!(s.run, Run::Down("it exited (code 2): Error".into()));

    // An init container that fails, and one that ran another digest.
    let failed = json!([{ "name": "init", "restartCount": 2,
                          "state": { "terminated": { "exitCode": 1 } } }]);
    let s = seen_of(
        &pod(
            json!({ "waiting": { "reason": "PodInitializing" } }),
            failed,
        ),
        "fleet-hub",
    );
    assert_eq!(s.restarts, 2);
    assert_eq!(
        s.run,
        Run::Down("init container init exited (code 1)".into())
    );
    let other = json!([{ "name": "init", "imageID": "r@sha256:bb", "restartCount": 0,
                         "state": { "terminated": { "exitCode": 0 } } }]);
    let s = seen_of(&pod(json!({ "running": {} }), other), "fleet-hub");
    assert!(
        matches!(&s.run, Run::Down(w) if w.contains("sha256:bb")),
        "{:?}",
        s.run
    );
}

#[test]
fn names_are_checked_before_they_reach_a_path() {
    assert!(KubeConfig::new("papayapos-fleet", "fleet-hub")
        .validate()
        .is_ok());
    assert!(KubeConfig::new("../x", "fleet-hub").validate().is_err());
    assert!(KubeConfig::new("ns", "Fleet").validate().is_err());
    let mut k = KubeConfig::new("ns", "fleet-hub");
    k.containers = Some(vec!["a?b".into()]);
    assert!(k.validate().is_err());
}

/// The real client against a socket: the token, the patch type, a 404, a
/// refusal's message, and exec through the upgrade.
#[tokio::test]
async fn the_api_client_speaks_http_and_the_exec_websocket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let mut seen = vec![];
        for _ in 0..4 {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                req.push(b[0]);
            }
            let head = String::from_utf8_lossy(&req).into_owned();
            if let Some(len) = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .and_then(|l| l.trim().parse::<usize>().ok())
            {
                let mut body = vec![0u8; len];
                s.read_exact(&mut body).await.unwrap();
                req.extend_from_slice(&body);
            }
            let answer: Vec<u8> = if head.starts_with("GET /missing") {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
            } else if head.starts_with("PATCH") {
                let body = br#"{"metadata":{"generation":7}}"#;
                let mut a = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                    .into_bytes();
                a.extend_from_slice(body);
                a
            } else if head.contains("/exec?") {
                let mut a = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Protocol: v4.channel.k8s.io\r\n\r\n".to_vec();
                a.extend_from_slice(&[0x82, 4, 1, b'o', b'k', b'\n']);
                let st = br#"{"status":"Success"}"#;
                a.extend_from_slice(&[0x82, (st.len() + 1) as u8, 3]);
                a.extend_from_slice(st);
                a
            } else {
                let body = br#"{"kind":"Status","message":"statefulsets.apps is forbidden"}"#;
                let mut a = format!(
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                a.extend_from_slice(body);
                a
            };
            s.write_all(&answer).await.unwrap();
            s.shutdown().await.unwrap();
            seen.push(String::from_utf8_lossy(&req).into_owned());
        }
        seen
    });
    let api = ApiServer::at(&format!("http://{addr}"), "sa-token");
    assert_eq!(api.call("GET", "/missing", None).await.unwrap(), None);
    let patched = api
        .call(
            "PATCH",
            "/apis/apps/v1/x",
            Some((STRATEGIC, &json!({"spec": {"replicas": 0}}))),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(patched["metadata"]["generation"], json!(7));
    let e = api.call("GET", "/apis/apps/v1/y", None).await.unwrap_err();
    assert!(e.contains("HTTP 403") && e.contains("forbidden"), "{e}");
    let out = api
        .exec("/api/v1/namespaces/n/pods/p-0/exec?container=c&stdout=true&command=true")
        .await
        .unwrap();
    assert_eq!((out.exit_code, out.stdout.as_str()), (0, "ok\n"));

    let seen = srv.await.unwrap();
    assert!(seen
        .iter()
        .all(|r| r.contains("\r\nAuthorization: Bearer sa-token\r\n")));
    assert!(seen[1].contains("\r\nContent-Type: application/strategic-merge-patch+json\r\n"));
    assert!(seen[1].ends_with(r#"{"spec":{"replicas":0}}"#));
}
