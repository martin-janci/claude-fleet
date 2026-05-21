//! Tauri commands for the Files & Diff viewer (iter 5).
//!
//! Each command takes a `session_id`, looks up the session's host + tmux
//! name, and runs `git` in the session's worktree — locally or over SSH. The
//! worktree root is resolved live at call time: we ask tmux for the session
//! pane's current path, then `git rev-parse --show-toplevel` from there. This
//! is host-correct for remote sessions (the DB's `projects.base_path` is a
//! *local* scan path and would be wrong on another machine).
//!
//! Every value interpolated into a shell script is quoted (`shell::quote`),
//! and the frontend-supplied file path is additionally validated
//! (`validate::repo_rel_path`) so it cannot escape the worktree.

use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

/// Per-call SSH timeout for git/file reads.
const REPO_TIMEOUT_SECS: u64 = 10;
/// Largest file body returned by `repo_file`. Larger files are truncated.
const MAX_FILE_BYTES: usize = 512 * 1024;
/// Largest diff returned by `repo_diff`. Larger diffs are truncated.
const MAX_DIFF_BYTES: usize = 1024 * 1024;
/// Largest worktree listing returned by `repo_tree`.
const MAX_TREE_ENTRIES: usize = 20_000;

// ─── wire types ───────────────────────────────────────────────────────────

/// One entry in `git status` for a session's worktree.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    /// Friendly status: modified / added / deleted / renamed / copied /
    /// untracked / conflict.
    pub status: String,
    /// Whether the index (staged side) carries a change for this file.
    pub staged: bool,
    /// For renames/copies, the path the file came from.
    pub orig_path: Option<String>,
}

/// Flat worktree listing — tracked files plus untracked, gitignore respected.
#[derive(Debug, Clone, Serialize)]
pub struct RepoTree {
    pub entries: Vec<String>,
    pub truncated: bool,
}

/// The content of one worktree file.
#[derive(Debug, Clone, Serialize)]
pub struct FileContent {
    pub path: String,
    /// Empty when `binary` is true.
    pub content: String,
    pub truncated: bool,
    pub binary: bool,
    /// Byte size when fully read; `None` when the file was truncated.
    pub size: Option<u64>,
}

/// A unified diff for one worktree file.
#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub path: String,
    /// Empty when `binary` is true.
    pub diff: String,
    pub binary: bool,
    pub truncated: bool,
}

// ─── shared helpers ───────────────────────────────────────────────────────

/// Resolve a session id to its `(host_alias, tmux_name)`, validating both.
fn session_target(store: &Mutex<Store>, session_id: i64) -> Result<(String, String), IpcError> {
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

/// Wrap `body` in a script that first resolves the worktree root. `body` may
/// reference `"$root"` (the absolute worktree root). `tmux display-message`
/// reads the session pane's cwd; `git rev-parse --show-toplevel` walks up to
/// the repo root. `set -e` aborts (→ non-zero exit → `E_REPO`) if the session
/// is gone or the path isn't inside a git repo.
fn repo_script(tmux_name: &str, body: &str) -> String {
    format!(
        "set -e\n\
         p=\"$(tmux display-message -t {name} -p '#{{pane_current_path}}')\"\n\
         root=\"$(git -C \"$p\" rev-parse --show-toplevel)\"\n\
         {body}",
        name = shq(tmux_name),
    )
}

/// Run a script in the session's repo — locally via `bash -lc`, or remotely
/// via the multiplexed SSH client. Remote scripts are quoted as one word (the
/// ssh argv-join rule, see `tmux::RemoteTmux::remote_bash`).
async fn run_in_repo(
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
            &["bash", "-lc", &shq(script)],
            Duration::from_secs(REPO_TIMEOUT_SECS),
        )
        .await
    }
}

/// Turn a failed `Output` into an `E_REPO` error carrying stderr (or stdout).
fn repo_err(out: &std::process::Output) -> IpcError {
    let stderr = String::from_utf8_lossy(&out.stderr);
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

/// Parse `git status --porcelain=v1 -z` output. Entries are NUL-separated;
/// a rename/copy entry is followed by a second token (the original path).
fn parse_status_z(raw: &[u8]) -> Vec<ChangedFile> {
    let text = String::from_utf8_lossy(raw);
    let tokens: Vec<&str> = text.split('\0').filter(|t| !t.is_empty()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        // `XY␠path` — need at least the 2 status chars + space + 1 char.
        let mut chars = tok.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        let path = tok.get(3..).unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }
        let (status, staged) = classify(x, y);
        let orig_path = if (x == 'R' || x == 'C') && i < tokens.len() {
            let orig = tokens[i].to_string();
            i += 1;
            Some(orig)
        } else {
            None
        };
        out.push(ChangedFile {
            path,
            status: status.to_string(),
            staged,
            orig_path,
        });
    }
    out
}

/// Map a porcelain XY status pair to a friendly label + staged flag.
fn classify(x: char, y: char) -> (&'static str, bool) {
    if x == '?' && y == '?' {
        return ("untracked", false);
    }
    let unmerged = x == 'U' || y == 'U' || matches!((x, y), ('D', 'D') | ('A', 'A'));
    if unmerged {
        return ("conflict", false);
    }
    let staged = x != ' ' && x != '?';
    let status = if x == 'R' {
        "renamed"
    } else if x == 'C' {
        "copied"
    } else if x == 'D' || y == 'D' {
        "deleted"
    } else if x == 'A' || y == 'A' {
        "added"
    } else {
        "modified"
    };
    (status, staged)
}

// ─── commands ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SessionIdArgs {
    pub session_id: i64,
}

/// `git status` for a session's worktree.
#[tauri::command]
pub async fn repo_changes(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<ChangedFile>, IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let script = repo_script(
        &name,
        "git -C \"$root\" status --porcelain=v1 -z --untracked-files=all",
    );
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(parse_status_z(&out.stdout))
}

/// Flat worktree listing (tracked + untracked, gitignore respected).
#[tauri::command]
pub async fn repo_tree(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepoTree, IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let script = repo_script(
        &name,
        "git -C \"$root\" ls-files -z --cached --others --exclude-standard",
    );
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries: Vec<String> = text
        .split('\0')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    entries.sort();
    entries.dedup();
    let truncated = entries.len() > MAX_TREE_ENTRIES;
    if truncated {
        entries.truncate(MAX_TREE_ENTRIES);
    }
    Ok(RepoTree { entries, truncated })
}

#[derive(Deserialize)]
pub struct RepoFileArgs {
    pub session_id: i64,
    pub path: String,
}

/// Read one worktree file's content (capped at `MAX_FILE_BYTES`).
#[tauri::command]
pub async fn repo_file(
    args: RepoFileArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileContent, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(&store, args.session_id)?;
    // Read one byte past the cap so we can tell "exactly cap" from "truncated".
    let body = format!(
        "head -c {} -- \"$root\"/{}",
        MAX_FILE_BYTES + 1,
        shq(&args.path),
    );
    let script = repo_script(&name, &body);
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    let bytes = out.stdout;
    let truncated = bytes.len() > MAX_FILE_BYTES;
    let view = &bytes[..bytes.len().min(MAX_FILE_BYTES)];
    let binary = view.contains(&0u8);
    Ok(FileContent {
        path: args.path,
        content: if binary {
            String::new()
        } else {
            String::from_utf8_lossy(view).into_owned()
        },
        truncated,
        binary,
        size: if truncated {
            None
        } else {
            Some(bytes.len() as u64)
        },
    })
}

/// Unified diff for one worktree file. Tracked changes diff against `HEAD`;
/// an untracked file falls back to `git diff --no-index` against `/dev/null`
/// so it still renders as an all-added diff.
#[tauri::command]
pub async fn repo_diff(
    args: RepoFileArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileDiff, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let quoted = shq(&args.path);

    // Tracked diff vs HEAD.
    let script = repo_script(&name, &format!("git -C \"$root\" diff HEAD -- {quoted}"));
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    let mut raw = out.stdout;

    // Empty diff + an untracked file → show it as all-added via --no-index.
    if raw.iter().all(|b| b.is_ascii_whitespace()) {
        let body = format!("git -C \"$root\" diff --no-index -- /dev/null {quoted} || true");
        let script = repo_script(&name, &body);
        let fallback = run_in_repo(&ssh, &host, &script).await?;
        if fallback.status.success() {
            raw = fallback.stdout;
        }
    }

    let text = String::from_utf8_lossy(&raw);
    let binary = text.contains("Binary files ") || text.contains("GIT binary patch");
    let truncated = raw.len() > MAX_DIFF_BYTES;
    let diff = if binary {
        String::new()
    } else if truncated {
        // Cut on a UTF-8 boundary near the cap: back off any continuation
        // bytes (0b10xx_xxxx) so we don't split a multi-byte char.
        let mut end = MAX_DIFF_BYTES;
        while end > 0 && (raw[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        String::from_utf8_lossy(&raw[..end]).into_owned()
    } else {
        text.into_owned()
    };
    Ok(FileDiff {
        path: args.path,
        diff,
        binary,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_common_states() {
        assert_eq!(classify('?', '?'), ("untracked", false));
        assert_eq!(classify(' ', 'M'), ("modified", false));
        assert_eq!(classify('M', ' '), ("modified", true));
        assert_eq!(classify('A', ' '), ("added", true));
        assert_eq!(classify(' ', 'D'), ("deleted", false));
        assert_eq!(classify('R', ' '), ("renamed", true));
        assert_eq!(classify('C', ' '), ("copied", true));
        assert_eq!(classify('U', 'U'), ("conflict", false));
        assert_eq!(classify('D', 'D'), ("conflict", false));
        assert_eq!(classify('A', 'U'), ("conflict", false));
    }

    #[test]
    fn parse_status_z_plain_entries() {
        // ` M src/a.ts` and `?? new.txt`, NUL-separated.
        let raw = b" M src/a.ts\0?? new.txt\0";
        let files = parse_status_z(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.ts");
        assert_eq!(files[0].status, "modified");
        assert!(!files[0].staged);
        assert_eq!(files[1].path, "new.txt");
        assert_eq!(files[1].status, "untracked");
    }

    #[test]
    fn parse_status_z_rename_consumes_orig_path() {
        // A rename: `R  new.ts` then the original path as the next token.
        let raw = b"R  new.ts\0old.ts\0 M other.ts\0";
        let files = parse_status_z(raw);
        assert_eq!(files.len(), 2, "rename + its orig must be one entry");
        assert_eq!(files[0].path, "new.ts");
        assert_eq!(files[0].status, "renamed");
        assert_eq!(files[0].orig_path.as_deref(), Some("old.ts"));
        assert_eq!(files[1].path, "other.ts");
        assert_eq!(files[1].orig_path, None);
    }

    #[test]
    fn parse_status_z_empty_input() {
        assert!(parse_status_z(b"").is_empty());
        assert!(parse_status_z(b"\0\0").is_empty());
    }

    #[test]
    fn repo_script_embeds_quoted_name_and_body() {
        let s = repo_script("dev-foo", "git -C \"$root\" status");
        assert!(s.contains("display-message -t 'dev-foo'"), "got: {s}");
        assert!(s.contains("#{pane_current_path}"), "got: {s}");
        assert!(s.contains("rev-parse --show-toplevel"), "got: {s}");
        assert!(s.trim_end().ends_with("git -C \"$root\" status"));
    }
}
