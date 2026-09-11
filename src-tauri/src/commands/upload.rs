//! `upload_to_session` — stage dropped files on the session's host so their
//! remote path can be pasted into the prompt. Local sessions copy with
//! `std::fs`; remote sessions stream bytes over the ControlMaster
//! (`SshClient::upload_file`). No cleanup (per the design — files accumulate
//! under ~/.claude-fleet/uploads/<session>/).
//!
//! The webview never gets to name an arbitrary local path: only paths the
//! user actually dropped onto the window — recorded Rust-side from the Tauri
//! drag-drop event into [`UploadAllowList`] with a short TTL — are accepted.
//! Anything else is `E_FORBIDDEN` (SEC-9).

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;

/// Generous timeout — covers an image upload over a slow link.
const UPLOAD_TIMEOUT_SECS: u64 = 60;

/// How long a dropped path stays uploadable. A drop is followed by the
/// upload within milliseconds; the window only needs to cover a slow IPC
/// round-trip and a user who drops, then waits for a session to open.
pub const UPLOAD_ALLOW_TTL: Duration = Duration::from_secs(10 * 60);

/// Rust-side record of the paths the OS drag-drop handed the window.
/// Managed in Tauri state as `Arc<UploadAllowList>`; populated from
/// `on_window_event` / `on_webview_event` in `lib.rs`.
#[derive(Default)]
pub struct UploadAllowList {
    entries: Mutex<HashMap<PathBuf, Instant>>,
}

impl UploadAllowList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record freshly dropped paths.
    pub fn allow(&self, paths: &[PathBuf]) {
        self.allow_at(paths, Instant::now());
    }

    pub fn allow_at(&self, paths: &[PathBuf], now: Instant) {
        let mut e = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        e.retain(|_, at| now.saturating_duration_since(*at) < UPLOAD_ALLOW_TTL);
        for p in paths {
            e.insert(p.clone(), now);
        }
    }

    /// True if `path` was dropped within the TTL. Paths compare verbatim —
    /// the frontend echoes back exactly what the drop event delivered.
    pub fn is_allowed(&self, path: &Path) -> bool {
        self.is_allowed_at(path, Instant::now())
    }

    pub fn is_allowed_at(&self, path: &Path, now: Instant) -> bool {
        let e = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        e.get(path)
            .is_some_and(|at| now.saturating_duration_since(*at) < UPLOAD_ALLOW_TTL)
    }
}

#[derive(Deserialize)]
pub struct UploadArgs {
    pub host_alias: String,
    /// The session's tmux name — used as the per-session staging subdir.
    pub session_name: String,
    /// Absolute local paths of the dropped files.
    pub local_paths: Vec<String>,
}

/// Pure gate: every requested path must be on the allow-list.
pub fn check_paths_allowed(allow: &UploadAllowList, paths: &[String]) -> Result<(), IpcError> {
    for p in paths {
        if !allow.is_allowed(Path::new(p)) {
            return Err(IpcError::new(
                "E_FORBIDDEN",
                format!("{p} was not dropped onto the window; only dropped files can be uploaded"),
            ));
        }
    }
    Ok(())
}

/// Stage `local_paths` under `~/.claude-fleet/uploads/<session>/` on the
/// session's host and return the resulting absolute remote paths, in order.
#[tauri::command]
pub async fn upload_to_session(
    args: UploadArgs,
    ssh: State<'_, Arc<SshClient>>,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Vec<String>, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name_addressable(&args.session_name)?;
    if args.local_paths.is_empty() {
        return Ok(vec![]);
    }
    check_paths_allowed(&allow, &args.local_paths)?;

    // Collision-free destination basenames.
    let basenames: Vec<String> = args
        .local_paths
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        })
        .collect();
    let names = dedupe_names(&basenames);

    let timeout = Duration::from_secs(UPLOAD_TIMEOUT_SECS);
    let is_local = args.host_alias == "local";

    // Resolve the staging dir (absolute, so the returned paths are pasteable).
    let home = if is_local {
        std::env::var("HOME").map_err(|_| IpcError::new("E_UPLOAD", "HOME not set"))?
    } else {
        ssh.remote_home(&args.host_alias).await?
    };
    let dir = format!("{home}/.claude-fleet/uploads/{}", args.session_name);

    let mut remote_paths = Vec::with_capacity(names.len());

    if is_local {
        std::fs::create_dir_all(&dir)
            .map_err(|e| IpcError::new("E_UPLOAD", format!("mkdir {dir}: {e}")))?;
        for (src, name) in args.local_paths.iter().zip(&names) {
            let dest = format!("{dir}/{name}");
            std::fs::copy(src, &dest)
                .map_err(|e| IpcError::new("E_UPLOAD", format!("copy {src}: {e}")))?;
            remote_paths.push(dest);
        }
    } else {
        let mkdir = ssh
            .run(&args.host_alias, &["mkdir", "-p", &quote(&dir)], timeout)
            .await?;
        if !mkdir.status.success() {
            return Err(IpcError::new(
                "E_UPLOAD",
                format!(
                    "mkdir on {} failed: {}",
                    args.host_alias,
                    String::from_utf8_lossy(&mkdir.stderr).trim()
                ),
            ));
        }
        for (src, name) in args.local_paths.iter().zip(&names) {
            let dest = format!("{dir}/{name}");
            ssh.upload_file(&args.host_alias, Path::new(src), &dest, timeout)
                .await?;
            remote_paths.push(dest);
        }
    }

    Ok(remote_paths)
}

/// Make a batch of basenames collision-free, preserving order. The first
/// occurrence keeps its name; a later duplicate gets `-1`, `-2`, … inserted
/// before its extension (`a.png` → `a-1.png`; `notes` → `notes-1`).
pub fn dedupe_names(names: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let mut candidate = name.clone();
        let mut n = 1;
        while seen.contains(&candidate) {
            candidate = suffix_name(name, n);
            n += 1;
        }
        seen.insert(candidate.clone());
        out.push(candidate);
    }
    out
}

/// Insert `-{n}` before the final extension (if any). `file_stem`/`extension`
/// semantics: a leading-dot name like `.bashrc` has no extension, so the
/// suffix goes at the end.
fn suffix_name(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{n}.{ext}"),
        _ => format!("{name}-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unique_names() {
        let got = dedupe_names(&["a.png".into(), "b.png".into()]);
        assert_eq!(got, vec!["a.png", "b.png"]);
    }

    #[test]
    fn suffixes_collisions_before_extension() {
        let got = dedupe_names(&["a.png".into(), "a.png".into(), "a.png".into()]);
        assert_eq!(got, vec!["a.png", "a-1.png", "a-2.png"]);
    }

    #[test]
    fn handles_names_without_extension() {
        let got = dedupe_names(&["notes".into(), "notes".into()]);
        assert_eq!(got, vec!["notes", "notes-1"]);
    }

    #[test]
    fn leading_dot_name_has_no_extension() {
        let got = dedupe_names(&[".env".into(), ".env".into()]);
        assert_eq!(got, vec![".env", ".env-1"]);
    }

    #[test]
    fn allow_list_admits_only_dropped_paths_within_ttl() {
        let al = UploadAllowList::new();
        let t0 = Instant::now();
        let dropped = PathBuf::from("/tmp/shot.png");
        assert!(!al.is_allowed_at(&dropped, t0), "nothing dropped yet");
        al.allow_at(std::slice::from_ref(&dropped), t0);
        assert!(al.is_allowed_at(&dropped, t0));
        assert!(al.is_allowed_at(&dropped, t0 + Duration::from_secs(60)));
        assert!(
            !al.is_allowed_at(Path::new("/etc/passwd"), t0),
            "a path the webview names on its own is refused"
        );
        assert!(
            !al.is_allowed_at(&dropped, t0 + UPLOAD_ALLOW_TTL),
            "entries expire after the TTL"
        );
    }

    #[test]
    fn allow_list_prunes_stale_entries_on_insert() {
        let al = UploadAllowList::new();
        let t0 = Instant::now();
        al.allow_at(&[PathBuf::from("/tmp/old.png")], t0);
        al.allow_at(&[PathBuf::from("/tmp/new.png")], t0 + UPLOAD_ALLOW_TTL);
        let e = al.entries.lock().unwrap();
        assert!(!e.contains_key(Path::new("/tmp/old.png")));
        assert!(e.contains_key(Path::new("/tmp/new.png")));
    }

    #[test]
    fn check_paths_allowed_rejects_any_unlisted_path_with_e_forbidden() {
        let al = UploadAllowList::new();
        al.allow(&[PathBuf::from("/tmp/a.png")]);
        assert!(check_paths_allowed(&al, &["/tmp/a.png".into()]).is_ok());
        let err =
            check_paths_allowed(&al, &["/tmp/a.png".into(), "/etc/passwd".into()]).unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
        assert!(check_paths_allowed(&al, &[]).is_ok());
    }
}
