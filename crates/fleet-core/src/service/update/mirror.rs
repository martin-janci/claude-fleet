//! The hub-served artifact mirror (update-channel design §6.4, slice S9).
//!
//! With `update.mirror` on, the hub serves the release files its verified
//! manifests list — agent and hub tarballs, desktop bundles, the phone's APK
//! — at `/update/artifact/<sha256>`, for targets that cannot reach GitHub.
//! A file is fetched from its release URL the first time a target asks,
//! checked against the sha256 and size in the signed manifest, kept under
//! `<data_dir>/update-mirror/<sha256>`, and dropped once no kept manifest
//! lists it. A target still checks the sha256 itself: the mirror is trusted
//! for nothing.
//!
//! Container images are not mirrored: they are pulled by digest from the
//! registry, which is its own mirror story.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fleet_update::verify::sha256_hex;
use fleet_update::{Fetch, Track, TrustedKeys};

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::store::Store;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// The hub's data dir, as `init` was given it (the hub's update trigger).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The data dir `init` was given, when it has run.
pub fn data_dir() -> Option<&'static Path> {
    DATA_DIR.get().map(PathBuf::as_path)
}

/// One fetch per file at a time: a second request for the same sha waits
/// for the first rather than downloading it again.
static FETCHING: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

/// The path a decision names for `sha256` (relative to the hub's base URL).
pub fn path_for(sha256: &str) -> String {
    format!("/update/artifact/{sha256}")
}

/// Make `<data_dir>/update-mirror` (0700) the mirror's directory. Once per
/// process; a second call keeps the first.
pub fn init(data_dir: &Path) -> std::io::Result<PathBuf> {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    let dir = data_dir.join("update-mirror");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(DIR.get_or_init(|| dir).clone())
}

fn dir() -> Result<&'static Path, IpcError> {
    DIR.get().map(PathBuf::as_path).ok_or_else(|| {
        IpcError::new(
            codes::E_UNSUPPORTED,
            "the update mirror is not set up on this machine",
        )
    })
}

/// Whether the operator turned the mirror on, and this process can serve it.
pub fn enabled(store: &Store) -> bool {
    DIR.get().is_some() && settings::get_bool(store, settings::UPDATE_MIRROR)
}

fn is_sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Where a mirrored file comes from: the URL and size its signed manifest
/// gives for `sha256`, from any release the cached channel of `track` lists.
pub fn source_of(
    store: &Store,
    track: Track,
    keys: &TrustedKeys,
    sha256: &str,
    now: i64,
) -> Option<(String, u64)> {
    let cached = super::load_cached(store, track, keys, now)?;
    for m in cached.manifests.verified.values() {
        for c in m.components.values() {
            for a in &c.artifacts {
                if let Some((sha, size)) = a.content() {
                    if sha == sha256 {
                        return a.url(&m.release.assets_base).map(|u| (u, size));
                    }
                }
            }
        }
    }
    None
}

/// Every sha256 the cached manifests list: what the mirror keeps.
pub fn wanted(store: &Store, track: Track, keys: &TrustedKeys, now: i64) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(cached) = super::load_cached(store, track, keys, now) {
        for m in cached.manifests.verified.values() {
            for c in m.components.values() {
                for a in &c.artifacts {
                    if let Some((sha, _)) = a.content() {
                        out.insert(sha.to_string());
                    }
                }
            }
        }
    }
    out
}

/// The local copy of `sha256`, fetching it first when it is not there yet.
/// `E_NOTFOUND` for a sha no cached manifest lists (or the mirror off);
/// `E_UPDATE_UNVERIFIED` when the fetched bytes are not what the manifest
/// signed.
pub async fn local_copy(
    store: &Mutex<Store>,
    fetch: &dyn Fetch,
    keys: &TrustedKeys,
    sha256: &str,
    now: i64,
) -> Result<PathBuf, IpcError> {
    let not_found = || IpcError::new(codes::E_NOTFOUND, "no such artifact");
    if !is_sha(sha256) {
        return Err(not_found());
    }
    let (url, size) = {
        let s = lock(store)?;
        if !enabled(&s) {
            return Err(not_found());
        }
        source_of(&s, super::track(&s), keys, sha256, now).ok_or_else(not_found)?
    };
    let path = dir()?.join(sha256);
    if path.is_file() {
        return Ok(path);
    }
    let _one = FETCHING
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    if path.is_file() {
        return Ok(path);
    }
    let bytes = fetch
        .get(&url, size.max(1))
        .await
        .map_err(super::update_err)?;
    let got = sha256_hex(&bytes);
    if got != sha256 || bytes.len() as u64 != size {
        return Err(IpcError::new(
            codes::E_UPDATE_UNVERIFIED,
            format!(
                "{url}: {} bytes with sha256 {got}, the manifest signed {size} bytes with {sha256}",
                bytes.len()
            ),
        ));
    }
    let part = path.with_extension("part");
    tokio::fs::write(&part, &bytes)
        .await
        .and_then(|()| std::fs::rename(&part, &path))
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("cannot keep the artifact: {e}")))?;
    tracing::info!(sha256, size, "[update] mirrored a release artifact");
    Ok(path)
}

/// Drop every mirrored file no cached manifest lists any more. Returns how
/// many went.
pub fn prune(store: &Store, keys: &TrustedKeys, now: i64) -> usize {
    // With the mirror off nothing is served or fetched, and what is kept
    // waits for it to come back on.
    if !enabled(store) {
        return 0;
    }
    let Ok(dir) = dir() else { return 0 };
    let keep = wanted(store, super::track(store), keys, now);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let sha = name.trim_end_matches(".part");
        if !keep.contains(sha) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}
