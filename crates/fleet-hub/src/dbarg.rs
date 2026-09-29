//! The `--db` argument the offline commands (`census`, `decide`, `decide
//! bench`) share: a database file named on the command line, else the hub's
//! own `state.db`, opened read-only and unmigrated.

use crate::config::{self, HubOptions};
use crate::serve;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `--db`, else the hub's own `state.db` (which must exist).
pub(crate) fn db_path(
    db: Option<&Path>,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    match db {
        Some(p) if p.is_file() => Ok(p.to_path_buf()),
        Some(p) => Err(format!("no database at {}", p.display())),
        None => serve::existing_db(&config::resolve_data_dir(opts, env)),
    }
}

/// [`Store::open_read_only`], with the path in the error.
pub(crate) fn open_read_only(path: &Path) -> Result<Store, String> {
    Store::open_read_only(path).map_err(|e| format!("open {} read-only: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_db_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.db");
        std::fs::write(&p, b"").unwrap();
        let got = db_path(Some(&p), &HubOptions::default(), &HashMap::new()).unwrap();
        assert_eq!(got, p);
    }

    #[test]
    fn a_missing_db_is_named() {
        let e = db_path(
            Some(Path::new("/nonexistent/state.db")),
            &HubOptions::default(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(e.contains("no database at /nonexistent/state.db"), "{e}");
    }

    #[test]
    fn no_db_falls_back_to_the_hubs_own() {
        let dir = tempfile::tempdir().unwrap();
        let env = HashMap::from([(
            "FLEET_HUB_DATA_DIR".to_string(),
            dir.path().display().to_string(),
        )]);
        let opts = HubOptions::default();
        let e = db_path(None, &opts, &env).unwrap_err();
        assert!(e.contains("no hub database at"), "{e}");
        let p = dir.path().join("state.db");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(db_path(None, &opts, &env).unwrap(), p);
    }
}
