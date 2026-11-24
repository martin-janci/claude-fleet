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
use std::sync::atomic::{AtomicBool, Ordering};
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
/// A reading in flight this long is hung (a NAS mount that stopped
/// answering): the last one is no longer served as current (review w02-4).
const READING_HUNG_AFTER: Duration = Duration::from_secs(5 * 60);

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Name the hub's data dir (the one holding `state.db`). Once per process;
/// a second call keeps the first.
pub fn init(data_dir: &Path) {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    // The first reading, so the first `fleet_health` has one.
    if let Some(dir) = DATA_DIR.get() {
        refresh_in_background(dir);
    }
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
    /// When the disk was last read (unix seconds).
    #[serde(default)]
    pub read_at: Option<i64>,
    /// A reading of the data dir's disk has hung past
    /// [`READING_HUNG_AFTER`]: the free space is unknown (`None`), not the
    /// last value seen. A client warns.
    #[serde(default)]
    pub disk_unreadable: bool,
}

/// The hub's storage health at `now`; `None` where no data dir was named
/// (the desktop), and until the first reading of it has landed.
///
/// It never touches the disk itself (review 20 w14-1): a hung mount would
/// park the caller, a tokio worker serving `fleet_health`. It answers the
/// last reading and, once that is older than [`DISK_CACHE`], starts one
/// fresh reading on a thread of its own, at most one at a time.
pub fn health(now: i64) -> Option<StorageHealth> {
    let dir = DATA_DIR.get()?;
    let last = cached_reading(dir);
    let in_flight_for = (*READING_STARTED.lock().unwrap_or_else(|p| p.into_inner()))
        .filter(|_| READING_IN_FLIGHT.load(Ordering::Acquire))
        .map(|at| at.elapsed());
    assess(last, in_flight_for, now)
}

/// PURE: the health from the last reading (`None` before the first) and how
/// long the current one has been in flight. A reading hung past
/// [`READING_HUNG_AFTER`] makes the disk unknown rather than leaving the
/// pre-hang free space (and `low_disk: false`) on show for good.
fn assess(
    last: Option<(i64, Reading)>,
    in_flight_for: Option<Duration>,
    now: i64,
) -> Option<StorageHealth> {
    let hung = in_flight_for.is_some_and(|d| d >= READING_HUNG_AFTER);
    let (read_at, (space, last_backup_at)) = match last {
        Some(r) => r,
        None if hung => (0, (None, None)),
        None => return None,
    };
    let (free_bytes, total_bytes) = if hung { (None, None) } else { space.unzip() };
    Some(StorageHealth {
        free_bytes,
        total_bytes,
        low_disk: free_bytes
            .zip(total_bytes)
            .is_some_and(|(f, t)| low_disk(f, t)),
        last_backup_at,
        backup_stale: backup_stale(last_backup_at, now),
        read_at: Some(read_at).filter(|t| *t > 0),
        disk_unreadable: hung,
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

/// One reading of the disk and the backups.
type Reading = (Space, Option<i64>);

/// The last reading, when it was taken, and that time in unix seconds.
static READING: Mutex<Option<(Instant, i64, Reading)>> = Mutex::new(None);
/// Whether a reading is under way (claimed before its thread starts).
static READING_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
/// When the reading under way started.
static READING_STARTED: Mutex<Option<Instant>> = Mutex::new(None);

/// The last reading of `dir`'s disk and backups with its unix time, `None`
/// before the first; starts a fresh one in the background when it is
/// missing or stale. The lock is never held across the reading.
fn cached_reading(dir: &Path) -> Option<(i64, Reading)> {
    let last = *READING.lock().unwrap_or_else(|p| p.into_inner());
    if last.is_none_or(|(at, _, _)| at.elapsed() >= DISK_CACHE) {
        refresh_in_background(dir);
    }
    last.map(|(_, unix, r)| (unix, r))
}

/// Read `dir`'s disk and backups on a thread of its own, unless a reading
/// is already under way.
fn refresh_in_background(dir: &Path) {
    if READING_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    *READING_STARTED.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    let dir = dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("hub-storage-reading".into())
        .spawn(move || {
            let reading = (disk_space(&dir), newest_backup(&backup_dirs(&dir)));
            *READING.lock().unwrap_or_else(|p| p.into_inner()) =
                Some((Instant::now(), crate::store::now_unix(), reading));
            READING_IN_FLIGHT.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        READING_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Free and total bytes of the volume that holds `dir`: `statvfs` of the
/// directory itself, so no other mount (one that hangs, say) is touched.
#[cfg(unix)]
pub fn disk_space(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` a plain C struct
    // that statvfs fills in; it is read only when the call succeeded.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    let frsize = (st.f_frsize as u64).max(1);
    #[allow(clippy::unnecessary_cast)]
    Some((st.f_bavail as u64 * frsize, st.f_blocks as u64 * frsize))
}

/// Free and total bytes of the mounted volume that holds `dir`: the disk
/// whose mount point is the longest prefix of its canonical path.
#[cfg(not(unix))]
pub fn disk_space(dir: &Path) -> Option<(u64, u64)> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    // Windows canonicalizes to a verbatim `\\?\D:\…` path, which no
    // mount point (`D:\`) prefixes: drop the verbatim marker first.
    let plain = dir
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .filter(|rest| !rest.starts_with("UNC\\"))
        .map(std::path::PathBuf::from);
    let dir = plain.unwrap_or(dir);
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
    crate::store::backup::create_private_dir(&dir)?;
    // A `.part` a kill left behind (the stop watchdog's exit while a copy
    // ran) is never reused: the next run takes a new stamp.
    prune_parts(&dir, now);
    // Room for the copy first (review w11-4): writing it until ENOSPC
    // starves the live hub's WAL (SQLITE_FULL), and would every hour.
    let db = data_dir.join("state.db");
    let needed = db_bytes(&db).saturating_add(BACKUP_MARGIN_BYTES);
    // The dailies are usually what filled the volume (14 copies of the
    // database): the oldest go first, down to a floor, before a new one is
    // given up on. Otherwise the newest backup only grew older while 13
    // older ones held the space a fresh one needs (review w14-7).
    let free_now = || disk_space(&dir).map(|(free, _)| free);
    if let Some(free) = make_room(&dir, needed, &free_now) {
        if free < needed {
            return Err(format!(
                "skipped: {} free under {}, a copy of the database needs about {}",
                free,
                dir.display(),
                needed
            ));
        }
    }
    let dest = daily_path(&dir, now);
    let info = backup_to(&db, &dest)?;
    prune_daily(&dir, KEEP_DAILY);
    Ok(Some(info))
}

/// The dailies never pruned to make room for a new one.
const KEEP_DAILY_FLOOR: usize = 3;

/// The free space under `dir` once the oldest dailies were removed, one at a
/// time down to [`KEEP_DAILY_FLOOR`], until `needed` fits. Nothing is
/// removed when it fits already. `None` when the volume cannot be read (the
/// copy is then tried, as before).
fn make_room(dir: &Path, needed: u64, free: &dyn Fn() -> Option<u64>) -> Option<u64> {
    let mut now = free()?;
    let mut keep = KEEP_DAILY;
    while now < needed && keep > KEEP_DAILY_FLOOR {
        keep -= 1;
        prune_daily(dir, keep);
        now = free()?;
    }
    Some(now)
}

/// Free space kept beyond the copy's size.
const BACKUP_MARGIN_BYTES: u64 = 256 * 1024 * 1024;

/// The database's size on disk, its WAL included.
fn db_bytes(db: &Path) -> u64 {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    size(db).saturating_add(size(Path::new(&wal)))
}

/// Remove `daily-*.db.part` files in `dir` older than a day.
fn prune_parts(dir: &Path, now: i64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{DAILY_PREFIX}-");
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with(&prefix) && name.ends_with(".db.part")) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|t| now - t.as_secs() as i64 > BACKUP_EVERY_SECS);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
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

    /// The disk and backups are read off the caller's thread: the first
    /// call answers nothing yet, and a later one the reading.
    #[test]
    fn storage_is_read_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let (free, total) = disk_space(dir.path()).expect("a tempdir's volume");
        assert!(total > 0 && free <= total);
        let _ = cached_reading(dir.path());
        let deadline = Instant::now() + Duration::from_secs(20);
        let reading = loop {
            if let Some(r) = cached_reading(dir.path()) {
                break r;
            }
            assert!(Instant::now() < deadline, "no reading landed");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(reading.1 .0.is_some());
    }

    /// Review w11-4: a stranded `.part` older than a day is removed; a fresh
    /// one (a copy under way) and other files are not.
    #[test]
    fn a_stranded_part_is_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let now = crate::store::now_unix();
        let old = dir.path().join("daily-20200101-000000.db.part");
        let fresh = dir.path().join("daily-20990101-000000.db.part");
        let other = dir.path().join("pre-0.6.4.db.part");
        for p in [&old, &fresh, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        let two_days_ago =
            std::time::UNIX_EPOCH + Duration::from_secs((now - 2 * BACKUP_EVERY_SECS) as u64);
        for p in [&old, &other] {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(two_days_ago)
                .unwrap();
        }
        prune_parts(dir.path(), now);
        assert!(!old.exists());
        assert!(fresh.exists() && other.exists());
    }

    /// Review w02-4: a reading that never returns (a hung NAS mount) stops
    /// the last free space from being served as current.
    #[test]
    fn a_hung_reading_makes_the_disk_unknown() {
        const GB: u64 = 1024 * 1024 * 1024;
        let now = 1_800_000_000;
        let last = Some((now - 30, (Some((50 * GB, 100 * GB)), Some(now - 60))));
        let fresh = assess(last, Some(Duration::from_secs(10)), now).unwrap();
        assert_eq!(fresh.free_bytes, Some(50 * GB));
        assert!(!fresh.disk_unreadable);
        assert_eq!(fresh.read_at, Some(now - 30));
        let hung = assess(last, Some(READING_HUNG_AFTER), now).unwrap();
        assert!(hung.disk_unreadable);
        assert_eq!((hung.free_bytes, hung.total_bytes), (None, None));
        assert!(!hung.low_disk);
        assert_eq!(
            hung.last_backup_at,
            Some(now - 60),
            "the backup age still shows"
        );
        // Hung before the first reading ever landed: still said.
        assert!(
            assess(None, Some(READING_HUNG_AFTER), now)
                .unwrap()
                .disk_unreadable
        );
        assert_eq!(assess(None, Some(Duration::from_secs(1)), now), None);
    }

    /// A day's backup is taken once, named like `backup.sh`'s, and only the
    /// newest daily ones are kept; other prefixes are never pruned.
    /// Review w14-7: on a tight volume the oldest dailies make room for a
    /// new one, down to the floor; with room already, nothing is removed.
    #[test]
    fn the_oldest_dailies_make_room_for_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=KEEP_DAILY {
            std::fs::write(
                dir.path().join(format!("daily-202001{day:02}-000000.db")),
                b"x",
            )
            .unwrap();
        }
        let count = || std::fs::read_dir(dir.path()).unwrap().count() as u64;
        // Each daily holds 10 units of a 140-unit volume that is full.
        let free = || Some((KEEP_DAILY as u64 - count()) * 10);
        assert_eq!(make_room(dir.path(), 0, &free), Some(0));
        assert_eq!(count(), KEEP_DAILY as u64, "it fits: nothing removed");
        assert_eq!(make_room(dir.path(), 25, &free), Some(30));
        assert_eq!(count(), KEEP_DAILY as u64 - 3);
        assert!(!dir.path().join("daily-20200103-000000.db").exists());
        assert!(dir.path().join("daily-20200104-000000.db").exists());
        // Never below the floor, however much is needed.
        let got = make_room(dir.path(), u64::MAX, &free).unwrap();
        assert_eq!(count(), KEEP_DAILY_FLOOR as u64);
        assert!(got < u64::MAX);
        // An unreadable volume: nothing removed, the copy is tried.
        assert_eq!(make_room(dir.path(), u64::MAX, &|| None), None);
    }

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
