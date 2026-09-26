//! Staging a file on the session's host: naming it, finding the worktree, and
//! moving the bytes.
//!
//! This was `src-tauri/src/commands/upload.rs` until the hub needed it too.
//! The desktop reaches it through the `upload_attachments` Tauri command; the
//! hub reaches it through `POST /attachment`. Both do the same three things in
//! the same order — resolve the worktree root, stage the directory, transfer —
//! so the order lives here rather than twice.

use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// How long any one upload step may take.
pub const UPLOAD_TIMEOUT_SECS: u64 = 60;

/// Basenames of `paths`, in order — not yet collision-free (see
/// `dedupe_names`). Shared by `upload_to_session` and `upload_attachments`.
pub fn basenames_of(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        })
        .collect()
}

/// Run `script` on `host` (local via `bash -lc`, remote over the
/// ControlMaster — `crate::ssh::run_shell` picks the branch) and map a
/// non-zero exit to `E_UPLOAD` with the host's stderr.
pub async fn run_script(
    ssh: &Arc<SshClient>,
    host: &str,
    script: &str,
    timeout: Duration,
) -> Result<std::process::Output, IpcError> {
    let out = crate::ssh::run_shell(ssh.as_ref(), host, script, timeout).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(IpcError::new(
            codes::E_UPLOAD,
            format!("on {host}: {detail}"),
        ));
    }
    Ok(out)
}

/// Resolve the session's worktree root: ask tmux for the pane's cwd, then
/// git for the toplevel — the same live resolution every Files-tab read
/// already does (`crate::service::repo::repo_script`). `SessionRow`
/// only carries `worktree_id`/`worktree_key` (an id and a name, not a path),
/// so there is nothing to resolve this from except the live pane.
pub async fn resolve_worktree_root(
    ssh: &Arc<SshClient>,
    host: &str,
    session_name: &str,
    timeout: Duration,
) -> Result<String, IpcError> {
    let script = crate::service::attachments::root_script(session_name);
    let out = run_script(ssh, host, &script, timeout).await?;
    last_nonempty_line(&String::from_utf8_lossy(&out.stdout), host)
}

/// The worktree root is whichever line the script printed LAST — a login
/// shell's `bash -lc` can prepend banner/profile output ahead of the real
/// `git rev-parse` line — and must not be empty: building a path from
/// nothing would silently point at `/`.
pub fn last_nonempty_line(stdout: &str, host: &str) -> Result<String, IpcError> {
    let root = stdout.lines().next_back().unwrap_or("").trim();
    if root.is_empty() {
        return Err(IpcError::new(
            codes::E_UPLOAD,
            format!("on {host}: resolving the worktree root produced no output"),
        ));
    }
    Ok(root.to_string())
}

/// Copy `local_paths` (named `names`, in order) into `dir` on `host` and
/// return their absolute destination paths, in order. The one local-vs-remote
/// branch `upload_to_session` and `upload_attachments` share: a local session
/// copies with `std::fs`, a remote one streams over the ControlMaster
/// (`SshClient::upload_file`).
pub async fn transfer_all(
    ssh: &Arc<SshClient>,
    host: &str,
    local_paths: &[String],
    names: &[String],
    dir: &str,
    timeout: Duration,
) -> Result<Vec<String>, IpcError> {
    let mut remote_paths = Vec::with_capacity(names.len());

    if host == "local" {
        std::fs::create_dir_all(dir)
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("mkdir {dir}: {e}")))?;
        for (src, name) in local_paths.iter().zip(names) {
            let dest = format!("{dir}/{name}");
            std::fs::copy(src, &dest)
                .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("copy {src}: {e}")))?;
            remote_paths.push(dest);
        }
    } else {
        let mkdir = ssh
            .run(host, &["mkdir", "-p", &quote(dir)], timeout)
            .await?;
        if !mkdir.status.success() {
            return Err(IpcError::new(
                codes::E_UPLOAD,
                format!(
                    "mkdir on {host} failed: {}",
                    String::from_utf8_lossy(&mkdir.stderr).trim()
                ),
            ));
        }
        for (src, name) in local_paths.iter().zip(names) {
            let dest = format!("{dir}/{name}");
            ssh.upload_file(host, Path::new(src), &dest, timeout)
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
    fn resolve_worktree_root_takes_the_last_line() {
        // A chatty `bash -lc` login profile can print banner output before
        // the real `git rev-parse` line — the root is whichever line came
        // LAST, not the whole trimmed blob.
        let got = last_nonempty_line("Welcome to bash!\n/home/user/worktree\n", "host").unwrap();
        assert_eq!(got, "/home/user/worktree");
    }

    #[test]
    fn resolve_worktree_root_refuses_empty_output() {
        let err = last_nonempty_line("   \n\n", "myhost").unwrap_err();
        assert_eq!(err.code, codes::E_UPLOAD);
        assert!(
            err.message.contains("myhost"),
            "the error should name the host: {}",
            err.message
        );
    }

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
