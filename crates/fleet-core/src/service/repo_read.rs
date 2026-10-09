//! Read-only git views of a session's worktree: changed files, the file
//! tree, one file's content, diff or blame, the commit log, branches (with
//! whether each is merged into the base branch), one commit's metadata and a
//! file's diff within a commit.
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

use crate::ipc_error::{codes, IpcError};
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
//
// These eight also come back *into* a hub-client desktop, which reads the
// same JSON out of the matching MCP tool (`mcp::tools::repo`) instead of
// running git itself, so each derives `Deserialize` as well.
//
// Deliberately **without** `#[serde(default)]` on the `Option` fields, unlike
// the list rows in `store::rows`. Those go out through `ok_json_compact`,
// which strips nulls recursively, so absent is the normal encoding of `None`
// and the default is forced. Every tool in `mcp::tools::repo` uses plain
// `ok_json`, which keeps `"orig_path": null` on the wire — so `Option` parses
// from an explicit null, and a *renamed or dropped* field still fails loudly
// rather than becoming a silent `None`. That is the stronger contract. Keep
// it unless one of those tools switches to the compact encoder; if one does,
// these must change together.

/// One entry in `git status` for a session's worktree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// Friendly status: modified / added / deleted / renamed / copied /
    /// untracked / conflict.
    pub status: String,
    /// Whether the index (staged side) carries a change for this file.
    pub staged: bool,
    /// For renames/copies, the path the file came from.
    pub orig_path: Option<String>,
    /// Lines added (M15 G1.10, the Files tab's per-file +N). Absent for a
    /// binary file, a file git does not diff (an untracked one), and from an
    /// older hub, so `#[serde(default)]` unlike the fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    /// Lines removed (the −N), absent as `added` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
}

/// Flat worktree listing — tracked files plus untracked, gitignore respected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoTree {
    pub entries: Vec<String>,
    pub truncated: bool,
}

/// The content of one worktree file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    /// Empty when `binary` is true.
    pub diff: String,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub tip_hash: String,
    /// The branch's tip is reachable from the base branch (`origin/HEAD`,
    /// else `origin/main`, `origin/master`, `main`, `master`), so deleting it
    /// loses no commit. Never set on the base itself, its local twin, a
    /// `*/HEAD` alias or the checked-out branch. `#[serde(default)]` because
    /// a hub built before this field omits it, and a desktop paired with one
    /// must still read the list (as "nothing merged").
    #[serde(default)]
    pub merged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitRef {
    pub name: String,
    /// branch | remote | tag | head
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub hash: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub date: String,
    pub files: Vec<ChangedFile>,
    /// Whether a remote-tracking branch contains the commit (M15 G1.10:
    /// "pushed" / "not pushed"). Absent from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<bool>,
}

/// One run of consecutive lines last changed by the same commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlameHunk {
    /// 1-based first line of the run in the current worktree file.
    pub start: u32,
    /// Number of lines in the run.
    pub lines: u32,
    pub hash: String,
    pub author: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub summary: String,
    /// Lines changed in the worktree and not committed yet (git's all-zero
    /// hash).
    pub uncommitted: bool,
}

/// `git blame` of one worktree file, as runs of lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileBlame {
    pub path: String,
    pub hunks: Vec<BlameHunk>,
    /// More than `MAX_BLAME_LINES` lines; the hunks stop there.
    pub truncated: bool,
}

// ─── parsers ─────────────────────────────────────────────────────────────

/// Parse `git status --porcelain=v1 -z` output. Entries are NUL-separated;
/// a rename/copy entry is followed by a second token (the original path).
pub(crate) fn parse_status_z(raw: &[u8]) -> Vec<ChangedFile> {
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
        // Either column: `git add -N` of a moved file reports a WORK-TREE
        // rename (` R new\0old`), whose original path is a token too.
        let orig_path = if (matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C')) && i < tokens.len() {
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
            added: None,
            removed: None,
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

/// Parse `%D` decoration into structured refs. The log runs with
/// `--decorate=full` ("HEAD -> refs/heads/main, refs/remotes/origin/main,
/// tag: refs/tags/v1, refs/heads/feat/x"), so a ref's kind comes from its
/// prefix: a short name cannot tell a local `feat/x` from a remote one.
/// Short names (no prefix) still parse, `/` meaning a remote.
fn parse_decoration(d: &str) -> Vec<GitRef> {
    let r = |name: &str, kind: &str| GitRef {
        name: name.trim().into(),
        kind: kind.into(),
    };
    let mut out = Vec::new();
    for raw in d.split(',') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let t = match t.strip_prefix("HEAD -> ") {
            Some(rest) => {
                out.push(r("HEAD", "head"));
                let rest = rest.trim();
                out.push(r(
                    rest.strip_prefix("refs/heads/").unwrap_or(rest),
                    "branch",
                ));
                continue;
            }
            None => t,
        };
        if t == "HEAD" {
            out.push(r("HEAD", "head"));
        } else if let Some(tag) = t.strip_prefix("tag: ") {
            let tag = tag.trim();
            out.push(r(tag.strip_prefix("refs/tags/").unwrap_or(tag), "tag"));
        } else if let Some(b) = t.strip_prefix("refs/heads/") {
            out.push(r(b, "branch"));
        } else if let Some(rm) = t.strip_prefix("refs/remotes/") {
            out.push(r(rm, "remote"));
        } else if t.contains('/') {
            out.push(r(t, "remote"));
        } else {
            out.push(r(t, "branch"));
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
            merged: false,
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
                added: None,
                removed: None,
            });
        } else if i < tokens.len() {
            let path = tokens[i].to_string();
            i += 1;
            out.push(ChangedFile {
                path,
                status: status.to_string(),
                staged: false,
                orig_path: None,
                added: None,
                removed: None,
            });
        }
    }
    out
}

/// Parse `git diff/show --numstat -z`: per path, (lines added, lines
/// removed), `None` for a binary file (`-`). A plain entry is
/// `A\tR\tpath\0`; a rename or copy is `A\tR\t\0old\0new\0`, keyed by the
/// new path as `ChangedFile.path` is.
fn parse_numstat_z(raw: &[u8]) -> std::collections::HashMap<String, (Option<u32>, Option<u32>)> {
    let text = String::from_utf8_lossy(raw);
    let mut tokens = text.split('\0');
    let mut out = std::collections::HashMap::new();
    while let Some(tok) = tokens.next() {
        let tok = tok.trim_start_matches('\n');
        let mut f = tok.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            // Rename / copy: the old path, then the new.
            let _old = tokens.next();
            match tokens.next() {
                Some(p) => p,
                None => break,
            }
        } else {
            path
        };
        out.insert(path.to_string(), (a.parse().ok(), r.parse().ok()));
    }
    out
}

/// Set each file's `added` / `removed` from a `parse_numstat_z` map.
fn apply_numstat(files: &mut [ChangedFile], raw: &[u8]) {
    let counts = parse_numstat_z(raw);
    for f in files {
        if let Some(&(a, r)) = counts.get(&f.path) {
            f.added = a;
            f.removed = r;
        }
    }
}

/// `repo_changes`' script: the porcelain status, an RS byte, then the
/// worktree's line counts against HEAD (staged and unstaged together, as
/// the Files tab shows one row per file); none on an unborn HEAD.
fn changes_body() -> &'static str {
    "git -C \"$root\" status --porcelain=v1 -z --untracked-files=all\n\
     printf '\\036'\n\
     if git -C \"$root\" rev-parse -q --verify HEAD >/dev/null 2>&1; then\n\
       git -C \"$root\" diff --numstat -z HEAD\n\
     fi"
}

/// Split `changes_body`'s output into the changed files with their counts.
fn parse_changes(raw: &[u8]) -> Vec<ChangedFile> {
    let (status, numstat) = match raw.iter().position(|&b| b == 0x1e) {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, &[][..]),
    };
    let mut files = parse_status_z(status);
    apply_numstat(&mut files, numstat);
    files
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
    let out = run_git(ssh, &host, &name, changes_body()).await?;
    Ok(parse_changes(&out.stdout))
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

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RepoPathParams")]
pub struct RepoFileArgs {
    /// Fleet session id.
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
}

/// Emitted on stderr when a path leaves the worktree through a symlink.
const OUTSIDE_SENTINEL: &str = "cf-outside";

/// Refuse `$f` when its directory resolves outside `$root` (a symlinked
/// directory on the way), or, with `final_link`, when `$f` itself is a
/// symlink. `repo_rel_path` only checks the text, and `head` and
/// `git diff --no-index` follow links, so without this a watch-tier caller
/// reads `~/.ssh` through a committed `docs -> ../.ssh` (review r04 T1/T2).
fn confine(final_link: bool) -> String {
    format!(
        "rr=$(cd \"$root\" && pwd -P)\n\
         d=$(cd \"$(dirname -- \"$f\")\" 2>/dev/null && pwd -P) || d=\"$rr\"\n\
         case \"$d/\" in \"$rr\"/*) ;; *) echo {OUTSIDE_SENTINEL} >&2; exit 8;; esac\n{}",
        if final_link {
            format!("if [ -L \"$f\" ]; then echo {OUTSIDE_SENTINEL} >&2; exit 8; fi\n")
        } else {
            String::new()
        }
    )
}

fn outside_err(out: &std::process::Output) -> Option<IpcError> {
    String::from_utf8_lossy(&out.stderr)
        .contains(OUTSIDE_SENTINEL)
        .then(|| {
            IpcError::new(
                codes::E_INVALID,
                "that path leads out of the session's worktree through a symlink",
            )
        })
}

/// `repo_file`'s shell body for a validated worktree-relative `path`.
fn file_body(path: &str) -> String {
    format!(
        "f=\"$root\"/{path}\n{confine}\
         if [ -d \"$f\" ]; then echo cf-is-dir >&2; exit 9; fi\n\
         head -c {cap} -- \"$f\"",
        path = quote(path),
        confine = confine(true),
        cap = MAX_FILE_BYTES + 1,
    )
}

/// `repo_diff`'s untracked fallback: the file as all-added. A symlinked
/// final file shows only its target's name, so only the directory is
/// confined.
fn untracked_diff_body(path: &str) -> String {
    // A file HEAD has is not untracked: an empty `diff HEAD` means it reads
    // as committed (say, staged and then reverted), not as all added.
    format!(
        "f=\"$root\"/{quoted}\n{confine}\
         if [ -n \"$(git -C \"$root\" ls-tree --name-only HEAD -- {quoted} 2>/dev/null)\" ]; then exit 0; fi\n\
         git -C \"$root\" diff --no-index -- /dev/null {quoted} || true",
        quoted = quote(path),
        confine = confine(false),
    )
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
    let body = file_body(&args.path);
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
        if let Some(e) = outside_err(&out) {
            return Err(e);
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
        let body = untracked_diff_body(&args.path);
        let script = repo_script(&name, &body);
        let fallback = run_in_repo(ssh, &host, &script).await?;
        if let Some(e) = outside_err(&fallback) {
            return Err(e);
        }
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

/// Most lines `repo_blame` returns; past it the hunks stop and `truncated`
/// is set. Matches the order of `MAX_FILE_BYTES` for ordinary source lines.
pub const MAX_BLAME_LINES: u32 = 20_000;

/// `git blame` of one worktree file — committed lines with their commit, and
/// lines changed in the worktree as `uncommitted`. Line numbers match
/// `repo_file`'s content. Untracked files fail with git's own message.
pub async fn repo_blame(
    args: RepoFileArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<FileBlame, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(store, args.session_id)?;
    let out = run_git(ssh, &host, &name, &blame_body(&args.path)).await?;
    let (hunks, truncated) = parse_blame_porcelain(&out.stdout, MAX_BLAME_LINES);
    Ok(FileBlame {
        path: args.path,
        hunks,
        truncated,
    })
}

fn blame_body(path: &str) -> String {
    format!("git -C \"$root\" blame --porcelain -- {}", quote(path))
}

/// Parse `git blame --porcelain`: each line is a `<hash> <orig> <final>
/// [<count>]` header, the commit's `key value` lines the first time that
/// commit appears, then the line itself after a TAB. Consecutive lines of
/// one commit fold into one hunk; parsing stops after `max_lines`.
fn parse_blame_porcelain(raw: &[u8], max_lines: u32) -> (Vec<BlameHunk>, bool) {
    #[derive(Default, Clone)]
    struct Meta {
        author: String,
        time: i64,
        summary: String,
    }
    let text = String::from_utf8_lossy(raw);
    let mut metas: std::collections::HashMap<String, Meta> = std::collections::HashMap::new();
    let mut hunks: Vec<BlameHunk> = Vec::new();
    // (hash, final line) of the header we are inside.
    let mut cur: Option<(String, u32)> = None;
    let mut seen = 0u32;
    let mut truncated = false;
    for line in text.split('\n') {
        if line.starts_with('\t') {
            let Some((hash, final_line)) = cur.take() else {
                continue;
            };
            if seen == max_lines {
                truncated = true;
                break;
            }
            seen += 1;
            let meta = metas.get(&hash).cloned().unwrap_or_default();
            match hunks.last_mut() {
                Some(h) if h.hash == hash && h.start + h.lines == final_line => h.lines += 1,
                _ => hunks.push(BlameHunk {
                    start: final_line,
                    lines: 1,
                    uncommitted: hash.bytes().all(|b| b == b'0'),
                    hash,
                    author: meta.author,
                    time: meta.time,
                    summary: meta.summary,
                }),
            }
            continue;
        }
        let mut parts = line.split(' ');
        let first = parts.next().unwrap_or("");
        if cur.is_none() && first.len() >= 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let final_line = parts.nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            metas.entry(first.to_string()).or_default();
            cur = Some((first.to_string(), final_line));
            continue;
        }
        let Some((hash, _)) = &cur else { continue };
        let Some(m) = metas.get_mut(hash) else {
            continue;
        };
        let rest = line.split_once(' ').map(|(_, v)| v).unwrap_or("");
        match first {
            "author" => m.author = rest.to_string(),
            "author-time" => m.time = rest.trim().parse().unwrap_or(0),
            "summary" => m.summary = rest.to_string(),
            _ => {}
        }
    }
    (hunks, truncated)
}

// ─── log / branches / commit ─────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
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
        "git -C \"$root\" log {all} --decorate=full --date=iso-strict {fmt} --max-count={limit} --skip={skip}",
        all = all,
        fmt = quote(LOG_FORMAT),
        limit = limit,
        skip = args.skip,
    );
    let out = run_git(ssh, &host, &name, &body).await?;
    Ok(parse_log(&out.stdout))
}

/// Shell that sets `$base` to the branch "merged" is measured against:
/// `origin/HEAD`'s target, else the first of `origin/main`, `origin/master`,
/// `main`, `master` that exists, else empty. `$base_local` is the same name
/// without its remote (`origin/main` → `main`), so the local twin of the base
/// is never offered for deletion. Needs `$root` (see `repo_script`). Every
/// failing probe sits in an `if`/`||`, so `set -e` never aborts on it.
pub(crate) const BASE_BRANCH_SH: &str = r#"base=""
for c in "$(git -C "$root" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)" origin/main origin/master main master; do
  if [ -n "$c" ] && git -C "$root" rev-parse -q --verify "$c^{commit}" >/dev/null 2>&1; then base="$c"; break; fi
done
base_local="$base"
if [ -n "$base" ] && git -C "$root" rev-parse -q --verify "refs/remotes/$base" >/dev/null 2>&1; then base_local="${base#*/}"; fi
"#;

/// Local + remote branches for a session's worktree, each flagged `merged`
/// when the base branch already contains its tip.
pub async fn repo_branches(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<Branch>, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    // `BRANCH_FORMAT` contains `%(refname)` etc. — the parens are shell
    // metacharacters, so it MUST be quoted or bash aborts the line with
    // "syntax error near unexpected token `('".
    let out = run_git(ssh, &host, &name, &branches_body()).await?;
    Ok(parse_branches_with_merged(&out.stdout))
}

/// The branch list, an RS byte, then (when a base exists) the base's name on
/// one line and the refs it contains, one per line.
fn branches_body() -> String {
    format!(
        "git -C \"$root\" for-each-ref {fmt} refs/heads refs/remotes\n\
         printf '\\036'\n\
         {base}\
         if [ -n \"$base\" ]; then\n\
           printf '%s\\n%s\\n' \"$base\" \"$base_local\"\n\
           git -C \"$root\" for-each-ref --merged \"$base\" {merged} refs/heads refs/remotes\n\
         fi",
        fmt = quote(BRANCH_FORMAT),
        base = BASE_BRANCH_SH,
        merged = quote("--format=%(refname)"),
    )
}

/// Split `branches_body`'s output and set each branch's `merged` flag.
fn parse_branches_with_merged(raw: &[u8]) -> Vec<Branch> {
    let (list, merged) = match raw.iter().position(|&b| b == 0x1e) {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, &[][..]),
    };
    let mut branches = parse_branches(list);
    let text = String::from_utf8_lossy(merged);
    let mut lines = text.lines();
    let (Some(base), Some(base_local)) = (lines.next(), lines.next()) else {
        return branches;
    };
    let contained: std::collections::HashSet<&str> = lines.collect();
    for b in &mut branches {
        let refname = if b.is_remote {
            format!("refs/remotes/{}", b.name)
        } else {
            format!("refs/heads/{}", b.name)
        };
        let is_base = if b.is_remote {
            b.name == base
        } else {
            b.name == base_local || b.name == base
        };
        b.merged = contained.contains(refname.as_str())
            && !is_base
            && !b.is_current
            && !b.name.ends_with("/HEAD");
    }
    branches
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
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
    let out = run_git(ssh, &host, &name, &commit_body(&args.hash)).await?;
    Ok(parse_commit(&out.stdout))
}

/// `repo_commit`'s script: metadata (US-separated), then RS and the NUL
/// name-status, RS and the NUL numstat (M15 G1.10), RS and `1` / `0` for
/// whether a remote-tracking branch contains the commit. `set -e` (from
/// repo_script) aborts on a bad hash.
fn commit_body(hash: &str) -> String {
    let h = quote(hash);
    format!(
        "git -C \"$root\" show -s --date=iso-strict \
           --pretty=format:%H%x1f%s%x1f%b%x1f%an%x1f%aI {h}; \
         printf '\\036'; \
         git -C \"$root\" show --first-parent --name-status -z --pretty=format: {h}; \
         printf '\\036'; \
         git -C \"$root\" show --first-parent --numstat -z --pretty=format: {h}; \
         printf '\\036'; \
         if [ -n \"$(git -C \"$root\" for-each-ref --count=1 --format=x --contains {h} refs/remotes)\" ]; \
         then echo 1; else echo 0; fi"
    )
}

fn parse_commit(raw: &[u8]) -> CommitDetail {
    let text = String::from_utf8_lossy(raw);
    // Sections split on the RS bytes printed between them.
    let mut parts = text.split('\u{1e}');
    let meta = parts.next().unwrap_or("");
    let names = parts.next().unwrap_or("");
    let numstat = parts.next();
    let pushed = parts.next().and_then(|p| match p.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    });
    let f: Vec<&str> = meta.splitn(5, '\u{1f}').collect();
    let mut files = parse_name_status_z(names.trim_start_matches('\n').as_bytes());
    if let Some(n) = numstat {
        apply_numstat(&mut files, n.as_bytes());
    }
    CommitDetail {
        hash: f.first().unwrap_or(&"").to_string(),
        subject: f.get(1).unwrap_or(&"").to_string(),
        body: f.get(2).unwrap_or(&"").trim_end().to_string(),
        author: f.get(3).unwrap_or(&"").to_string(),
        date: f.get(4).unwrap_or(&"").trim().to_string(),
        files,
        pushed,
    }
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
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

// ─── what the branch carries (redesign 5.6) ──────────────────────────────

/// Most unpushed commits `repo_branch_diff` lists; past it `truncated` is set.
pub const MAX_UNPUSHED: u32 = 200;

/// What a session's branch carries that is not on the remote yet, and what
/// it changes against the base branch: the Files tab's Changed section shows
/// both under the worktree's own changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDiff {
    /// The checked-out branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Its upstream (`origin/feat`); `None` when it was never pushed.
    pub upstream: Option<String>,
    /// Commits no remote has, newest first: `@{u}..HEAD`, or with no
    /// upstream every commit on HEAD that no remote-tracking ref contains.
    pub unpushed: Vec<Commit>,
    /// The files those commits change, as one diff from where they start.
    pub unpushed_files: Vec<ChangedFile>,
    /// More than `MAX_UNPUSHED` unpushed commits; the list stops there.
    pub truncated: bool,
    /// The base branch (`origin/HEAD`'s target, else `origin/main`,
    /// `origin/master`, `main`, `master`); `None` without one.
    pub base: Option<String>,
    /// Commits on HEAD since it left the base (`merge-base..HEAD`).
    pub ahead_of_base: u32,
    /// The files HEAD changes against its merge base with the base branch.
    pub base_files: Vec<ChangedFile>,
    /// Commits on the base since HEAD left it (`HEAD..base`, M15 G1.10: "N
    /// behind main"). Absent without a base, and from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind_base: Option<u32>,
}

/// Shell that sets `$up` (HEAD's upstream, or empty), `$from` (where the
/// unpushed commits start: `$up`'s merge base with HEAD, the parent of the
/// oldest commit no remote has, the empty tree when that is the root
/// commit, or empty when nothing is unpushed) and `$mb` (HEAD's merge base
/// with `$base`, or empty). Needs `$root` and `BASE_BRANCH_SH` before it,
/// and a born HEAD. Every probe that may fail sits in an `if` or `||`.
const RANGE_SH: &str = r#"up="$(git -C "$root" rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null || true)"
from=""
if [ -n "$up" ]; then
  from="$(git -C "$root" merge-base "$up" HEAD 2>/dev/null || true)"
else
  first="$(git -C "$root" rev-list --reverse HEAD --not --remotes | head -n 1)"
  if [ -n "$first" ]; then
    if git -C "$root" rev-parse -q --verify "$first^" >/dev/null 2>&1; then from="$first^"
    else from="$(git -C "$root" hash-object -t tree /dev/null)"; fi
  fi
fi
mb=""
if [ -n "$base" ]; then mb="$(git -C "$root" merge-base "$base" HEAD 2>/dev/null || true)"; fi
"#;

/// Group separator between `branch_diff_body`'s sections (the log's own
/// records start with RS, so RS cannot split them).
const GS: u8 = 0x1d;

/// Six GS-separated sections: `branch\nupstream\nbase\n`, the unpushed log
/// (`LOG_FORMAT`), their name-status, the count ahead of the base, the base
/// diff's name-status, and the count behind the base (empty without one).
/// An unborn HEAD prints the header and empties.
fn branch_diff_body() -> String {
    format!(
        "{base}\
         br=\"$(git -C \"$root\" symbolic-ref -q --short HEAD || true)\"\n\
         if ! git -C \"$root\" rev-parse -q --verify HEAD >/dev/null 2>&1; then\n\
           printf '%s\\n\\n%s\\n\\035\\035\\035\\035\\035' \"$br\" \"$base\"; exit 0\n\
         fi\n\
         {range}\
         printf '%s\\n%s\\n%s\\n\\035' \"$br\" \"$up\" \"$base\"\n\
         if [ -n \"$up\" ]; then\n\
           git -C \"$root\" log --decorate=full --date=iso-strict {fmt} --max-count={max} \"$up..HEAD\"\n\
         else\n\
           git -C \"$root\" log --decorate=full --date=iso-strict {fmt} --max-count={max} HEAD --not --remotes\n\
         fi\n\
         printf '\\035'\n\
         if [ -n \"$from\" ]; then git -C \"$root\" diff --name-status -z \"$from\" HEAD; fi\n\
         printf '\\035'\n\
         if [ -n \"$mb\" ]; then git -C \"$root\" rev-list --count \"$mb..HEAD\"; else echo 0; fi\n\
         printf '\\035'\n\
         if [ -n \"$mb\" ]; then git -C \"$root\" diff --name-status -z \"$mb\" HEAD; fi\n\
         printf '\\035'\n\
         if [ -n \"$mb\" ]; then git -C \"$root\" rev-list --count \"HEAD..$base\"; fi",
        base = BASE_BRANCH_SH,
        range = RANGE_SH,
        fmt = quote(LOG_FORMAT),
        max = MAX_UNPUSHED + 1,
    )
}

fn parse_branch_diff(raw: &[u8]) -> BranchDiff {
    let mut parts = raw.split(|&b| b == GS);
    let head = String::from_utf8_lossy(parts.next().unwrap_or(&[])).into_owned();
    let mut lines = head.lines();
    let opt = |l: Option<&str>| {
        l.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let branch = opt(lines.next());
    let upstream = opt(lines.next());
    let base = opt(lines.next());
    let mut unpushed = parse_log(parts.next().unwrap_or(&[]));
    let truncated = unpushed.len() > MAX_UNPUSHED as usize;
    unpushed.truncate(MAX_UNPUSHED as usize);
    let unpushed_files = parse_name_status_z(parts.next().unwrap_or(&[]));
    let ahead_of_base = String::from_utf8_lossy(parts.next().unwrap_or(&[]))
        .trim()
        .parse()
        .unwrap_or(0);
    let base_files = parse_name_status_z(parts.next().unwrap_or(&[]));
    let behind_base = parts
        .next()
        .and_then(|p| String::from_utf8_lossy(p).trim().parse().ok());
    BranchDiff {
        branch,
        upstream,
        unpushed,
        unpushed_files,
        truncated,
        base,
        ahead_of_base,
        base_files,
        behind_base,
    }
}

/// What the session's branch has not pushed, and what it changes against
/// the base branch.
pub async fn repo_branch_diff(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<BranchDiff, IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let out = run_git(ssh, &host, &name, &branch_diff_body()).await?;
    Ok(parse_branch_diff(&out.stdout))
}

/// Which of `BranchDiff`'s two ranges a file diff is taken over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "lowercase")]
pub enum DiffRange {
    /// From where the unpushed commits start to HEAD.
    Unpushed,
    /// From HEAD's merge base with the base branch to HEAD.
    Base,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RepoRangeDiffParams")]
pub struct RepoRangeDiffArgs {
    /// Fleet session id.
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
    /// `unpushed` or `base`.
    pub range: DiffRange,
}

fn range_diff_body(range: DiffRange, path: &str) -> String {
    let from = match range {
        DiffRange::Unpushed => "$from",
        DiffRange::Base => "$mb",
    };
    format!(
        "{base}{range_sh}if [ -n \"{from}\" ]; then git -C \"$root\" diff \"{from}\" HEAD -- {p}; fi",
        base = BASE_BRANCH_SH,
        range_sh = RANGE_SH,
        p = quote(path),
    )
}

/// One file's diff over the unpushed commits or against the base branch.
/// Empty when the range is (nothing unpushed, no base).
pub async fn repo_range_diff(
    args: RepoRangeDiffArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<FileDiff, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(store, args.session_id)?;
    let out = run_git(ssh, &host, &name, &range_diff_body(args.range, &args.path)).await?;
    let (diff, binary, truncated) = diff_from_bytes(&out.stdout);
    Ok(FileDiff {
        path: args.path,
        diff,
        binary,
        truncated,
    })
}

/// A real git repository for the repo tests, and a way to run a service's
/// shell body against it the way `repo_script` would (with `$root` set and
/// `set -e`), minus the tmux lookup. Unix only: these bodies run in the
/// host's bash, and a Windows runner's `bash` is not that shell.
#[cfg(all(test, unix))]
pub(crate) mod git_fixture {
    use crate::shell::quote;
    use std::path::Path;

    pub fn git(dir: &Path, args: &[&str]) {
        let out = crate::proc::std_command("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Ada",
                "-c",
                "user.email=ada@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub fn commit_file(dir: &Path, file: &str, content: &str, msg: &str) {
        std::fs::write(dir.join(file), content).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", msg]);
    }

    /// Run `body` with `$root` set to `root`, as `run_git` would on a host.
    pub fn run_body(root: &Path, body: &str) -> std::process::Output {
        let script = format!("set -e\nroot={}\n{body}", quote(&root.to_string_lossy()));
        let out = crate::proc::std_command("bash")
            .arg("-c")
            .arg(script)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run bash");
        assert!(
            out.status.success(),
            "body failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// `origin` (bare) and a clone-like `work` repo:
    /// - `done` merged into `main` with a merge commit, pushed;
    /// - `old` at main's first commit (merged, local only);
    /// - `open` one commit past main (not merged);
    /// - `fresh` at main's tip, checked out;
    /// - `origin/HEAD` → `origin/main`.
    pub fn branches_repo(tmp: &Path) -> std::path::PathBuf {
        let origin = tmp.join("origin.git");
        let work = tmp.join("work");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        git(&origin, &["init", "-q", "--bare"]);
        git(&work, &["init", "-q"]);
        commit_file(&work, "a.txt", "a\n", "first");
        git(&work, &["branch", "old"]);
        git(&work, &["checkout", "-q", "-b", "done"]);
        commit_file(&work, "b.txt", "b\n", "done work");
        git(&work, &["checkout", "-q", "main"]);
        git(
            &work,
            &["merge", "-q", "--no-ff", "-m", "merge done", "done"],
        );
        git(&work, &["checkout", "-q", "-b", "open"]);
        commit_file(&work, "c.txt", "c\n", "open work");
        git(&work, &["checkout", "-q", "main"]);
        git(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&work, &["push", "-q", "origin", "main", "done"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        git(&work, &["checkout", "-q", "-b", "fresh"]);
        work
    }
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
    fn parse_status_z_worktree_rename_consumes_orig_path() {
        let files = parse_status_z(b" R b.txt\0a.txt\0 M c.ts\0");
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!(files[0].path, "b.txt");
        assert_eq!(files[0].orig_path.as_deref(), Some("a.txt"));
        assert_eq!(files[1].path, "c.ts");
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
    fn full_decoration_tells_a_local_slash_branch_from_a_remote() {
        let refs = parse_decoration(
            "HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1, \
             refs/heads/claude/x",
        );
        let got: Vec<(&str, &str)> = refs
            .iter()
            .map(|r| (r.name.as_str(), r.kind.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("HEAD", "head"),
                ("main", "branch"),
                ("origin/main", "remote"),
                ("v1", "tag"),
                ("claude/x", "branch"),
            ]
        );
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

    #[cfg(unix)]
    fn by_name<'a>(bs: &'a [Branch], name: &str) -> &'a Branch {
        bs.iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("no branch {name} in {bs:?}"))
    }

    #[cfg(unix)]
    #[test]
    fn branches_flag_what_the_base_already_contains() {
        let tmp = tempfile::tempdir().unwrap();
        let work = git_fixture::branches_repo(tmp.path());
        let out = git_fixture::run_body(&work, &branches_body());
        let bs = parse_branches_with_merged(&out.stdout);

        assert!(by_name(&bs, "done").merged, "merged with a merge commit");
        assert!(by_name(&bs, "old").merged, "an ancestor of the base");
        assert!(by_name(&bs, "origin/done").merged, "a remote branch too");
        assert!(!by_name(&bs, "open").merged, "has a commit main lacks");
        assert!(!by_name(&bs, "main").merged, "the base's local twin");
        assert!(!by_name(&bs, "origin/main").merged, "the base itself");
        assert!(!by_name(&bs, "origin/HEAD").merged, "an alias");
        let fresh = by_name(&bs, "fresh");
        assert!(
            fresh.is_current && !fresh.merged,
            "never the checked-out one"
        );
    }

    #[cfg(unix)]
    #[test]
    fn branches_without_any_base_flag_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git_fixture::git(dir, &["init", "-q", "-b", "trunk"]);
        git_fixture::commit_file(dir, "a.txt", "a\n", "first");
        git_fixture::git(dir, &["branch", "side"]);
        let out = git_fixture::run_body(dir, &branches_body());
        let bs = parse_branches_with_merged(&out.stdout);
        assert_eq!(bs.len(), 2, "{bs:?}");
        assert!(bs.iter().all(|b| !b.merged), "{bs:?}");
    }

    #[test]
    fn branch_list_without_the_merged_section_still_parses() {
        // An older hub's answer, or a body cut before the RS byte.
        let raw = b"refs/heads/main\x1fabc\x1f*\x1f\x1f\n";
        let bs = parse_branches_with_merged(raw);
        assert_eq!(bs.len(), 1);
        assert!(!bs[0].merged);
    }

    #[test]
    fn a_branch_without_merged_on_the_wire_reads_as_not_merged() {
        let b: Branch = serde_json::from_str(
            r#"{"name":"x","isCurrent":false,"isRemote":false,"upstream":null,"ahead":0,"behind":0,"tipHash":"a"}"#,
        )
        .unwrap();
        assert!(!b.merged);
    }

    #[cfg(unix)]
    #[test]
    fn blame_splits_a_file_into_commits_and_uncommitted_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git_fixture::git(dir, &["init", "-q"]);
        git_fixture::commit_file(dir, "f.txt", "one\ntwo\n", "first two");
        git_fixture::commit_file(dir, "f.txt", "one\ntwo\nthree\nfour\n", "add three, four");
        std::fs::write(dir.join("f.txt"), "ONE\ntwo\nthree\nfour\n").unwrap();

        let out = git_fixture::run_body(dir, &blame_body("f.txt"));
        let (hunks, truncated) = parse_blame_porcelain(&out.stdout, MAX_BLAME_LINES);
        assert!(!truncated);
        let shape: Vec<(u32, u32, bool, &str)> = hunks
            .iter()
            .map(|h| (h.start, h.lines, h.uncommitted, h.summary.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (1, 1, true, "Version of f.txt from f.txt"),
                (2, 1, false, "first two"),
                (3, 2, false, "add three, four"),
            ]
        );
        assert_eq!(hunks[1].author, "Ada");
        assert!(hunks[1].time > 1_500_000_000, "{}", hunks[1].time);
        assert_eq!(hunks[1].hash.len(), 40);
        // A commit seen a second time keeps the metadata from its first
        // appearance (porcelain prints it once).
        let (capped, truncated) = parse_blame_porcelain(&out.stdout, 2);
        assert!(truncated);
        assert_eq!(capped.iter().map(|h| h.lines).sum::<u32>(), 2);
    }

    /// Review r04 T1/T2: a symlinked directory (or, for a file read, a
    /// symlinked file) never reads past the worktree; real files still read.
    #[cfg(unix)]
    #[test]
    fn a_symlink_never_reads_past_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("id_ed25519"), "SECRET-R04\n").unwrap();
        let dir = tmp.path().join("repo");
        std::fs::create_dir(&dir).unwrap();
        git_fixture::git(&dir, &["init", "-q"]);
        git_fixture::commit_file(&dir, "a.txt", "a\n", "first");
        std::os::unix::fs::symlink("../outside", dir.join("docs")).unwrap();
        std::os::unix::fs::symlink("../outside/id_ed25519", dir.join("leak")).unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let run = |body: String| {
            let script = format!("set -e\nroot={}\n{body}", quote(&dir.to_string_lossy()));
            crate::proc::std_command("bash")
                .arg("-c")
                .arg(script)
                .output()
                .unwrap()
        };
        for body in [
            file_body("docs/id_ed25519"),
            file_body("leak"),
            untracked_diff_body("docs/id_ed25519"),
        ] {
            let out = run(body);
            assert!(!out.status.success());
            assert!(outside_err(&out).is_some(), "{out:?}");
            assert!(!String::from_utf8_lossy(&out.stdout).contains("SECRET-R04"));
        }
        let ok = run(file_body("a.txt"));
        assert!(ok.status.success(), "{ok:?}");
        assert_eq!(ok.stdout, b"a\n");
        let diff = run(untracked_diff_body("new.txt"));
        assert!(
            String::from_utf8_lossy(&diff.stdout).contains("+fresh"),
            "{diff:?}"
        );
    }

    /// A committed file whose change was staged and then reverted in the
    /// worktree has an empty `diff HEAD`; the fallback must not then show
    /// it as all added.
    #[cfg(unix)]
    #[test]
    fn a_staged_then_reverted_file_is_not_shown_as_all_added() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git_fixture::git(dir, &["init", "-q"]);
        git_fixture::commit_file(dir, "a.txt", "a\n", "first");
        std::fs::write(dir.join("a.txt"), "b\n").unwrap();
        git_fixture::git(dir, &["add", "a.txt"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        let out = git_fixture::run_body(dir, &untracked_diff_body("a.txt"));
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty(), "{out:?}");
        // A repo with no commit yet still shows a staged file as added.
        let fresh = tempfile::tempdir().unwrap();
        git_fixture::git(fresh.path(), &["init", "-q"]);
        std::fs::write(fresh.path().join("n.txt"), "n\n").unwrap();
        git_fixture::git(fresh.path(), &["add", "n.txt"]);
        let out = git_fixture::run_body(fresh.path(), &untracked_diff_body("n.txt"));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("+n"),
            "{out:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn blame_of_an_untracked_file_is_git_s_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git_fixture::git(dir, &["init", "-q"]);
        git_fixture::commit_file(dir, "a.txt", "a\n", "first");
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        let script = format!(
            "set -e\nroot={}\n{}",
            quote(&dir.to_string_lossy()),
            blame_body("new.txt")
        );
        let out = crate::proc::std_command("bash")
            .arg("-c")
            .arg(script)
            .output()
            .unwrap();
        assert!(!out.status.success());
    }
}

/// Redesign 5.6: the plan's "repo tests on a branch two commits ahead".
#[cfg(all(test, unix))]
mod branch_diff_tests {
    use super::git_fixture::{commit_file, git, run_body};
    use super::*;
    use std::path::{Path, PathBuf};

    /// `origin` with `main` pushed (`origin/HEAD` → `origin/main`), and
    /// `feat` two commits ahead of it: `x.txt`, then `y.txt`.
    fn two_ahead(tmp: &Path) -> PathBuf {
        let origin = tmp.join("origin.git");
        let work = tmp.join("work");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        git(&origin, &["init", "-q", "--bare"]);
        git(&work, &["init", "-q"]);
        commit_file(&work, "a.txt", "a\n", "first");
        git(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&work, &["push", "-q", "origin", "main"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        git(&work, &["checkout", "-q", "-b", "feat"]);
        commit_file(&work, "x.txt", "x\n", "add x");
        commit_file(&work, "y.txt", "y\n", "add y");
        work
    }

    fn diff_of(work: &Path) -> BranchDiff {
        parse_branch_diff(&run_body(work, &branch_diff_body()).stdout)
    }

    fn paths(files: &[ChangedFile]) -> Vec<&str> {
        files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn a_branch_never_pushed_has_every_commit_unpushed() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        let d = diff_of(&work);
        assert_eq!(d.branch.as_deref(), Some("feat"));
        assert_eq!(d.upstream, None);
        assert_eq!(
            d.unpushed
                .iter()
                .map(|c| c.subject.as_str())
                .collect::<Vec<_>>(),
            ["add y", "add x"]
        );
        assert_eq!(paths(&d.unpushed_files), ["x.txt", "y.txt"]);
        assert!(!d.truncated);
        assert_eq!(d.base.as_deref(), Some("origin/main"));
        assert_eq!(d.ahead_of_base, 2);
        assert_eq!(paths(&d.base_files), ["x.txt", "y.txt"]);
        assert!(d.base_files.iter().all(|f| f.status == "added"));
    }

    #[test]
    fn a_pushed_branch_lists_only_what_follows_its_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        git(
            &work,
            &["push", "-q", "-u", "origin", "HEAD~1:refs/heads/feat"],
        );
        git(&work, &["branch", "-q", "--set-upstream-to=origin/feat"]);
        let d = diff_of(&work);
        assert_eq!(d.upstream.as_deref(), Some("origin/feat"));
        assert_eq!(d.unpushed.len(), 1);
        assert_eq!(d.unpushed[0].subject, "add y");
        assert_eq!(paths(&d.unpushed_files), ["y.txt"]);
        // Against the base it is still both commits.
        assert_eq!(d.ahead_of_base, 2);
        assert_eq!(paths(&d.base_files), ["x.txt", "y.txt"]);
    }

    #[test]
    fn a_branch_in_step_with_its_remote_and_base_carries_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        git(&work, &["checkout", "-q", "main"]);
        git(&work, &["branch", "-q", "--set-upstream-to=origin/main"]);
        let d = diff_of(&work);
        assert_eq!(d.branch.as_deref(), Some("main"));
        assert!(d.unpushed.is_empty() && d.unpushed_files.is_empty());
        assert_eq!(d.ahead_of_base, 0);
        assert!(d.base_files.is_empty());
    }

    #[test]
    fn an_unborn_head_answers_empty() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q"]);
        let d = diff_of(tmp.path());
        assert_eq!(d.branch.as_deref(), Some("main"));
        assert!(d.unpushed.is_empty() && d.base_files.is_empty());
        assert_eq!(d.ahead_of_base, 0);
    }

    #[test]
    fn a_range_diff_covers_its_range_only() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        git(
            &work,
            &["push", "-q", "-u", "origin", "HEAD~1:refs/heads/feat"],
        );
        git(&work, &["branch", "-q", "--set-upstream-to=origin/feat"]);
        let diff = |range, path| {
            String::from_utf8(run_body(&work, &range_diff_body(range, path)).stdout).unwrap()
        };
        assert!(diff(DiffRange::Unpushed, "y.txt").contains("+y"));
        assert_eq!(diff(DiffRange::Unpushed, "x.txt"), "", "x is pushed");
        assert!(diff(DiffRange::Base, "x.txt").contains("+x"));
        // A path with a shell metacharacter stays one argument.
        assert_eq!(diff(DiffRange::Base, "a b;$(true).txt"), "");
    }

    // --- M15 G1.10: what the phone's Files tab shows per file and commit.

    fn counts(files: &[ChangedFile]) -> Vec<(&str, Option<u32>, Option<u32>)> {
        files
            .iter()
            .map(|f| (f.path.as_str(), f.added, f.removed))
            .collect()
    }

    #[test]
    fn the_branch_counts_commits_behind_its_base() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        assert_eq!(diff_of(&work).behind_base, Some(0));
        // main moves on by two commits on the remote.
        git(&work, &["checkout", "-q", "main"]);
        commit_file(&work, "m1.txt", "1\n", "m1");
        commit_file(&work, "m2.txt", "2\n", "m2");
        git(&work, &["push", "-q", "origin", "main"]);
        git(&work, &["checkout", "-q", "feat"]);
        let d = diff_of(&work);
        assert_eq!((d.ahead_of_base, d.behind_base), (2, Some(2)));
        // An unborn HEAD has no base to be behind.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        assert_eq!(diff_of(&empty).behind_base, None);
    }

    #[test]
    fn the_worktree_changes_carry_their_line_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        std::fs::write(work.join("x.txt"), "x\nmore\nand more\n").unwrap();
        std::fs::write(work.join("y.txt"), "").unwrap();
        git(&work, &["add", "y.txt"]);
        git(&work, &["mv", "a.txt", "b c.txt"]);
        std::fs::write(work.join("bin.dat"), [0u8, 1, 2, 0, 255]).unwrap();
        git(&work, &["add", "bin.dat"]);
        std::fs::write(work.join("new.txt"), "n\n").unwrap();
        let mut files = parse_changes(&run_body(&work, changes_body()).stdout);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            counts(&files),
            [
                ("b c.txt", Some(0), Some(0)),
                ("bin.dat", None, None),
                ("new.txt", None, None),
                ("x.txt", Some(2), Some(0)),
                ("y.txt", Some(0), Some(1)),
            ]
        );
        assert_eq!(files[0].orig_path.as_deref(), Some("a.txt"));
        assert_eq!(files[2].status, "untracked");
        // An unborn HEAD lists its files, with no counts.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        std::fs::write(empty.join("f.txt"), "f\n").unwrap();
        let files = parse_changes(&run_body(&empty, changes_body()).stdout);
        assert_eq!(counts(&files), [("f.txt", None, None)]);
    }

    #[test]
    fn a_commit_carries_its_line_counts_and_whether_it_is_pushed() {
        let tmp = tempfile::tempdir().unwrap();
        let work = two_ahead(tmp.path());
        std::fs::write(work.join("x.txt"), "x2\nx3\n").unwrap();
        git(&work, &["add", "x.txt"]);
        git(&work, &["rm", "-q", "y.txt"]);
        git(
            &work,
            &[
                "-c",
                "user.name=Ada",
                "commit",
                "-q",
                "-m",
                "rework\n\nbody",
            ],
        );
        let show = |rev: &str| {
            let hash = String::from_utf8(
                crate::proc::std_command("git")
                    .arg("-C")
                    .arg(&work)
                    .args(["rev-parse", rev])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap();
            parse_commit(&run_body(&work, &commit_body(hash.trim())).stdout)
        };
        let c = show("HEAD");
        assert_eq!((c.subject.as_str(), c.body.as_str()), ("rework", "body"));
        let mut files = c.files.clone();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            counts(&files),
            [("x.txt", Some(2), Some(1)), ("y.txt", Some(0), Some(1))]
        );
        assert_eq!(c.pushed, Some(false), "feat was never pushed");
        assert_eq!(show("main").pushed, Some(true), "main is on origin");
    }

    #[test]
    fn numstat_reads_renames_and_binaries() {
        let raw = b"3\t1\tsrc/a.rs\0-\t-\tlogo.png\0\x30\t2\t\0old.rs\0new.rs\0";
        let m = parse_numstat_z(raw);
        assert_eq!(m.get("src/a.rs"), Some(&(Some(3), Some(1))));
        assert_eq!(m.get("logo.png"), Some(&(None, None)));
        assert_eq!(m.get("new.rs"), Some(&(Some(0), Some(2))));
        assert!(!m.contains_key("old.rs"));
    }

    /// An older hub sends none of the new fields; a newer one leaves them
    /// out when unknown, so the desktop and the phone read both.
    #[test]
    fn the_new_fields_are_optional_on_the_wire() {
        let f: ChangedFile = serde_json::from_str(
            r#"{"path":"a","status":"modified","staged":false,"orig_path":null}"#,
        )
        .unwrap();
        assert_eq!((f.added, f.removed), (None, None));
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.get("added").is_none() && v.get("removed").is_none());
        let d: BranchDiff = serde_json::from_str(
            r#"{"branch":null,"upstream":null,"unpushed":[],"unpushedFiles":[],
                "truncated":false,"base":null,"aheadOfBase":0,"baseFiles":[]}"#,
        )
        .unwrap();
        assert_eq!(d.behind_base, None);
        let c: CommitDetail = serde_json::from_str(
            r#"{"hash":"h","subject":"s","body":"","author":"a","date":"d","files":[]}"#,
        )
        .unwrap();
        assert_eq!(c.pushed, None);
    }

    #[test]
    fn the_range_parses_in_lowercase() {
        let a: RepoRangeDiffArgs =
            serde_json::from_str(r#"{"session_id":1,"path":"a","range":"base"}"#).unwrap();
        assert_eq!(a.range, DiffRange::Base);
        assert!(serde_json::from_str::<RepoRangeDiffArgs>(
            r#"{"session_id":1,"path":"a","range":"x"}"#
        )
        .is_err());
    }
}
