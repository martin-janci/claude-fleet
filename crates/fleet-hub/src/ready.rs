//! Readiness, for `fleet-updater`'s health gate (update-channel design §8.4).
//!
//! `/healthz` says the process answers; it deliberately names no version and
//! stays that way. Readiness is more — the store is open and migrated, the
//! listener is bound, the first reconcile pass has finished — and it carries
//! the build identity the updater checks against the release manifest. None
//! of that goes on the network: `serve` writes it to `<data_dir>/run/ready.json`
//! every few seconds, and `fleet-hub healthcheck --ready --json`, run inside
//! the container through `docker exec`, reads it back beside the `/healthz`
//! probe. A file whose heartbeat stopped is stale, and stale is not ready: a
//! hub that died without removing it is never read as healthy.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fleet_core::service::tick::ReconcileStats;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Relative to the data directory.
pub const READY_FILE: &str = "run/ready.json";
/// How often `serve` rewrites the file.
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// A heartbeat older than this is a hub that stopped writing it.
pub const STALE_AFTER_SECS: i64 = 20;

/// The build's commit and CI build, from `build.rs`.
pub const COMMIT: &str = env!("FLEET_GIT_SHA");
pub const BUILD_ID: &str = env!("FLEET_BUILD_ID");

/// What `serve` writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    /// This process's own verdict; `healthcheck` adds liveness and freshness.
    pub ready: bool,
    pub pid: u32,
    pub version: String,
    pub commit: String,
    pub build_id: String,
    /// The protocol windows this build serves (the release manifest's
    /// `compatibility`, from the binary itself).
    pub contract: u32,
    pub agent_proto: [u32; 2],
    pub peer_proto: u32,
    pub schema: Option<i64>,
    pub started_at: i64,
    pub heartbeat_at: i64,
    pub checks: Checks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub store: String,
    pub listener: String,
    /// `ok`, `pending`, `failed`, or `disabled` (`reconcile.interval_secs=0`).
    pub first_reconcile: String,
}

/// What does not change while `serve` runs.
#[derive(Debug, Clone)]
pub struct Facts {
    pub schema: Option<i64>,
    pub started_at: i64,
    pub reconcile_enabled: bool,
}

pub fn first_reconcile(enabled: bool, stats: &ReconcileStats, started_at: i64) -> &'static str {
    if !enabled {
        return "disabled";
    }
    match stats.last_finished_at {
        Some(t) if t >= started_at && stats.consecutive_failures == 0 => "ok",
        Some(t) if t >= started_at => "failed",
        _ => "pending",
    }
}

pub fn snapshot(facts: &Facts, stats: &ReconcileStats, now: i64) -> Readiness {
    let first = first_reconcile(facts.reconcile_enabled, stats, facts.started_at);
    Readiness {
        ready: matches!(first, "ok" | "disabled"),
        pid: std::process::id(),
        version: fleet_core::app_version::get().to_string(),
        commit: COMMIT.to_string(),
        build_id: BUILD_ID.to_string(),
        contract: fleet_core::wire_contract::CONTRACT_REVISION,
        agent_proto: [fleet_proto::MIN_SUPPORTED_PROTO, fleet_proto::PROTO_VERSION],
        peer_proto: fleet_core::service::peer::wire::PROTO,
        schema: facts.schema,
        started_at: facts.started_at,
        heartbeat_at: now,
        // `serve` starts the writer only after the store migrated and the
        // listener bound: either failing ends `serve` before this runs.
        checks: Checks {
            store: "ok".into(),
            listener: "ok".into(),
            first_reconcile: first.into(),
        },
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(READY_FILE)
}

/// Write atomically: a reader sees the old file or the new one, never half.
pub fn write(data_dir: &Path, r: &Readiness) -> std::io::Result<()> {
    let p = path(data_dir);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(r).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &p)
}

/// Rewrite the file every [`HEARTBEAT`] until `cancel`, then remove it: a hub
/// that is stopping is not ready.
pub fn spawn_writer(
    data_dir: PathBuf,
    facts: Facts,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut warned = false;
        loop {
            let stats = fleet_core::service::tick::tick_stats().reconcile();
            if let Err(e) = write(&data_dir, &snapshot(&facts, &stats, unix_now())) {
                if !warned {
                    tracing::warn!(error = %e, path = %path(&data_dir).display(), "cannot write the readiness file");
                    warned = true;
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(HEARTBEAT) => {}
            }
        }
        let _ = std::fs::remove_file(path(&data_dir));
    })
}

/// What `healthcheck --ready --json` prints.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// The overall verdict, and the exit status: live, fresh and ready.
    pub ready: bool,
    /// `/healthz` answered with the hub's body.
    pub live: bool,
    /// The file exists, its heartbeat is recent and its process is running.
    pub fresh: bool,
    pub hub: Option<Readiness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Judge the readiness file against liveness and the clock.
pub fn judge(
    live: Result<(), String>,
    file: Result<Readiness, String>,
    now: i64,
    pid_alive: impl Fn(u32) -> bool,
) -> Report {
    let mut error = live.as_ref().err().cloned();
    let (fresh, hub) = match file {
        Ok(r) => {
            let age = now - r.heartbeat_at;
            let fresh = (0..=STALE_AFTER_SECS).contains(&age) && pid_alive(r.pid);
            if !fresh && error.is_none() {
                error = Some(format!(
                    "the readiness file is stale (heartbeat {age}s ago, pid {}): the hub is not writing it",
                    r.pid
                ));
            }
            if fresh && !r.ready && error.is_none() {
                error = Some(format!(
                    "not ready: first reconcile {}",
                    r.checks.first_reconcile
                ));
            }
            (fresh, Some(r))
        }
        Err(e) => {
            error.get_or_insert(e);
            (false, None)
        }
    };
    let live = live.is_ok();
    Report {
        ready: live && fresh && hub.as_ref().is_some_and(|h| h.ready),
        live,
        fresh,
        hub,
        error,
    }
}

pub fn read(data_dir: &Path) -> Result<Readiness, String> {
    let p = path(data_dir);
    let body =
        std::fs::read(&p).map_err(|e| format!("no readiness file at {}: {e}", p.display()))?;
    serde_json::from_slice(&body)
        .map_err(|e| format!("unreadable readiness file {}: {e}", p.display()))
}

/// Linux: the pid has a `/proc` entry. Elsewhere (or without `/proc`) the
/// heartbeat alone decides.
pub fn pid_alive(pid: u32) -> bool {
    let proc_root = Path::new("/proc");
    !proc_root.is_dir() || proc_root.join(pid.to_string()).exists()
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(enabled: bool) -> Facts {
        Facts {
            schema: Some(73),
            started_at: 1000,
            reconcile_enabled: enabled,
        }
    }

    fn stats(finished: Option<i64>, failures: u32) -> ReconcileStats {
        ReconcileStats {
            last_finished_at: finished,
            consecutive_failures: failures,
            ..Default::default()
        }
    }

    #[test]
    fn ready_only_after_the_first_good_reconcile() {
        fleet_core::app_version::set(env!("CARGO_PKG_VERSION"));
        assert_eq!(first_reconcile(true, &stats(None, 0), 1000), "pending");
        // A pass that finished before this process started is not ours.
        assert_eq!(first_reconcile(true, &stats(Some(999), 0), 1000), "pending");
        assert_eq!(first_reconcile(true, &stats(Some(1001), 0), 1000), "ok");
        assert_eq!(first_reconcile(true, &stats(Some(1001), 2), 1000), "failed");
        assert_eq!(first_reconcile(false, &stats(None, 0), 1000), "disabled");

        assert!(!snapshot(&facts(true), &stats(None, 0), 1001).ready);
        assert!(snapshot(&facts(true), &stats(Some(1001), 0), 1002).ready);
        assert!(snapshot(&facts(false), &stats(None, 0), 1001).ready);
        let s = snapshot(&facts(true), &stats(Some(1001), 1), 1002);
        assert!(!s.ready);
        assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(s.commit, COMMIT);
        assert_eq!(s.contract, fleet_core::wire_contract::CONTRACT_REVISION);
    }

    #[test]
    fn judge_needs_live_fresh_and_ready() {
        fleet_core::app_version::set(env!("CARGO_PKG_VERSION"));
        let good = snapshot(&facts(false), &stats(None, 0), 2000);
        let alive = |_| true;

        let r = judge(Ok(()), Ok(good.clone()), 2005, alive);
        assert!(r.ready && r.live && r.fresh && r.error.is_none());

        let r = judge(
            Err("unhealthy: refused".into()),
            Ok(good.clone()),
            2005,
            alive,
        );
        assert!(!r.ready && !r.live);

        let r = judge(Ok(()), Ok(good.clone()), 2000 + STALE_AFTER_SECS + 1, alive);
        assert!(!r.ready && !r.fresh);
        assert!(r.error.unwrap().contains("stale"));

        let r = judge(Ok(()), Ok(good.clone()), 2005, |_| false);
        assert!(!r.ready && !r.fresh, "a dead pid's file is stale");

        let pending = snapshot(&facts(true), &stats(None, 0), 2000);
        let r = judge(Ok(()), Ok(pending), 2005, alive);
        assert!(!r.ready && r.fresh);
        assert!(r.error.unwrap().contains("pending"));

        let r = judge(Ok(()), Err("no readiness file".into()), 2005, alive);
        assert!(!r.ready && r.hub.is_none());
    }

    #[test]
    fn write_then_read_round_trips() {
        fleet_core::app_version::set(env!("CARGO_PKG_VERSION"));
        let dir = tempfile::tempdir().unwrap();
        let r = snapshot(&facts(false), &stats(None, 0), 3000);
        write(dir.path(), &r).unwrap();
        assert_eq!(read(dir.path()).unwrap(), r);
        assert!(!dir.path().join("run/ready.json.tmp").exists());
    }

    #[tokio::test]
    async fn the_writer_removes_the_file_when_stopping() {
        fleet_core::app_version::set(env!("CARGO_PKG_VERSION"));
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let h = spawn_writer(dir.path().to_path_buf(), facts(false), cancel.clone());
        for _ in 0..100 {
            if path(dir.path()).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(read(dir.path()).unwrap().ready);
        cancel.cancel();
        h.await.unwrap();
        assert!(!path(dir.path()).exists());
    }
}
