//! The binary target (update-channel design §8.6, slice S9): `fleet-agent`
//! on a host the hub cannot reach, and a hub run from `fleet-hub.service`
//! without Docker. The same state machine as the container
//! (`fleet-updater`'s engine), on a different layout:
//!
//! ```text
//! <root>/<version>/<bin>      one directory per installed release
//! <root>/current -> <version> the release the unit runs
//! <link> -> <root>/current/<bin>   the path in the unit's ExecStart
//! ```
//!
//! One [`BinaryUpdater::tick`]:
//!
//! ```text
//! downloading  the tarball: the hub's mirror first, then its release URL
//! verifying    sha256 and size against the signed manifest; unpack;
//!              the new binary's --version must be the target's
//! ready        (hub) `fleet-hub backup` while the old build still runs
//! installing   `current` → the new release; restart the unit
//! validating   active, the right build, ready (the agent: connected to
//!              its hub), then still so through the soak
//! success      or: failed → rolling_back (`current` back; the hub's
//!              database restored when the candidate may have migrated
//!              it) → recovered | rollback_failed
//! ```
//!
//! It runs outside the unit it restarts (a systemd timer's oneshot), so a
//! restart does not take it down mid-install.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use fleet_update::manifest::Artifact;
use fleet_update::phase::{BadEntry, BadList};
use fleet_update::verify::sha256_hex;
use fleet_update::wire::{Installed, Speaks};
use fleet_update::{
    CheckOutcome, CheckRequest, Component, Decision, Platform, Report, Status, UpdateChannel,
    UpdatePhase, VerifiedTarget, Version, UPDATE_PROTO,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::common::{attempt_id, now_unix, prune_backups, restore_db, safe, wants_install};

/// The state file's name in [`BinaryConfig::state_dir`].
pub const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone)]
pub struct BinaryConfig {
    /// `agent` or `hub`.
    pub component: Component,
    /// The binary's file name inside a release tarball: `fleet-agent`.
    pub bin: String,
    /// Where the versioned directories and `current` live.
    pub root: PathBuf,
    /// The path the unit runs; made a symlink to `<root>/current/<bin>`.
    pub link: PathBuf,
    pub state_dir: PathBuf,
    pub platform: Platform,
    pub speaks: Speaks,
    /// The hub's data dir, for a restore; `None` for the agent.
    pub data_dir: Option<PathBuf>,
    pub ready_timeout: Duration,
    pub soak: Duration,
    pub poll: Duration,
    /// Between checks when the decision names no interval.
    pub interval: Duration,
    /// Installed releases kept besides the current and the previous one.
    pub keep_versions: usize,
    pub keep_backups: usize,
    /// Prefix of every log line (`fleet-agent update`).
    pub name: String,
}

impl BinaryConfig {
    pub fn new(
        component: Component,
        bin: &str,
        root: &Path,
        link: &Path,
        state_dir: &Path,
    ) -> Self {
        BinaryConfig {
            component,
            bin: bin.to_string(),
            root: root.to_path_buf(),
            link: link.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            platform: Platform::new("linux", std::env::consts::ARCH, "tarball"),
            speaks: Speaks::default(),
            data_dir: None,
            ready_timeout: Duration::from_secs(90),
            soak: Duration::from_secs(120),
            poll: Duration::from_secs(3),
            interval: Duration::from_secs(21_600),
            keep_versions: 2,
            keep_backups: 3,
            name: format!("{bin} update"),
        }
    }
}

/// What the unit says about the build it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Health {
    Up {
        /// The running build's version, when it can be read.
        version: Option<Version>,
        /// Ready to serve: the hub's `--ready` report; the agent connected.
        ready: bool,
        /// The hub's store schema, when it says.
        schema: Option<i64>,
        /// systemd's automatic restarts of the unit so far.
        restarts: u32,
        /// Why it is not ready, when it is not.
        why: Option<String>,
    },
    /// Not running (failed, activating, gone).
    Down(String),
}

/// Everything the loop does to the machine, so the tests can run it against
/// a fake one. [`SystemdHost`] is the real one.
#[async_trait]
pub trait Host: Send + Sync {
    /// A release file from its release URL.
    async fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String>;
    /// The same file from the hub's mirror (`/update/artifact/<sha256>`).
    async fn fetch_mirror(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, String>;
    /// Unpack `archive` (`.tar.gz`) into the empty directory `into`.
    fn unpack(&self, archive: &Path, into: &Path) -> Result<(), String>;
    /// What `<bin> --version` says.
    fn version_of(&self, bin: &Path) -> Result<Version, String>;
    fn restart(&self) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    async fn health(&self) -> Health;
    /// The hub: `fleet-hub backup --prefix <prefix> --json` in the running
    /// build, as `(path, schema)`. The agent has nothing to back up.
    fn backup(&self, prefix: &str) -> Result<Option<(PathBuf, Option<i64>)>, String>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub version: Version,
    pub sha256: String,
    /// The newest store schema it migrates to, from the manifest.
    #[serde(default)]
    pub schema_to: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Before {
    pub version: Version,
    /// The store schema when the backup was taken (the hub).
    #[serde(default)]
    pub schema: Option<i64>,
    #[serde(default)]
    pub backup: Option<PathBuf>,
}

/// The state file: written atomically, so a crash leaves the old one or the
/// new one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BinState {
    #[serde(default)]
    pub phase: UpdatePhase,
    #[serde(default)]
    pub attempt: Option<String>,
    #[serde(default)]
    pub current: Option<Version>,
    #[serde(default)]
    pub candidate: Option<Candidate>,
    #[serde(default)]
    pub previous: Option<Before>,
    #[serde(default)]
    pub bad: BadList,
    #[serde(default)]
    pub announced: Option<Version>,
    #[serde(default)]
    pub pending_reports: Vec<Report>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub candidate_schema: Option<i64>,
    #[serde(default)]
    pub restored: bool,
}

impl BinState {
    pub fn load(dir: &Path) -> Result<BinState, String> {
        let p = dir.join(STATE_FILE);
        match std::fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BinState::default()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        use std::io::Write;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = dir.join(STATE_FILE);
        let tmp = dir.join(format!("{STATE_FILE}.tmp"));
        let body = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(&body)
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// What a tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Updated(Version),
    RolledBack {
        to: Version,
        data_restored: bool,
    },
    /// Before anything that runs was touched.
    Failed(String),
    RollbackFailed(String),
    Blocked,
}

pub struct BinaryUpdater<H: Host> {
    pub host: H,
    pub channel: Box<dyn UpdateChannel>,
    pub cfg: BinaryConfig,
    pub state: BinState,
}

impl<H: Host> BinaryUpdater<H> {
    pub fn new(
        host: H,
        channel: Box<dyn UpdateChannel>,
        cfg: BinaryConfig,
        state: BinState,
    ) -> Self {
        BinaryUpdater {
            host,
            channel,
            cfg,
            state,
        }
    }

    fn log(&self, msg: &str) {
        eprintln!(
            "{} {}: {msg}",
            fleet_update::time::format_rfc3339(now_unix()),
            self.cfg.name
        );
    }

    fn save(&self) {
        if let Err(e) = self.state.save(&self.cfg.state_dir) {
            self.log(&format!("cannot write the state file: {e}"));
        }
    }

    fn set_phase(&mut self, to: UpdatePhase) {
        if self.state.phase != to && !self.state.phase.can_become(to) {
            self.log(&format!(
                "phase {} → {} is not a legal step",
                self.state.phase.as_str(),
                to.as_str()
            ));
        }
        self.state.phase = to;
        self.save();
    }

    async fn report(
        &mut self,
        phase: UpdatePhase,
        installed: &Version,
        detail: Value,
        error: Option<String>,
    ) {
        self.set_phase(phase);
        let r = Report {
            update_proto: UPDATE_PROTO,
            component: self.cfg.component,
            installed: Installed::version(installed.clone()),
            phase,
            attempt: self.state.attempt.clone(),
            from: self
                .state
                .previous
                .as_ref()
                .map(|p| p.version.clone())
                .or_else(|| self.state.current.clone()),
            to: self.state.candidate.as_ref().map(|c| c.version.clone()),
            detail,
            error,
        };
        self.state.pending_reports.push(r);
        self.save();
        self.flush_reports().await;
    }

    /// Send the queued reports in order; stop at the first the hub refuses.
    pub async fn flush_reports(&mut self) {
        while let Some(r) = self.state.pending_reports.first().cloned() {
            match self.channel.report(&r).await {
                Ok(()) => {
                    self.state.pending_reports.remove(0);
                    self.save();
                }
                Err(e) => {
                    self.log(&format!(
                        "the hub did not take the {} report ({e}); {} queued",
                        r.phase.as_str(),
                        self.state.pending_reports.len()
                    ));
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
            Ok(v) => v,
            Err(e) => {
                self.log(&format!("cannot see what runs: {e}"));
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
            component: self.cfg.component,
            platform: self.cfg.platform.clone(),
            installed: Installed::version(installed.clone()),
            speaks: self.cfg.speaks.clone(),
            phase: self.state.phase,
            attempt: None,
        };
        let CheckOutcome { decision, verified } = match self.channel.check(&req).await {
            Ok(o) => o,
            Err(e) => {
                self.log(&format!("check failed: {e}"));
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
        let mirror = decision.target.as_ref().and_then(|t| t.mirror.clone());
        let outcome = self.apply(&decision, target, mirror, installed).await;
        (outcome, wait)
    }

    async fn announce(&mut self, d: &Decision, installed: &Version) {
        let Some(t) = &d.target else { return };
        if self.state.announced.as_ref() == Some(&t.version) {
            return;
        }
        self.log(&format!(
            "{} is available ({}); pin it to install: update_admin pin {} {}",
            t.version,
            d.reason.text,
            self.cfg.component.as_str(),
            t.version
        ));
        self.state.announced = Some(t.version.clone());
        self.state.pending_reports.push(Report {
            update_proto: UPDATE_PROTO,
            component: self.cfg.component,
            installed: Installed::version(installed.clone()),
            phase: UpdatePhase::Available,
            attempt: None,
            from: Some(installed.clone()),
            to: Some(t.version.clone()),
            detail: json!({ "mode": d.mode }),
            error: None,
        });
        self.save();
        self.flush_reports().await;
    }

    /// The running build's version: what the unit reports, else what the
    /// link points at.
    async fn observe(&mut self) -> Result<Version, String> {
        let v = match self.host.health().await {
            Health::Up {
                version: Some(v), ..
            } => v,
            _ => self.host.version_of(&self.cfg.link)?,
        };
        if self.state.current.as_ref() != Some(&v) {
            self.state.current = Some(v.clone());
            self.save();
        }
        Ok(v)
    }

    fn version_dir(&self, v: &Version) -> PathBuf {
        self.cfg.root.join(safe(&v.to_string()))
    }

    // ── one install ──

    async fn apply(
        &mut self,
        d: &Decision,
        t: VerifiedTarget,
        mirror: Option<String>,
        installed: Version,
    ) -> Outcome {
        let Artifact::Tarball { sha256, size, .. } = &t.artifact else {
            return Outcome::Failed(format!("{} is not a tarball for this platform", t.version));
        };
        if self.state.bad.is_bad(&t.version, Some(sha256)) {
            self.log(&format!(
                "{} failed here before; not retrying it",
                t.version
            ));
            return Outcome::Nothing;
        }
        self.log(&format!("{installed} → {} ({})", t.version, d.reason.text));
        self.state.attempt = Some(attempt_id());
        self.state.candidate = Some(Candidate {
            version: t.version.clone(),
            sha256: sha256.clone(),
            schema_to: t.manifest.compatibility.store.map(|s| s.schema_to),
        });
        self.state.previous = None;
        self.state.restored = false;
        self.state.last_error = None;
        self.state.candidate_schema = None;
        if self.state.phase != UpdatePhase::Idle {
            self.state.phase = UpdatePhase::Idle;
        }
        self.set_phase(UpdatePhase::Checking);
        self.set_phase(UpdatePhase::Available);
        self.report(
            UpdatePhase::Downloading,
            &installed,
            json!({ "sha256": sha256, "mirror": mirror.is_some() }),
            None,
        )
        .await;
        match self
            .prepare(&t, sha256, *size, mirror.as_deref(), &installed)
            .await
        {
            Ok(()) => {}
            Err(e) => {
                self.log(&format!("not updating: {e}"));
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

    /// Download, check, unpack, back up. Nothing that runs is touched yet.
    async fn prepare(
        &mut self,
        t: &VerifiedTarget,
        sha256: &str,
        size: u64,
        mirror: Option<&str>,
        installed: &Version,
    ) -> Result<(), String> {
        let bytes = self.download(t, sha256, size, mirror).await?;
        self.report(UpdatePhase::Verifying, installed, Value::Null, None)
            .await;
        let staging = self.cfg.root.join(".staging");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        let archive = staging.join(format!("{sha256}.tar.gz"));
        std::fs::write(&archive, &bytes).map_err(|e| format!("{}: {e}", archive.display()))?;
        let unpacked = staging.join("unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| format!("{}: {e}", unpacked.display()))?;
        self.host.unpack(&archive, &unpacked)?;
        let found = find_binary(&unpacked, &self.cfg.bin)
            .ok_or_else(|| format!("no {} inside the release tarball", self.cfg.bin))?;
        let got = self.host.version_of(&found)?;
        if got != t.version {
            return Err(format!(
                "the unpacked {} says it is {got}, not {}",
                self.cfg.bin, t.version
            ));
        }
        let dir = self.version_dir(&t.version);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let bin = dir.join(&self.cfg.bin);
        std::fs::rename(&found, &bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        set_executable(&bin)?;
        let _ = std::fs::remove_dir_all(&staging);
        self.report(UpdatePhase::Ready, installed, Value::Null, None)
            .await;

        // What runs now must be reachable through `current` to come back.
        self.adopt(installed)?;
        let prefix = format!("pre-{}", safe(&t.version.to_string()));
        let (backup, schema) = match self.host.backup(&prefix)? {
            Some((path, schema)) => {
                if let Some(dir) = path.parent() {
                    prune_backups(dir, self.cfg.keep_backups, &path);
                }
                self.log(&format!(
                    "backup {} (schema {})",
                    path.display(),
                    schema.map_or("none".into(), |s| s.to_string())
                ));
                (Some(path), schema)
            }
            None => (None, None),
        };
        self.state.previous = Some(Before {
            version: installed.clone(),
            schema,
            backup,
        });
        self.save();
        Ok(())
    }

    /// The tarball: the hub's mirror first when the decision names one, then
    /// the release URL. Either way it must be the bytes the manifest signed.
    async fn download(
        &self,
        t: &VerifiedTarget,
        sha256: &str,
        size: u64,
        mirror: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let check = |bytes: Vec<u8>, from: &str| -> Result<Vec<u8>, String> {
            let got = sha256_hex(&bytes);
            if got != sha256 || bytes.len() as u64 != size {
                Err(format!(
                    "{from}: {} bytes with sha256 {got}; the manifest signed {size} bytes with {sha256}",
                    bytes.len()
                ))
            } else {
                Ok(bytes)
            }
        };
        let mut why = Vec::new();
        if let Some(path) = mirror {
            match self.host.fetch_mirror(path, size).await {
                Ok(b) => match check(b, "the hub's mirror") {
                    Ok(b) => return Ok(b),
                    Err(e) => why.push(e),
                },
                Err(e) => why.push(e),
            }
        }
        match &t.url {
            Some(url) => match self.host.fetch(url, size).await {
                Ok(b) => return check(b, url),
                Err(e) => why.push(e),
            },
            None => why.push("the release names no URL for it".into()),
        }
        Err(why.join("; "))
    }

    /// Make the running build reachable through the layout: the first time,
    /// the binary at `link` moves to `<root>/<installed>/<bin>`, `current`
    /// points there, and `link` becomes a symlink through `current`.
    fn adopt(&self, installed: &Version) -> Result<(), String> {
        let current = self.cfg.root.join("current");
        let link_is_ours = std::fs::read_link(&self.cfg.link)
            .ok()
            .is_some_and(|t| t == current.join(&self.cfg.bin));
        if link_is_ours && current.exists() {
            return Ok(());
        }
        let dir = self.version_dir(installed);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let kept = dir.join(&self.cfg.bin);
        if !kept.exists() {
            // Follow whatever `link` is (a file, or someone else's symlink).
            let real = std::fs::canonicalize(&self.cfg.link)
                .map_err(|e| format!("{}: {e}", self.cfg.link.display()))?;
            std::fs::copy(&real, &kept).map_err(|e| format!("{}: {e}", kept.display()))?;
            set_executable(&kept)?;
        }
        point(&current, Path::new(&safe(&installed.to_string())))?;
        point(&self.cfg.link, &current.join(&self.cfg.bin))?;
        self.log(&format!(
            "{} now runs through {}",
            self.cfg.link.display(),
            current.display()
        ));
        Ok(())
    }

    async fn install_and_validate(&mut self, installed: Version) -> Outcome {
        let cand = self
            .state
            .candidate
            .clone()
            .expect("prepare set the candidate");
        self.report(UpdatePhase::Installing, &installed, Value::Null, None)
            .await;
        let switched = point(
            &self.cfg.root.join("current"),
            Path::new(&safe(&cand.version.to_string())),
        )
        .and_then(|()| self.host.restart());
        if let Err(e) = switched {
            return self
                .fail_and_roll_back(&cand, format!("install: {e}"), None)
                .await;
        }
        self.report(UpdatePhase::Validating, &cand.version, Value::Null, None)
            .await;
        self.validate(cand).await
    }

    async fn validate(&mut self, cand: Candidate) -> Outcome {
        match self.gates(&cand.version, self.cfg.soak).await {
            Ok(schema) => {
                self.report(
                    UpdatePhase::Success,
                    &cand.version,
                    json!({ "health": "ready", "soak_secs": self.cfg.soak.as_secs(), "schema": schema }),
                    None,
                )
                .await;
                self.log(&format!("now running {}", cand.version));
                self.state.current = Some(cand.version.clone());
                self.state.announced = None;
                self.set_phase(UpdatePhase::Idle);
                self.prune_versions(&cand.version);
                Outcome::Updated(cand.version)
            }
            Err((why, schema)) => self.fail_and_roll_back(&cand, why, schema).await,
        }
    }

    async fn fail_and_roll_back(
        &mut self,
        cand: &Candidate,
        why: String,
        schema: Option<i64>,
    ) -> Outcome {
        self.log(&format!("{} failed: {why}", cand.version));
        self.state.bad.add(BadEntry {
            version: cand.version.clone(),
            digest: Some(cand.sha256.clone()),
            why: why.clone(),
            at: fleet_update::time::format_rfc3339(now_unix()),
        });
        self.state.last_error = Some(why.clone());
        self.state.candidate_schema = schema;
        self.report(UpdatePhase::Failed, &cand.version, Value::Null, Some(why))
            .await;
        self.roll_back().await
    }

    /// `current` back to the previous release, the database back too when
    /// the candidate may have migrated it, and the unit restarted.
    async fn roll_back(&mut self) -> Outcome {
        let Some(prev) = self.state.previous.clone() else {
            return self
                .rollback_failed("no previous build recorded to roll back to".into())
                .await;
        };
        let cand = self.state.candidate.clone();
        if self.state.phase != UpdatePhase::RollingBack {
            self.report(UpdatePhase::RollingBack, &prev.version, Value::Null, None)
                .await;
        }
        let restore = !self.state.restored
            && self.cfg.data_dir.is_some()
            && needs_restore(&prev, cand.as_ref(), self.state.candidate_schema);
        if restore {
            let (Some(data), Some(backup)) = (self.cfg.data_dir.clone(), prev.backup.clone())
            else {
                return self
                    .rollback_failed(
                        "the candidate may have migrated the database and there is no backup"
                            .into(),
                    )
                    .await;
            };
            if let Err(e) = self.host.stop() {
                return self
                    .rollback_failed(format!("stopping the candidate: {e}"))
                    .await;
            }
            let v = cand
                .as_ref()
                .map(|c| c.version.to_string())
                .unwrap_or_default();
            if let Err(e) = restore_db(&data, &backup, &v) {
                return self
                    .rollback_failed(format!("restoring {}: {e}", backup.display()))
                    .await;
            }
            self.state.restored = true;
            self.save();
            self.log(&format!("database restored from {}", backup.display()));
        }
        let back = point(
            &self.cfg.root.join("current"),
            Path::new(&safe(&prev.version.to_string())),
        )
        .and_then(|()| self.host.restart());
        if let Err(e) = back {
            return self
                .rollback_failed(format!("starting {} again: {e}", prev.version))
                .await;
        }
        match self.gates(&prev.version, Duration::ZERO).await {
            Ok(_) => {
                let restored = self.state.restored;
                self.report(
                    UpdatePhase::Recovered,
                    &prev.version,
                    json!({
                        "data_restored": restored,
                        "lost": if restored { "writes the candidate made while it was being validated" } else { "nothing" },
                        "backup": prev.backup,
                    }),
                    None,
                )
                .await;
                self.log(&format!(
                    "back on {}{}",
                    prev.version,
                    if restored { " (database restored)" } else { "" }
                ));
                self.state.current = Some(prev.version.clone());
                self.state.restored = false;
                self.state.candidate_schema = None;
                self.set_phase(UpdatePhase::Idle);
                Outcome::RolledBack {
                    to: prev.version,
                    data_restored: restored,
                }
            }
            Err((why, _)) => {
                self.rollback_failed(format!("{} did not come back: {why}", prev.version))
                    .await
            }
        }
    }

    async fn rollback_failed(&mut self, why: String) -> Outcome {
        self.log(&format!(
            "ROLLBACK FAILED: {why}. Leaving everything in place for an operator; `clear` once it is sorted."
        ));
        let installed = self
            .state
            .previous
            .as_ref()
            .map(|p| p.version.clone())
            .or_else(|| self.state.current.clone())
            .unwrap_or_else(|| Version::new(0, 0, 0));
        if self.state.phase != UpdatePhase::RollingBack {
            self.set_phase(UpdatePhase::RollingBack);
        }
        self.state.last_error = Some(why.clone());
        let _ = self.host.stop();
        self.report(
            UpdatePhase::RollbackFailed,
            &installed,
            Value::Null,
            Some(why.clone()),
        )
        .await;
        Outcome::RollbackFailed(why)
    }

    /// Up, the right build and ready within `ready_timeout`, then still so —
    /// with no automatic restart — through `soak`. The error carries the
    /// last store schema the build reported.
    async fn gates(
        &self,
        want: &Version,
        soak: Duration,
    ) -> Result<Option<i64>, (String, Option<i64>)> {
        let deadline = Instant::now() + self.cfg.ready_timeout;
        let mut schema = None;
        let restarts = loop {
            match self.host.health().await {
                Health::Up {
                    version,
                    ready,
                    schema: s,
                    restarts,
                    why,
                } => {
                    schema = s.or(schema);
                    if let Some(v) = &version {
                        if v != want {
                            return Err((format!("it runs {v}, expected {want}"), schema));
                        }
                    }
                    if ready && version.is_some() {
                        break restarts;
                    }
                    if Instant::now() >= deadline {
                        return Err((
                            format!(
                                "not ready within {} s: {}",
                                self.cfg.ready_timeout.as_secs(),
                                why.unwrap_or_else(|| "it does not say why".into())
                            ),
                            schema,
                        ));
                    }
                }
                Health::Down(why) => {
                    if Instant::now() >= deadline {
                        return Err((
                            format!(
                                "not running within {} s: {why}",
                                self.cfg.ready_timeout.as_secs()
                            ),
                            schema,
                        ));
                    }
                }
            }
            tokio::time::sleep(self.cfg.poll).await;
        };
        let end = Instant::now() + soak;
        while Instant::now() < end {
            tokio::time::sleep(self.cfg.poll.min(end - Instant::now())).await;
            match self.host.health().await {
                Health::Up {
                    ready: true,
                    restarts: r,
                    schema: s,
                    ..
                } if r == restarts => schema = s.or(schema),
                Health::Up { restarts: r, .. } if r != restarts => {
                    return Err((
                        format!("it restarted {} time(s) during the soak", r - restarts),
                        schema,
                    ))
                }
                Health::Up { why, .. } => {
                    return Err((
                        format!(
                            "stopped being ready during the soak: {}",
                            why.unwrap_or_else(|| "it does not say why".into())
                        ),
                        schema,
                    ))
                }
                Health::Down(why) => {
                    return Err((format!("stopped during the soak: {why}"), schema))
                }
            }
        }
        Ok(schema)
    }

    /// Keep `current`, the previous release and the newest `keep_versions`
    /// others; drop the rest.
    fn prune_versions(&self, current: &Version) {
        let Ok(rd) = std::fs::read_dir(&self.cfg.root) else {
            return;
        };
        let keep_names: Vec<String> = [
            Some(current.clone()),
            self.state.previous.as_ref().map(|p| p.version.clone()),
        ]
        .into_iter()
        .flatten()
        .map(|v| safe(&v.to_string()))
        .collect();
        let mut others: Vec<(Version, PathBuf)> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if keep_names.contains(&name) {
                    return None;
                }
                Version::parse(&name).ok().map(|v| (v, e.path()))
            })
            .collect();
        others.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in others.into_iter().skip(self.cfg.keep_versions) {
            let _ = std::fs::remove_dir_all(p);
        }
    }

    /// Finish what a crash left half done.
    async fn resume(&mut self) -> Option<Outcome> {
        use UpdatePhase::*;
        match self.state.phase {
            Idle | RollbackFailed => None,
            Checking | Available | Success | Recovered | Unknown => {
                self.state.phase = Idle;
                self.save();
                None
            }
            Downloading | Verifying | Ready => {
                self.log("an interrupted attempt is abandoned; nothing that runs was touched");
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
                self.log(&format!(
                    "resuming the validation of {} after an interruption",
                    cand.version
                ));
                if self.state.phase == Installing {
                    let on_cand = std::fs::read_link(self.cfg.root.join("current"))
                        .ok()
                        .is_some_and(|t| t == Path::new(&safe(&cand.version.to_string())));
                    if !on_cand {
                        return Some(
                            self.fail_and_roll_back(
                                &cand,
                                "the install was interrupted".into(),
                                None,
                            )
                            .await,
                        );
                    }
                    self.report(Validating, &cand.version, Value::Null, None)
                        .await;
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

    /// An operator sorted out a rollback failure.
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

/// Whether going back needs the backup: the candidate's database is newer
/// than what the previous build opens. Unknown counts as yes.
pub fn needs_restore(prev: &Before, cand: Option<&Candidate>, cand_schema: Option<i64>) -> bool {
    let Some(before) = prev.schema else {
        return true;
    };
    match cand_schema.or_else(|| cand.and_then(|c| c.schema_to)) {
        Some(after) => after > before,
        None => true,
    }
}

/// `path` as a symlink to `target`, replaced atomically (a temporary link,
/// then a rename over the old one).
pub fn point(path: &Path, target: &Path) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}: no parent", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.new"));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// `<bin>` in `dir` or one directory below it (a tarball holds
/// `<bin>-<version>-<target>/<bin>`).
pub fn find_binary(dir: &Path, bin: &str) -> Option<PathBuf> {
    let direct = dir.join(bin);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path().join(bin))
        .find(|p| p.is_file())
}

fn set_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// `X.Y.Z` out of what `<bin> --version` printed (`fleet-agent 0.5.4`).
pub fn parse_version_output(out: &str) -> Result<Version, String> {
    out.split_whitespace()
        .rev()
        .find_map(|w| Version::parse(w.trim_start_matches('v')).ok())
        .ok_or_else(|| format!("no version in {:?}", out.trim()))
}

#[cfg(test)]
mod tests;
