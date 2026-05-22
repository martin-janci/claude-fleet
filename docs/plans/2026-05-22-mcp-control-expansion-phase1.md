# MCP Control Expansion — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand the embedded MCP control API with ~13 tools so an AI can observe sessions (capture pane / peek logs), use the newer lifecycle actions (recreate / dismiss-ghost / background sessions), and read a session's worktree files + git state.

**Architecture:** MCP tools (`mcp/tools.rs`) call shared logic, like the existing session tools call `service::sessions::*`. Lifecycle/bg functions already live in `service/`; the Files/git command bodies in `commands/files.rs` + `commands/history.rs` are refactored into plain `*_impl` functions (taking `&Mutex<Store>` + `&Arc<SshClient>`) that both the Tauri command and the new MCP tool call. A small `capture_session_output` service helper wraps tmux pane capture.

**Tech Stack:** Rust (Tauri 2, rmcp MCP server, `cargo test`).

> **Build/test:** from `src-tauri/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

> **Spec:** `docs/specs/2026-05-22-mcp-control-expansion-design.md` (Phase 1 section).
> **Working dir:** `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/mcp-control-expansion` (branch `mcp-control-expansion`).

> **Key reuse facts (already verified):**
> - `commands::repo::session_target(&Mutex<Store>, i64) -> Result<(String,String), IpcError>` (host, tmux_name). Plus `repo_script`, `run_in_repo`, `repo_err`, `diff_from_bytes`.
> - `&State<Arc<Mutex<Store>>>` deref-coerces to `&Mutex<Store>`, so a `*_impl(args, store: &Mutex<Store>, ssh: &Arc<SshClient>)` is callable from both the Tauri command (`*_impl(args, &store, &ssh)`) and MCP (`*_impl(args, &self.store, &self.ssh)`).
> - MCP `FleetTools { store: Arc<Mutex<Store>>, ssh: Arc<SshClient>, reg: Arc<CancellationRegistry> }`; tools are `#[tool(description=…)] async fn x(&self, Parameters(p): Parameters<XParams>) -> Result<CallToolResult, McpError>` in the `#[tool_router] impl FleetTools` block; helpers `audit(tool, detail)`, `to_mcp_err`, `ok_json(&v)`. Param structs `#[derive(serde::Deserialize, schemars::JsonSchema)]`.
> - Service fns: `service::sessions::recreate_session(RecreateSessionArgs{session_id}, &store, &ssh)`, `service::sessions::dismiss_ghost_session(DismissGhostSessionArgs{session_id}, &store)`, `service::bg_sessions::new_bg_session(NewBgSessionArgs{host_alias,name,prompt}, &ssh)` → `NewBgSessionResult{claude_session_id}`, `service::bg_sessions::peek_session(PeekSessionArgs{host_alias,claude_session_id}, &ssh) -> String`.
> - `store.get_session_by_id(i64) -> Result<Option<SessionRow>>`; `SessionRow` has `host_alias`, `claude_session_id: Option<String>`.

---

## File structure

Modified:
- `src-tauri/src/commands/files.rs` — extract `repo_changes_impl`/`repo_tree_impl`/`repo_file_impl`/`repo_diff_impl`; commands become wrappers.
- `src-tauri/src/commands/history.rs` — extract `repo_log_impl`/`repo_branches_impl`/`repo_commit_impl`/`repo_commit_diff_impl`.
- `src-tauri/src/tmux.rs` — add `capture_pane_scrollback` to `TmuxExec` (+ impls).
- `src-tauri/src/service/sessions.rs` — add `capture_session_output` (+ its 2 test-mock impls of the new trait method).
- `src-tauri/src/mcp/tools.rs` — add ~13 tools + param structs.
- `docs/control-api.md` — document the new tools.

---

## Phase A — Reuse refactor (extract `*_impl`)

### Task 1: Extract `*_impl` from `commands/files.rs`

**Files:** Modify `src-tauri/src/commands/files.rs`

Each of the four commands (`repo_changes`, `repo_tree`, `repo_file`, `repo_diff`) currently is `#[tauri::command] pub async fn NAME(args: ARGS, store: State<'_, Arc<Mutex<Store>>>, ssh: State<'_, Arc<SshClient>>) -> Result<T, IpcError> { <body> }`. Refactor each into an impl + a thin wrapper, moving the body verbatim.

- [ ] **Step 1: Refactor `repo_changes`** to:

```rust
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

#[tauri::command]
pub async fn repo_changes(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<ChangedFile>, IpcError> {
    repo_changes_impl(args, &store, &ssh).await
}
```

Note the body changes `&store`→`store` and `&ssh`→`ssh` (the impl already holds references). Apply the same transformation to the other three:

- [ ] **Step 2: `repo_tree_impl`** — signature `(args: SessionIdArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<RepoTree, IpcError>`; move `repo_tree`'s body verbatim (adjusting `&store`→`store`, `&ssh`→`ssh`); command wrapper calls `repo_tree_impl(args, &store, &ssh).await`.
- [ ] **Step 3: `repo_file_impl`** — `(args: RepoFileArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<FileContent, IpcError>`; wrapper `repo_file(args, store, ssh)` → `repo_file_impl(args, &store, &ssh).await`.
- [ ] **Step 4: `repo_diff_impl`** — `(args: RepoFileArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<FileDiff, IpcError>`; wrapper delegates.

- [ ] **Step 5: Build + test + commit**

Run: `cd src-tauri && cargo test commands::files && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS/clean (behavior unchanged; existing `files` tests still pass).

```bash
git add src-tauri/src/commands/files.rs
git commit -m "refactor(files): extract repo_* command bodies into reusable *_impl fns"
```

### Task 2: Extract `*_impl` from `commands/history.rs`

**Files:** Modify `src-tauri/src/commands/history.rs`

Same transformation for the four history commands.

- [ ] **Step 1: `repo_log_impl`** — `(args: RepoLogArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<Vec<Commit>, IpcError>`; move `repo_log`'s body (change `&store`→`store`, `&ssh`→`ssh`); `#[tauri::command] repo_log` wrapper calls `repo_log_impl(args, &store, &ssh).await`.
- [ ] **Step 2: `repo_branches_impl`** — `(args: SessionIdArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<Vec<Branch>, IpcError>`. (Note: `repo_branches` uses `crate::commands::files::SessionIdArgs` — keep that arg type.) Wrapper delegates.
- [ ] **Step 3: `repo_commit_impl`** — `(args: RepoCommitArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<CommitDetail, IpcError>`. Wrapper delegates.
- [ ] **Step 4: `repo_commit_diff_impl`** — `(args: RepoCommitDiffArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<FileDiff, IpcError>`. Wrapper delegates.

- [ ] **Step 5: Build + test + commit**

Run: `cd src-tauri && cargo test commands::history && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS/clean.

```bash
git add src-tauri/src/commands/history.rs
git commit -m "refactor(history): extract repo_* command bodies into reusable *_impl fns"
```

---

## Phase B — Capture-output helper

### Task 3: `capture_pane_scrollback` + `capture_session_output`

**Files:** Modify `src-tauri/src/tmux.rs`, `src-tauri/src/service/sessions.rs`

- [ ] **Step 1: Add a scrollback method to the `TmuxExec` trait**

In `src-tauri/src/tmux.rs`, in the `pub trait TmuxExec`, add next to `capture_pane`:

```rust
    /// Capture the pane including `lines` rows of scrollback history.
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError>;
```

- [ ] **Step 2: Implement it for `LocalTmux`** — mirror the existing `LocalTmux::capture_pane` (which runs `tmux capture-pane -t <name> -p`), inserting `-S` and `-<lines>` before `-p`:

```rust
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError> {
        let start = format!("-{lines}");
        let output = tokio::process::Command::new("tmux")
            .args(["capture-pane", "-t", name, "-S", &start, "-p"])
            .output()
            .await
            .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Ok(String::new())
        }
    }
```

- [ ] **Step 3: Implement it for `RemoteTmux`** — read `RemoteTmux::capture_pane` and mirror it, building the remote `tmux capture-pane -t <name> -S -<lines> -p` script (same quoting/`remote_bash` path the existing `capture_pane` uses). Keep the "empty string on failure" behavior.

- [ ] **Step 4: Add the new method to the two test-mock `impl TmuxExec` blocks in `service/sessions.rs`** (`SleepyTmux` and `FakeTmux`). Mirror however they implement `capture_pane` (likely returning `Ok(String::new())` or a canned value):

```rust
            async fn capture_pane_scrollback(&self, _name: &str, _lines: u32) -> Result<String, IpcError> {
                Ok(String::new())
            }
```

(If any other `impl TmuxExec` exists — `grep -n "impl TmuxExec" src-tauri/src` — add the method there too. `cargo build` will flag any missing impl.)

- [ ] **Step 5: Write the failing test for the service helper**

Add to `#[cfg(test)] mod tests` in `service/sessions.rs`:

```rust
    #[test]
    fn capture_scrollback_arg_is_negative_lines() {
        // The scrollback start offset passed to tmux is `-<lines>`.
        assert_eq!(scrollback_start(120), "-120");
        assert_eq!(scrollback_start(0), "-0");
    }
```

- [ ] **Step 6: Run to verify it fails**

Run: `cd src-tauri && cargo test capture_scrollback_arg`
Expected: FAIL — `scrollback_start` not found.

- [ ] **Step 7: Implement the helper + the pure `scrollback_start`**

Add to `service/sessions.rs`:

```rust
/// tmux `-S` start offset for `lines` rows of scrollback (a negative count).
fn scrollback_start(lines: u32) -> String {
    format!("-{lines}")
}

/// Capture a session's terminal output. With `scrollback_lines = None` returns
/// the visible pane; with `Some(n)` includes `n` rows of scrollback history.
pub async fn capture_session_output(
    session_id: i64,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    scrollback_lines: Option<u32>,
) -> Result<String, IpcError> {
    let (host, name) = crate::commands::repo::session_target(store, session_id)?;
    let tmux = exec_for(&host, ssh);
    match scrollback_lines {
        Some(n) => tmux.capture_pane_scrollback(&name, n).await,
        None => tmux.capture_pane(&name).await,
    }
}
```

(`scrollback_start` is used by the `LocalTmux` impl too — if you prefer, inline it there; the test targets the pure fn. Keep whichever keeps clippy clean — if `scrollback_start` ends up unused in non-test code, either use it in `capture_pane_scrollback` or drop it and test the format inline. Simplest: have `LocalTmux::capture_pane_scrollback` call `crate::service::sessions::scrollback_start`… but that crosses module boundaries; instead keep `scrollback_start` in `service/sessions.rs`, used by `capture_session_output`? It isn't. **Resolution:** put `scrollback_start` in `tmux.rs` next to the impl, make it `pub(crate)`, use it in `LocalTmux::capture_pane_scrollback`, and test it in `tmux.rs`'s test module instead.)

> Cleaner final arrangement (do this): define `pub(crate) fn scrollback_start(lines: u32) -> String` in `tmux.rs`, call it in `LocalTmux::capture_pane_scrollback` (`.args(["capture-pane","-t",name,"-S",&scrollback_start(lines),"-p"])`) and in the `RemoteTmux` script, and put the `capture_scrollback_arg_is_negative_lines` test in `tmux.rs`'s `#[cfg(test)] mod tests`. `capture_session_output` then needs no `scrollback_start`.

- [ ] **Step 8: Run + clippy/fmt + commit**

Run: `cd src-tauri && cargo test scrollback capture && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS/clean.

```bash
git add src-tauri/src/tmux.rs src-tauri/src/service/sessions.rs
git commit -m "feat(tmux): pane scrollback capture + capture_session_output helper"
```

---

## Phase C — MCP tools

> All tools are added inside the `#[tool_router] impl FleetTools { … }` block in `src-tauri/src/mcp/tools.rs`. Param structs go near the other `*Params` structs (above the impl), each `#[derive(serde::Deserialize, schemars::JsonSchema)]` with `///` doc comments on fields. `use` paths: the impls/services are referenced as `crate::commands::files::*`, `crate::commands::history::*`, `crate::service::bg_sessions`, `sessions` (already imported as `crate::service::sessions`).

### Task 4: Read-output tools (`capture_session`, `peek_session`)

**Files:** Modify `src-tauri/src/mcp/tools.rs`

- [ ] **Step 1: Add param structs**

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CaptureSessionParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Rows of scrollback history to include; omit for just the visible pane.
    pub scrollback_lines: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionIdParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
}
```

- [ ] **Step 2: Add the tools** (inside the `#[tool_router] impl FleetTools`):

```rust
    #[tool(description = "Capture a session's terminal output — the visible \
        tmux pane, or include scrollback history. Use after send_prompt to read \
        the session's reply. Returns the pane text.")]
    async fn capture_session(
        &self,
        Parameters(p): Parameters<CaptureSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("capture_session", &format!("session_id={}", p.session_id));
        let text = sessions::capture_session_output(
            p.session_id,
            &self.store,
            &self.ssh,
            p.scrollback_lines,
        )
        .await
        .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Peek at a session's background Claude logs (claude \
        logs). Returns an informational message for interactive sessions that \
        have no background job. Returns the log text.")]
    async fn peek_session(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("peek_session", &format!("session_id={}", p.session_id));
        let (host_alias, claude_id) = {
            let s = self.store.lock().map_err(|_| {
                McpError::internal_error("store mutex poisoned", None)
            })?;
            let row = s
                .get_session_by_id(p.session_id)
                .map_err(to_mcp_err)?
                .ok_or_else(|| McpError::invalid_params("session not found", None))?;
            (row.host_alias, row.claude_session_id)
        };
        let Some(claude_id) = claude_id else {
            return Ok(CallToolResult::success(vec![Content::text(
                "This session has no Claude session id yet — nothing to peek.".to_string(),
            )]));
        };
        let args = crate::service::bg_sessions::PeekSessionArgs {
            host_alias,
            claude_session_id: claude_id,
        };
        let logs = crate::service::bg_sessions::peek_session(args, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(logs)]))
    }
```

> Check how `Content::text` / `CallToolResult::success` are constructed elsewhere — if the file already has an `ok_text(...)` helper or imports `Content`, mirror it. If `Content` isn't imported, either import it (`use rmcp::model::Content;` — confirm the path from the existing `ok_json`) or, simplest, return `ok_json(&text)` (a JSON string) instead of `Content::text`. Pick whichever matches the file's existing return style; `ok_json(&text)` is the safe default. Also confirm `McpError::internal_error`/`invalid_params` constructor names against the rmcp version in use (grep existing `McpError::` calls); if they differ, use `to_mcp_err` with a constructed `IpcError` instead.

- [ ] **Step 3: Build + clippy/fmt + commit**

Run: `cd src-tauri && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

```bash
git add src-tauri/src/mcp/tools.rs
git commit -m "feat(mcp): capture_session + peek_session tools (read session output)"
```

### Task 5: Lifecycle tools (`recreate_session`, `dismiss_ghost_session`, `new_bg_session`)

**Files:** Modify `src-tauri/src/mcp/tools.rs`

- [ ] **Step 1: Param struct for bg session**

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewBgSessionParams {
    /// Host alias to launch the background session on.
    pub host_alias: String,
    /// Display name for the session (also its tmux/agent name).
    pub name: String,
    /// Initial prompt for the headless Claude session.
    pub prompt: String,
}
```

(`recreate_session` and `dismiss_ghost_session` reuse `SessionIdParams` from Task 4.)

- [ ] **Step 2: Add the tools**

```rust
    #[tool(description = "Recreate a session: kill its tmux session and rebuild \
        it fresh in the same worktree, resuming the same Claude conversation. \
        Works for running or ghost sessions. Returns the session row as JSON.")]
    async fn recreate_session(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("recreate_session", &format!("session_id={}", p.session_id));
        let row = sessions::recreate_session(
            sessions::RecreateSessionArgs { session_id: p.session_id },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Dismiss a ghost session (a session lost from tmux): \
        permanently delete its row. Errors if the session is not a ghost.")]
    async fn dismiss_ghost_session(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("dismiss_ghost_session", &format!("session_id={}", p.session_id));
        sessions::dismiss_ghost_session(
            sessions::DismissGhostSessionArgs { session_id: p.session_id },
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({ "dismissed": p.session_id }))
    }

    #[tool(description = "Launch a supervised headless (background) Claude \
        session on a host with an initial prompt. Returns the new Claude \
        session id as JSON; peek its progress with peek_session.")]
    async fn new_bg_session(
        &self,
        Parameters(p): Parameters<NewBgSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("new_bg_session", &format!("host={} name={}", p.host_alias, p.name));
        let args = crate::service::bg_sessions::NewBgSessionArgs {
            host_alias: p.host_alias,
            name: p.name,
            prompt: p.prompt,
        };
        let res = crate::service::bg_sessions::new_bg_session(args, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&res)
    }
```

> `dismiss_ghost_session` is **sync** (`pub fn`, no `.await`). `recreate_session` takes `&store, &ssh`. Confirm `serde_json` is in scope (the file already serializes JSON via `ok_json`; if `serde_json::json!` isn't imported, build a small `#[derive(Serialize)]` struct instead, or return `ok_json(&p.session_id)`).

- [ ] **Step 3: Build + clippy/fmt + commit**

Run: `cd src-tauri && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

```bash
git add src-tauri/src/mcp/tools.rs
git commit -m "feat(mcp): recreate_session, dismiss_ghost_session, new_bg_session tools"
```

### Task 6: Files read tools (`repo_changes`, `repo_tree`, `repo_file`, `repo_diff`)

**Files:** Modify `src-tauri/src/mcp/tools.rs`

- [ ] **Step 1: Param structs**

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoPathParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
}
```

(`repo_changes` / `repo_tree` reuse `SessionIdParams`.)

- [ ] **Step 2: Add the tools** — each builds the files `Args` struct and calls the `_impl`:

```rust
    #[tool(description = "List a session's changed files (git status) in its \
        worktree. Returns JSON array of {path,status,staged,origPath}.")]
    async fn repo_changes(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_changes", &format!("session_id={}", p.session_id));
        let v = crate::commands::files::repo_changes_impl(
            crate::commands::files::SessionIdArgs { session_id: p.session_id },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "List a session's worktree files (tracked + untracked, \
        gitignore respected). Returns JSON {entries,truncated}.")]
    async fn repo_tree(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_tree", &format!("session_id={}", p.session_id));
        let v = crate::commands::files::repo_tree_impl(
            crate::commands::files::SessionIdArgs { session_id: p.session_id },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Read one worktree file's contents (capped). Returns \
        JSON {path,content,truncated,binary,size}.")]
    async fn repo_file(
        &self,
        Parameters(p): Parameters<RepoPathParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_file", &format!("session_id={} path={}", p.session_id, p.path));
        let v = crate::commands::files::repo_file_impl(
            crate::commands::files::RepoFileArgs { session_id: p.session_id, path: p.path },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Unified diff for one worktree file vs HEAD (untracked \
        files render as all-added). Returns JSON {path,diff,binary,truncated}.")]
    async fn repo_diff(
        &self,
        Parameters(p): Parameters<RepoPathParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_diff", &format!("session_id={} path={}", p.session_id, p.path));
        let v = crate::commands::files::repo_diff_impl(
            crate::commands::files::RepoFileArgs { session_id: p.session_id, path: p.path },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }
```

- [ ] **Step 3: Build + clippy/fmt + commit**

Run: `cd src-tauri && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

```bash
git add src-tauri/src/mcp/tools.rs
git commit -m "feat(mcp): repo_changes/tree/file/diff read tools"
```

### Task 7: History read tools (`repo_log`, `repo_branches`, `repo_commit`, `repo_commit_diff`)

**Files:** Modify `src-tauri/src/mcp/tools.rs`

- [ ] **Step 1: Param structs**

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoLogParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Show all branches/refs (default true) instead of just HEAD.
    pub all: Option<bool>,
    /// Max commits to return (default 200).
    pub limit: Option<u32>,
    /// Commits to skip (pagination).
    pub skip: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoCommitParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoCommitDiffParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
    /// Worktree-relative file path.
    pub path: String,
}
```

(`repo_branches` reuses `SessionIdParams`.)

- [ ] **Step 2: Add the tools**

```rust
    #[tool(description = "Commit log (branch graph) for a session's worktree. \
        all=true (default) includes every branch. Returns JSON array of commits \
        with parents + ref decorations.")]
    async fn repo_log(
        &self,
        Parameters(p): Parameters<RepoLogParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_log", &format!("session_id={}", p.session_id));
        let v = crate::commands::history::repo_log_impl(
            crate::commands::history::RepoLogArgs {
                session_id: p.session_id,
                all: p.all.unwrap_or(true),
                limit: p.limit.unwrap_or(0),
                skip: p.skip.unwrap_or(0),
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "List local + remote branches for a session's worktree \
        with ahead/behind. Returns JSON array.")]
    async fn repo_branches(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_branches", &format!("session_id={}", p.session_id));
        let v = crate::commands::history::repo_branches_impl(
            crate::commands::files::SessionIdArgs { session_id: p.session_id },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "One commit's metadata + changed files. Returns JSON \
        {hash,subject,body,author,date,files}.")]
    async fn repo_commit(
        &self,
        Parameters(p): Parameters<RepoCommitParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_commit", &format!("session_id={} hash={}", p.session_id, p.hash));
        let v = crate::commands::history::repo_commit_impl(
            crate::commands::history::RepoCommitArgs { session_id: p.session_id, hash: p.hash },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Diff of one file within a commit. Returns JSON \
        {path,diff,binary,truncated}.")]
    async fn repo_commit_diff(
        &self,
        Parameters(p): Parameters<RepoCommitDiffParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_commit_diff",
            &format!("session_id={} hash={} path={}", p.session_id, p.hash, p.path),
        );
        let v = crate::commands::history::repo_commit_diff_impl(
            crate::commands::history::RepoCommitDiffArgs {
                session_id: p.session_id,
                hash: p.hash,
                path: p.path,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }
```

> Confirm the field names of `RepoLogArgs` (`session_id`, `all`, `limit`, `skip`), `RepoCommitArgs` (`session_id`, `hash`), `RepoCommitDiffArgs` (`session_id`, `hash`, `path`) against `history.rs` — they match the verified signatures.

- [ ] **Step 3: Build + clippy/fmt + commit**

Run: `cd src-tauri && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

```bash
git add src-tauri/src/mcp/tools.rs
git commit -m "feat(mcp): repo_log/branches/commit/commit_diff read tools"
```

---

## Phase D — Docs + verification

### Task 8: Document new tools + full verification

**Files:** Modify `docs/control-api.md`

- [ ] **Step 1: Document the new tools**

In `docs/control-api.md`, in the tool-list section, add entries for the new tools grouped as: **Read session output** (`capture_session`, `peek_session`), **Session lifecycle** (`recreate_session`, `dismiss_ghost_session`, `new_bg_session`), **Files & git (read-only)** (`repo_changes`, `repo_tree`, `repo_file`, `repo_diff`, `repo_log`, `repo_branches`, `repo_commit`, `repo_commit_diff`) — each a one-line description matching its `#[tool(description=…)]`.

- [ ] **Step 2: Full backend verification**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all PASS/clean.

- [ ] **Step 3: Confirm tool count**

Run: `grep -c '#\[tool(' src-tauri/src/mcp/tools.rs`
Expected: previous count + 13.

- [ ] **Step 4: Commit**

```bash
git add docs/control-api.md
git commit -m "docs(control-api): document the new read/lifecycle/files MCP tools"
```

---

## Self-review notes (resolved)

- **Spec coverage (Phase 1):** reuse refactor (Tasks 1–2), capture helper (Task 3), read-output tools (Task 4), lifecycle tools (Task 5), files read tools (Task 6), history read tools (Task 7), docs + verification (Task 8). All Phase-1 tools from the spec are covered.
- **Reuse correctness:** `*_impl` take `&Mutex<Store>` + `&Arc<SshClient>`; Tauri wrappers pass `&store`/`&ssh` (deref-coercion), MCP passes `&self.store`/`&self.ssh`. Bodies move verbatim (behavior-preserving) — covered by existing `commands::files`/`commands::history` parser tests.
- **rmcp API uncertainty:** the only unknowns are `Content::text`/`CallToolResult::success`/`McpError` constructor names for the rmcp version in use. Task 4 instructs the implementer to grep the existing file for the real return helpers (`ok_json` is known-good) and prefer them, falling back to `ok_json(&text)` for the two text-returning tools. This is a "match the existing pattern" instruction, not a placeholder.
- **Type consistency:** param→args field names verified against the real `RepoLogArgs`/`RepoCommitArgs`/`RepoCommitDiffArgs`/`RepoFileArgs`/`SessionIdArgs`/`NewBgSessionArgs`/`PeekSessionArgs`/`RecreateSessionArgs`/`DismissGhostSessionArgs` structs. `repo_branches` correctly uses `commands::files::SessionIdArgs`.
- **`dismiss_ghost_session` is sync** (no `.await`) — reflected in Task 5.
