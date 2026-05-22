//! `upload_to_session` — stage dropped files on the session's host so their
//! remote path can be pasted into the prompt. Local sessions copy with
//! `std::fs`; remote sessions stream bytes over the ControlMaster
//! (`SshClient::upload_file`). No cleanup (per the design — files accumulate
//! under ~/.claude-fleet/uploads/<session>/).

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

/// Generous timeout — covers an image upload over a slow link.
const UPLOAD_TIMEOUT_SECS: u64 = 60;

#[derive(Deserialize)]
pub struct UploadArgs {
    pub host_alias: String,
    /// The session's tmux name — used as the per-session staging subdir.
    pub session_name: String,
    /// Absolute local paths of the dropped files.
    pub local_paths: Vec<String>,
}

/// Stage `local_paths` under `~/.claude-fleet/uploads/<session>/` on the
/// session's host and return the resulting absolute remote paths, in order.
#[tauri::command]
pub async fn upload_to_session(
    args: UploadArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<String>, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.session_name)?;
    if args.local_paths.is_empty() {
        return Ok(vec![]);
    }

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
    use super::dedupe_names;

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
}
