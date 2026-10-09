//! The update loop for the Docker hub (design §8.2–§8.4).
//!
//! One [`Updater::tick`]: finish whatever a crash interrupted, replay the
//! reports the hub did not take, ask the channel what the hub should run, and
//! when the answer is an install — a required update, the operator's pin or
//! rollback, or anything under `automatic` — carry it through:
//!
//! ```text
//! downloading  pull image@digest; RepoDigests must name it
//! verifying    (the signatures were checked by verify_target already)
//! ready        backup through `fleet-hub backup` in the running hub
//! installing   stop + remove the hub, create it again on the new image
//! validating   live, ready, identity, then the soak
//! success      or: failed → rolling_back → recovered | rollback_failed
//! ```
//!
//! Nothing here trusts the hub for content: the target arrives proven
//! against the release key (`verify_target`, inside the channel's `check`),
//! and the digest pulled is the one in that signed manifest.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fleet_update::manifest::Artifact;
use fleet_update::phase::BadEntry;
use fleet_update::wire::{Installed, Speaks};
use fleet_update::{
    CheckOutcome, CheckRequest, Component, Decision, Mode, Platform, ReasonCode, Report, Status,
    UpdateChannel, UpdatePhase, VerifiedTarget, Version, UPDATE_PROTO,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::docker::{self, Docker};
use crate::spec;
use crate::state::{Build, Previous, State};

macro_rules! log {
    ($($t:tt)*) => {
        eprintln!("{} fleet-updater: {}", fleet_update::time::format_rfc3339(crate::engine::now_unix()), format!($($t)*))
    };
}
pub(crate) use log;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The hub container's name (compose's `<project>-fleet-hub-1`).
    pub container: String,
    /// The hub's data volume as mounted here (backups and restores go
    /// through it).
    pub data_mount: PathBuf,
    pub state_dir: PathBuf,
    pub platform: Platform,
    pub ready_timeout: Duration,
    pub soak: Duration,
    pub poll: Duration,
    pub stop_timeout_secs: u64,
    pub keep_backups: usize,
    /// Between checks when the decision names no interval.
    pub interval: Duration,
}

impl Config {
    pub fn defaults(container: &str, data_mount: &Path, state_dir: &Path) -> Config {
        Config {
            container: container.to_string(),
            data_mount: data_mount.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            platform: Platform::new("linux", std::env::consts::ARCH, "oci"),
            ready_timeout: Duration::from_secs(90),
            soak: Duration::from_secs(120),
            poll: Duration::from_secs(3),
            stop_timeout_secs: 30,
            keep_backups: 3,
            interval: Duration::from_secs(21600),
        }
    }
}

/// The data dir the hub image uses when the container sets none.
pub const HUB_DATA_DIR: &str = "/var/lib/fleet-hub";

/// `fleet-hub healthcheck --ready --json` (crates/fleet-hub/src/ready.rs).
#[derive(Debug, Clone, Deserialize)]
pub struct ReadyReport {
    pub ready: bool,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub hub: Option<HubReady>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubReady {
    pub version: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub schema: Option<i64>,
    #[serde(default)]
    pub checks: HubChecks,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HubChecks {
    #[serde(default)]
    pub reconcile_failures: u32,
}

/// `fleet-hub backup --json` (fleet_core::store::backup::BackupInfo).
#[derive(Debug, Clone, Deserialize)]
struct BackupInfo {
    path: PathBuf,
    #[serde(default)]
    schema: Option<i64>,
}

/// Why a candidate (or the previous build, on the way back) is not healthy.
#[derive(Debug, Clone, PartialEq)]
pub struct GateFailure {
    pub why: String,
    /// The last store schema the build reported, if it ever did.
    pub schema: Option<i64>,
}

/// One probe's verdict.
enum Probe {
    Ready(ReadyReport),
    /// Not yet; keep trying until the deadline.
    NotYet(String, Option<i64>),
    /// It will not get better (a crash loop, the wrong build).
    Fatal(String, Option<i64>),
}

/// What a tick did, for `once` and the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to install (up to date, notify, hold, unknown).
    Nothing,
    Updated(Version),
    /// Rolled back to the previous build; `data_restored` says whether the
    /// database went back to the pre-update backup.
    RolledBack {
        to: Version,
        data_restored: bool,
    },
    /// Before anything was touched: the hub still runs what it ran.
    Failed(String),
    /// The previous build did not come back either: an operator's turn.
    RollbackFailed(String),
    /// Waiting for an operator to clear a rollback failure.
    Blocked,
}

pub struct Updater<D: Docker> {
    pub docker: D,
    pub channel: Box<dyn UpdateChannel>,
    pub cfg: Config,
    pub state: State,
}

/// Whether a decision asks this updater to install. `notify` offers and
/// waits for a person; a person saying so *is* a pin (`update_admin pin`),
/// which arrives here as `Pinned`.
pub fn wants_install(d: &Decision) -> bool {
    match d.status {
        Status::UpdateRequired | Status::Rollback => true,
        Status::UpdateAvailable => d.mode == Mode::Automatic || d.reason.code == ReasonCode::Pinned,
        _ => false,
    }
}

impl<D: Docker> Updater<D> {
    pub fn new(docker: D, channel: Box<dyn UpdateChannel>, cfg: Config, state: State) -> Self {
        Updater {
            docker,
            channel,
            cfg,
            state,
        }
    }

    fn save(&self) {
        if let Err(e) = self.state.save(&self.cfg.state_dir) {
            log!("cannot write the state file: {e}");
        }
    }

    fn set_phase(&mut self, to: UpdatePhase) {
        if self.state.phase != to && !self.state.phase.can_become(to) {
            log!(
                "phase {} → {} is not a legal step",
                self.state.phase.as_str(),
                to.as_str()
            );
            debug_assert!(false, "{:?} → {:?}", self.state.phase, to);
        }
        self.state.phase = to;
        self.save();
    }

    /// Record a transition, and tell the hub (now, or once it is back).
    async fn report(
        &mut self,
        phase: UpdatePhase,
        installed: &Build,
        detail: Value,
        error: Option<String>,
    ) {
        self.set_phase(phase);
        let r = Report {
            update_proto: UPDATE_PROTO,
            component: Component::Hub,
            installed: Installed {
                version: installed.version.clone(),
                commit: installed.commit.clone(),
                build_id: installed.build_id.clone(),
                digest: installed.digest.clone(),
            },
            phase,
            attempt: self.state.attempt.clone(),
            from: self
                .state
                .previous
                .as_ref()
                .map(|p| p.version.clone())
                .or_else(|| self.state.current.as_ref().map(|c| c.version.clone())),
            to: self.state.candidate.as_ref().map(|c| c.version.clone()),
            detail,
            error,
        };
        self.state.pending_reports.push(r);
        self.save();
        self.flush_reports().await;
    }

    /// Send the queued reports in order; stop at the first the hub does not take.
    pub async fn flush_reports(&mut self) {
        while let Some(r) = self.state.pending_reports.first().cloned() {
            match self.channel.report(&r).await {
                Ok(()) => {
                    self.state.pending_reports.remove(0);
                    self.save();
                }
                Err(e) => {
                    log!(
                        "the hub did not take the {} report ({e}); {} queued",
                        r.phase.as_str(),
                        self.state.pending_reports.len()
                    );
                    return;
                }
            }
        }
    }

    /// One pass. Returns what it did and how long to wait before the next.
    pub async fn tick(&mut self) -> (Outcome, Duration) {
        self.flush_reports().await;
        if let Some(o) = self.resume().await {
            return (o, self.cfg.poll);
        }
        if self.state.phase == UpdatePhase::RollbackFailed {
            return (Outcome::Blocked, self.cfg.interval);
        }

        let installed = match self.observe().await {
            Ok(b) => b,
            Err(e) => {
                log!("cannot see what the hub runs: {e}");
                self.state.last_error = Some(e);
                self.save();
                return (
                    Outcome::Nothing,
                    self.cfg.interval.min(Duration::from_secs(300)),
                );
            }
        };
        let req = CheckRequest {
            update_proto: UPDATE_PROTO,
            component: Component::Hub,
            platform: self.cfg.platform.clone(),
            installed: Installed {
                version: installed.version.clone(),
                commit: installed.commit.clone(),
                build_id: installed.build_id.clone(),
                digest: installed.digest.clone(),
            },
            speaks: Speaks::default(),
            phase: self.state.phase,
            attempt: None,
        };
        let CheckOutcome { decision, verified } = match self.channel.check(&req).await {
            Ok(o) => o,
            Err(e) => {
                // An unverifiable answer and an unreachable hub end the same
                // way: keep what runs.
                log!("check failed: {e}");
                self.state.last_error = Some(format!("check: {e}"));
                self.save();
                return (
                    Outcome::Nothing,
                    self.cfg.interval.min(Duration::from_secs(900)),
                );
            }
        };
        let wait =
            Duration::from_secs(decision.next_check_secs.clamp(60, 86_400)).min(self.cfg.interval);
        let Some(target) = verified.filter(|_| wants_install(&decision)) else {
            if decision.status == Status::UpdateAvailable {
                self.announce(&decision, &installed).await;
            }
            return (Outcome::Nothing, wait);
        };
        let outcome = self.apply(&decision, target, installed).await;
        (outcome, wait)
    }

    /// `notify`: say once that a version is available, and wait for a person.
    async fn announce(&mut self, d: &Decision, installed: &Build) {
        let Some(t) = &d.target else { return };
        if self.state.announced.as_ref() == Some(&t.version) {
            return;
        }
        log!(
            "{} is available ({}); pin it to install: update_admin pin hub {}",
            t.version,
            d.reason.text,
            t.version
        );
        self.state.announced = Some(t.version.clone());
        let r = Report {
            update_proto: UPDATE_PROTO,
            component: Component::Hub,
            installed: Installed::version(installed.version.clone()),
            phase: UpdatePhase::Available,
            attempt: None,
            from: Some(installed.version.clone()),
            to: Some(t.version.clone()),
            detail: json!({ "mode": d.mode }),
            error: None,
        };
        self.state.pending_reports.push(r);
        self.save();
        self.flush_reports().await;
    }

    /// What the hub runs now: its readiness when it answers, else the last
    /// build this updater put there.
    async fn observe(&mut self) -> Result<Build, String> {
        let c = self
            .docker
            .inspect_container(&self.cfg.container)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no container named {}", self.cfg.container))?;
        let image_id = docker::container_image_id(&c).map(String::from);
        let digest = match &image_id {
            Some(id) => self
                .docker
                .inspect_image(id)
                .await
                .ok()
                .flatten()
                .and_then(|i| docker::first_repo_digest(&i)),
            None => None,
        };
        let id = docker::container_id(&c)
            .unwrap_or(&self.cfg.container)
            .to_string();
        let ready = self.readiness(&id).await;
        let hub = ready.ok().and_then(|r| r.hub);
        let build = match (hub, &self.state.current) {
            (Some(h), _) => Build {
                version: Version::parse(&h.version)
                    .map_err(|e| format!("the hub reports version {:?}: {e}", h.version))?,
                image: None,
                digest,
                image_id,
                commit: known(&h.commit),
                build_id: known(&h.build_id),
                schema_to: h.schema,
            },
            (None, Some(cur)) if cur.image_id == image_id => cur.clone(),
            (None, _) => {
                return Err("the hub is not ready and this updater never installed it".into())
            }
        };
        if self
            .state
            .current
            .as_ref()
            .map(|c| (&c.version, &c.image_id))
            != Some((&build.version, &build.image_id))
        {
            self.state.current = Some(build.clone());
            self.save();
        }
        Ok(build)
    }

    async fn readiness(&self, container_id: &str) -> Result<ReadyReport, String> {
        let out = self
            .docker
            .exec(
                container_id,
                &["fleet-hub", "healthcheck", "--ready", "--json"].map(String::from),
            )
            .await
            .map_err(|e| e.to_string())?;
        // Not ready exits 1 and still prints the report.
        serde_json::from_str(out.stdout.trim()).map_err(|e| {
            format!(
                "healthcheck --ready --json (exit {}): {e}: {}",
                out.exit_code,
                out.stderr.trim()
            )
        })
    }

    // ── one install ──

    async fn apply(&mut self, d: &Decision, t: VerifiedTarget, installed: Build) -> Outcome {
        let Artifact::Oci { image, digest, .. } = &t.artifact else {
            return Outcome::Failed(format!("{} is not an image for this platform", t.version));
        };
        if self.state.bad.is_bad(&t.version, Some(digest)) {
            log!(
                "{} ({digest}) failed here before; not retrying it",
                t.version
            );
            return Outcome::Nothing;
        }
        log!(
            "{} → {} ({}: {})",
            installed.version,
            t.version,
            serde_json::to_value(d.status)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            d.reason.text
        );
        let manifest = &t.manifest;
        let candidate = Build {
            version: t.version.clone(),
            image: Some(image.clone()),
            digest: Some(digest.clone()),
            image_id: None,
            commit: known(&manifest.release.commit),
            build_id: known(&manifest.release.build_id),
            schema_to: manifest.compatibility.store.map(|s| s.schema_to),
        };
        self.state.attempt = Some(attempt_id());
        self.state.candidate = Some(candidate.clone());
        self.state.previous = None;
        self.state.restored = false;
        self.state.last_error = None;
        if self.state.phase != UpdatePhase::Idle {
            self.state.phase = UpdatePhase::Idle;
        }
        self.set_phase(UpdatePhase::Checking);
        self.set_phase(UpdatePhase::Available);
        self.report(
            UpdatePhase::Downloading,
            &installed,
            json!({ "image": image, "digest": digest }),
            None,
        )
        .await;

        match self.prepare(&candidate, &installed).await {
            Ok(()) => {}
            Err(e) => {
                log!("not updating: {e}");
                self.state.last_error = Some(e.clone());
                self.report(
                    UpdatePhase::Failed,
                    &installed,
                    Value::Null,
                    Some(e.clone()),
                )
                .await;
                self.set_phase(UpdatePhase::Idle);
                return Outcome::Failed(e);
            }
        }
        self.install_and_validate(installed).await
    }

    /// Pull, check the digest, back up. Nothing that runs is touched yet.
    async fn prepare(&mut self, cand: &Build, installed: &Build) -> Result<(), String> {
        let image = cand.image.clone().unwrap_or_default();
        let digest = cand.digest.clone().unwrap_or_default();
        self.docker
            .pull(&image, &digest)
            .await
            .map_err(|e| e.to_string())?;
        self.report(UpdatePhase::Verifying, installed, Value::Null, None)
            .await;
        let reference = format!("{image}@{digest}");
        let img = self
            .docker
            .inspect_image(&reference)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{reference} is not there after the pull"))?;
        if !docker::has_repo_digest(&img, &image, &digest) {
            return Err(format!("the pulled image does not carry {reference}"));
        }
        let image_id = docker::str_at(&img, &["Id"])
            .ok_or("the pulled image has no Id")?
            .to_string();
        if let Some(c) = self.state.candidate.as_mut() {
            c.image_id = Some(image_id);
        }
        self.save();
        self.report(UpdatePhase::Ready, installed, Value::Null, None)
            .await;

        // What runs now, and how to run it again.
        let c = self
            .docker
            .inspect_container(&self.cfg.container)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no container named {}", self.cfg.container))?;
        if !docker::is_running(&c) {
            return Err("the hub is not running, so it cannot take a backup".into());
        }
        let old_image_id = docker::container_image_id(&c)
            .ok_or("the hub container names no image")?
            .to_string();
        let old_image = self
            .docker
            .inspect_image(&old_image_id)
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or(Value::Null);
        let spec = spec::create_body(&c, &old_image, &old_image_id);
        let id = docker::container_id(&c)
            .unwrap_or(&self.cfg.container)
            .to_string();
        let hub_data = docker::container_env(&c, "FLEET_HUB_DATA_DIR")
            .unwrap_or(HUB_DATA_DIR)
            .to_string();
        let (backup, schema) = self
            .backup(&id, &cand.version, Path::new(&hub_data))
            .await?;
        self.state.previous = Some(Previous {
            version: installed.version.clone(),
            image_id: old_image_id,
            spec,
            schema,
            backup: Some(backup),
        });
        self.save();
        Ok(())
    }

    /// `fleet-hub backup` in the running hub; the copy must be visible here
    /// through the data mount, or a restore could not reach it.
    async fn backup(
        &mut self,
        container_id: &str,
        to: &Version,
        hub_data: &Path,
    ) -> Result<(PathBuf, Option<i64>), String> {
        let prefix = format!("pre-{}", safe(&to.to_string()));
        let out = self
            .docker
            .exec(
                container_id,
                &["fleet-hub", "backup", "--prefix", &prefix, "--json"].map(String::from),
            )
            .await
            .map_err(|e| e.to_string())?;
        if out.exit_code != 0 {
            return Err(format!(
                "fleet-hub backup failed (exit {}): {}",
                out.exit_code,
                out.stderr.trim()
            ));
        }
        let info: BackupInfo = serde_json::from_str(out.stdout.trim())
            .map_err(|e| format!("fleet-hub backup printed something else: {e}"))?;
        let rel = info.path.strip_prefix(hub_data).map_err(|_| {
            format!(
                "the backup {} is outside the hub's data dir {}",
                info.path.display(),
                hub_data.display()
            )
        })?;
        let here = self.cfg.data_mount.join(rel);
        if !here.is_file() {
            return Err(format!(
                "the backup is not visible at {} — mount the hub's data volume there",
                here.display()
            ));
        }
        prune_backups(
            &self.cfg.data_mount.join("backups"),
            self.cfg.keep_backups,
            &here,
        );
        log!(
            "backup {} (schema {})",
            here.display(),
            info.schema.map_or("none".into(), |s| s.to_string())
        );
        Ok((here, info.schema))
    }

    async fn install_and_validate(&mut self, installed: Build) -> Outcome {
        let cand = self
            .state
            .candidate
            .clone()
            .expect("prepare set the candidate");
        self.report(UpdatePhase::Installing, &installed, Value::Null, None)
            .await;
        if let Err(e) = self.swap(&cand).await {
            log!("install failed: {e}");
            return self.fail_and_roll_back(&cand, e, None).await;
        }
        self.report(UpdatePhase::Validating, &cand, Value::Null, None)
            .await;
        self.validate(cand).await
    }

    /// Stop and remove the hub, create it again on the candidate's image.
    async fn swap(&mut self, cand: &Build) -> Result<(), String> {
        let prev = self
            .state
            .previous
            .clone()
            .ok_or("no previous build recorded")?;
        let reference = format!(
            "{}@{}",
            cand.image.as_deref().unwrap_or_default(),
            cand.digest.as_deref().unwrap_or_default()
        );
        self.replace_container(&prev.spec, &reference).await
    }

    async fn replace_container(&mut self, spec: &Value, image: &str) -> Result<(), String> {
        let name = self.cfg.container.clone();
        if let Some(c) = self
            .docker
            .inspect_container(&name)
            .await
            .map_err(|e| e.to_string())?
        {
            let id = docker::container_id(&c).unwrap_or(&name).to_string();
            self.docker
                .stop(&id, self.cfg.stop_timeout_secs)
                .await
                .map_err(|e| e.to_string())?;
            self.docker.remove(&id).await.map_err(|e| e.to_string())?;
        }
        let mut body = spec.clone();
        body["Image"] = Value::String(image.to_string());
        let id = self
            .docker
            .create(&name, &body)
            .await
            .map_err(|e| e.to_string())?;
        self.docker.start(&id).await.map_err(|e| e.to_string())
    }

    async fn validate(&mut self, cand: Build) -> Outcome {
        match self.gates(&cand, self.cfg.soak).await {
            Ok(r) => {
                let digest = cand.digest.clone();
                let mut now = cand.clone();
                if let Some(h) = &r.hub {
                    now.schema_to = h.schema.or(now.schema_to);
                }
                self.report(
                    UpdatePhase::Success,
                    &now,
                    json!({ "health": "ready", "soak_secs": self.cfg.soak.as_secs(), "digest": digest }),
                    None,
                )
                .await;
                log!("now running {}", cand.version);
                self.state.current = Some(now);
                self.state.announced = None;
                self.set_phase(UpdatePhase::Idle);
                Outcome::Updated(cand.version)
            }
            Err(g) => self.fail_and_roll_back(&cand, g.why, g.schema).await,
        }
    }

    async fn fail_and_roll_back(
        &mut self,
        cand: &Build,
        why: String,
        schema: Option<i64>,
    ) -> Outcome {
        log!("{} failed: {why}", cand.version);
        self.state.bad.add(BadEntry {
            version: cand.version.clone(),
            digest: cand.digest.clone(),
            why: why.clone(),
            at: fleet_update::time::format_rfc3339(now_unix()),
        });
        self.state.last_error = Some(why.clone());
        self.state.candidate_schema = schema;
        self.report(UpdatePhase::Failed, cand, Value::Null, Some(why.clone()))
            .await;
        self.roll_back().await
    }

    /// Bring the previous build back (design §8.3), restoring the database
    /// when the candidate may have migrated it.
    async fn roll_back(&mut self) -> Outcome {
        let Some(prev) = self.state.previous.clone() else {
            let why = "no previous build recorded to roll back to".to_string();
            return self.rollback_failed(why).await;
        };
        let cand = self.state.candidate.clone();
        let prev_build = Build {
            version: prev.version.clone(),
            image: None,
            digest: None,
            image_id: Some(prev.image_id.clone()),
            commit: None,
            build_id: None,
            schema_to: prev.schema,
        };
        if self.state.phase != UpdatePhase::RollingBack {
            self.report(UpdatePhase::RollingBack, &prev_build, Value::Null, None)
                .await;
        }

        // Keep the failed candidate's last words, then clear it away.
        let name = self.cfg.container.clone();
        if let Ok(Some(c)) = self.docker.inspect_container(&name).await {
            if docker::container_image_id(&c) != Some(prev.image_id.as_str()) {
                let id = docker::container_id(&c).unwrap_or(&name).to_string();
                if let Ok(logs) = self.docker.logs(&id, 500).await {
                    let v = cand
                        .as_ref()
                        .map(|c| c.version.to_string())
                        .unwrap_or_default();
                    let p = self
                        .cfg
                        .state_dir
                        .join(format!("failed-{}-{}.log", safe(&v), stamp()));
                    if std::fs::write(&p, logs).is_ok() {
                        log!("the failed hub's log is in {}", p.display());
                    }
                }
                let _ = self.docker.stop(&id, self.cfg.stop_timeout_secs).await;
            }
        }

        let restore = !self.state.restored
            && needs_restore(&prev, cand.as_ref(), self.state.candidate_schema);
        if restore {
            let backup = prev.backup.clone();
            match backup {
                Some(b) => {
                    // The candidate must be stopped before its database moves.
                    if let Ok(Some(c)) = self.docker.inspect_container(&name).await {
                        let id = docker::container_id(&c).unwrap_or(&name).to_string();
                        let _ = self.docker.stop(&id, self.cfg.stop_timeout_secs).await;
                    }
                    let v = cand
                        .as_ref()
                        .map(|c| c.version.to_string())
                        .unwrap_or_default();
                    if let Err(e) = restore_db(&self.cfg.data_mount, &b, &v) {
                        return self
                            .rollback_failed(format!("restoring {}: {e}", b.display()))
                            .await;
                    }
                    self.state.restored = true;
                    self.save();
                    log!("database restored from {}", b.display());
                }
                None => {
                    return self
                        .rollback_failed(
                            "the candidate migrated the database and there is no backup".into(),
                        )
                        .await
                }
            }
        }

        if let Err(e) = self.replace_container(&prev.spec, &prev.image_id).await {
            return self
                .rollback_failed(format!("starting {} again: {e}", prev.version))
                .await;
        }
        match self.gates(&prev_build, Duration::ZERO).await {
            Ok(_) => {
                let restored = self.state.restored;
                self.report(
                    UpdatePhase::Recovered,
                    &prev_build,
                    json!({
                        "data_restored": restored,
                        "lost": if restored { "writes the candidate made while it was being validated" } else { "nothing" },
                        "backup": prev.backup,
                    }),
                    None,
                )
                .await;
                log!(
                    "back on {}{}",
                    prev.version,
                    if restored { " (database restored)" } else { "" }
                );
                self.state.current = Some(prev_build);
                self.state.restored = false;
                self.state.candidate_schema = None;
                self.set_phase(UpdatePhase::Idle);
                Outcome::RolledBack {
                    to: prev.version,
                    data_restored: restored,
                }
            }
            Err(g) => {
                self.rollback_failed(format!("{} did not come back: {}", prev.version, g.why))
                    .await
            }
        }
    }

    async fn rollback_failed(&mut self, why: String) -> Outcome {
        log!("ROLLBACK FAILED: {why}. Leaving everything in place for an operator; `fleet-updater clear` once it is sorted.");
        let prev = self.state.previous.clone();
        let installed = Build {
            version: prev
                .as_ref()
                .map(|p| p.version.clone())
                .or_else(|| self.state.current.as_ref().map(|c| c.version.clone()))
                .unwrap_or_else(|| Version::new(0, 0, 0)),
            image: None,
            digest: None,
            image_id: None,
            commit: None,
            build_id: None,
            schema_to: None,
        };
        if self.state.phase != UpdatePhase::RollingBack {
            self.set_phase(UpdatePhase::RollingBack);
        }
        self.state.last_error = Some(why.clone());
        // Stop whatever half-runs, so nothing writes to a database in an
        // unknown state; the containers and files stay for the operator.
        if let Ok(Some(c)) = self.docker.inspect_container(&self.cfg.container).await {
            let id = docker::container_id(&c)
                .unwrap_or(&self.cfg.container)
                .to_string();
            let _ = self.docker.stop(&id, self.cfg.stop_timeout_secs).await;
        }
        self.report(
            UpdatePhase::RollbackFailed,
            &installed,
            Value::Null,
            Some(why.clone()),
        )
        .await;
        Outcome::RollbackFailed(why)
    }

    /// The health gates (design §8.4): live, ready and the right build within
    /// `ready_timeout`, then still so for `soak`.
    async fn gates(&self, want: &Build, soak: Duration) -> Result<ReadyReport, GateFailure> {
        let deadline = Instant::now() + self.cfg.ready_timeout;
        let mut schema = None;
        let ready = loop {
            match self.probe(want).await {
                Probe::Ready(r) => break r,
                Probe::Fatal(why, s) => {
                    return Err(GateFailure {
                        why,
                        schema: s.or(schema),
                    })
                }
                Probe::NotYet(why, s) => {
                    schema = s.or(schema);
                    if Instant::now() >= deadline {
                        return Err(GateFailure {
                            why: format!(
                                "not ready within {} s: {why}",
                                self.cfg.ready_timeout.as_secs()
                            ),
                            schema,
                        });
                    }
                }
            }
            tokio::time::sleep(self.cfg.poll).await;
        };
        let schema = ready.hub.as_ref().and_then(|h| h.schema).or(schema);
        let baseline = ready
            .hub
            .as_ref()
            .map(|h| h.checks.reconcile_failures)
            .unwrap_or(0);
        let end = Instant::now() + soak;
        while Instant::now() < end {
            tokio::time::sleep(self.cfg.poll.min(end - Instant::now())).await;
            match self.probe(want).await {
                Probe::Ready(r) => {
                    let f = r
                        .hub
                        .as_ref()
                        .map(|h| h.checks.reconcile_failures)
                        .unwrap_or(0);
                    if f > baseline + 1 {
                        return Err(GateFailure {
                            why: format!(
                                "reconcile failures rose from {baseline} to {f} during the soak"
                            ),
                            schema,
                        });
                    }
                }
                Probe::NotYet(why, _) | Probe::Fatal(why, _) => {
                    return Err(GateFailure {
                        why: format!("stopped being healthy during the soak: {why}"),
                        schema,
                    })
                }
            }
        }
        Ok(ready)
    }

    async fn probe(&self, want: &Build) -> Probe {
        let c = match self.docker.inspect_container(&self.cfg.container).await {
            Ok(Some(c)) => c,
            Ok(None) => return Probe::Fatal("the container is gone".into(), None),
            Err(e) => return Probe::NotYet(e.to_string(), None),
        };
        if let Some(id) = &want.image_id {
            if docker::container_image_id(&c) != Some(id.as_str()) {
                return Probe::Fatal(
                    format!(
                        "the container runs {}, not {id}",
                        docker::container_image_id(&c).unwrap_or("nothing")
                    ),
                    None,
                );
            }
        }
        if docker::restart_count(&c) > 0 {
            return Probe::Fatal(
                format!("it restarted {} time(s)", docker::restart_count(&c)),
                None,
            );
        }
        if !docker::is_running(&c) {
            let code = c
                .pointer("/State/ExitCode")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            return Probe::Fatal(format!("it exited (code {code})"), None);
        }
        let id = docker::container_id(&c)
            .unwrap_or(&self.cfg.container)
            .to_string();
        let r = match self.readiness(&id).await {
            Ok(r) => r,
            Err(e) => return Probe::NotYet(e, None),
        };
        let schema = r.hub.as_ref().and_then(|h| h.schema);
        if let Some(h) = &r.hub {
            if Version::parse(&h.version).ok().as_ref() != Some(&want.version) {
                return Probe::Fatal(
                    format!(
                        "it reports version {}, expected {}",
                        h.version, want.version
                    ),
                    schema,
                );
            }
            if let (Some(w), Some(got)) = (&want.commit, known(&h.commit)) {
                if !got.eq_ignore_ascii_case(w) {
                    return Probe::Fatal(
                        format!("it reports commit {got}, the manifest says {w}"),
                        schema,
                    );
                }
            }
            if let (Some(w), Some(got)) = (&want.build_id, known(&h.build_id)) {
                if &got != w {
                    return Probe::Fatal(
                        format!("it reports build {got}, the manifest says {w}"),
                        schema,
                    );
                }
            }
        }
        if !(r.ready && r.live && r.fresh) {
            return Probe::NotYet(
                r.error.clone().unwrap_or_else(|| "not ready yet".into()),
                schema,
            );
        }
        match docker::health(&c) {
            None | Some("healthy") => Probe::Ready(r),
            Some(h) => Probe::NotYet(format!("container health is {h}"), schema),
        }
    }

    /// Finish what a crash (of this process, or the host) left half done.
    async fn resume(&mut self) -> Option<Outcome> {
        use UpdatePhase::*;
        match self.state.phase {
            Idle | RollbackFailed => None,
            Checking | Available | Success | Recovered | Unknown => {
                self.state.phase = Idle;
                self.save();
                None
            }
            // Nothing that runs was touched yet.
            Downloading | Verifying | Ready => {
                log!("an interrupted attempt is abandoned; the hub was not touched");
                let installed = self.state.current.clone()?;
                self.report(
                    Failed,
                    &installed,
                    Value::Null,
                    Some("the updater was interrupted".into()),
                )
                .await;
                self.set_phase(Idle);
                Some(Outcome::Failed("interrupted".into()))
            }
            Installing | Validating => {
                let cand = self.state.candidate.clone()?;
                log!(
                    "resuming the validation of {} after an interruption",
                    cand.version
                );
                if self.state.phase == Installing {
                    let running_cand = matches!(
                        self.docker.inspect_container(&self.cfg.container).await,
                        Ok(Some(c)) if docker::container_image_id(&c) == cand.image_id.as_deref()
                    );
                    if !running_cand {
                        return Some(
                            self.fail_and_roll_back(
                                &cand,
                                "the install was interrupted".into(),
                                None,
                            )
                            .await,
                        );
                    }
                    self.report(Validating, &cand, Value::Null, None).await;
                }
                Some(self.validate(cand).await)
            }
            Failed | RollingBack => {
                if self.state.phase == Failed {
                    self.set_phase(RollingBack);
                }
                Some(self.roll_back().await)
            }
        }
    }

    /// `fleet-updater clear`: an operator sorted a rollback failure out.
    pub fn clear(&mut self) {
        self.state.phase = UpdatePhase::Idle;
        self.state.bad.clear();
        self.state.candidate = None;
        self.state.restored = false;
        self.state.candidate_schema = None;
        self.state.last_error = None;
        self.save();
    }
}

/// Whether going back needs the pre-update backup: the candidate's database
/// is newer than what the previous build opens. Unknown counts as yes —
/// restoring a backup taken seconds before costs only the candidate's own
/// writes, and opening a migrated database with the old build fails anyway.
pub fn needs_restore(prev: &Previous, cand: Option<&Build>, cand_schema: Option<i64>) -> bool {
    let Some(before) = prev.schema else {
        return true;
    };
    match cand_schema.or_else(|| cand.and_then(|c| c.schema_to)) {
        Some(after) => after > before,
        None => true,
    }
}

/// Move `state.db*` aside into `failed-<v>-<stamp>/` (never deleted) and put
/// a copy of `backup` in their place, owned like the database it replaces.
pub fn restore_db(data: &Path, backup: &Path, failed_version: &str) -> Result<(), String> {
    let db = data.join("state.db");
    let owner = std::fs::metadata(&db).ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        (m.uid(), m.gid())
    });
    let aside = data.join(format!("failed-{}-{}", safe(failed_version), stamp()));
    std::fs::create_dir_all(&aside).map_err(|e| format!("{}: {e}", aside.display()))?;
    for name in ["state.db", "state.db-wal", "state.db-shm"] {
        let p = data.join(name);
        if p.exists() {
            std::fs::rename(&p, aside.join(name)).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    let tmp = data.join("state.db.restore");
    std::fs::copy(backup, &tmp).map_err(|e| format!("{}: {e}", backup.display()))?;
    if let Some((uid, gid)) = owner {
        // Only root may give a file away; an updater running as the hub's
        // own user already owns it.
        let _ = std::os::unix::fs::chown(&tmp, Some(uid), Some(gid));
    }
    std::fs::File::open(&tmp)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &db).map_err(|e| format!("{}: {e}", db.display()))?;
    Ok(())
}

/// Keep the newest `keep` `pre-*.db` backups (and always `just_made`).
fn prune_backups(dir: &Path, keep: usize, just_made: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pre-") && n.ends_with(".db"))
        })
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, p) in files.into_iter().skip(keep.max(1)) {
        if p != just_made {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// A value the build left unknown is no value.
fn known(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s != "unknown" && s != "local").then(|| s.to_string())
}

/// `[A-Za-z0-9._-]` only (a semver's `+build` included).
fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn stamp() -> String {
    fleet_update::time::format_rfc3339(now_unix()).replace([':', '-'], "")
}

fn attempt_id() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:012x}{:08x}",
        t.as_millis(),
        t.subsec_nanos() ^ std::process::id()
    )
}

#[cfg(test)]
mod tests;
