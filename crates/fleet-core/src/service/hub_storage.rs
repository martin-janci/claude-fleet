//! The hub's own disk and backups (hardening survey 2026-10-26).
//!
//! Hosts' disks were watched (`HOST_HEALTH_SCRIPT`) but the hub's data dir
//! was not: a full NAS volume showed first as SQLite write failures. Daily
//! backups relied on a cron of `deploy/hub/backup.sh` that nothing checked.
//! So `fleet-hub serve` names its data dir here ([`init`]) and runs
//! [`spawn_backup_tick`]: a consistent copy of `state.db` once a day into
//! `<data>/backups/daily-*.db`, the newest [`KEEP_DAILY`] kept, skipped while
//! a newer backup (the script's, an update's) already exists. `fleet_health`
//! carries [`StorageHealth`]: free space under the data dir and the newest
//! backup, each with the line past which a client warns.
//!
//! A desktop never calls [`init`], so it reports nothing here.

use crate::store::backup::{backup_to, BackupInfo};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// The daily backups' file prefix: `daily-YYYYmmdd-HHMMSS.db`. Not
/// `backup.sh`'s `state`, so neither prunes the other's files.
pub const DAILY_PREFIX: &str = "daily";
/// Daily backups kept (as `backup.sh` keeps 14).
pub const KEEP_DAILY: usize = 14;
/// A backup is taken when the newest one is older than this.
pub const BACKUP_EVERY_SECS: i64 = 86_400;
/// How often the tick looks.
pub const CHECK_EVERY: Duration = Duration::from_secs(3_600);
/// Past this age the newest backup is stale and a client warns.
pub const BACKUP_STALE_SECS: i64 = 2 * 86_400;
/// Under this much free space (or [`LOW_DISK_PCT`]) a client warns.
pub const LOW_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const LOW_DISK_PCT: u64 = 5;
/// How long one disk reading is reused: `fleet_health` is polled often.
const DISK_CACHE: Duration = Duration::from_secs(60);

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Name the hub's data dir (the one holding `state.db`). Once per process;
/// a second call keeps the first.
pub fn init(data_dir: &Path) {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
}

/// `fleet_health.storage`: the hub's own disk and backups.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StorageHealth {
    /// Free and total bytes of the volume holding the data dir; `None`
    /// when it could not be read.
    #[serde(default)]
    pub free_bytes: Option<u64>,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// Under [`LOW_DISK_BYTES`] or [`LOW_DISK_PCT`] free.
    #[serde(default)]
    pub low_disk: bool,
    /// The newest backup's time (unix seconds): a daily one, an update's or
    /// `backup.sh`'s; `None` when there is none.
    #[serde(default)]
    pub last_backup_at: Option<i64>,
    /// No backup, or the newest is older than [`BACKUP_STALE_SECS`].
    #[serde(default)]
    pub backup_stale: bool,
}

/// The hub's storage health at `now`; `None` where no data dir was named
/// (the desktop).
pub fn health(now: i64) -> Option<StorageHealth> {
    let dir = DATA_DIR.get()?;
    let (free_bytes, total_bytes) = cached_disk_space(dir).unzip();
    let last_backup_at = newest_backup(&backup_dirs(dir));
    Some(StorageHealth {
        free_bytes,
        total_bytes,
        low_disk: free_bytes
            .zip(total_bytes)
            .is_some_and(|(f, t)| low_disk(f, t)),
        last_backup_at,
        backup_stale: backup_stale(last_backup_at, now),
    })
}

/// Whether `free` of `total` bytes is under the warning line.
pub fn low_disk(free: u64, total: u64) -> bool {
    free < LOW_DISK_BYTES || (total > 0 && free.saturating_mul(100) / total < LOW_DISK_PCT)
}

/// Whether the newest backup (at `last`) is missing or too old at `now`.
pub fn backup_stale(last: Option<i64>, now: i64) -> bool {
    last.is_none_or(|t| now - t > BACKUP_STALE_SECS)
}

/// Where backups land: `<data>/backups` (`fleet-hub backup`, the updater,
/// the daily tick) and `<data>/../backups` (`backup.sh`'s default).
fn backup_dirs(data_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![data_dir.join("backups")];
    if let Some(parent) = data_dir.parent() {
        dirs.push(parent.join("backups"));
    }
    dirs
}

/// The newest `*.db` file's modification time across `dirs`.
pub fn newest_backup(dirs: &[PathBuf]) -> Option<i64> {
    dirs.iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "db"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .max()
}

/// Free and total bytes, as `disk_space` returns them.
type Space = Option<(u64, u64)>;

fn cached_disk_space(dir: &Path) -> Space {
    static CACHE: Mutex<Option<(Instant, Space)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, v)) = *cache {
        if at.elapsed() < DISK_CACHE {
            return v;
        }
    }
    let v = disk_space(dir);
    *cache = Some((Instant::now(), v));
    v
}

/// Free and total bytes of the mounted volume that holds `dir`: the disk
/// whose mount point is the longest prefix of its canonical path.
pub fn disk_space(dir: &Path) -> Option<(u64, u64)> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| (d.available_space(), d.total_space()))
}

/// Take a daily backup of `<data_dir>/state.db` when the newest backup is
/// older than [`BACKUP_EVERY_SECS`], then keep the newest [`KEEP_DAILY`]
/// daily ones. `Ok(None)` when none was due.
pub fn backup_if_due(data_dir: &Path, now: i64) -> Result<Option<BackupInfo>, String> {
    if newest_backup(&backup_dirs(data_dir)).is_some_and(|t| now - t < BACKUP_EVERY_SECS) {
        return Ok(None);
    }
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let dest = daily_path(&dir, now);
    let info = backup_to(&data_dir.join("state.db"), &dest)?;
    prune_daily(&dir, KEEP_DAILY);
    Ok(Some(info))
}

/// `<dir>/daily-YYYYmmdd-HHMMSS.db` in UTC (`backup.sh`'s stamp), with
/// `-<n>` when the name is taken.
fn daily_path(dir: &Path, now: i64) -> PathBuf {
    let rfc = fleet_update::time::format_rfc3339(now);
    let stamp = rfc
        .trim_end_matches('Z')
        .replace(['-', ':'], "")
        .replace('T', "-");
    let first = dir.join(format!("{DAILY_PREFIX}-{stamp}.db"));
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| dir.join(format!("{DAILY_PREFIX}-{stamp}-{n}.db")))
        .find(|p| !p.exists())
        .expect("an unbounded range always finds a free name")
}

/// Keep the newest `keep` `daily-<digit>….db` files in `dir` (the stamp
/// sorts by time); remove the rest.
pub fn prune_daily(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{DAILY_PREFIX}-");
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            n.ends_with(".db")
                && n.strip_prefix(&prefix)
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .collect();
    names.sort_unstable();
    let drop = names.len().saturating_sub(keep);
    for name in &names[..drop] {
        if let Err(e) = std::fs::remove_file(dir.join(name)) {
            tracing::warn!(file = %name, error = %e, "[backup] could not prune an old daily backup");
        }
    }
}

/// The daily backup tick: looks every [`CHECK_EVERY`] (the first look at
/// once) and backs up when one is due, until `token`. It only copies the
/// hub's own database, so Pause all leaves it running.
pub fn spawn_backup_tick(
    data_dir: PathBuf,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(crate::service::tick::run_cancellable_tick(
        tokio::time::interval(CHECK_EVERY),
        token,
        move || {
            let dir = data_dir.clone();
            async move {
                let now = crate::store::now_unix();
                let taken = tokio::task::spawn_blocking(move || backup_if_due(&dir, now))
                    .await
                    .unwrap_or_else(|e| Err(format!("the backup task failed: {e}")));
                match &taken {
                    Ok(Some(info)) => {
                        tracing::info!(path = %info.path.display(), bytes = info.bytes, "[backup] daily backup taken");
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "[backup] daily backup failed"),
                }
                crate::service::loops::report("backup", taken.map(drop), Some(CHECK_EVERY));
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEventBus;
    use crate::store::Store;
    use std::sync::Arc;

    #[test]
    fn the_warning_lines() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert!(low_disk(GB, 100 * GB), "under 2 GB");
        assert!(low_disk(4 * GB, 100 * GB), "under 5%");
        assert!(!low_disk(10 * GB, 100 * GB));
        assert!(backup_stale(None, 1_000));
        assert!(backup_stale(Some(0), BACKUP_STALE_SECS + 1));
        assert!(!backup_stale(Some(0), BACKUP_STALE_SECS));
    }

    /// A day's backup is taken once, named like `backup.sh`'s, and only the
    /// newest daily ones are kept; other prefixes are never pruned.
    #[test]
    fn a_daily_backup_is_taken_once_a_day_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let _store = Store::open_with_bus(&data.join("state.db"), Arc::new(NoopEventBus)).unwrap();
        let now = crate::store::now_unix();

        let info = backup_if_due(&data, now)
            .unwrap()
            .expect("no backup yet: one is due");
        let name = info
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("daily-2") && name.ends_with(".db"),
            "{name}"
        );
        assert!(backup_if_due(&data, now).unwrap().is_none(), "one a day");
        assert!(newest_backup(&backup_dirs(&data)).is_some_and(|t| (now - t).abs() < 60));

        let backups = data.join("backups");
        for stamp in ["20200101-000000", "20200102-000000", "20200103-000000"] {
            std::fs::write(backups.join(format!("daily-{stamp}.db")), b"x").unwrap();
        }
        std::fs::write(backups.join("pre-0.6.4-20200101-000000.db"), b"x").unwrap();
        prune_daily(&backups, 2);
        let mut left: Vec<String> = std::fs::read_dir(&backups)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "daily-20200103-000000.db".to_string(),
                name,
                "pre-0.6.4-20200101-000000.db".to_string()
            ]
        );
    }
}
