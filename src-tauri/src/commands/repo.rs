//! Shared plumbing for the git-backed Files/History/Branches commands.
//!
//! Every command resolves the session's worktree live: ask tmux for the
//! session pane's cwd, then `git rev-parse --show-toplevel`. This is
//! host-correct for remote sessions. Every interpolated value is shell-quoted
//! (`shell::quote`); frontend paths/refs/hashes are additionally validated.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Per-call SSH timeout for git/file reads.
pub const REPO_TIMEOUT_SECS: u64 = 10;
/// Largest file body returned by `repo_file`. Larger files are truncated.
pub const MAX_FILE_BYTES: usize = 512 * 1024;
/// Largest diff returned by `repo_diff`/`repo_commit_diff`.
pub const MAX_DIFF_BYTES: usize = 1024 * 1024;
/// Largest worktree listing returned by `repo_tree`.
pub const MAX_TREE_ENTRIES: usize = 20_000;

/// Emitted on stderr by `repo_script` when the session's worktree directory no
/// longer exists on disk (e.g. the git worktree was removed while the tmux
/// session lives on). `repo_err` maps this to the `E_NO_WORKTREE` code so the
/// frontend can show a clean "worktree gone" state instead of a raw git error.
pub const NO_WORKTREE_SENTINEL: &str = "__CF_NO_WORKTREE__";

/// Resolve a session id to its `(host_alias, tmux_name)`, validating both.
pub fn session_target(store: &Mutex<Store>, session_id: i64) -> Result<(String, String), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let sess = s
        .get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {session_id} not found")))?;
    crate::validate::host_alias(&sess.host_alias)?;
    crate::validate::tmux_name(&sess.tmux_name)?;
    Ok((sess.host_alias, sess.tmux_name))
}

/// Wrap `body` in a script that first resolves the worktree root into `$root`.
pub fn repo_script(tmux_name: &str, body: &str) -> String {
    format!(
        "set -e\n\
         p=\"$(tmux display-message -t {name} -p '#{{pane_current_path}}')\"\n\
         if [ ! -d \"$p\" ]; then printf '%s\\n' '{sentinel}' >&2; exit 3; fi\n\
         root=\"$(git -C \"$p\" rev-parse --show-toplevel)\"\n\
         {body}",
        name = quote(tmux_name),
        sentinel = NO_WORKTREE_SENTINEL,
    )
}

/// Run a script in the session's repo — locally via `bash -lc`, or remotely
/// via the multiplexed SSH client.
pub async fn run_in_repo(
    ssh: &Arc<SshClient>,
    host: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    if host == "local" {
        tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_REPO", format!("spawn bash: {e}")))
    } else {
        ssh.run(
            host,
            &["bash", "-lc", &quote(script)],
            Duration::from_secs(REPO_TIMEOUT_SECS),
        )
        .await
    }
}

/// Turn a failed `Output` into an `E_REPO` error carrying stderr (or stdout).
pub fn repo_err(out: &std::process::Output) -> IpcError {
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains(NO_WORKTREE_SENTINEL) {
        return IpcError::new("E_NO_WORKTREE", "worktree directory no longer exists");
    }
    let msg = if stderr.trim().is_empty() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        stderr.trim().to_string()
    };
    IpcError::new(
        "E_REPO",
        if msg.is_empty() {
            "git command failed".to_string()
        } else {
            msg
        },
    )
}

/// Post-process raw diff bytes into a `(diff, binary, truncated)` triple,
/// cutting on a UTF-8 boundary near `MAX_DIFF_BYTES`. Shared by `repo_diff`
/// and `repo_commit_diff`.
pub fn diff_from_bytes(raw: &[u8]) -> (String, bool, bool) {
    let text = String::from_utf8_lossy(raw);
    let binary = text.contains("Binary files ") || text.contains("GIT binary patch");
    let truncated = raw.len() > MAX_DIFF_BYTES;
    let diff = if binary {
        String::new()
    } else if truncated {
        let mut end = MAX_DIFF_BYTES;
        while end > 0 && (raw[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        String::from_utf8_lossy(&raw[..end]).into_owned()
    } else {
        text.into_owned()
    };
    (diff, binary, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_script_embeds_quoted_name_and_body() {
        let s = repo_script("dev-foo", "git -C \"$root\" status");
        assert!(s.contains("display-message -t 'dev-foo'"), "got: {s}");
        assert!(s.contains("#{pane_current_path}"), "got: {s}");
        assert!(s.contains("rev-parse --show-toplevel"), "got: {s}");
        assert!(s.trim_end().ends_with("git -C \"$root\" status"));
    }

    #[test]
    fn repo_script_guards_missing_worktree_dir() {
        let s = repo_script("dev-foo", "true");
        // The dir check must run before the git call, and emit the sentinel.
        assert!(s.contains("[ ! -d \"$p\" ]"), "got: {s}");
        assert!(s.contains(NO_WORKTREE_SENTINEL), "got: {s}");
        let guard = s.find("[ ! -d").unwrap();
        let revparse = s.find("rev-parse").unwrap();
        assert!(guard < revparse, "dir guard must precede rev-parse: {s}");
    }

    // `repo_err` ignores the exit status entirely (it only reads stderr/stdout),
    // so any ExitStatus works; build one via the unix extension since
    // `ExitStatus` has no portable public constructor.
    #[cfg(unix)]
    fn output_with_stderr(stderr: &[u8]) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(256), // exit code 1
            stdout: Vec::new(),
            stderr: stderr.to_vec(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn repo_err_maps_sentinel_to_no_worktree() {
        let out = output_with_stderr(format!("{NO_WORKTREE_SENTINEL}\n").as_bytes());
        assert_eq!(repo_err(&out).code, "E_NO_WORKTREE");
    }

    #[cfg(unix)]
    #[test]
    fn repo_err_keeps_generic_repo_errors() {
        let out = output_with_stderr(b"fatal: not a git repository\n");
        let e = repo_err(&out);
        assert_eq!(e.code, "E_REPO");
        assert!(e.message.contains("not a git repository"));
    }

    #[test]
    fn diff_from_bytes_flags_binary() {
        let (d, bin, trunc) = diff_from_bytes(b"Binary files a/x and b/x differ\n");
        assert!(bin);
        assert!(!trunc);
        assert_eq!(d, "");
    }

    #[test]
    fn diff_from_bytes_passes_text_through() {
        let (d, bin, trunc) = diff_from_bytes(b"@@ -1 +1 @@\n-a\n+b\n");
        assert!(!bin);
        assert!(!trunc);
        assert!(d.contains("+b"));
    }
}
