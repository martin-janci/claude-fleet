//! An online, consistent copy of `state.db` (`fleet-hub backup`).
//!
//! The same guarantee as `deploy/hub/backup.sh`, without a `sqlite3` binary on
//! the host: `fleet-updater` takes this copy through `docker exec` before it
//! replaces the hub, and restores it when the candidate migrated and then
//! failed (update-channel design §8.2–§8.3).
//!
//! The source is opened READ-ONLY and never migrated, so a backup taken by
//! any build never changes the database it copies. `VACUUM INTO` reads one
//! consistent snapshot while the hub keeps writing (WAL), and writes a
//! compacted copy with no `-wal` / `-shm` beside it. The copy is written as
//! `<dest>.part` (created `0600` on unix, since it holds every secret in
//! the database), checked with `PRAGMA integrity_check`, synced, and only
//! then renamed into place: a `<dest>` that exists is always a whole, sound
//! backup.

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

/// What a finished backup is, as `fleet-hub backup` prints it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BackupInfo {
    pub path: PathBuf,
    /// The copy's `MAX(schema_version)`: what a restore brings the hub back
    /// to. `None` for a database that was never migrated.
    pub schema: Option<i64>,
    pub bytes: u64,
}

/// Copy the database at `src` to `dest`. Refuses to overwrite `dest`.
pub fn backup_to(src: &Path, dest: &Path) -> Result<BackupInfo, String> {
    if !src.is_file() {
        return Err(format!("no database at {}", src.display()));
    }
    if dest.exists() {
        return Err(format!(
            "{} already exists; a backup never overwrites",
            dest.display()
        ));
    }
    let part = part_path(dest);
    // A leftover from an interrupted run is ours to replace: `.part` is never
    // a finished backup.
    let _ = std::fs::remove_file(&part);
    let part_str = part
        .to_str()
        .ok_or_else(|| format!("backup path {} is not UTF-8", part.display()))?;

    let conn = Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open {} read-only: {e}", src.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    let result = (|| {
        // The copy holds the master token and every stored secret: `0600` from
        // its first byte, as `deploy/hub/backup.sh` writes it. `VACUUM INTO`
        // fills an existing empty file and keeps its mode; without this it would
        // create the file `0666 & ~umask`.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&part)
                .map_err(|e| format!("create {}: {e}", part.display()))?;
        }
        conn.execute("VACUUM INTO ?1", [part_str])
            .map_err(|e| format!("copy {}: {e}", src.display()))?;
        let copy = Connection::open_with_flags(&part, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("open the copy: {e}"))?;
        let check: String = copy
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| format!("integrity_check: {e}"))?;
        if check != "ok" {
            return Err(format!("integrity_check on the copy said: {check}"));
        }
        let schema = schema_of(&copy)?;
        drop(copy);
        // Opened for write, not read: on Windows sync_all is FlushFileBuffers,
        // which refuses a read-only handle with "Access is denied".
        std::fs::OpenOptions::new()
            .write(true)
            .open(&part)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("sync the copy: {e}"))?;
        Ok(schema)
    })();
    let schema = match result {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    std::fs::rename(&part, dest).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("move the copy to {}: {e}", dest.display())
    })?;
    if let Some(dir) = dest.parent() {
        // The rename itself is durable only once the directory is synced.
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(BackupInfo {
        path: dest.to_path_buf(),
        schema,
        bytes,
    })
}

/// `MAX(version)` of `schema_version`, `None` when the table is absent.
fn schema_of(conn: &Connection) -> Result<Option<i64>, String> {
    let has_table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if has_table.is_none() {
        return Ok(None);
    }
    conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEventBus;
    use crate::store::Store;
    use std::sync::Arc;

    #[test]
    fn copies_a_live_database_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let store = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        store.set_setting("backup.test", "kept").unwrap();
        let schema = store.schema_version().unwrap();

        // The writer stays open: the hub is serving while it is backed up.
        let dest = dir.path().join("backups-pre.db");
        let info = backup_to(&db, &dest).unwrap();
        assert_eq!(info.schema, Some(schema));
        assert!(info.bytes > 0);
        assert!(!part_path(&dest).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "a backup holds secrets: {mode:o}");
        }

        let copy = Store::open_read_only(&dest).unwrap();
        assert_eq!(
            copy.get_setting("backup.test").unwrap().as_deref(),
            Some("kept")
        );
        assert_eq!(copy.schema_version().unwrap(), schema);

        // Writes after the backup are not in it.
        store.set_setting("backup.test", "later").unwrap();
        assert_eq!(
            copy.get_setting("backup.test").unwrap().as_deref(),
            Some("kept")
        );
    }

    #[test]
    fn never_overwrites_and_refuses_a_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let _store = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        let dest = dir.path().join("b.db");
        std::fs::write(&dest, b"precious").unwrap();
        let err = backup_to(&db, &dest).unwrap_err();
        assert!(err.contains("never overwrites"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"precious");

        let err = backup_to(&dir.path().join("nope.db"), &dir.path().join("c.db")).unwrap_err();
        assert!(err.contains("no database"), "{err}");
    }

    #[test]
    fn a_stale_part_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let _store = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        let dest = dir.path().join("b.db");
        std::fs::write(part_path(&dest), b"half a backup").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let loose = std::fs::Permissions::from_mode(0o644);
            std::fs::set_permissions(part_path(&dest), loose).unwrap();
        }
        backup_to(&db, &dest).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the stale part's mode is not kept");
        }
        assert!(Store::open_read_only(&dest)
            .unwrap()
            .schema_version()
            .is_ok());
    }
}
