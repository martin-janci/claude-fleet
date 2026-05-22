# Git history, branches & branch tree — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a commit-history view (interactive branch-tree graph), a branches view, and git actions (checkout, branch create/delete, stage & commit, checkout commit, fetch/pull/push) to the Files tab.

**Architecture:** Extend the existing files command module. A new `commands/repo.rs` holds the shared git plumbing extracted from `files.rs`; `commands/history.rs` adds read commands and `commands/mutate.rs` adds mutating commands, all running `git` in the session's worktree (local or over SSH) via the existing transport. The commit graph is computed by a pure, unit-tested frontend module (`src/lib/graph.ts`) and rendered with SVG; commit details reuse the existing FileList + DiffView split.

**Tech Stack:** Rust (Tauri 2 commands), Svelte 5 runes, TypeScript, Vitest (frontend), `cargo test` (backend).

> **Build caveat (from CLAUDE.md):** `cargo` builds need Tauri system libs; on a headless box `cargo test`/`clippy` fail in a build script — an environment gap, not a code error. Run backend tests where those libs exist. Frontend (`pnpm test`) runs anywhere. Some pre-existing frontend tests fail with `localStorage is undefined`; verify against `main` before blaming a change.

> **Reference spec:** `docs/specs/2026-05-22-git-history-branches-design.md`

---

## File structure

New backend:
- `src-tauri/src/commands/repo.rs` — shared git plumbing (extracted from `files.rs`) + diff post-processing helper.
- `src-tauri/src/commands/history.rs` — `repo_log`, `repo_branches`, `repo_commit`, `repo_commit_diff` + their parsers.
- `src-tauri/src/commands/mutate.rs` — `repo_checkout`, `repo_checkout_commit`, `repo_create_branch`, `repo_delete_branch`, `repo_stage`, `repo_unstage`, `repo_commit_create`, `repo_fetch`, `repo_pull`, `repo_push`.

New frontend:
- `src/lib/graph.ts` — pure lane-assignment algorithm for the commit graph.
- `src/lib/graph.test.ts` — graph fixtures.
- `src/lib/history.ts` — IPC wrappers + TS types.
- `src/lib/CommitGraph.svelte` — full-width graph view.
- `src/lib/BranchList.svelte` — branches view.

Modified:
- `src-tauri/src/commands/files.rs` — import shared plumbing from `repo.rs`; expose `ChangedFile`/`FileDiff`/`classify` for reuse.
- `src-tauri/src/commands/mod.rs` — `pub mod repo; pub mod history; pub mod mutate;`.
- `src-tauri/src/lib.rs` — register new commands in `invoke_handler`.
- `src-tauri/src/validate.rs` — add `commit_hash`.
- `src/lib/FilesPanel.svelte` — new modes, routing, remote toolbar, commit-detail drill-in, mutation wiring.
- `src/lib/FileList.svelte` — stage/unstage checkboxes + commit footer in Changed mode.
- `src/lib/FileViewer.svelte` — optional `commit` prop.

---

## Phase 0 — Extract shared git plumbing

### Task 1: Create `commands/repo.rs` and move the shared helpers out of `files.rs`

**Files:**
- Create: `src-tauri/src/commands/repo.rs`
- Modify: `src-tauri/src/commands/files.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: Create `commands/repo.rs` with the extracted helpers**

Move these *verbatim* out of `files.rs` into a new file (they currently live in `files.rs`): the constants `REPO_TIMEOUT_SECS`, `MAX_FILE_BYTES`, `MAX_DIFF_BYTES`, `MAX_TREE_ENTRIES`; and the functions `session_target`, `repo_script`, `run_in_repo`, `repo_err`. Make them `pub`. Also add a new `diff_from_bytes` helper (extracted from the tail of `repo_diff`) so commit diffs reuse the same binary/truncation handling.

```rust
//! Shared plumbing for the git-backed Files/History/Branches commands.
//!
//! Every command resolves the session's worktree live: ask tmux for the
//! session pane's cwd, then `git rev-parse --show-toplevel`. This is
//! host-correct for remote sessions. Every interpolated value is shell-quoted
//! (`shell::quote`); frontend paths/refs/hashes are additionally validated.

use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
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

/// Resolve a session id to its `(host_alias, tmux_name)`, validating both.
pub fn session_target(
    store: &Mutex<Store>,
    session_id: i64,
) -> Result<(String, String), IpcError> {
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
         root=\"$(git -C \"$p\" rev-parse --show-toplevel)\"\n\
         {body}",
        name = shq(tmux_name),
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
            &["bash", "-lc", &shq(script)],
            Duration::from_secs(REPO_TIMEOUT_SECS),
        )
        .await
    }
}

/// Turn a failed `Output` into an `E_REPO` error carrying stderr (or stdout).
pub fn repo_err(out: &std::process::Output) -> IpcError {
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
```

- [ ] **Step 2: Update `files.rs` to use `repo.rs`**

Delete the moved constants/functions from `files.rs`. Add at the top:

```rust
use crate::commands::repo::{
    diff_from_bytes, repo_err, repo_script, run_in_repo, session_target, MAX_DIFF_BYTES,
    MAX_FILE_BYTES, MAX_TREE_ENTRIES,
};
```

Make the items History needs reusable — change these `files.rs` declarations to `pub`:

```rust
pub struct ChangedFile { /* unchanged fields */ }
pub struct FileDiff { /* unchanged fields */ }
pub fn classify(x: char, y: char) -> (&'static str, bool) { /* unchanged body */ }
```

In `repo_diff`, replace the inline binary/truncation tail (the block computing `binary`, `truncated`, `diff`) with:

```rust
    let (diff, binary, truncated) = diff_from_bytes(&raw);
    Ok(FileDiff { path: args.path, diff, binary, truncated })
```

Keep the `repo_script`/`session_target` test in `files.rs` only if it still references local items; otherwise move `repo_script_embeds_quoted_name_and_body` into `repo.rs`'s test module (see Step 4). The `classify` and `parse_status_z` tests stay in `files.rs`.

- [ ] **Step 3: Register the module**

In `src-tauri/src/commands/mod.rs` add the line (keep alphabetical-ish ordering):

```rust
pub mod repo;
```

- [ ] **Step 4: Move the `repo_script` unit test into `repo.rs`**

Add to `repo.rs`:

```rust
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
```

Remove the now-duplicated `repo_script_embeds_quoted_name_and_body` from `files.rs`'s test module.

- [ ] **Step 5: Build & test backend**

Run: `cd src-tauri && cargo test commands::repo && cargo test commands::files`
Expected: PASS (parsers + repo_script + diff_from_bytes). On a headless box, see the build caveat above.

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 6: Confirm frontend untouched still passes**

Run: `pnpm test -- files_view`
Expected: PASS (no frontend changes yet).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands/repo.rs src-tauri/src/commands/files.rs src-tauri/src/commands/mod.rs
git commit -m "refactor(repo): extract shared git plumbing from files.rs into repo.rs"
```

---

## Phase 1 — Backend read commands

### Task 2: Add `validate::commit_hash`

**Files:**
- Modify: `src-tauri/src/validate.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `validate.rs`:

```rust
#[test]
fn commit_hash_accepts_hex_rejects_junk() {
    assert!(commit_hash("a1b2c3d").is_ok());
    assert!(commit_hash("0123456789abcdef0123456789abcdef01234567").is_ok());
    assert!(commit_hash("ABC").is_err());        // uppercase not produced by git short hashes we use
    assert!(commit_hash("xyz").is_err());        // non-hex
    assert!(commit_hash("").is_err());
    assert!(commit_hash("-rf").is_err());
    assert!(commit_hash("123").is_err());        // too short (<4)
    assert!(commit_hash(&"a".repeat(41)).is_err()); // too long (>40)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test commit_hash`
Expected: FAIL — `cannot find function commit_hash`.

- [ ] **Step 3: Implement `commit_hash`**

Add to `validate.rs` (after `git_ref`):

```rust
/// Validate a git commit hash supplied by the frontend (the commit the user
/// clicked in the History graph). Git object names are lowercase hex; we
/// accept an abbreviated or full SHA-1 (4–40 chars) and nothing else, so the
/// value cannot be read as an option or inject shell/git syntax.
pub fn commit_hash(value: &str) -> Result<(), IpcError> {
    if value.len() < 4 || value.len() > 40 {
        return Err(IpcError::new(
            "E_INVALID",
            "commit hash must be 4–40 characters",
        ));
    }
    if !value.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')) {
        return Err(IpcError::new(
            "E_INVALID",
            "commit hash must be lowercase hexadecimal",
        ));
    }
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test commit_hash`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/validate.rs
git commit -m "feat(validate): add commit_hash validator"
```

### Task 3: `parse_log` + `repo_log`

**Files:**
- Create: `src-tauri/src/commands/history.rs`
- Modify: `src-tauri/src/commands/mod.rs`

The git format string uses RS (`\x1e`, 0x1e) to start each record and US (`\x1f`, 0x1f) between fields. Fields, in order: full hash, short hash, space-separated parents (`%P`), ref decoration (`%D`), author name, ISO-strict date, subject. `%s` is single-line, so no embedded record/field separators occur in normal data.

- [ ] **Step 1: Write the failing test (parser)**

Create `history.rs` with the wire types, the format constant, the parser, and a test module:

```rust
//! Read commands for the History & Branches views: commit log, branch list,
//! one commit's metadata + changed files, and a file's diff within a commit.

use crate::commands::files::{classify, ChangedFile, FileDiff};
use crate::commands::repo::{
    diff_from_bytes, repo_err, repo_script, run_in_repo, session_target,
};
use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Default page size for `repo_log`.
const LOG_DEFAULT_LIMIT: u32 = 200;

/// git log record/field separators. RS starts a record, US separates fields.
/// `--pretty=format:` with these gives unambiguous parsing of multi-field rows.
const LOG_FORMAT: &str =
    "--pretty=format:%x1e%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%aI%x1f%s";

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
            out.push(GitRef { name: "HEAD".into(), kind: "head".into() });
            out.push(GitRef { name: rest.trim().into(), kind: "branch".into() });
        } else if t == "HEAD" {
            out.push(GitRef { name: "HEAD".into(), kind: "head".into() });
        } else if let Some(tag) = t.strip_prefix("tag: ") {
            out.push(GitRef { name: tag.trim().into(), kind: "tag".into() });
        } else if t.contains('/') {
            out.push(GitRef { name: t.into(), kind: "remote".into() });
        } else {
            out.push(GitRef { name: t.into(), kind: "branch".into() });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_log_reads_fields_and_parents() {
        // Two records: a merge (2 parents, decorated) then a root commit.
        let raw = "\u{1e}aaaa\u{1f}aaa\u{1f}bbbb cccc\u{1f}HEAD -> main, origin/main\u{1f}MJ\u{1f}2026-05-22T10:00:00+02:00\u{1f}Merge branch x\u{1e}dddd\u{1f}ddd\u{1f}\u{1f}\u{1f}MJ\u{1f}2026-05-20T09:00:00+02:00\u{1f}initial";
        let commits = parse_log(raw.as_bytes());
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "aaaa");
        assert_eq!(commits[0].parents, vec!["bbbb", "cccc"]);
        assert_eq!(commits[0].subject, "Merge branch x");
        assert_eq!(commits[0].refs, vec![
            GitRef { name: "HEAD".into(), kind: "head".into() },
            GitRef { name: "main".into(), kind: "branch".into() },
            GitRef { name: "origin/main".into(), kind: "remote".into() },
        ]);
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
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test commands::history`
Expected: FAIL — `history` module not declared in `mod.rs` yet, or compile error.

- [ ] **Step 3: Declare the module and add the `repo_log` command**

In `commands/mod.rs` add:

```rust
pub mod history;
```

Append the command to `history.rs` (before the test module):

```rust
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
    let limit = if args.limit == 0 { LOG_DEFAULT_LIMIT } else { args.limit.min(2000) };
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
```

Add the missing import at the top of `history.rs`:

```rust
use tauri::State;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test commands::history`
Expected: PASS.

- [ ] **Step 5: Register `repo_log` in the invoke handler**

In `src-tauri/src/lib.rs`, after `commands::files::repo_diff,` (line ~308) add:

```rust
            commands::history::repo_log,
```

- [ ] **Step 6: clippy/fmt + commit**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

```bash
git add src-tauri/src/commands/history.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "feat(history): repo_log command with structured commit parsing"
```

### Task 4: `parse_branches` + `repo_branches`

**Files:**
- Modify: `src-tauri/src/commands/history.rs`
- Modify: `src-tauri/src/lib.rs`

`git for-each-ref` format uses US separators. Per-ref fields: `%(refname)`, `%(objectname:short)`, `%(HEAD)` (`*` for current else ` `), `%(upstream:short)`, `%(upstream:track)` (e.g. `[ahead 1, behind 2]`).

- [ ] **Step 1: Write the failing test**

Add to `history.rs` types + test:

```rust
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
```

Add test:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test parse_branches`
Expected: FAIL — `cannot find function parse_branches`.

- [ ] **Step 3: Implement parser + command**

Add to `history.rs`:

```rust
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
            upstream: if f[3].is_empty() { None } else { Some(f[3].to_string()) },
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
```

Make `SessionIdArgs` reusable: in `files.rs` change `pub struct SessionIdArgs` (it is already `pub`; confirm and keep). If `parse_track` triggers a dead-code warning when only used in tests, that won't happen here since `parse_branches` uses it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test parse_branches`
Expected: PASS.

- [ ] **Step 5: Register `repo_branches`**

In `lib.rs` after `commands::history::repo_log,` add:

```rust
            commands::history::repo_branches,
```

- [ ] **Step 6: clippy/fmt + commit**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

```bash
git add src-tauri/src/commands/history.rs src-tauri/src/lib.rs
git commit -m "feat(history): repo_branches command with ahead/behind parsing"
```

### Task 5: `repo_commit` (metadata + changed files) and `repo_commit_diff`

**Files:**
- Modify: `src-tauri/src/commands/history.rs`
- Modify: `src-tauri/src/lib.rs`

`git show --name-status -z` emits, for each file, a status token (`M`, `A`, `D`, `R100`, `C075`) and the path(s), NUL-separated. For renames/copies a second path token follows.

- [ ] **Step 1: Write the failing test (name-status parser)**

Add to `history.rs`:

```rust
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
```

Add test:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test parse_name_status`
Expected: FAIL — function missing.

- [ ] **Step 3: Implement parser + both commands**

Add to `history.rs`. Reuse `classify` from `files.rs` by mapping the single status letter into the XY pair it expects:

```rust
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
            out.push(ChangedFile { path, status: status.to_string(), staged: false, orig_path: Some(orig) });
        } else if i < tokens.len() {
            let path = tokens[i].to_string();
            i += 1;
            out.push(ChangedFile { path, status: status.to_string(), staged: false, orig_path: None });
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
    Ok(FileDiff { path: args.path, diff, binary, truncated })
}

#[derive(Deserialize)]
pub struct RepoCommitDiffArgs {
    pub session_id: i64,
    pub hash: String,
    pub path: String,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test parse_name_status`
Expected: PASS.

- [ ] **Step 5: Register both commands**

In `lib.rs` after `commands::history::repo_branches,` add:

```rust
            commands::history::repo_commit,
            commands::history::repo_commit_diff,
```

- [ ] **Step 6: clippy/fmt + commit**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

```bash
git add src-tauri/src/commands/history.rs src-tauri/src/lib.rs
git commit -m "feat(history): repo_commit + repo_commit_diff commands"
```

---

## Phase 2 — Frontend graph algorithm & IPC

### Task 6: `graph.ts` lane-assignment algorithm

**Files:**
- Create: `src/lib/graph.ts`
- Create: `src/lib/graph.test.ts`

The algorithm assigns each commit a column (lane). Per row we capture the lane occupancy entering the row (`lanesIn`) and leaving it (`lanesOut`) plus the commit's `column` and `color`; the renderer draws connectors from `lanesIn` to `lanesOut` per row.

- [ ] **Step 1: Write the failing tests**

Create `src/lib/graph.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { computeGraph, type GraphInput } from './graph';

const lin: GraphInput[] = [
  { hash: 'c', parents: ['b'] },
  { hash: 'b', parents: ['a'] },
  { hash: 'a', parents: [] },
];

const branchMerge: GraphInput[] = [
  { hash: 'm', parents: ['c', 'f'] }, // merge of main(c) and feature(f)
  { hash: 'c', parents: ['b'] },
  { hash: 'f', parents: ['b'] },
  { hash: 'b', parents: ['a'] },
  { hash: 'a', parents: [] },
];

describe('computeGraph', () => {
  it('keeps a linear history in one column', () => {
    const rows = computeGraph(lin);
    expect(rows.map((r) => r.column)).toEqual([0, 0, 0]);
    expect(rows[0].color).toBe(rows[2].color); // first-parent inherits color
  });

  it('opens a second lane for a branch and closes it at the fork point', () => {
    const rows = computeGraph(branchMerge);
    const byHash = Object.fromEntries(rows.map((r) => [r.hash, r]));
    // merge sits in lane 0; its second parent f occupies a new lane.
    expect(byHash['m'].column).toBe(0);
    expect(byHash['f'].column).toBeGreaterThan(0);
    // after the merge row, two lanes are live (c and f).
    expect(byHash['m'].lanesOut.filter((x) => x !== null).length).toBe(2);
    // b is the common ancestor — both lanes converge, so at/after b only one lane remains.
    expect(byHash['b'].lanesOut.filter((x) => x !== null).length).toBe(1);
  });

  it('handles an empty input', () => {
    expect(computeGraph([])).toEqual([]);
  });

  it('handles disjoint roots without crashing', () => {
    const rows = computeGraph([
      { hash: 'x', parents: [] },
      { hash: 'y', parents: [] },
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0].column).toBe(0);
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm test -- graph`
Expected: FAIL — cannot find module `./graph`.

- [ ] **Step 3: Implement `graph.ts`**

Create `src/lib/graph.ts`:

```ts
// Pure lane-assignment for the commit graph (the "branch tree"). Input is
// commits in `git log` order (child before parent). Output is per-row layout:
// the commit's column + color, and the lane occupancy entering (`lanesIn`) and
// leaving (`lanesOut`) the row, from which CommitGraph.svelte draws connectors.
// No git ASCII --graph: keeping this here makes it testable and interactive.

export interface GraphInput {
  hash: string;
  parents: string[];
}

export interface GraphRow {
  hash: string;
  column: number;
  color: number;
  /** Lane→hash occupancy at the top of this row's cell. */
  lanesIn: (string | null)[];
  /** Lane→hash occupancy at the bottom of this row's cell. */
  lanesOut: (string | null)[];
  /** hash → color index, for every lane referenced by this row. */
  colors: Record<string, number>;
}

export function computeGraph(commits: GraphInput[]): GraphRow[] {
  const lanes: (string | null)[] = []; // persists across rows
  const color = new Map<string, number>();
  let nextColor = 0;
  const rows: GraphRow[] = [];

  const firstFree = (): number => {
    const i = lanes.indexOf(null);
    return i === -1 ? lanes.length : i;
  };

  for (const c of commits) {
    // Ensure a lane awaits this commit; a tip allocates a fresh lane+color.
    let col = lanes.indexOf(c.hash);
    if (col === -1) {
      col = firstFree();
      lanes[col] = c.hash;
      if (!color.has(c.hash)) color.set(c.hash, nextColor++);
    }
    const lanesIn = lanes.slice();
    const myColor = color.get(c.hash)!;

    // Free this commit's lane; parents are routed below.
    lanes[col] = null;

    c.parents.forEach((p, idx) => {
      if (lanes.indexOf(p) !== -1) return; // a lane already awaits this parent
      if (idx === 0) {
        lanes[col] = p; // first parent inherits the commit's lane + color
        if (!color.has(p)) color.set(p, myColor);
      } else {
        const f = firstFree();
        lanes[f] = p;
        if (!color.has(p)) color.set(p, nextColor++);
      }
    });

    while (lanes.length > 0 && lanes[lanes.length - 1] === null) lanes.pop();
    const lanesOut = lanes.slice();

    const colors: Record<string, number> = {};
    for (const h of new Set<string | null>([...lanesIn, ...lanesOut, c.hash])) {
      if (h && color.has(h)) colors[h] = color.get(h)!;
    }

    rows.push({ hash: c.hash, column: col, color: myColor, lanesIn, lanesOut, colors });
  }
  return rows;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm test -- graph`
Expected: PASS (4 tests).

- [ ] **Step 5: Type-check + commit**

Run: `pnpm check`
Expected: no new errors.

```bash
git add src/lib/graph.ts src/lib/graph.test.ts
git commit -m "feat(graph): pure lane-assignment algorithm for the commit graph"
```

### Task 7: `history.ts` IPC wrappers + types

**Files:**
- Create: `src/lib/history.ts`

- [ ] **Step 1: Implement `history.ts`**

Create `src/lib/history.ts` (mirror the Rust wire types; reuse `ChangedFile`/`FileDiff` from `files.ts`):

```ts
// IPC wrappers for the History & Branches views. Like files.ts, there is no
// long-lived store — FilesPanel holds component-local state.

import { invokeCmd, type Result } from './result';
import type { ChangedFile, FileDiff } from './files';

export interface GitRef {
  name: string;
  /** branch | remote | tag | head */
  kind: string;
}

export interface Commit {
  hash: string;
  shortHash: string;
  parents: string[];
  refs: GitRef[];
  author: string;
  date: string;
  subject: string;
}

export interface Branch {
  name: string;
  isCurrent: boolean;
  isRemote: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  tipHash: string;
}

export interface CommitDetail {
  hash: string;
  subject: string;
  body: string;
  author: string;
  date: string;
  files: ChangedFile[];
}

export function repoLog(
  sessionId: number,
  opts: { all?: boolean; limit?: number; skip?: number } = {},
): Promise<Result<Commit[]>> {
  return invokeCmd<Commit[]>('repo_log', {
    args: {
      session_id: sessionId,
      all: opts.all ?? true,
      limit: opts.limit ?? 0,
      skip: opts.skip ?? 0,
    },
  });
}

export function repoBranches(sessionId: number): Promise<Result<Branch[]>> {
  return invokeCmd<Branch[]>('repo_branches', { args: { session_id: sessionId } });
}

export function repoCommit(sessionId: number, hash: string): Promise<Result<CommitDetail>> {
  return invokeCmd<CommitDetail>('repo_commit', { args: { session_id: sessionId, hash } });
}

export function repoCommitDiff(
  sessionId: number,
  hash: string,
  path: string,
): Promise<Result<FileDiff>> {
  return invokeCmd<FileDiff>('repo_commit_diff', {
    args: { session_id: sessionId, hash, path },
  });
}

// ─── mutations (Phase 5 backend) ──────────────────────────────────────────

export function repoCheckout(sessionId: number, branch: string): Promise<Result<null>> {
  return invokeCmd<null>('repo_checkout', { args: { session_id: sessionId, branch } });
}

export function repoCheckoutCommit(sessionId: number, hash: string): Promise<Result<null>> {
  return invokeCmd<null>('repo_checkout_commit', { args: { session_id: sessionId, hash } });
}

export function repoCreateBranch(
  sessionId: number,
  name: string,
  opts: { startPoint?: string | null; checkout?: boolean } = {},
): Promise<Result<null>> {
  return invokeCmd<null>('repo_create_branch', {
    args: {
      session_id: sessionId,
      name,
      start_point: opts.startPoint ?? null,
      checkout: opts.checkout ?? false,
    },
  });
}

export function repoDeleteBranch(
  sessionId: number,
  name: string,
  force = false,
): Promise<Result<null>> {
  return invokeCmd<null>('repo_delete_branch', {
    args: { session_id: sessionId, name, force },
  });
}

export function repoStage(sessionId: number, paths: string[]): Promise<Result<null>> {
  return invokeCmd<null>('repo_stage', { args: { session_id: sessionId, paths } });
}

export function repoUnstage(sessionId: number, paths: string[]): Promise<Result<null>> {
  return invokeCmd<null>('repo_unstage', { args: { session_id: sessionId, paths } });
}

export function repoCommitCreate(
  sessionId: number,
  message: string,
  amend = false,
): Promise<Result<null>> {
  return invokeCmd<null>('repo_commit_create', {
    args: { session_id: sessionId, message, amend },
  });
}

export function repoFetch(sessionId: number): Promise<Result<null>> {
  return invokeCmd<null>('repo_fetch', { args: { session_id: sessionId } });
}

export function repoPull(sessionId: number): Promise<Result<null>> {
  return invokeCmd<null>('repo_pull', { args: { session_id: sessionId } });
}

export function repoPush(sessionId: number, setUpstream = false): Promise<Result<null>> {
  return invokeCmd<null>('repo_push', {
    args: { session_id: sessionId, set_upstream: setUpstream },
  });
}
```

- [ ] **Step 2: Type-check + commit**

Run: `pnpm check`
Expected: no new errors.

```bash
git add src/lib/history.ts
git commit -m "feat(history): frontend IPC wrappers and types"
```

---

## Phase 3 — History UI (graph + commit detail)

### Task 8: `CommitGraph.svelte`

**Files:**
- Create: `src/lib/CommitGraph.svelte`

- [ ] **Step 1: Implement the component**

Renders the SVG lane gutter + commit rows. Props: the commit list, layout rows from `computeGraph`, the selected hash, and callbacks. A small palette maps `color` index → CSS color.

```svelte
<script lang="ts" module>
  // Stable lane palette, indexed by GraphRow.color.
  const PALETTE = [
    '#58a6ff', '#3fb950', '#d29922', '#db61a2', '#a371f7',
    '#f85149', '#39c5cf', '#e3b341', '#bc8cff', '#7ee787',
  ];
  export function laneColor(i: number): string {
    return PALETTE[i % PALETTE.length];
  }
</script>

<script lang="ts">
  import type { Commit } from './history';
  import { computeGraph } from './graph';

  let {
    commits,
    selected,
    onSelect,
    onCreateBranch,
    onCheckoutCommit,
  }: {
    commits: Commit[];
    selected: string | null;
    onSelect: (hash: string) => void;
    onCreateBranch: (hash: string) => void;
    onCheckoutCommit: (hash: string) => void;
  } = $props();

  const rows = $derived(computeGraph(commits.map((c) => ({ hash: c.hash, parents: c.parents }))));
  const byHash = $derived(new Map(commits.map((c) => [c.hash, c])));

  // Geometry for the SVG gutter.
  const ROW_H = 26;
  const COL_W = 14;
  const DOT_R = 4;
  const cx = (col: number) => 8 + col * COL_W;

  const maxLanes = $derived(
    rows.reduce((m, r) => Math.max(m, r.lanesIn.length, r.lanesOut.length), 1),
  );
  const gutterW = $derived(cx(maxLanes) + 8);

  // For each row build the line segments to draw inside its cell:
  // top-half (lanesIn → dot/continuation) and bottom-half (dot/continuation → lanesOut).
  function segments(i: number): { x1: number; y1: number; x2: number; y2: number; color: string }[] {
    const r = rows[i];
    const segs: { x1: number; y1: number; x2: number; y2: number; color: string }[] = [];
    const midY = ROW_H / 2;
    const dotX = cx(r.column);
    // top half: each incoming lane goes to its continuation (same hash in lanesOut) or to the dot.
    r.lanesIn.forEach((h, col) => {
      if (h === null) return;
      const color = laneColor(r.colors[h] ?? r.color);
      if (h === r.hash) {
        segs.push({ x1: cx(col), y1: 0, x2: dotX, y2: midY, color });
      } else {
        const out = r.lanesOut.indexOf(h);
        if (out !== -1) segs.push({ x1: cx(col), y1: 0, x2: cx(out), y2: midY, color });
      }
    });
    // bottom half: dot → first-parent continuation; plus any new parent lanes.
    r.lanesOut.forEach((h, col) => {
      if (h === null) return;
      const color = laneColor(r.colors[h] ?? r.color);
      const cameFrom = r.lanesIn.indexOf(h);
      if (cameFrom === -1) {
        // a parent introduced by this commit → draw from the dot.
        segs.push({ x1: dotX, y1: midY, x2: cx(col), y2: ROW_H, color });
      } else {
        // lane passing through → straight segment bottom half.
        segs.push({ x1: cx(col), y1: midY, x2: cx(col), y2: ROW_H, color });
      }
    });
    return segs;
  }

  function rel(date: string): string {
    const t = Date.parse(date);
    if (Number.isNaN(t)) return date;
    const s = Math.floor((Date.now() - t) / 1000);
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    return `${Math.floor(s / 86400)}d`;
  }
</script>

<div class="graph" data-testid="commit-graph">
  {#each rows as r, i (r.hash)}
    {@const c = byHash.get(r.hash)}
    <div
      class="crow"
      class:sel={selected === r.hash}
      role="button"
      tabindex="0"
      onclick={() => onSelect(r.hash)}
      onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && onSelect(r.hash)}
    >
      <svg class="gutter" width={gutterW} height={ROW_H} aria-hidden="true">
        {#each segments(i) as s}
          <line x1={s.x1} y1={s.y1} x2={s.x2} y2={s.y2} stroke={s.color} stroke-width="1.5" />
        {/each}
        <circle cx={cx(r.column)} cy={ROW_H / 2} r={DOT_R} fill={laneColor(r.color)} />
      </svg>
      <span class="meta">
        {#each c?.refs ?? [] as ref}
          <span class="ref {ref.kind}">{ref.name}</span>
        {/each}
        <span class="subject" title={c?.subject}>{c?.subject}</span>
        <span class="author">{c?.author}</span>
        <span class="date">{c ? rel(c.date) : ''}</span>
      </span>
      <span class="actions">
        <button
          title="Create branch from here"
          onclick={(e) => { e.stopPropagation(); onCreateBranch(r.hash); }}>⎇</button
        >
        <button
          title="Checkout this commit (detached)"
          onclick={(e) => { e.stopPropagation(); onCheckoutCommit(r.hash); }}>⤓</button
        >
      </span>
    </div>
  {/each}
</div>

<style>
  .graph { font-size: 0.78rem; }
  .crow {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    height: 26px;
    background: transparent;
    border: none;
    color: var(--fg);
    cursor: pointer;
    padding: 0 0.4rem;
    text-align: left;
  }
  .crow:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .crow.sel { background: color-mix(in srgb, var(--accent) 22%, transparent); }
  .crow:hover .actions { visibility: visible; }
  .gutter { flex: 0 0 auto; }
  .meta {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    overflow: hidden;
  }
  .subject {
    flex: 1 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .author, .date { flex: 0 0 auto; color: var(--fg-muted); font-size: 0.72rem; }
  .ref {
    flex: 0 0 auto;
    border-radius: 3px;
    padding: 0 0.3rem;
    font-size: 0.66rem;
    font-family: var(--mono, monospace);
  }
  .ref.branch { background: color-mix(in srgb, #3fb950 30%, transparent); color: #3fb950; }
  .ref.remote { background: color-mix(in srgb, #58a6ff 28%, transparent); color: #58a6ff; }
  .ref.tag { background: color-mix(in srgb, #d29922 30%, transparent); color: #d29922; }
  .ref.head { background: color-mix(in srgb, #f85149 30%, transparent); color: #f85149; }
  .actions { flex: 0 0 auto; visibility: hidden; display: flex; gap: 0.2rem; }
  .actions button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.72rem;
    padding: 0 0.3rem;
  }
  .actions button:hover { color: var(--fg); border-color: var(--accent); }
</style>
```

- [ ] **Step 2: Type-check + commit**

Run: `pnpm check`
Expected: no new errors.

```bash
git add src/lib/CommitGraph.svelte
git commit -m "feat(history): CommitGraph SVG component"
```

### Task 9: `FileViewer` gains an optional `commit` prop

**Files:**
- Modify: `src/lib/FileViewer.svelte`

- [ ] **Step 1: Add the prop and route the diff fetch**

In `FileViewer.svelte`, add `repoCommitDiff` to the imports from `./history` and add a `commit` prop:

```ts
  import { repoCommitDiff } from './history';

  let {
    session,
    path,
    status,
    reloadKey,
    commit = null,
  }: {
    session: SessionRow;
    path: string | null;
    status: string | undefined;
    reloadKey: number;
    /** When set, show this file's diff *within* the commit, not the worktree. */
    commit?: string | null;
  } = $props();
```

In `cacheKey`, include the commit so a commit diff and a worktree diff for the same path don't collide:

```ts
  const cacheKey = (sid: number, p: string) => `${sid}:${reloadKey}:${commit ?? 'wt'}:${p}`;
```

In `load`, when fetching the diff, branch on `commit`:

```ts
      loading = true;
      const r = commit
        ? await repoCommitDiff(sid, commit, p)
        : await repoDiff(sid, p);
```

When a `commit` is set, the file's working-tree content view doesn't apply — force the diff view. Update the default-view effect:

```ts
  $effect(() => {
    if (path !== lastPath) {
      lastPath = path;
      view = commit ? 'diff' : canDiff ? 'diff' : 'file';
    }
  });
```

And hide the File toggle when viewing a commit (it would read the *worktree* file, which is misleading): add `disabled={commit !== null}` to the File button and a title explaining it.

- [ ] **Step 2: Type-check + run existing viewer tests**

Run: `pnpm check && pnpm test -- files_view`
Expected: no new type errors; existing tests still pass (the new prop is optional and defaults to `null`).

- [ ] **Step 3: Commit**

```bash
git add src/lib/FileViewer.svelte
git commit -m "feat(files): FileViewer optional commit prop for commit diffs"
```

### Task 10: History mode in `FilesPanel` — graph + commit drill-in

**Files:**
- Modify: `src/lib/FilesPanel.svelte`
- Modify: `src/lib/FileList.svelte` (mode-button labels)

- [ ] **Step 1: Extend the mode union and panel state**

In `FilesPanel.svelte`, change the mode type and add history state:

```ts
  import { repoLog, repoCommit, type Commit, type CommitDetail } from './history';
  import CommitGraph from './CommitGraph.svelte';

  let mode = $state<'changes' | 'tree' | 'history' | 'branches'>('changes');
  let commits = $state<Commit[]>([]);
  let historyLoaded = false;
  let logSkip = 0;
  let allBranches = $state(true);
  // Commit drill-in: when set, the panel shows that commit's files + diff.
  let openCommit = $state<CommitDetail | null>(null);
```

- [ ] **Step 2: Add load + paging functions**

```ts
  async function loadHistory(reset = true): Promise<void> {
    const sid = session.id;
    loading = true;
    error = null;
    if (reset) { logSkip = 0; commits = []; }
    const r = await repoLog(sid, { all: allBranches, skip: logSkip });
    if (sid !== session.id) return;
    loading = false;
    if (r.ok) {
      commits = reset ? r.value : [...commits, ...r.value];
      historyLoaded = true;
      logSkip = commits.length;
    } else {
      error = r.error.message;
    }
  }

  async function openCommitDetail(hash: string): Promise<void> {
    const sid = session.id;
    const r = await repoCommit(sid, hash);
    if (sid !== session.id) return;
    if (r.ok) { openCommit = r.value; selectedPath = r.value.files[0]?.path ?? null; }
    else error = r.error.message;
  }

  function backToGraph(): void { openCommit = null; selectedPath = null; }
```

Reset history state in the session-change effect (alongside the existing resets):

```ts
    commits = [];
    historyLoaded = false;
    openCommit = null;
```

Extend `onMode` and `onRefresh`:

```ts
  function onMode(m: typeof mode): void {
    mode = m;
    error = null;
    openCommit = null;
    if (m === 'tree' && !treeLoaded) void loadTree();
    if (m === 'history' && !historyLoaded) void loadHistory();
    if (m === 'branches') void loadBranches(); // defined in Task 11
  }

  function onRefresh(): void {
    if (mode === 'changes') void loadChanges();
    else if (mode === 'tree') void loadTree();
    else if (mode === 'history') void loadHistory();
    else void loadBranches();
    reloadKey++;
  }
```

- [ ] **Step 3: Render history mode**

Replace the single `files-panel` body with a mode-aware layout. For `history` with no `openCommit`, show the full-width graph; with an `openCommit`, show the existing list+viewer split scoped to the commit. Keep `changes`/`tree` exactly as today.

```svelte
{#if mode === 'history' && !openCommit}
  <div class="full-col" data-testid="history-view">
    <div class="hbar">
      <label><input type="checkbox" bind:checked={allBranches} onchange={() => loadHistory()} /> All branches</label>
      <RemoteToolbar {session} ondone={onRefresh} />
    </div>
    <div class="hscroll">
      {#if loading && commits.length === 0}
        <p class="hint">Loading…</p>
      {:else if error}
        <p class="hint err">{error}</p>
      {:else}
        <CommitGraph
          {commits}
          selected={null}
          onSelect={(h) => openCommitDetail(h)}
          onCreateBranch={(h) => promptCreateBranch(h)}
          onCheckoutCommit={(h) => confirmCheckoutCommit(h)}
        />
        {#if commits.length >= logSkip && commits.length > 0}
          <button class="more" onclick={() => loadHistory(false)}>Load more</button>
        {/if}
      {/if}
    </div>
  </div>
{:else}
  <!-- changes / tree / commit-drill-in: the existing split -->
  <div class="files-panel" data-testid="files-panel" style="--list-px: {listPx}px">
    <div class="list-col">
      {#if openCommit}
        <div class="commit-head">
          <button class="back" onclick={backToGraph}>← Back to graph</button>
          <div class="csub">{openCommit.subject}</div>
          <div class="cmeta">{openCommit.author} · {openCommit.hash.slice(0, 8)}</div>
        </div>
        <FileList
          mode="changes"
          changes={openCommit.files}
          tree={null}
          {loading}
          {error}
          {selectedPath}
          {onSelect}
          onMode={() => {}}
          onRefresh={() => openCommitDetail(openCommit.hash)}
        />
      {:else}
        <FileList {mode} {changes} {tree} {loading} {error} {selectedPath} {onSelect} {onMode} {onRefresh} />
      {/if}
    </div>
    <Resizer id="files-list" onresize={onResize} />
    <div class="viewer-col">
      <FileViewer {session} path={selectedPath} status={selectedStatus} {reloadKey} commit={openCommit?.hash ?? null} />
    </div>
  </div>
{/if}
```

Add styles for `.full-col`, `.hbar`, `.hscroll`, `.more`, `.commit-head`, `.back`, `.csub`, `.cmeta` (follow the existing muted/border variables). `RemoteToolbar`, `promptCreateBranch`, and `confirmCheckoutCommit` are defined in Phase 6 (Tasks 12–13); for this task, stub them as no-ops so the panel compiles, e.g.:

```ts
  // Replaced with real implementations in Phase 6.
  function promptCreateBranch(_hash: string): void {}
  function confirmCheckoutCommit(_hash: string): void {}
```

And temporarily render nothing for `RemoteToolbar` (remove its tag until Task 12) — or add an empty placeholder component. Simplest: omit `<RemoteToolbar/>` from the markup in this task and add it in Task 12.

- [ ] **Step 4: Add the History/Branches buttons to the mode toggle**

In `FileList.svelte`, widen the `mode` prop type and add buttons:

```ts
    mode: 'changes' | 'tree' | 'history' | 'branches';
    onMode: (m: 'changes' | 'tree' | 'history' | 'branches') => void;
```

```svelte
    <div class="modes">
      <button class:active={mode === 'changes'} onclick={() => onMode('changes')}>Changed</button>
      <button class:active={mode === 'tree'} onclick={() => onMode('tree')}>All files</button>
      <button class:active={mode === 'history'} onclick={() => onMode('history')}>History</button>
      <button class:active={mode === 'branches'} onclick={() => onMode('branches')}>Branches</button>
    </div>
```

Adjust the `.modes button` border-radius rules so only the first/last buttons round (the middle two get no rounding; `border-left: none` on every button after the first).

> Note: the mode toggle now lives inside `FileList`, which only renders in the split layout. Lift the mode toggle into a small shared header in `FilesPanel` (above both layouts) so History/Branches are reachable from the full-width views too. Concretely: move the `.modes` markup out of `FileList.svelte` into `FilesPanel.svelte`'s top bar, pass `mode`/`onMode` to it there, and have `FileList` accept the remaining props. Keep `FileList`'s filter/rows unchanged.

- [ ] **Step 5: Type-check + manual smoke via existing tests**

Run: `pnpm check && pnpm test -- files_view`
Expected: no new type errors; `files_view` tests pass (they test `buildTree`/`parseUnifiedDiff`, unaffected).

- [ ] **Step 6: Commit**

```bash
git add src/lib/FilesPanel.svelte src/lib/FileList.svelte
git commit -m "feat(history): History mode — graph view + commit drill-in"
```

### Task 11: Branches view — `BranchList.svelte`

**Files:**
- Create: `src/lib/BranchList.svelte`
- Modify: `src/lib/FilesPanel.svelte`

- [ ] **Step 1: Implement `BranchList.svelte`**

Read-only rendering here; action callbacks are wired by the panel (checkout/delete/new are implemented in Phase 6, but the buttons + callbacks exist now).

```svelte
<script lang="ts">
  import type { Branch } from './history';

  let {
    branches,
    loading,
    error,
    onCheckout,
    onDelete,
    onNew,
  }: {
    branches: Branch[];
    loading: boolean;
    error: string | null;
    onCheckout: (name: string) => void;
    onDelete: (name: string) => void;
    onNew: () => void;
  } = $props();

  const locals = $derived(branches.filter((b) => !b.isRemote));
  const remotes = $derived(branches.filter((b) => b.isRemote));
</script>

<div class="branches" data-testid="branch-list">
  <div class="bbar">
    <button class="new" onclick={onNew}>+ New branch</button>
  </div>
  {#if loading}
    <p class="hint">Loading…</p>
  {:else if error}
    <p class="hint err">{error}</p>
  {:else}
    <div class="group-label">Local</div>
    {#each locals as b (b.name)}
      <div class="brow" class:cur={b.isCurrent}>
        <span class="bname">{b.isCurrent ? '● ' : ''}{b.name}</span>
        {#if b.ahead || b.behind}
          <span class="track">{b.ahead ? `↑${b.ahead}` : ''}{b.behind ? `↓${b.behind}` : ''}</span>
        {/if}
        <span class="bactions">
          {#if !b.isCurrent}
            <button onclick={() => onCheckout(b.name)}>Checkout</button>
            <button class="del" onclick={() => onDelete(b.name)}>Delete</button>
          {/if}
        </span>
      </div>
    {/each}
    {#if remotes.length}
      <div class="group-label">Remote</div>
      {#each remotes as b (b.name)}
        <div class="brow">
          <span class="bname">{b.name}</span>
          <span class="bactions">
            <button onclick={() => onCheckout(b.name)}>Checkout</button>
          </span>
        </div>
      {/each}
    {/if}
  {/if}
</div>

<style>
  .branches { font-size: 0.8rem; overflow: auto; height: 100%; }
  .bbar { padding: 0.4rem 0.5rem; }
  .new {
    background: transparent; border: 1px solid var(--border); border-radius: 4px;
    color: var(--fg); cursor: pointer; font-size: 0.74rem; padding: 0.2rem 0.5rem;
  }
  .group-label {
    color: var(--fg-muted); font-size: 0.68rem; text-transform: uppercase;
    padding: 0.4rem 0.6rem 0.2rem;
  }
  .brow {
    display: flex; align-items: center; gap: 0.5rem; padding: 0.25rem 0.6rem;
  }
  .brow:hover { background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .brow:hover .bactions { visibility: visible; }
  .bname { flex: 1 1 auto; font-family: var(--mono, monospace); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cur .bname { color: var(--accent); }
  .track { flex: 0 0 auto; color: var(--fg-muted); font-size: 0.72rem; }
  .bactions { flex: 0 0 auto; visibility: hidden; display: flex; gap: 0.3rem; }
  .bactions button {
    background: transparent; border: 1px solid var(--border); border-radius: 3px;
    color: var(--fg-muted); cursor: pointer; font-size: 0.7rem; padding: 0 0.4rem;
  }
  .bactions button:hover { color: var(--fg); border-color: var(--accent); }
  .bactions button.del:hover { color: #f85149; border-color: #f85149; }
  .hint { color: var(--fg-muted); padding: 0.5rem 0.7rem; }
  .hint.err { color: #e64a4a; }
</style>
```

- [ ] **Step 2: Wire branches mode into `FilesPanel`**

Add state + loader:

```ts
  import { repoBranches, type Branch } from './history';
  import BranchList from './BranchList.svelte';

  let branches = $state<Branch[]>([]);

  async function loadBranches(): Promise<void> {
    const sid = session.id;
    loading = true;
    error = null;
    const r = await repoBranches(sid);
    if (sid !== session.id) return;
    loading = false;
    if (r.ok) branches = r.value;
    else error = r.error.message;
  }
```

Render (full-width, like history):

```svelte
{#if mode === 'branches'}
  <div class="full-col" data-testid="branches-view">
    <BranchList
      {branches}
      {loading}
      {error}
      onCheckout={(n) => confirmCheckout(n)}
      onDelete={(n) => confirmDeleteBranch(n)}
      onNew={() => promptCreateBranch(null)}
    />
  </div>
{:else if mode === 'history' && !openCommit}
  ... (from Task 10)
```

`confirmCheckout`, `confirmDeleteBranch`, `promptCreateBranch` are implemented in Phase 6; stub them for now (no-ops) so this compiles, then replace in Task 13. (`promptCreateBranch` already stubbed in Task 10; add the other two stubs.)

- [ ] **Step 3: Type-check + commit**

Run: `pnpm check`
Expected: no new errors.

```bash
git add src/lib/BranchList.svelte src/lib/FilesPanel.svelte
git commit -m "feat(branches): Branches view"
```

---

## Phase 4 — Backend mutations

### Task 12a: `commands/mutate.rs` — checkout, branch create/delete, checkout-commit

**Files:**
- Create: `src-tauri/src/commands/mutate.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write the dirty-check unit test**

Create `mutate.rs` with a small pure helper and its test (the commands themselves run git and aren't unit-tested, matching how `repo_changes` etc. are structured):

```rust
//! Mutating git commands for the Files tab: checkout, branch create/delete,
//! stage/commit, and remote sync. Reuses the shared plumbing in `repo.rs`.
//! Branch names go through `validate::git_ref`, hashes through
//! `validate::commit_hash`, paths through `validate::repo_rel_path`; every
//! interpolated value is shell-quoted.

use crate::commands::repo::{repo_err, repo_script, run_in_repo, session_target};
use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

/// True when `git status --porcelain` output indicates a dirty worktree.
fn is_dirty(porcelain: &[u8]) -> bool {
    !String::from_utf8_lossy(porcelain).trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_dirty_detects_changes() {
        assert!(!is_dirty(b""));
        assert!(!is_dirty(b"   \n"));
        assert!(is_dirty(b" M src/x.rs\n"));
        assert!(is_dirty(b"?? new.txt\n"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test commands::mutate`
Expected: FAIL — module not declared in `mod.rs`.

- [ ] **Step 3: Declare module + implement checkout/branch commands**

In `commands/mod.rs` add `pub mod mutate;`. Append to `mutate.rs`:

```rust
#[derive(Deserialize)]
pub struct CheckoutArgs {
    pub session_id: i64,
    pub branch: String,
}

/// Checkout a branch. Refuses (E_DIRTY) when the worktree has uncommitted
/// changes — the agent may be mid-edit. Never `--force`.
#[tauri::command]
pub async fn repo_checkout(
    args: CheckoutArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.branch)?;
    let (host, name) = session_target(&store, args.session_id)?;
    // Guard: check dirty first.
    let status = repo_script(&name, "git -C \"$root\" status --porcelain");
    let so = run_in_repo(&ssh, &host, &status).await?;
    if !so.status.success() {
        return Err(repo_err(&so));
    }
    if is_dirty(&so.stdout) {
        return Err(IpcError::new(
            "E_DIRTY",
            "worktree has uncommitted changes — the agent may have work in progress",
        ));
    }
    let body = format!("git -C \"$root\" checkout {}", shq(&args.branch));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CheckoutCommitArgs {
    pub session_id: i64,
    pub hash: String,
}

/// Checkout a commit (detached HEAD). Same dirty guard as `repo_checkout`.
#[tauri::command]
pub async fn repo_checkout_commit(
    args: CheckoutCommitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let status = repo_script(&name, "git -C \"$root\" status --porcelain");
    let so = run_in_repo(&ssh, &host, &status).await?;
    if !so.status.success() {
        return Err(repo_err(&so));
    }
    if is_dirty(&so.stdout) {
        return Err(IpcError::new(
            "E_DIRTY",
            "worktree has uncommitted changes — the agent may have work in progress",
        ));
    }
    let body = format!("git -C \"$root\" checkout {}", shq(&args.hash));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateBranchArgs {
    pub session_id: i64,
    pub name: String,
    pub start_point: Option<String>,
    pub checkout: bool,
}

/// Create a branch from HEAD or a start point (branch name or commit hash),
/// optionally checking it out.
#[tauri::command]
pub async fn repo_create_branch(
    args: CreateBranchArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.name)?;
    // A start point may be a ref or a hash — accept either, validated.
    if let Some(sp) = &args.start_point {
        if crate::validate::commit_hash(sp).is_err() {
            crate::validate::git_ref(sp)?;
        }
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let sp = args
        .start_point
        .as_ref()
        .map(|s| format!(" {}", shq(s)))
        .unwrap_or_default();
    let verb = if args.checkout { "checkout -b" } else { "branch" };
    let body = format!("git -C \"$root\" {verb} {}{sp}", shq(&args.name));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct DeleteBranchArgs {
    pub session_id: i64,
    pub name: String,
    pub force: bool,
}

/// Delete a local branch (`-d`, or `-D` when `force`).
#[tauri::command]
pub async fn repo_delete_branch(
    args: DeleteBranchArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.name)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let flag = if args.force { "-D" } else { "-d" };
    let body = format!("git -C \"$root\" branch {flag} {}", shq(&args.name));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}
```

- [ ] **Step 4: Register and test**

In `lib.rs` add (after the history commands):

```rust
            commands::mutate::repo_checkout,
            commands::mutate::repo_checkout_commit,
            commands::mutate::repo_create_branch,
            commands::mutate::repo_delete_branch,
```

Run: `cd src-tauri && cargo test commands::mutate && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS + clean.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/mutate.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "feat(mutate): checkout, checkout-commit, create/delete branch"
```

### Task 12b: `commands/mutate.rs` — stage/unstage, commit, fetch/pull/push

**Files:**
- Modify: `src-tauri/src/commands/mutate.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Implement staging + commit + remote commands**

Append to `mutate.rs`:

```rust
#[derive(Deserialize)]
pub struct StageArgs {
    pub session_id: i64,
    pub paths: Vec<String>,
}

/// Stage the given worktree paths (`git add --`).
#[tauri::command]
pub async fn repo_stage(
    args: StageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    for p in &args.paths {
        crate::validate::repo_rel_path(p)?;
    }
    if args.paths.is_empty() {
        return Ok(());
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let quoted: Vec<String> = args.paths.iter().map(|p| shq(p)).collect();
    let body = format!("git -C \"$root\" add -- {}", quoted.join(" "));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

/// Unstage the given paths (`git restore --staged --`).
#[tauri::command]
pub async fn repo_unstage(
    args: StageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    for p in &args.paths {
        crate::validate::repo_rel_path(p)?;
    }
    if args.paths.is_empty() {
        return Ok(());
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let quoted: Vec<String> = args.paths.iter().map(|p| shq(p)).collect();
    let body = format!("git -C \"$root\" restore --staged -- {}", quoted.join(" "));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CommitCreateArgs {
    pub session_id: i64,
    pub message: String,
    pub amend: bool,
}

/// Commit the staged changes. The message is passed via stdin-free `-m` with
/// shell quoting; an empty message is rejected.
#[tauri::command]
pub async fn repo_commit_create(
    args: CommitCreateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    if args.message.trim().is_empty() {
        return Err(IpcError::new("E_INVALID", "commit message must not be empty"));
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let amend = if args.amend { " --amend" } else { "" };
    let body = format!(
        "git -C \"$root\" commit{amend} -m {}",
        shq(&args.message)
    );
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct SessionIdArgs {
    pub session_id: i64,
}

/// `git fetch` (all remotes).
#[tauri::command]
pub async fn repo_fetch(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let out = run_in_repo(&ssh, &host, &repo_script(&name, "git -C \"$root\" fetch --all --prune")).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

/// `git pull --ff-only` (refuse to create a merge commit silently).
#[tauri::command]
pub async fn repo_pull(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let out = run_in_repo(&ssh, &host, &repo_script(&name, "git -C \"$root\" pull --ff-only")).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct PushArgs {
    pub session_id: i64,
    pub set_upstream: bool,
}

/// `git push`; with `set_upstream`, push the current branch and set upstream.
#[tauri::command]
pub async fn repo_push(
    args: PushArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let body = if args.set_upstream {
        "b=\"$(git -C \"$root\" rev-parse --abbrev-ref HEAD)\"; git -C \"$root\" push -u origin \"$b\"".to_string()
    } else {
        "git -C \"$root\" push".to_string()
    };
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}
```

> Note: `SessionIdArgs` is defined locally here to avoid a cross-module dependency; the `files.rs` one stays where it is.

- [ ] **Step 2: Register the commands**

In `lib.rs` after the Task 12a commands:

```rust
            commands::mutate::repo_stage,
            commands::mutate::repo_unstage,
            commands::mutate::repo_commit_create,
            commands::mutate::repo_fetch,
            commands::mutate::repo_pull,
            commands::mutate::repo_push,
```

- [ ] **Step 3: Build/clippy/fmt + commit**

Run: `cd src-tauri && cargo test commands::mutate && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS + clean.

```bash
git add src-tauri/src/commands/mutate.rs src-tauri/src/lib.rs
git commit -m "feat(mutate): stage/unstage, commit, fetch/pull/push"
```

---

## Phase 5 — Wire mutations into the UI

### Task 13: Branch & commit actions + confirmations in `FilesPanel`

**Files:**
- Modify: `src/lib/FilesPanel.svelte`

- [ ] **Step 1: Replace the Phase-3 stubs with real action handlers**

Use the browser `confirm()` for destructive ops (the app already runs in a webview; matches the lightweight UX of the panel). Surface `E_DIRTY` specifically.

```ts
  import {
    repoCheckout, repoCheckoutCommit, repoCreateBranch, repoDeleteBranch,
  } from './history';

  async function runAction(p: Promise<Result<unknown>>, after: () => void): Promise<void> {
    const r = await p;
    if (r.ok) { after(); }
    else { error = r.error.message; }
  }

  function confirmCheckout(branch: string): void {
    void runAction(repoCheckout(session.id, branch), () => { loadBranches(); reloadKey++; });
  }

  function confirmCheckoutCommit(hash: string): void {
    if (!confirm(`Checkout ${hash.slice(0, 8)} as a detached HEAD? The agent's branch will change.`)) return;
    void runAction(repoCheckoutCommit(session.id, hash), () => { onRefresh(); });
  }

  function confirmDeleteBranch(name: string): void {
    if (!confirm(`Delete branch "${name}"?`)) return;
    void runAction(repoDeleteBranch(session.id, name, false), () => loadBranches());
  }

  function promptCreateBranch(startPoint: string | null): void {
    const name = prompt('New branch name:')?.trim();
    if (!name) return;
    const checkout = confirm('Check out the new branch now?');
    void runAction(
      repoCreateBranch(session.id, name, { startPoint, checkout }),
      () => { loadBranches(); if (mode === 'history') loadHistory(); },
    );
  }
```

Import `Result`:

```ts
  import type { Result } from './result';
```

- [ ] **Step 2: Type-check + commit**

Run: `pnpm check`
Expected: no new errors.

```bash
git add src/lib/FilesPanel.svelte
git commit -m "feat(files): wire branch/commit actions with confirmations"
```

### Task 14: Stage & commit footer in Changed mode

**Files:**
- Modify: `src/lib/FileList.svelte`
- Modify: `src/lib/FilesPanel.svelte`

- [ ] **Step 1: Add stage checkboxes + commit footer to `FileList` (changes mode only)**

Extend `FileList`'s props with optional staging callbacks and a flag to enable the footer (so the commit-drill-in reuse from Task 10 does *not* show it — there, staging is meaningless):

```ts
    onStageToggle,
    onCommit,
    enableStaging = false,
  }: {
    // ...existing props...
    onStageToggle?: (path: string, staged: boolean) => void;
    onCommit?: (message: string) => void;
    enableStaging?: boolean;
  } = $props();

  let commitMsg = $state('');
  const stagedCount = $derived(changes.filter((c) => c.staged).length);
```

In each changed-file row (changes mode), prepend a checkbox when `enableStaging`:

```svelte
{#if enableStaging}
  <input
    type="checkbox"
    checked={c.staged}
    title={c.staged ? 'Unstage' : 'Stage'}
    onclick={(e) => { e.stopPropagation(); onStageToggle?.(c.path, !c.staged); }}
  />
{/if}
```

After the `.rows` div, when `enableStaging`, render the footer:

```svelte
{#if enableStaging && mode === 'changes'}
  <div class="commit-footer">
    <textarea bind:value={commitMsg} placeholder="Commit message…" rows="2"></textarea>
    <button
      disabled={stagedCount === 0 || commitMsg.trim() === ''}
      onclick={() => { onCommit?.(commitMsg.trim()); commitMsg = ''; }}
    >Commit {stagedCount} file{stagedCount === 1 ? '' : 's'}</button>
  </div>
{/if}
```

Add minimal styles for `.commit-footer` (border-top, padding) and `textarea`/`button` using existing variables.

- [ ] **Step 2: Wire it in `FilesPanel` (only the live Changed view, not the commit drill-in)**

For the non-commit `FileList` in changes/tree mode, pass:

```svelte
<FileList
  {mode} {changes} {tree} {loading} {error} {selectedPath}
  {onSelect} {onMode} {onRefresh}
  enableStaging={true}
  onStageToggle={stageToggle}
  onCommit={commitStaged}
/>
```

The commit-drill-in `FileList` (from Task 10) keeps `enableStaging` unset (defaults false).

Add handlers:

```ts
  import { repoStage, repoUnstage, repoCommitCreate } from './history';

  function stageToggle(path: string, staged: boolean): void {
    const p = staged ? repoStage(session.id, [path]) : repoUnstage(session.id, [path]);
    void runAction(p, () => loadChanges());
  }

  function commitStaged(message: string): void {
    void runAction(repoCommitCreate(session.id, message), () => { loadChanges(); reloadKey++; });
  }
```

- [ ] **Step 3: Type-check + run tests + commit**

Run: `pnpm check && pnpm test -- files_view`
Expected: no new type errors; tests pass.

```bash
git add src/lib/FileList.svelte src/lib/FilesPanel.svelte
git commit -m "feat(files): stage checkboxes + commit footer in Changed mode"
```

### Task 15: Remote toolbar (Fetch / Pull / Push)

**Files:**
- Create: `src/lib/RemoteToolbar.svelte`
- Modify: `src/lib/FilesPanel.svelte`

- [ ] **Step 1: Implement `RemoteToolbar.svelte`**

```svelte
<script lang="ts">
  import { repoFetch, repoPull, repoPush } from './history';
  import type { SessionRow } from './sessions';

  let { session, ondone }: { session: SessionRow; ondone: () => void } = $props();
  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);

  async function run(label: string, fn: () => Promise<{ ok: boolean; error?: { message: string } }>): Promise<void> {
    busy = label;
    err = null;
    const r = await fn();
    busy = null;
    if (!r.ok && r.error) err = r.error.message;
    else ondone();
  }
</script>

<div class="remote">
  <button disabled={busy !== null} onclick={() => run('fetch', () => repoFetch(session.id))}>
    {busy === 'fetch' ? '…' : 'Fetch'}
  </button>
  <button disabled={busy !== null} onclick={() => run('pull', () => repoPull(session.id))}>
    {busy === 'pull' ? '…' : 'Pull'}
  </button>
  <button disabled={busy !== null} onclick={() => run('push', () => repoPush(session.id, false))}>
    {busy === 'push' ? '…' : 'Push'}
  </button>
  {#if err}<span class="err" title={err}>!</span>{/if}
</div>

<style>
  .remote { display: flex; gap: 0.3rem; align-items: center; }
  .remote button {
    background: transparent; border: 1px solid var(--border); border-radius: 4px;
    color: var(--fg-muted); cursor: pointer; font-size: 0.72rem; padding: 0.15rem 0.5rem;
  }
  .remote button:hover:not(:disabled) { color: var(--fg); border-color: var(--accent); }
  .remote button:disabled { opacity: 0.5; cursor: default; }
  .err { color: #f85149; font-weight: 700; cursor: help; }
</style>
```

- [ ] **Step 2: Mount it in History and Branches headers**

Add `import RemoteToolbar from './RemoteToolbar.svelte';` and place `<RemoteToolbar {session} ondone={onRefresh} />` in the history `.hbar` (replacing the Task-10 placeholder) and in the branches `.bbar` area (e.g., wrap BranchList's header). On push failing because no upstream, the error surfaces; a follow-up "set upstream?" prompt can call `repoPush(session.id, true)` — add:

```ts
  // In RemoteToolbar, on a push error mentioning 'upstream', offer set-upstream:
```

Keep it simple for v1: if `push` errors, show the message; the user can create the upstream via the terminal pane, or we add a dedicated "Publish branch" button later (out of scope).

- [ ] **Step 3: Type-check + commit**

Run: `pnpm check`
Expected: no new errors.

```bash
git add src/lib/RemoteToolbar.svelte src/lib/FilesPanel.svelte
git commit -m "feat(files): remote Fetch/Pull/Push toolbar"
```

---

## Phase 6 — Component tests & polish

### Task 16: Component test for mode switching & branch list

**Files:**
- Create: `src/lib/history_view.test.ts`

- [ ] **Step 1: Write the test**

Follow the `files_view.test.ts` style (it imports exported pure functions). Test the pieces that are pure/exported: `laneColor` from `CommitGraph.svelte` and the graph integration. Component-mount tests in this repo are limited by the `localStorage` env issue, so keep to exported helpers.

```ts
import { describe, it, expect } from 'vitest';
import { laneColor } from './CommitGraph.svelte';
import { computeGraph } from './graph';

describe('laneColor', () => {
  it('is stable and wraps the palette', () => {
    expect(laneColor(0)).toBe(laneColor(0));
    expect(laneColor(0)).toBe(laneColor(10)); // 10-color palette wraps
    expect(laneColor(1)).not.toBe(laneColor(0));
  });
});

describe('graph + color integration', () => {
  it('assigns the merge commit and its branch parent distinct colors', () => {
    const rows = computeGraph([
      { hash: 'm', parents: ['c', 'f'] },
      { hash: 'c', parents: ['b'] },
      { hash: 'f', parents: ['b'] },
      { hash: 'b', parents: [] },
    ]);
    const m = rows.find((r) => r.hash === 'm')!;
    const f = rows.find((r) => r.hash === 'f')!;
    expect(m.color).not.toBe(f.color);
  });
});
```

- [ ] **Step 2: Run + commit**

Run: `pnpm test -- history_view graph`
Expected: PASS.

```bash
git add src/lib/history_view.test.ts
git commit -m "test(history): laneColor + graph color integration"
```

### Task 17: Full verification pass

**Files:** none (verification only).

- [ ] **Step 1: Frontend suite**

Run: `pnpm test`
Expected: PASS except the pre-existing `localStorage is undefined` failures noted in CLAUDE.md. Compare against `main` to confirm no new failures.

- [ ] **Step 2: Type-check**

Run: `pnpm check`
Expected: clean.

- [ ] **Step 3: Backend suite (where Tauri libs exist)**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS + clean. (On a headless box, document the build-script env gap instead.)

- [ ] **Step 4: Manual smoke (run the app)**

Use the `run` skill or `pnpm tauri dev`. With a session whose worktree is a git repo:
- History mode shows the graph (all branches); toggling "All branches" reloads; "Load more" pages.
- Clicking a commit drills into its files; clicking a file shows the commit diff; "Back to graph" returns.
- Branches mode lists local + remote; checkout on a clean tree works; checkout on a dirty tree shows the E_DIRTY warning.
- Changed mode: stage a file, type a message, Commit; the change disappears from the list.
- Fetch/Pull/Push run and report errors inline.

- [ ] **Step 5: Update docs**

Add a short "History & Branches" subsection to the Files section of any user-facing doc that documents the Files tab (e.g. `docs/` notes), and note the new commands in `docs/control-api.md` only if they're exposed via MCP (they are *not* in this plan — MCP exposure is out of scope).

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "docs: note History & Branches in the Files tab"
```

---

## Self-review notes (resolved)

- **Spec coverage:** read commands (Tasks 3–5), graph (Task 6), branches (Tasks 4/11), all four action groups (Tasks 12a/12b wired in 13/14/15), safety guard `E_DIRTY` + confirms (Tasks 12a/13), all-branches default + toggle (Tasks 7/10), full-width layout + commit drill-in reuse (Task 10), tests (Tasks 6/16/17). Covered.
- **Spec correction:** `validate::git_ref` already exists — Task 2 only adds `commit_hash`. Command registration is in `lib.rs`, not `mod.rs` (reflected throughout).
- **Type consistency:** Rust `Commit`/`Branch`/`CommitDetail`/`GitRef` use `#[serde(rename_all = "camelCase")]` to match the TS interfaces in `history.ts` (`shortHash`, `isCurrent`, `tipHash`, etc.). `repo_commit_diff` returns the existing `FileDiff`. The frontend `commit` prop threads `string | null` consistently from `FilesPanel` → `FileViewer`.
- **Ordering:** UI tasks (Phase 3) stub the Phase-5 action handlers so each commit compiles; Task 13/14/15 replace the stubs.
