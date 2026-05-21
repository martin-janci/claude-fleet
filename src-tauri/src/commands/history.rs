//! Read commands for the History & Branches views: commit log, branch list,
//! one commit's metadata + changed files, and a file's diff within a commit.

use crate::commands::files::{classify, ChangedFile, FileDiff};
use crate::commands::repo::{diff_from_bytes, repo_err, repo_script, run_in_repo, session_target};
use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Default page size for `repo_log`.
const LOG_DEFAULT_LIMIT: u32 = 200;

/// git log record/field separators. RS starts a record, US separates fields.
/// `--pretty=format:` with these gives unambiguous parsing of multi-field rows.
const LOG_FORMAT: &str = "--pretty=format:%x1e%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%aI%x1f%s";

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

/// Commit log for a session's worktree. `all` includes every ref so the
/// frontend can draw a branch tree; otherwise it's HEAD's history.
#[tauri::command]
pub async fn repo_log(
    args: RepoLogArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<Commit>, IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let limit = if args.limit == 0 {
        LOG_DEFAULT_LIMIT
    } else {
        args.limit.min(2000)
    };
    let all = if args.all { "--all" } else { "" };
    let body = format!(
        "git -C \"$root\" log {all} --date=iso-strict {fmt} --max-count={limit} --skip={skip}",
        all = all,
        fmt = LOG_FORMAT,
        limit = limit,
        skip = args.skip,
    );
    let script = repo_script(&name, &body);
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(parse_log(&out.stdout))
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

/// Local + remote branches for a session's worktree.
#[tauri::command]
pub async fn repo_branches(
    args: crate::commands::files::SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<Branch>, IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let body = format!(
        "git -C \"$root\" for-each-ref {fmt} refs/heads refs/remotes",
        fmt = BRANCH_FORMAT,
    );
    let script = repo_script(&name, &body);
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(parse_branches(&out.stdout))
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

#[derive(Deserialize)]
pub struct RepoCommitArgs {
    pub session_id: i64,
    pub hash: String,
}

/// One commit's metadata + the files it changed (first-parent for merges).
#[tauri::command]
pub async fn repo_commit(
    args: RepoCommitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<CommitDetail, IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let h = shq(&args.hash);
    // Two git calls: metadata (US-separated) then NUL name-status. `set -e`
    // (from repo_script) aborts on a bad hash.
    let body = format!(
        "git -C \"$root\" show -s --date=iso-strict \
           --pretty=format:%H%x1f%s%x1f%b%x1f%an%x1f%aI {h}; \
         printf '\\036'; \
         git -C \"$root\" show --first-parent --name-status -z --pretty=format: {h}"
    );
    let script = repo_script(&name, &body);
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
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

#[derive(Deserialize)]
pub struct RepoCommitDiffArgs {
    pub session_id: i64,
    pub hash: String,
    pub path: String,
}

/// A single file's diff *within* a commit (first-parent for merges), so the
/// existing DiffView can render it.
#[tauri::command]
pub async fn repo_commit_diff(
    args: RepoCommitDiffArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileDiff, IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    crate::validate::repo_rel_path(&args.path)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let body = format!(
        "git -C \"$root\" show --first-parent --format= {h} -- {p}",
        h = shq(&args.hash),
        p = shq(&args.path),
    );
    let script = repo_script(&name, &body);
    let out = run_in_repo(&ssh, &host, &script).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
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
