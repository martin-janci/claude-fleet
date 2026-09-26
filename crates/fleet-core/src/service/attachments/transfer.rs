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

/// The last path component of each input, as a name that cannot escape a
/// directory. `Path::file_name` alone is not enough: it answers `None` for
/// `..`, and on Unix a `\` is an ordinary character, so a Windows-style path
/// arrives as one long "name". Anything with no usable component left
/// becomes an empty string, which every caller must treat as a refusal.
pub fn basenames_of(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            let last = p.rsplit(['/', '\\']).next().unwrap_or("");
            if last == "." || last == ".." {
                ""
            } else {
                last
            }
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
/// (`SshClient::upload_file`). Every name must be non-empty: names from
/// `basenames_of` are empty when the input has no usable path component, and
/// those are refused with an error rather than silently creating a directory path.
pub async fn transfer_all(
    ssh: &Arc<SshClient>,
    host: &str,
    local_paths: &[String],
    names: &[String],
    dir: &str,
    timeout: Duration,
) -> Result<Vec<String>, IpcError> {
    // Enforce the invariant: every name must be non-empty. An empty name from
    // `basenames_of` indicates the input had no usable path component (e.g. "." or "..").
    if let Some(empty_idx) = names.iter().position(|n| n.is_empty()) {
        return Err(IpcError::new(
            codes::E_UPLOAD,
            format!(
                "file {} has no usable name (possibly a path component like . or ..)",
                empty_idx
            ),
        ));
    }
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

    #[test]
    fn a_filename_can_never_be_a_path() {
        // Every hostile input must reduce to the exact right basename, or empty.
        // Weaker assertions (not-slash, not-.., not-.) would pass even if the
        // hardening were reverted, so the test would fail to catch regressions.
        let cases = vec![
            ("../../.ssh/authorized_keys", "authorized_keys"),
            ("/etc/passwd", "passwd"),
            ("a/b/c.txt", "c.txt"),
            (r"..\..\windows\system32", "system32"),
        ];
        for (hostile, expected) in cases {
            let got = basenames_of(&[hostile.to_string()])[0].clone();
            assert_eq!(
                got, expected,
                "{hostile} should reduce to {expected}, got {got}"
            );
        }
        // . and .. must become empty, not a default like "file"
        assert!(
            basenames_of(&["..".to_string()])[0].is_empty(),
            ".. should reduce to empty"
        );
        assert!(
            basenames_of(&[".".to_string()])[0].is_empty(),
            ". should reduce to empty"
        );
        assert!(
            basenames_of(&["///".to_string()])[0].is_empty(),
            "/// should reduce to empty"
        );
    }

    #[test]
    fn a_name_with_nothing_usable_in_it_reduces_to_empty() {
        // The route turns this into 400 rather than inventing a name.
        assert!(basenames_of(&["".to_string()]).pop().unwrap().is_empty());
    }
}
