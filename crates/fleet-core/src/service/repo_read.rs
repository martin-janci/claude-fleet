//! Read-only git views of a session's worktree: changed files, the file
//! tree, one file's content or diff, the commit log, branches, one commit's
//! metadata and a file's diff within a commit.
//!
//! The worktree root is resolved live at call time (see `service::repo`):
//! we ask tmux for the session pane's current path, then `git rev-parse
//! --show-toplevel` from there. This is host-correct for remote sessions
//! (the DB's `projects.base_path` is a *local* scan path and would be wrong
//! on another machine). Every value interpolated into a shell script is
//! quoted (`shell::quote`), and caller-supplied paths/hashes are additionally
//! validated (`validate::repo_rel_path`, `validate::commit_hash`).
//!
//! Shared by the Tauri commands (`commands/files.rs`, `commands/history.rs`)
//! and the MCP `repo_*` tools.

use crate::ipc_error::IpcError;
use crate::service::repo::{
    diff_from_bytes, repo_err, repo_script, run_git, run_in_repo, session_target, SessionIdArgs,
    MAX_FILE_BYTES, MAX_TREE_ENTRIES,
};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

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

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub tip_hash: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitRef {
    pub name: String,
    /// branch | remote | tag | head
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<GitRef>,
    pub author: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub hash: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub date: String,
    pub files: Vec<ChangedFile>,
}

// ─── parsers ─────────────────────────────────────────────────────────────

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

/// Default page size for `repo_log`.
const LOG_DEFAULT_LIMIT: u32 = 200;

/// git log record/field separators. RS starts a record, US separates fields.
/// `--pretty=format:` with these gives unambiguous parsing of multi-field rows.
const LOG_FORMAT: &str = "--pretty=format:%x1e%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%aI%x1f%s";

/// Parse `%D` decoration (e.g. "HEAD -> main, origin/main, tag: v1, feat/x")
/// into structured refs.
fn parse_decoration(d: &str) -> Vec<GitRef> {
    let mut out = Vec::new();
    for raw in d.split(',') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("HEAD -> ") {
            out.push(GitRef {
                name: "HEAD".into(),
                kind: "head".into(),
            });
            out.push(GitRef {
                name: rest.trim().into(),
                kind: "branch".into(),
            });
        } else if t == "HEAD" {
            out.push(GitRef {
                name: "HEAD".into(),
                kind: "head".into(),
            });
        } else if let Some(tag) = t.strip_prefix("tag: ") {
            out.push(GitRef {
                name: tag.trim().into(),
                kind: "tag".into(),
            });
        } else if t.contains('/') {
            out.push(GitRef {
                name: t.into(),
                kind: "remote".into(),
            });
        } else {
            out.push(GitRef {
                name: t.into(),
                kind: "branch".into(),
            });
        }
    }
    out
}

/// Parse the RS/US-delimited `git log` output into commits.
fn parse_log(raw: &[u8]) -> Vec<Commit> {
    let text = String::from_utf8_lossy(raw);
    let mut out = Vec::new();
    for rec in text.split('\u{1e}') {
        if rec.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.splitn(7, '\u{1f}').collect();
        if f.len() < 7 {
            continue;
        }
        let parents = f[2]
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        out.push(Commit {
            hash: f[0].to_string(),
            short_hash: f[1].to_string(),
            parents,
            refs: parse_decoration(f[3]),
            author: f[4].to_string(),
            date: f[5].to_string(),
            subject: f[6].trim_end_matches('\n').to_string(),
        });
    }
    out
}

const BRANCH_FORMAT: &str =
    "--format=%(refname)%1f%(objectname:short)%1f%(HEAD)%1f%(upstream:short)%1f%(upstream:track)";

/// Parse `[ahead N, behind M]` (either part may be absent) into `(ahead, behind)`.
fn parse_track(s: &str) -> (u32, u32) {
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(',') {
        let p = part.trim();
        if let Some(n) = p.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = p.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn parse_branches(raw: &[u8]) -> Vec<Branch> {
    let text = String::from_utf8_lossy(raw);
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.splitn(5, '\u{1f}').collect();
        if f.len() < 5 {
            continue;
        }
        let refname = f[0];
        let (name, is_remote) = if let Some(n) = refname.strip_prefix("refs/heads/") {
            (n.to_string(), false)
        } else if let Some(n) = refname.strip_prefix("refs/remotes/") {
            (n.to_string(), true)
        } else {
            continue;
        };
        let (ahead, behind) = parse_track(f[4]);
        out.push(Branch {
            name,
            is_current: f[2] == "*",
            is_remote,
            upstream: if f[3].is_empty() {
                None
            } else {
                Some(f[3].to_string())
            },
            ahead,
            behind,
            tip_hash: f[1].to_string(),
        });
    }
    out
}

/// Parse `git show/diff-tree --name-status -z` output into `ChangedFile`s.
/// Tokens are NUL-separated: a status code, then the path; rename/copy codes
/// (`R*`/`C*`) are followed by the *old* path and then the *new* path.
fn parse_name_status_z(raw: &[u8]) -> Vec<ChangedFile> {
    let text = String::from_utf8_lossy(raw);
    let tokens: Vec<&str> = text.split('\0').filter(|t| !t.is_empty()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let code = tokens[i];
        i += 1;
        let letter = code.chars().next().unwrap_or('M');
        // `classify` takes the porcelain XY pair; a commit's name-status is a
        // single staged code, so present it as (letter, ' ').
        let (status, _) = classify(letter, ' ');
        if (letter == 'R' || letter == 'C') && i + 1 < tokens.len() {
            let orig = tokens[i].to_string();
            let path = tokens[i + 1].to_string();
            i += 2;
            out.push(ChangedFile {
                path,
                status: status.to_string(),
                staged: false,
                orig_path: Some(orig),
            });
        } else if i < tokens.len() {
            let path = tokens[i].to_string();
            i += 1;
            out.push(ChangedFile {
                path,
                status: status.to_string(),
                staged: false,
                orig_path: None,
            });
        }
    }
    out
}

// ─── changes / tree / file / diff ────────────────────────────────────────

/// `git status` for a session's worktree. Shared by the Tauri command and the
/// MCP tool.
pub async fn repo_changes(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<ChangedFile>, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let out = run_git(
        ssh,
        &host,
        &name,
        "git -C \"$root\" status --porcelain=v1 -z --untracked-files=all",
    )
    .await?;
    Ok(parse_status_z(&out.stdout))
}

/// Flat worktree listing (tracked + untracked, gitignore respected).
pub async fn repo_tree(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RepoTree, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let out = run_git(
        ssh,
        &host,
        &name,
        "git -C \"$root\" ls-files -z --cached --others --exclude-standard",
    )
    .await?;
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

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RepoPathParams")]
pub struct RepoFileArgs {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
}

/// Read one worktree file's content (capped at `MAX_FILE_BYTES`).
/// Keeps its own exit check: the `cf-is-dir` sentinel is a success path.
pub async fn repo_file(
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

/// Unified diff for one worktree file. Tracked changes diff against `HEAD`; an
/// untracked file falls back to `git diff --no-index` against `/dev/null` so
/// it still renders as an all-added diff.
pub async fn repo_diff(
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
    let out = run_git(
        ssh,
        &host,
        &name,
        &format!(
            "if git -C \"$root\" rev-parse --verify -q HEAD >/dev/null 2>&1; then \
             git -C \"$root\" diff HEAD -- {quoted}; fi"
        ),
    )
    .await?;
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

// ─── log / branches / commit ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RepoLogArgs {
    pub session_id: i64,
    /// Show all branches/refs (`--all`) instead of just current HEAD.
    #[serde(default)]
    pub all: bool,
    /// Page size; falls back to the default when 0/missing.
    #[serde(default)]
    pub limit: u32,
    /// Number of commits to skip (pagination).
    #[serde(default)]
    pub skip: u32,
}

/// Commit log for a session's worktree. `all` includes every ref so the frontend
/// can draw a branch tree; otherwise it's HEAD's history.
pub async fn repo_log(
    args: RepoLogArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<Commit>, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let limit = if args.limit == 0 {
        LOG_DEFAULT_LIMIT
    } else {
        args.limit.min(2000)
    };
    let all = if args.all { "--all" } else { "" };
    // Quote the format string for the same reason as `repo_branches`: keep any
    // shell metacharacter in the `--pretty=format:` value inert.
    let body = format!(
        "git -C \"$root\" log {all} --date=iso-strict {fmt} --max-count={limit} --skip={skip}",
        all = all,
        fmt = quote(LOG_FORMAT),
        limit = limit,
        skip = args.skip,
    );
    let out = run_git(ssh, &host, &name, &body).await?;
    Ok(parse_log(&out.stdout))
}

/// Local + remote branches for a session's worktree.
pub async fn repo_branches(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<Branch>, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    // `BRANCH_FORMAT` contains `%(refname)` etc. — the parens are shell
    // metacharacters, so it MUST be quoted or bash aborts the line with
    // "syntax error near unexpected token `('".
    let body = format!(
        "git -C \"$root\" for-each-ref {fmt} refs/heads refs/remotes",
        fmt = quote(BRANCH_FORMAT),
    );
    let out = run_git(ssh, &host, &name, &body).await?;
    Ok(parse_branches(&out.stdout))
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RepoCommitParams")]
pub struct RepoCommitArgs {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
}

/// One commit's metadata + the files it changed (first-parent for merges).
pub async fn repo_commit(
    args: RepoCommitArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<CommitDetail, IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    let (host, name) = session_target(store, args.session_id)?;
    let h = quote(&args.hash);
    // Two git calls: metadata (US-separated) then NUL name-status. `set -e`
    // (from repo_script) aborts on a bad hash.
    let body = format!(
        "git -C \"$root\" show -s --date=iso-strict \
           --pretty=format:%H%x1f%s%x1f%b%x1f%an%x1f%aI {h}; \
         printf '\\036'; \
         git -C \"$root\" show --first-parent --name-status -z --pretty=format: {h}"
    );
    let out = run_git(ssh, &host, &name, &body).await?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Split metadata from name-status on the RS byte we printed between them.
    let (meta, names) = match text.split_once('\u{1e}') {
        Some(p) => p,
        None => (text.as_ref(), ""),
    };
    let f: Vec<&str> = meta.splitn(5, '\u{1f}').collect();
    let detail = CommitDetail {
        hash: f.first().unwrap_or(&"").to_string(),
        subject: f.get(1).unwrap_or(&"").to_string(),
        body: f.get(2).unwrap_or(&"").trim_end().to_string(),
        author: f.get(3).unwrap_or(&"").to_string(),
        date: f.get(4).unwrap_or(&"").trim().to_string(),
        files: parse_name_status_z(names.trim_start_matches('\n').as_bytes()),
    };
    Ok(detail)
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RepoCommitDiffParams")]
pub struct RepoCommitDiffArgs {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
    /// Worktree-relative file path.
    pub path: String,
}

/// A single file's diff *within* a commit (first-parent for merges).
pub async fn repo_commit_diff(
    args: RepoCommitDiffArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<FileDiff, IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(store, args.session_id)?;
    let body = format!(
        "git -C \"$root\" show --first-parent --format= {h} -- {p}",
        h = quote(&args.hash),
        p = quote(&args.path),
    );
    let out = run_git(ssh, &host, &name, &body).await?;
    let (diff, binary, truncated) = diff_from_bytes(&out.stdout);
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
    fn parse_name_status_handles_rename_and_plain() {
        // "M\0a.ts\0R100\0old.ts\0new.ts\0A\0added.ts\0"
        let raw = b"M\0a.ts\0R100\0old.ts\0new.ts\0A\0added.ts\0";
        let files = parse_name_status_z(raw);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "a.ts");
        assert_eq!(files[0].status, "modified");
        assert_eq!(files[1].status, "renamed");
        assert_eq!(files[1].path, "new.ts");
        assert_eq!(files[1].orig_path.as_deref(), Some("old.ts"));
        assert_eq!(files[2].path, "added.ts");
        assert_eq!(files[2].status, "added");
    }

    #[test]
    fn parse_log_reads_fields_and_parents() {
        // Two records: a merge (2 parents, decorated) then a root commit.
        let raw = "\u{1e}aaaa\u{1f}aaa\u{1f}bbbb cccc\u{1f}HEAD -> main, origin/main\u{1f}MJ\u{1f}2026-05-22T10:00:00+02:00\u{1f}Merge branch x\u{1e}dddd\u{1f}ddd\u{1f}\u{1f}\u{1f}MJ\u{1f}2026-05-20T09:00:00+02:00\u{1f}initial";
        let commits = parse_log(raw.as_bytes());
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "aaaa");
        assert_eq!(commits[0].parents, vec!["bbbb", "cccc"]);
        assert_eq!(commits[0].subject, "Merge branch x");
        assert_eq!(
            commits[0].refs,
            vec![
                GitRef {
                    name: "HEAD".into(),
                    kind: "head".into()
                },
                GitRef {
                    name: "main".into(),
                    kind: "branch".into()
                },
                GitRef {
                    name: "origin/main".into(),
                    kind: "remote".into()
                },
            ]
        );
        assert!(commits[1].parents.is_empty());
        assert_eq!(commits[1].subject, "initial");
    }

    #[test]
    fn parse_log_handles_empty() {
        assert!(parse_log(b"").is_empty());
    }

    #[test]
    fn parse_decoration_classifies_tag_and_remote() {
        let refs = parse_decoration("tag: v1.0, upstream/feat/x, local-branch");
        assert_eq!(refs[0].kind, "tag");
        assert_eq!(refs[1].kind, "remote");
        assert_eq!(refs[2].kind, "branch");
    }

    #[test]
    fn parse_branches_reads_current_remote_and_track() {
        // refname US short US HEAD US upstream US track  — one ref per line.
        let raw = "refs/heads/main\u{1f}aaaa\u{1f}*\u{1f}origin/main\u{1f}[ahead 2, behind 1]\n\
                   refs/heads/feat\u{1f}bbbb\u{1f} \u{1f}\u{1f}\n\
                   refs/remotes/origin/main\u{1f}aaaa\u{1f} \u{1f}\u{1f}\n";
        let bs = parse_branches(raw.as_bytes());
        assert_eq!(bs.len(), 3);
        assert_eq!(bs[0].name, "main");
        assert!(bs[0].is_current);
        assert!(!bs[0].is_remote);
        assert_eq!(bs[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(bs[0].ahead, 2);
        assert_eq!(bs[0].behind, 1);
        assert_eq!(bs[1].name, "feat");
        assert!(!bs[1].is_current);
        assert_eq!(bs[1].ahead, 0);
        assert_eq!(bs[2].name, "origin/main");
        assert!(bs[2].is_remote);
    }
}
