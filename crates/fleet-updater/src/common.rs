//! What every updater target shares: the install rule, the release keys, the
//! replay guard, the pairing with a hub, and the small helpers the loops use.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use fleet_update::{Decision, Mode, ReasonCode, SequenceStore, Status, Track, TrustedKeys};

use crate::http;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

/// The release keys: the compiled-in ones, plus `FLEET_UPDATE_E2E_KEYS` in an
/// `e2e` build only (as the hub's `trusted_keys`).
pub fn trusted_keys() -> TrustedKeys {
    #[allow(unused_mut)]
    let mut keys: Vec<String> = fleet_update::keys::RELEASE_KEYS
        .iter()
        .map(|k| k.to_string())
        .collect();
    #[cfg(feature = "e2e")]
    if let Ok(extra) = std::env::var("FLEET_UPDATE_E2E_KEYS") {
        keys.extend(
            extra
                .split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(String::from),
        );
    }
    TrustedKeys::from_base64(keys.iter().map(String::as_str))
        .unwrap_or_else(|_| fleet_update::keys::release_keys())
}

/// The replay guard, kept beside the state file so a restart does not forget
/// the newest channel it has seen.
pub struct FileSequences {
    path: PathBuf,
    seen: Mutex<BTreeMap<Track, u64>>,
}

impl FileSequences {
    pub fn open(dir: &Path) -> FileSequences {
        let path = dir.join("sequences.json");
        let seen = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        FileSequences {
            path,
            seen: Mutex::new(seen),
        }
    }
}

impl SequenceStore for FileSequences {
    fn seen(&self, track: Track) -> u64 {
        let m = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&track).copied().unwrap_or(0)
    }

    fn record(&self, track: Track, sequence: u64) {
        let mut m = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let e = m.entry(track).or_insert(0);
        if sequence > *e {
            *e = sequence;
            let tmp = self.path.with_extension("json.tmp");
            if let Ok(b) = serde_json::to_vec(&*m) {
                if std::fs::write(&tmp, b).is_ok() {
                    let _ = std::fs::rename(&tmp, &self.path);
                }
            }
        }
    }
}

/// The file the hub's `update_admin update_now` writes in its data dir.
pub const UPDATE_NOW_FILE: &str = "update-now";

/// Sleep `wait`, or less: return as soon as `trigger` appears (it is
/// removed, so it fires once). `true` when it was the trigger.
pub async fn sleep_or_poked(trigger: &Path, wait: Duration) -> bool {
    let end = tokio::time::Instant::now() + wait;
    loop {
        if take_trigger(trigger) {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= end {
            return false;
        }
        tokio::time::sleep((end - now).min(Duration::from_secs(5))).await;
    }
}

/// Remove `trigger` if it is there; `true` when it was.
pub fn take_trigger(trigger: &Path) -> bool {
    trigger.exists() && std::fs::remove_file(trigger).is_ok()
}

/// The code out of what `fleet-hub pair` printed: the URL (`…/pair#CODE`)
/// or the bare code.
pub fn pairing_code(arg: &str) -> Result<String, String> {
    let code = arg
        .trim()
        .rsplit_once('#')
        .map_or(arg.trim(), |(_, c)| c)
        .trim();
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(format!("{arg:?} is not a pairing URL or code"));
    }
    Ok(code.to_string())
}

/// Redeem a `fleet-hub pair --mode updater` code at `hub_url`'s `/pair`
/// (outside the bearer token and the Host allowlist by design: the code is
/// the credential). Returns the token and the name it was minted for.
pub async fn redeem(hub_url: &str, arg: &str) -> Result<(String, String), String> {
    let code = pairing_code(arg)?;
    let base = http::Url::parse(hub_url)?;
    let client = http::Client::new(base.tls, Duration::from_secs(30))?;
    let body = serde_json::json!({ "code": code }).to_string();
    let r = client
        .send(
            "POST",
            &format!("{}/pair", hub_url.trim_end_matches('/')),
            &[("Content-Type", "application/json")],
            Some(body.as_bytes()),
            1 << 16,
        )
        .await?;
    if !r.ok() {
        return Err(format!(
            "the hub refused the code (HTTP {}): {} — codes work once and expire; mint another",
            r.status,
            r.text().trim()
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&r.body).map_err(|e| format!("pairing answer: {e}"))?;
    let mode = v["mode"].as_str().unwrap_or_default();
    if mode != "updater" {
        return Err(format!(
            "that code paired a `{mode}` client, not an updater; revoke it (fleet-hub client revoke) \
             and mint one with --mode updater"
        ));
    }
    let token = v["token"]
        .as_str()
        .ok_or("the pairing answer has no token")?
        .to_string();
    Ok((token, v["name"].as_str().unwrap_or("?").to_string()))
}

/// Write `token` to `dir/<name>`, readable by its owner only.
pub fn save_token(dir: &Path, name: &str, token: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(token.as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
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
pub fn prune_backups(dir: &Path, keep: usize, just_made: &Path) {
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
pub fn known(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s != "unknown" && s != "local").then(|| s.to_string())
}

/// `[A-Za-z0-9._-]` only (a semver's `+build` included).
pub fn safe(s: &str) -> String {
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

pub fn stamp() -> String {
    fleet_update::time::format_rfc3339(now_unix()).replace([':', '-'], "")
}

pub fn attempt_id() -> String {
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
mod tests {
    use super::*;

    #[test]
    fn pairing_codes() {
        assert_eq!(
            pairing_code("https://fleet.example.com/pair#ABCD1234").unwrap(),
            "ABCD1234"
        );
        assert_eq!(pairing_code(" ABCD1234\n").unwrap(), "ABCD1234");
        assert!(pairing_code("https://fleet.example.com/pair#").is_err());
        assert!(pairing_code("x&y").is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_poke_cuts_the_wait_short_and_fires_once() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join(UPDATE_NOW_FILE);
        assert!(!sleep_or_poked(&t, Duration::from_secs(30)).await);
        std::fs::write(&t, b"").unwrap();
        assert!(sleep_or_poked(&t, Duration::from_secs(3600)).await);
        assert!(!t.exists());
    }

    #[test]
    fn a_token_is_kept_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = save_token(d.path(), "token", "tok").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "tok");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
