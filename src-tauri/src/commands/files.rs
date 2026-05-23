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

use crate::commands::repo::{
    diff_from_bytes, repo_err, repo_script, run_in_repo, session_target, MAX_FILE_BYTES,
    MAX_TREE_ENTRIES,
};
use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

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
    /// True when `path` is a directory (e.g. an embedded git repo) rather
    /// than a file; `content` is empty.
    pub is_dir: bool,
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
pub fn classify(x: char, y: char) -> (&'static str, bool) {
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

/// `git status` for a session's worktree — reusable logic called by the Tauri
/// command and (later) MCP tools.
pub async fn repo_changes_impl(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<ChangedFile>, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let script = repo_script(
        &name,
        "git -C \"$root\" status --porcelain=v1 -z --untracked-files=all",
    );
    let out = run_in_repo(ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(parse_status_z(&out.stdout))
}

/// `git status` for a session's worktree.
#[tauri::command]
pub async fn repo_changes(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<ChangedFile>, IpcError> {
    repo_changes_impl(args, &store, &ssh).await
}

/// Flat worktree listing (tracked + untracked, gitignore respected) — reusable
/// logic called by the Tauri command and (later) MCP tools.
pub async fn repo_tree_impl(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RepoTree, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let script = repo_script(
        &name,
        "git -C \"$root\" ls-files -z --cached --others --exclude-standard",
    );
    let out = run_in_repo(ssh, &host, &script).await?;
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

/// Flat worktree listing (tracked + untracked, gitignore respected).
#[tauri::command]
pub async fn repo_tree(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepoTree, IpcError> {
    repo_tree_impl(args, &store, &ssh).await
}

#[derive(Deserialize)]
pub struct RepoFileArgs {
    pub session_id: i64,
    pub path: String,
}

/// Read one worktree file's content (capped at `MAX_FILE_BYTES`) — reusable
/// logic called by the Tauri command and (later) MCP tools.
pub async fn repo_file_impl(
    args: RepoFileArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<FileContent, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(store, args.session_id)?;
    // `git ls-files --others` reports an embedded git repo (e.g. a nested
    // worktree) as a single directory entry; running `head` on it would leak
    // a raw "Is a directory" error. Detect that first and flag it as a
    // directory so the viewer can show a calm message.
    // Read one byte past the cap so we can tell "exactly cap" from "truncated".
    let body = format!(
        "f=\"$root\"/{path}\n\
         if [ -d \"$f\" ]; then echo cf-is-dir >&2; exit 9; fi\n\
         head -c {cap} -- \"$f\"",
        path = quote(&args.path),
        cap = MAX_FILE_BYTES + 1,
    );
    let script = repo_script(&name, &body);
    let out = run_in_repo(ssh, &host, &script).await?;
    if !out.status.success() {
        if String::from_utf8_lossy(&out.stderr).contains("cf-is-dir") {
            return Ok(FileContent {
                path: args.path,
                content: String::new(),
                truncated: false,
                binary: false,
                is_dir: true,
                size: None,
            });
        }
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
        is_dir: false,
        size: if truncated {
            None
        } else {
            Some(bytes.len() as u64)
        },
    })
}

/// Read one worktree file's content (capped at `MAX_FILE_BYTES`).
#[tauri::command]
pub async fn repo_file(
    args: RepoFileArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileContent, IpcError> {
    repo_file_impl(args, &store, &ssh).await
}

/// Unified diff for one worktree file — reusable logic called by the Tauri
/// command and (later) MCP tools. Tracked changes diff against `HEAD`; an
/// untracked file falls back to `git diff --no-index` against `/dev/null` so
/// it still renders as an all-added diff.
pub async fn repo_diff_impl(
    args: RepoFileArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<FileDiff, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(store, args.session_id)?;
    let quoted = quote(&args.path);

    // Tracked diff vs HEAD. A repo with no commits yet has no HEAD — `git
    // diff HEAD` would abort with "bad revision", so skip it when HEAD is
    // unborn and let the untracked `--no-index` fallback below render the
    // file as all-added.
    let script = repo_script(
        &name,
        &format!(
            "if git -C \"$root\" rev-parse --verify -q HEAD >/dev/null 2>&1; then \
             git -C \"$root\" diff HEAD -- {quoted}; fi"
        ),
    );
    let out = run_in_repo(ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    let mut raw = out.stdout;

    // Empty diff + an untracked file → show it as all-added via --no-index.
    if raw.iter().all(|b| b.is_ascii_whitespace()) {
        let body = format!("git -C \"$root\" diff --no-index -- /dev/null {quoted} || true");
        let script = repo_script(&name, &body);
        let fallback = run_in_repo(ssh, &host, &script).await?;
        if fallback.status.success() {
            raw = fallback.stdout;
        }
    }

    let (diff, binary, truncated) = diff_from_bytes(&raw);
    Ok(FileDiff {
        path: args.path,
        diff,
        binary,
        truncated,
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
    repo_diff_impl(args, &store, &ssh).await
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
}
