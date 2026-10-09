//! The updater's state file (design §8.1): what runs, what was running before
//! it and how to bring that back, the attempt in flight, the bad list, and
//! the reports the hub has not taken yet. Written atomically (tmp + fsync +
//! rename + fsync of the directory), so a crash leaves the old file or the
//! new one, never half of either.

use std::io::Write;
use std::path::{Path, PathBuf};

use fleet_update::phase::BadList;
use fleet_update::{Report, UpdatePhase, Version};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FILE: &str = "state.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Always `hub:self` for now (`node:<name>` later, design §8.6).
    #[serde(default = "hub_self")]
    pub target: String,
    #[serde(default)]
    pub phase: UpdatePhase,
    #[serde(default)]
    pub attempt: Option<String>,
    /// What runs now, as of the last success (or first sight).
    #[serde(default)]
    pub current: Option<Build>,
    /// The attempt's target.
    #[serde(default)]
    pub candidate: Option<Build>,
    /// How to bring back what ran before the attempt: its config and image,
    /// and the backup taken just before it was stopped.
    #[serde(default)]
    pub previous: Option<Previous>,
    #[serde(default)]
    pub bad: BadList,
    /// The last version reported as `available` (so `notify` reports once).
    #[serde(default)]
    pub announced: Option<Version>,
    /// Reports the hub did not take, oldest first (it is down while it
    /// restarts, which is exactly when the updater has most to say).
    #[serde(default)]
    pub pending_reports: Vec<Report>,
    /// The last thing that went wrong, for `fleet-updater status`.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The candidate's last reported store schema, kept for a rollback that
    /// a crash interrupted.
    #[serde(default)]
    pub candidate_schema: Option<i64>,
    /// This rollback already put the backup in place; a resumed one must not
    /// do it twice.
    #[serde(default)]
    pub restored: bool,
}

fn hub_self() -> String {
    "hub:self".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Build {
    pub version: Version,
    /// `ghcr.io/…/fleet-hub`.
    #[serde(default)]
    pub image: Option<String>,
    /// The manifest digest pulled (`sha256:…`).
    #[serde(default)]
    pub digest: Option<String>,
    /// The local image id (`sha256:…`) the container must run.
    #[serde(default)]
    pub image_id: Option<String>,
    /// From the release manifest, for the identity gate.
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub build_id: Option<String>,
    /// The newest store schema this build migrates to, from the manifest.
    #[serde(default)]
    pub schema_to: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Previous {
    pub version: Version,
    pub image_id: String,
    /// The `containers/create` body that runs it again (`spec::create_body`).
    pub spec: Value,
    /// The schema the database had when the backup was taken.
    #[serde(default)]
    pub schema: Option<i64>,
    #[serde(default)]
    pub backup: Option<PathBuf>,
}

impl State {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// A missing file is a fresh state; an unreadable one is an error, not a
    /// fresh start: it may hold the only record of how to roll back.
    pub fn load(dir: &Path) -> Result<State, String> {
        let p = Self::path(dir);
        match std::fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State {
                target: hub_self(),
                ..State::default()
            }),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = Self::path(dir);
        let tmp = dir.join(format!("{FILE}.tmp"));
        let body = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&body)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &p)?;
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(())
        };
        write().map_err(|e| format!("{}: {e}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = State::load(dir.path()).unwrap();
        assert_eq!(fresh.target, "hub:self");
        assert_eq!(fresh.phase, UpdatePhase::Idle);
        let mut s = fresh.clone();
        s.phase = UpdatePhase::Validating;
        s.attempt = Some("a1".into());
        s.previous = Some(Previous {
            version: Version::new(0, 5, 3),
            image_id: "sha256:old".into(),
            spec: serde_json::json!({"Image": "x"}),
            schema: Some(140),
            backup: Some("/hub-data/backups/pre-0.5.4.db".into()),
        });
        s.save(dir.path()).unwrap();
        assert_eq!(State::load(dir.path()).unwrap(), s);
        assert!(!dir.path().join("state.json.tmp").exists());
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(State::path(dir.path()), "{not json").unwrap();
        assert!(State::load(dir.path()).is_err());
    }
}
