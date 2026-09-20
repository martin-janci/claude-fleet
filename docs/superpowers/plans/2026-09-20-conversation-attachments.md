# Conversation attachments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attach files to a prompt sent from the Conversations composer, so the running Claude Code session can actually read them.

**Architecture:** A file-attach path already ships — `upload_to_session` stages OS-dropped files on the session's host and `TerminalView` pastes the remote paths into the REPL. This plan adds the two entry points that path lacks (a picker and a clipboard paste) **without widening its threat model**, lands the files where Claude can read them without a permission prompt, and gives the composer shell a thumbnail strip.

**Tech Stack:** Rust (Tauri 2 commands, `tauri-plugin-dialog`), `fleet-core` services over SSH, Svelte 5 runes, Vitest.

**Spec:** `docs/superpowers/specs/2026-09-20-conversation-controls-and-attachments-design.md` (stages 5–6)

**Depends on:** `docs/superpowers/plans/2026-09-20-conversation-controls.md` — Task 9 (`.composer-shell`, the element this plan mounts into), Task 12 (`preserveThread`) and Task 4 (the window-level drag guard). Do not start this plan before those three tasks are green. Both plans land in one PR.

## Global Constraints

- **The webview never names a local path.** The SEC-9 invariant in `src-tauri/src/commands/upload.rs:6-10` is the reason `check_paths_allowed` exists. Every new entry point records its own paths into `UploadAllowList` Rust-side. A command that accepts a path string from the frontend and trusts it is a plan failure.
- **One authorisation, one use.** `UploadAllowList::consume` is called before the transfer, not after, so a failed upload leaves no reusable authorisation behind.
- **A prompt is capped at ~128 KiB on Linux hosts.** `build_send_commands` puts the whole script in one `bash -lc` argv word and `MAX_ARG_STRLEN` is 128 KiB (`crates/fleet-core/src/service/move_session/carry.rs:46`). Never inline file *contents* into a prompt.
- **Any new Tauri command requires all of:** a row in `src-tauri/src/backend/verdicts.rs` in `generate_handler!` order; `route` or `refuse_local_only` **by command name**, never a pasted sentence or a second tool literal; `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`; `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`. The verdict regen **fails once by design** so the diff gets read — re-run it to confirm.
- **`cargo` is a shell function here** that forces a target dir on an external volume. Use `command cargo` if you need to bypass it. CI's clippy is newer than local clippy, so locally-clean can still fail CI.
- **Never run a dev build of the desktop app on this machine.** Verification is `cargo test` + Vitest.
- **Every value interpolated into an SSH/bash string is quoted with `fleet_core::shell::quote`.** There is one canonical implementation; do not reintroduce a local copy.
- Frontend commands: `npx vitest run`, `npx svelte-check --tsconfig ./tsconfig.json`. Backend: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`.

---

### Task 1: A picker that authorises its own result

`dialog:allow-open` is already granted in `src-tauri/capabilities/default.json`, and `AssetEditor.svelte:219` already uses the JS picker. But a JS-picked path would be rejected by `check_paths_allowed`, and adding it to the allow-list from the webview would hand the webview the ability to name any path — exactly what SEC-9 forbids. So the picker runs in Rust and records its own result.

**Files:**
- Modify: `src-tauri/src/commands/upload.rs`
- Modify: `src-tauri/src/lib.rs` (`generate_handler!`)
- Test: `src-tauri/src/commands/upload.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `UploadAllowList` from `upload.rs`.
- Produces: `#[tauri::command] pick_attachments(app, allow) -> Result<Vec<PickedFile>, IpcError>` where
  `PickedFile { path: String, name: String, size: u64, kind: AttachKind }` and
  `AttachKind` is `Image | Text | Binary`, serialised lowercase.
  Task 2 consumes `PickedFile.path`; Task 5 consumes the whole struct.

- [ ] **Step 1: Write the failing test**

In `src-tauri/src/commands/upload.rs`'s test module:

```rust
    #[test]
    fn a_picked_file_is_authorised_and_classified() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("shot.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();

        let picked = record_picked(&allow, vec![png.clone()]).unwrap();

        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "shot.png");
        assert_eq!(picked[0].kind, AttachKind::Image);
        assert_eq!(picked[0].size, 8);
        // The gate the whole design turns on.
        assert!(allow.is_allowed(&png));
        assert!(check_paths_allowed(&allow, &[png.to_string_lossy().into_owned()]).is_ok());
    }

    #[test]
    fn an_unpicked_path_stays_forbidden() {
        let allow = UploadAllowList::new();
        let err = check_paths_allowed(&allow, &["/etc/passwd".to_string()]).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p claude-fleet --lib upload::tests`
Expected: FAIL — `cannot find function 'record_picked'`

- [ ] **Step 3: Implement the classification and the recording**

Add to `upload.rs`:

```rust
/// How the composer should draw an attachment before it is uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachKind {
    Image,
    Text,
    Binary,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PickedFile {
    /// Absolute local path. Echoed back verbatim by the frontend, and only
    /// accepted again because THIS process put it on the allow-list.
    pub path: String,
    pub name: String,
    pub size: u64,
    pub kind: AttachKind,
}

fn classify(path: &Path) -> AttachKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => AttachKind::Image,
        "txt" | "md" | "log" | "json" | "yaml" | "yml" | "toml" | "csv" | "diff" | "patch"
        | "rs" | "ts" | "js" | "svelte" | "py" | "sh" => AttachKind::Text,
        _ => AttachKind::Binary,
    }
}

/// Record picked paths on the allow-list and describe them for the composer.
/// Split out of the command so the authorisation is unit-testable without a
/// Tauri app handle.
pub fn record_picked(
    allow: &UploadAllowList,
    paths: Vec<PathBuf>,
) -> Result<Vec<PickedFile>, IpcError> {
    allow.allow(&paths);
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let size = std::fs::metadata(&p)
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("stat {}: {e}", p.display())))?
            .len();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        // A newline in a basename would travel into the prompt text.
        if name.contains(['\n', '\r']) {
            return Err(IpcError::new(
                codes::E_UPLOAD,
                format!("{name:?} contains a newline in its name"),
            ));
        }
        let kind = classify(&p);
        out.push(PickedFile {
            path: p.to_string_lossy().into_owned(),
            name,
            size,
            kind,
        });
    }
    Ok(out)
}

/// Open the OS file picker and authorise whatever the user chooses. The
/// picker runs HERE, not in the webview, so the webview still never gets to
/// name a path (SEC-9).
#[tauri::command]
pub async fn pick_attachments(
    app: tauri::AppHandle,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Vec<PickedFile>, IpcError> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Attach files")
        .pick_files(move |paths| {
            let _ = tx.send(paths);
        });
    let picked = rx
        .await
        .map_err(|_| IpcError::new(codes::E_UPLOAD, "the file picker closed unexpectedly"))?;
    let Some(paths) = picked else { return Ok(vec![]) };
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    record_picked(&allow, paths)
}
```

Add `use fleet_core::ipc_error::codes;` if the test module does not already import it, and add `tempfile` to `[dev-dependencies]` in `src-tauri/Cargo.toml` if it is not already there.

- [ ] **Step 4: Register the command**

In `src-tauri/src/lib.rs`'s `generate_handler![…]`, add `commands::upload::pick_attachments` immediately after `commands::upload::upload_to_session`, so the verdict row in Task 4 mirrors the handler order.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p claude-fleet --lib upload`
Expected: PASS. `cargo test --workspace` will now FAIL on `every_command_has_a_verdict` — that is Task 4's job and is expected until then.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/upload.rs src-tauri/src/lib.rs src-tauri/Cargo.toml Cargo.lock
git commit -m "feat(upload): a file picker that authorises its own result"
```

---

### Task 2: A preview the webview can render

There is no `tauri-plugin-fs`, so the webview cannot read bytes — it only ever holds a path. Rust returns a data URL for images small enough to be worth inlining; anything else gets an extension tile in the strip. **No image crate, no downsampling**: `object-fit: cover` on a 44px tile does the scaling, and adding a decoder dependency would mean a `cargo deny` review for a thumbnail.

**Files:**
- Modify: `src-tauri/src/commands/upload.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/commands/upload.rs` tests

**Interfaces:**
- Consumes: `UploadAllowList`, `classify`, `AttachKind` from Task 1.
- Produces: `#[tauri::command] attachment_preview(path: String, allow) -> Result<Option<String>, IpcError>` — a `data:` URL, or `None` when the file is not an inlineable image. Task 6 consumes it.
  `PREVIEW_MAX_BYTES: u64 = 2 * 1024 * 1024`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn previews_only_small_allow_listed_images() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();

        let png = dir.path().join("a.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
        allow.allow(&[png.clone()]);
        let url = preview_for(&allow, png.to_str().unwrap()).unwrap().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));

        // A text file is not an image: no preview, no error.
        let log = dir.path().join("b.log");
        std::fs::write(&log, b"hello").unwrap();
        allow.allow(&[log.clone()]);
        assert!(preview_for(&allow, log.to_str().unwrap()).unwrap().is_none());

        // Not on the allow-list: refused, not read.
        let err = preview_for(&allow, "/etc/passwd").unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }

    #[test]
    fn a_large_image_gets_no_inline_preview() {
        let allow = UploadAllowList::new();
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        std::fs::write(&big, vec![0u8; (PREVIEW_MAX_BYTES + 1) as usize]).unwrap();
        allow.allow(&[big.clone()]);
        assert!(preview_for(&allow, big.to_str().unwrap()).unwrap().is_none());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p claude-fleet --lib upload::tests::previews`
Expected: FAIL — `cannot find function 'preview_for'`

- [ ] **Step 3: Implement it**

```rust
/// Above this, the strip shows an extension tile instead of the picture: a
/// data URL costs ~1.33× its bytes in the webview, and the tile is 44px.
pub const PREVIEW_MAX_BYTES: u64 = 2 * 1024 * 1024;

fn mime_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Split out of the command so the allow-list gate is unit-testable.
pub fn preview_for(allow: &UploadAllowList, path: &str) -> Result<Option<String>, IpcError> {
    let p = Path::new(path);
    if !allow.is_allowed(p) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!("{path} was not attached by the user; it cannot be previewed"),
        ));
    }
    let Some(mime) = mime_for(p) else { return Ok(None) };
    let len = std::fs::metadata(p)
        .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("stat {path}: {e}")))?
        .len();
    if len > PREVIEW_MAX_BYTES {
        return Ok(None);
    }
    let bytes = std::fs::read(p)
        .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("read {path}: {e}")))?;
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Some(format!("data:{mime};base64,{b64}")))
}

/// A data URL for an attached image, so the composer can show a thumbnail.
/// Reading bytes stays in Rust: there is no fs plugin, and the allow-list is
/// the only thing that decides which files this process will open.
#[tauri::command]
pub fn attachment_preview(
    path: String,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Option<String>, IpcError> {
    preview_for(&allow, &path)
}
```

`base64` is already a workspace dependency (the agent transport and the asset sync both use it); add it to `src-tauri/Cargo.toml` if that crate does not already depend on it directly.

**Note:** this command is synchronous and reads a file, so it must stay small — it runs on the macOS main thread. `PREVIEW_MAX_BYTES` is what keeps it bounded.

- [ ] **Step 4: Register it**

Add `commands::upload::attachment_preview` to `generate_handler!`, directly after `pick_attachments`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p claude-fleet --lib upload && cargo clippy -p claude-fleet --all-targets -- -D warnings`
Expected: PASS (`every_command_has_a_verdict` still red until Task 4).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/upload.rs src-tauri/src/lib.rs src-tauri/Cargo.toml Cargo.lock
git commit -m "feat(upload): inline previews for attached images"
```

---

### Task 3: Land the files where Claude can read them

`upload_to_session` stages under `~/.claude-fleet/uploads/<session>/`, which is outside the session's working directory. Claude Code asks permission before reading an absolute path outside cwd — fine in the terminal, where a human is watching, useless for a prompt sent from the composer.

`SessionRow` carries `worktree_id` and `worktree_key` — an id and a name, **not a path** — so the root is resolved live, the way every Files-tab read already does it (`crates/fleet-core/src/service/repo.rs:56`).

**Files:**
- Create: `crates/fleet-core/src/service/attachments.rs`
- Modify: `crates/fleet-core/src/service/mod.rs`
- Modify: `src-tauri/src/commands/upload.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `crates/fleet-core/src/service/attachments.rs` tests

**Interfaces:**
- Consumes: `fleet_core::shell::quote`, `SshClient`.
- Produces:
  `pub const ATTACH_DIR: &str = ".claude-fleet-attachments";`
  `pub fn root_script(tmux_name: &str) -> String` — the shell script that prints the worktree root.
  `pub fn stage_script(root: &str) -> String` — mkdir plus the `.git/info/exclude` append.
  `#[tauri::command] upload_attachments(args: AttachArgs, …) -> Result<Vec<String>, IpcError>` with
  `AttachArgs { host_alias: String, session_name: String, local_paths: Vec<String> }`.
  Task 7 consumes the returned absolute remote paths, in order.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleet-core/src/service/attachments.rs` with only its test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_script_asks_tmux_then_git() {
        let s = root_script("demo");
        assert!(s.contains("display-message"));
        assert!(s.contains("rev-parse --show-toplevel"));
        // The pane target is quoted exactly once, by the canonical quoter.
        assert!(s.contains("'=demo:'"));
    }

    #[test]
    fn a_hostile_session_name_cannot_escape_the_script() {
        let s = root_script("a'; rm -rf /; echo '");
        assert!(!s.contains("rm -rf /;\n"));
        assert!(s.contains(r"'\''"));
    }

    #[test]
    fn staging_creates_the_dir_and_excludes_it_untracked() {
        let s = stage_script("/w/proj");
        assert!(s.contains("mkdir -p '/w/proj/.claude-fleet-attachments'"));
        // .git/info/exclude, never the tracked .gitignore: an attachment must
        // not show up as a change the user has to explain.
        // --git-common-dir, because a linked worktree's .git is a FILE and its
        // excludes live in the common dir.
        assert!(s.contains("--git-common-dir"));
        assert!(!s.contains(".gitignore"));
        // Idempotent: appending twice must not double the line.
        assert!(s.contains("grep -qxF"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleet-core attachments`
Expected: FAIL — `cannot find function 'root_script'`

- [ ] **Step 3: Implement the scripts**

Above the test module in the same file:

```rust
//! Staging prompt attachments inside the session's worktree.
//!
//! The terminal's `upload_to_session` stages under ~/.claude-fleet/uploads/,
//! which is outside the working directory: Claude Code asks permission before
//! reading an absolute path from there. In the terminal a human approves it;
//! a prompt sent from the composer has nobody to answer. So attachments land
//! under the worktree root instead, and the directory is excluded untracked
//! so it never appears as a change the user has to explain.

use crate::shell::quote;

/// Where attachments live, relative to the worktree root.
pub const ATTACH_DIR: &str = ".claude-fleet-attachments";

/// Print the session's worktree root on stdout, or fail.
pub fn root_script(tmux_name: &str) -> String {
    let target = quote(&crate::tmux::exact_pane(tmux_name));
    format!(
        "p=$(tmux display-message -t {target} -p '#{{pane_current_path}}') && \
         cd \"$p\" && git rev-parse --show-toplevel"
    )
}

/// Create the attachment dir and make git ignore it without tracking that
/// decision. `grep -qxF` keeps a repeated run from doubling the line.
///
/// The exclude file is found with `--git-common-dir`, NOT `<root>/.git/info/`.
/// In a linked worktree `.git` is a FILE containing `gitdir: …`, so that path
/// does not exist — `mkdir -p` on it fails with "Not a directory" and takes the
/// whole `&&` chain down. Git also reads a linked worktree's excludes from the
/// COMMON dir, so writing beside the `.git` file would never take effect even
/// if it could be created. This repo develops in linked worktrees, so that is
/// the normal case here, not the exotic one.
pub fn stage_script(root: &str) -> String {
    let dir = quote(&format!("{root}/{ATTACH_DIR}"));
    let root_q = quote(root);
    let line = quote(&format!("/{ATTACH_DIR}/"));
    format!(
        "mkdir -p {dir} && \
         g=$(git -C {root_q} rev-parse --path-format=absolute --git-common-dir) && \
         mkdir -p \"$g/info\" && \
         {{ grep -qxF {line} \"$g/info/exclude\" 2>/dev/null || \
            printf '%s\\n' {line} >> \"$g/info/exclude\"; }}"
    )
}
```

Register the module in `crates/fleet-core/src/service/mod.rs` with `pub mod attachments;`.

- [ ] **Step 4: Run to verify the scripts pass**

Run: `cargo test -p fleet-core attachments`
Expected: PASS, 3 cases.

- [ ] **Step 5: Write the command**

In `src-tauri/src/commands/upload.rs`, add `upload_attachments`. It is `upload_to_session` with a different destination, so factor the transfer loop into a shared helper rather than copying it:

```rust
#[derive(Deserialize)]
pub struct AttachArgs {
    pub host_alias: String,
    pub session_name: String,
    pub local_paths: Vec<String>,
}

/// Stage attachments under the session's worktree root and return their
/// absolute remote paths, in order.
#[tauri::command]
pub async fn upload_attachments(
    args: AttachArgs,
    backend: State<'_, Arc<FleetBackend>>,
    ssh: State<'_, Arc<SshClient>>,
    allow: State<'_, Arc<UploadAllowList>>,
) -> Result<Vec<String>, IpcError> {
    // Same reason as upload_to_session: the bytes are on THIS machine and the
    // host is the hub's to reach.
    backend.refuse_local_only("upload_attachments")?;
    fleet_core::validate::host_alias(&args.host_alias)?;
    fleet_core::validate::tmux_name_addressable(&args.session_name)?;
    if args.local_paths.is_empty() {
        return Ok(vec![]);
    }
    check_paths_allowed(&allow, &args.local_paths)?;
    allow.consume(&args.local_paths);

    let timeout = Duration::from_secs(UPLOAD_TIMEOUT_SECS);
    let root = resolve_worktree_root(&ssh, &args.host_alias, &args.session_name, timeout).await?;
    let dir = format!("{root}/{}", fleet_core::service::attachments::ATTACH_DIR);

    let names = dedupe_names(&basenames_of(&args.local_paths));
    run_script(
        &ssh,
        &args.host_alias,
        &fleet_core::service::attachments::stage_script(&root),
        timeout,
    )
    .await?;
    transfer_all(&ssh, &args.host_alias, &args.local_paths, &names, &dir, timeout).await
}
```

Extract `basenames_of`, `run_script` and `transfer_all` from the existing body of `upload_to_session` and have both commands call them, so the local-vs-remote branch exists once. `resolve_worktree_root` runs `attachments::root_script` and trims stdout, mapping a non-zero status to `E_UPLOAD` with the host's stderr.

Register `commands::upload::upload_attachments` in `generate_handler!` after `attachment_preview`.

- [ ] **Step 6: Run the backend suite**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: every `upload` and `attachments` test passes; `every_command_has_a_verdict` still fails for the three new commands. That is Task 4.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/attachments.rs crates/fleet-core/src/service/mod.rs src-tauri/src/commands/upload.rs src-tauri/src/lib.rs
git commit -m "feat(upload): attachments land inside the worktree, excluded untracked"
```

---

### Task 4: Hub verdicts for the three new commands

`upload_to_session` is `LocalOnly` because the bytes are on the desktop and the host is the hub's to reach. All three new commands inherit that. The difference that matters: `TerminalView`'s whole pane is swapped out in hub mode, so its button simply is not there — **the composer is not swapped out**, so an attach button would be visible and broken without a real reason and a disabled affordance.

**Files:**
- Modify: `src-tauri/src/backend/verdicts.rs`
- Modify: `src-tauri/src/backend/tests_routing.rs`
- Modify: `src/lib/hub.ts` (`REASONS`)
- Generated: `src/lib/hub_verdicts.generated.json`, `docs/hub.md`, `docs/control-api-reference.md`

**Interfaces:**
- Consumes: the three command names from Tasks 1–3.
- Produces: `hubActionBlocked('upload_attachments', …)` returns a reason string in hub mode. Task 6 consumes it.

- [ ] **Step 1: Run the suite to see exactly what is red**

Run: `cargo test -p claude-fleet --lib tests_routing`
Expected: FAIL — `every_command_has_a_verdict` names `pick_attachments`, `attachment_preview` and `upload_attachments`.

- [ ] **Step 2: Add the three verdict rows**

In `src-tauri/src/backend/verdicts.rs`, in `generate_handler!` order, immediately after the `upload_to_session` row:

```rust
    (
        "pick_attachments",
        Verdict::LocalOnly {
            instead: "the picker opens on this machine and the session's host is the hub's to reach; attach the file from a standalone app, or copy it to the host yourself",
        },
    ),
    (
        "attachment_preview",
        Verdict::LocalOnly {
            instead: "the file is on this machine; a hub client has nothing to preview",
        },
    ),
    (
        "upload_attachments",
        Verdict::LocalOnly {
            instead: "the file is on this machine and the session's host is the hub's to reach; attach it from a standalone app, or copy it to the host yourself",
        },
    ),
```

Each command's body already calls `backend.refuse_local_only("<name>")` — Tasks 1 and 2 must have it too if they do not yet; add it as the first statement in both.

- [ ] **Step 3: Add the routing-test rows**

In `src-tauri/src/backend/tests_routing.rs`, add each command to the handler list and give each a body case in `every_commands_body_does_what_its_row_says` asserting it refuses in hub mode. These are `LocalOnly`, so they take **no** `routed_mutation_cases()` entry — the non-default-args wire rule applies to routed commands only.

- [ ] **Step 4: Regenerate, twice**

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
```

The first run reports FAILED on purpose so the diff cannot go unread. Read `git diff src/lib/hub_verdicts.generated.json docs/hub.md` — the refusal table gains three rows and the "Of the N commands, M route…" sentence changes — then run it again to confirm it passes.

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

- [ ] **Step 5: Add the frontend reason**

In `src/lib/hub.ts`'s `REASONS`, add an entry for `upload_attachments` (and `pick_attachments`) so the composer can render a reason instead of a dead button. Do **not** take the `hub_verdicts.test.ts` allowlist route: that is for commands whose entire UI is swapped out in hub mode, which the composer is not.

- [ ] **Step 6: Run everything**

Run: `cargo test --workspace && npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS, including `hub_verdicts.test.ts`.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/backend/verdicts.rs src-tauri/src/backend/tests_routing.rs src/lib/hub.ts src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(hub): attachments refuse in client mode, with a reason the UI can show"
```

---

### Task 5: The attachment list, as logic

The limits, the naming and the error wording are pure functions, so they are tested without a DOM. The component in Task 6 only draws what this decides.

**Files:**
- Create: `src/lib/attachments.ts`
- Create: `src/lib/attachments.test.ts`

**Interfaces:**
- Consumes: `PickedFile` from Task 1, mirrored in TS.
- Produces:
  `Attachment = { id: string; path: string; name: string; size: number; kind: 'image' | 'text' | 'binary'; thumb: string | null; state: 'ready' | 'reading' | 'error'; error: string | null }`
  `MAX_FILES = 10`, `MAX_BYTES = 10 * 1024 * 1024`, `MAX_TOTAL = 25 * 1024 * 1024`
  `addFiles(current: Attachment[], picked: PickedFile[]): { next: Attachment[]; rejected: string[] }`
  `pastedName(now: Date): string`
  `fmtBytes(n: number): string`
  Task 6 and Task 7 consume all of these.

- [ ] **Step 1: Write the failing test**

Create `src/lib/attachments.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { addFiles, pastedName, fmtBytes, MAX_FILES, MAX_BYTES } from './attachments';

const file = (o: Partial<{ path: string; name: string; size: number; kind: 'image' | 'text' | 'binary' }> = {}) => ({
  path: '/tmp/a.png', name: 'a.png', size: 1024, kind: 'image' as const, ...o,
});

describe('addFiles', () => {
  it('adds a picked file as a reading tile, so the strip reserves space at once', () => {
    const { next, rejected } = addFiles([], [file()]);
    expect(rejected).toEqual([]);
    expect(next).toHaveLength(1);
    expect(next[0]).toMatchObject({ name: 'a.png', kind: 'image', state: 'reading', thumb: null });
    expect(next[0].id).toBeTruthy();
  });

  it('rejects a file over the per-file limit, naming the size and the limit', () => {
    const { next, rejected } = addFiles([], [file({ name: 'big.png', size: MAX_BYTES + 1 })]);
    expect(next).toHaveLength(0);
    expect(rejected[0]).toContain('big.png');
    expect(rejected[0]).toContain('10 MB');
  });

  it('rejects past the count cap instead of silently truncating', () => {
    const full = Array.from({ length: MAX_FILES }, (_, i) => file({ path: `/tmp/${i}.png`, name: `${i}.png` }));
    const { next } = addFiles([], full);
    expect(next).toHaveLength(MAX_FILES);
    const { next: after, rejected } = addFiles(next, [file({ path: '/tmp/x.png', name: 'x.png' })]);
    expect(after).toHaveLength(MAX_FILES);
    expect(rejected[0]).toContain('10 files');
  });

  it('rejects when the total would exceed the budget', () => {
    const { next } = addFiles([], [file({ size: 9 * 1024 * 1024 })]);
    const { rejected } = addFiles(next, [
      file({ path: '/tmp/b.png', name: 'b.png', size: 9 * 1024 * 1024 }),
      file({ path: '/tmp/c.png', name: 'c.png', size: 9 * 1024 * 1024 }),
    ]);
    expect(rejected.some((r) => r.includes('in total'))).toBe(true);
  });

  it('does not add the same path twice', () => {
    const { next } = addFiles([], [file()]);
    const { next: after } = addFiles(next, [file()]);
    expect(after).toHaveLength(1);
  });
});

describe('pastedName', () => {
  it('makes ten pastes distinguishable', () => {
    expect(pastedName(new Date('2026-09-20T14:05:09'))).toBe('pasted-14.05.09.png');
  });
});

describe('fmtBytes', () => {
  it('reads like a person wrote it', () => {
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(1024 * 1024)).toBe('1.0 MB');
    expect(fmtBytes(14.2 * 1024 * 1024)).toBe('14.2 MB');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/attachments.test.ts`
Expected: FAIL — `Failed to resolve import "./attachments"`

- [ ] **Step 3: Write the module**

```ts
/**
 * The composer's attachment list. The limits, the naming and the error
 * wording live here as pure functions so they are tested without a DOM; the
 * component only draws what this decides.
 */

export type AttachKind = 'image' | 'text' | 'binary';

/** What `pick_attachments` returns, per file. */
export interface PickedFile {
  path: string;
  name: string;
  size: number;
  kind: AttachKind;
}

export interface Attachment {
  id: string;
  path: string;
  name: string;
  size: number;
  kind: AttachKind;
  /** Data URL from `attachment_preview`; null until it arrives or never. */
  thumb: string | null;
  state: 'ready' | 'reading' | 'error';
  error: string | null;
}

export const MAX_FILES = 10;
export const MAX_BYTES = 10 * 1024 * 1024;
export const MAX_TOTAL = 25 * 1024 * 1024;

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** A screenshot pastes as "image.png"; ten of them would be indistinguishable. */
export function pastedName(now: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `pasted-${p(now.getHours())}.${p(now.getMinutes())}.${p(now.getSeconds())}.png`;
}

let seq = 0;

/**
 * Add picked files to the list. Rejections are returned rather than thrown:
 * every one of them is a sentence the composer shows, and the tiles that did
 * fit still get added.
 */
export function addFiles(
  current: Attachment[],
  picked: PickedFile[],
): { next: Attachment[]; rejected: string[] } {
  const next = [...current];
  const rejected: string[] = [];
  let total = current.reduce((n, a) => n + a.size, 0);

  for (const f of picked) {
    if (next.some((a) => a.path === f.path)) continue;
    if (next.length >= MAX_FILES) {
      rejected.push(`${f.name} was not added — the limit is ${MAX_FILES} files.`);
      continue;
    }
    if (f.size > MAX_BYTES) {
      rejected.push(`${f.name} is ${fmtBytes(f.size)} — the limit is ${fmtBytes(MAX_BYTES)}.`);
      continue;
    }
    if (total + f.size > MAX_TOTAL) {
      rejected.push(
        `${f.name} would make ${fmtBytes(total + f.size)} in total — the limit is ${fmtBytes(MAX_TOTAL)}.`,
      );
      continue;
    }
    total += f.size;
    next.push({
      id: `att-${++seq}`,
      path: f.path,
      name: f.name,
      size: f.size,
      kind: f.kind,
      // The tile is reserved at full size before the preview decodes, so
      // decoding causes no reflow.
      thumb: null,
      state: 'reading',
      error: null,
    });
  }
  return { next, rejected };
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npx vitest run src/lib/attachments.test.ts`
Expected: PASS, 8 cases.

- [ ] **Step 5: Commit**

```bash
git add src/lib/attachments.ts src/lib/attachments.test.ts
git commit -m "feat(composer): the attachment list, with its limits and its wording"
```

---

### Task 6: The strip, the drop target and the paste

The shell from the control plan's Task 9 gains a thumbnail strip, an attach button and a drop veil. The drop target is the shell, not the panel: a drop on the transcript does nothing.

**Files:**
- Modify: `src/lib/ConversationPanel.svelte`
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `addFiles`, `pastedName`, `fmtBytes`, `Attachment` from Task 5; `preserveThread` from the control plan's Task 12; `.composer-shell` from its Task 9; `pick_attachments` / `attachment_preview` from Tasks 1–2; `hubActionBlocked` from Task 4.
- Produces: testids `conv-attach-button`, `conv-attachments`, `conv-attachment`, `conv-attachment-remove`, `conv-attach-error`. Task 7 consumes the `attachments` state.

- [ ] **Step 1: Write the failing tests**

```ts
  it('shows a tile per attachment and removes one without losing the rest', async () => {
    await renderPanel();
    await addAttachments([
      { path: '/tmp/a.png', name: 'a.png', size: 1024, kind: 'image' },
      { path: '/tmp/b.log', name: 'b.log', size: 2048, kind: 'text' },
    ]);
    expect(screen.getAllByTestId('conv-attachment')).toHaveLength(2);
    await fireEvent.click(screen.getAllByTestId('conv-attachment-remove')[0]);
    const left = screen.getAllByTestId('conv-attachment');
    expect(left).toHaveLength(1);
    expect(left[0].getAttribute('title')).toContain('b.log');
  });

  it('a drop on the shell attaches; a drop on the transcript does not', async () => {
    await renderPanel();
    const shell = document.querySelector('.composer-shell')!;
    await fireEvent.drop(shell, { dataTransfer: { types: ['Files'], files: [] } });
    expect(shell.className).not.toContain('is-dragging');

    const scroller = screen.getByTestId('conv-scroller');
    const ev = new Event('drop', { bubbles: true, cancelable: true });
    await fireEvent(scroller, ev);
    expect(screen.queryByTestId('conv-attachments')).toBeNull();
  });

  it('a rejected file stays visible with its reason', async () => {
    await renderPanel();
    await addAttachments([{ path: '/tmp/big.png', name: 'big.png', size: 11 * 1024 * 1024, kind: 'image' }]);
    expect(screen.getByTestId('conv-attach-error').textContent).toContain('big.png');
    expect(screen.getByTestId('conv-attach-error').textContent).toContain('10 MB');
  });

  it('the attach button states why it is unavailable in hub mode', async () => {
    await renderPanelInHubMode();
    const btn = screen.getByTestId('conv-attach-button');
    expect(btn.getAttribute('aria-disabled')).toBe('true');
    expect(btn.getAttribute('title')).toContain('standalone');
  });
```

`addAttachments` and `renderPanelInHubMode` are local helpers in the test file; build them on the file's existing render helper and its existing hub-status mock rather than inventing new ones.

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/ConversationPanel.test.ts -t 'attachment'`
Expected: FAIL — `Unable to find an element by: [data-testid="conv-attachment"]`

- [ ] **Step 3: Add the state and the handlers**

```ts
  import { addFiles, pastedName, fmtBytes, type Attachment, type PickedFile } from './attachments';

  let attachments = $state<Attachment[]>([]);
  let attachErrors = $state<string[]>([]);
  /** dragleave fires on every child, so nesting is counted, not flagged. */
  let dragDepth = $state(0);

  const attachBlocked = $derived(hubActionBlocked('upload_attachments', $hubStatus, $hubConnection));

  function hasFiles(dt: DataTransfer | null): boolean {
    return !!dt && Array.from(dt.types).includes('Files');
  }

  async function attach(picked: PickedFile[]) {
    const { next, rejected } = addFiles(attachments, picked);
    attachErrors = rejected;
    preserveThread(() => (attachments = next));
    for (const a of next.filter((x) => x.state === 'reading')) {
      const r = await invokeCmd<string | null>('attachment_preview', { path: a.path });
      attachments = attachments.map((x) =>
        x.id !== a.id
          ? x
          : r.ok
            ? { ...x, thumb: r.value, state: 'ready' as const }
            : { ...x, state: 'error' as const, error: r.error.message },
      );
    }
  }

  function removeAttachment(id: string) {
    preserveThread(() => (attachments = attachments.filter((a) => a.id !== id)));
  }

  async function pickFiles() {
    if (attachBlocked !== null) return;
    const r = await invokeCmd<PickedFile[]>('pick_attachments', {});
    if (r.ok) await attach(r.value);
    else attachErrors = [r.error.message];
  }
```

Drop and paste:

```ts
  function onShellDragEnter(e: DragEvent) {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    dragDepth++;
  }
  function onShellDragOver(e: DragEvent) {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
  }
  function onShellDragLeave() {
    dragDepth = Math.max(0, dragDepth - 1);
  }
  function onShellDrop(e: DragEvent) {
    e.preventDefault();
    // The window-level guard must not also see this one.
    e.stopPropagation();
    dragDepth = 0;
    // NOTE: this is WRONG and is corrected below — a dropped File in a
    // WKWebView carries no filesystem path, so there is nothing here to echo.
    void attach(pickedFromDrop(e));
  }
  function onComposerPaste(e: ClipboardEvent) {
    const dt = e.clipboardData;
    if (!dt || dt.files.length === 0) return;
    // A rich-text paste carries both; the text half wins.
    if (dt.getData('text/plain').trim().length > 0) return;
    e.preventDefault();
    void attach(
      Array.from(dt.files).map((f) => ({
        path: f.name,
        name: f.name === 'image.png' ? pastedName(new Date()) : f.name,
        size: f.size,
        kind: f.type.startsWith('image/') ? ('image' as const) : ('binary' as const),
      })),
    );
  }
```

**Where the dropped paths actually come from.** The DOM `drop` event cannot
supply them: in a WKWebView a dropped `File` has no filesystem path, which is
exactly why `TerminalView.svelte:325` subscribes to
`getCurrentWebview().onDragDropEvent` instead — and that is the same event
`lib.rs` already listens to in order to record those paths on the allow-list.
So the composer subscribes to it too, hit-tests the drop point against the
shell's bounding rect the way `pointOverGrid` does for the terminal grid, and
attaches the paths the event carries. The DOM handlers stay, but only to drive
the drag veil and to `stopPropagation()` so the window-level `file://` guard
does not swallow the drop. Without this, the attach button works and dropping
silently does nothing.

**Note on paste:** a pasted file has no filesystem path, so it cannot go through the allow-list as-is. Until a `record_pasted_bytes` command exists, paste attaches the file for display and Task 7 rejects it at send time with a stated reason. Add that command to the deferred list in the spec rather than half-building it here.

- [ ] **Step 4: Add the markup inside `.composer-shell`**

Strip above the textarea, attach button in `.composer-actions` before the hint, veil last:

```svelte
        {#if attachments.length}
          <ul class="attach-strip" data-testid="conv-attachments" aria-label="Attachments">
            {#each attachments as a (a.id)}
              <li class="attach" data-testid="conv-attachment" data-state={a.state} title="{a.name} · {fmtBytes(a.size)}">
                {#if a.thumb}
                  <img class="attach-img" src={a.thumb} alt="" />
                {:else}
                  <span class="attach-ext">{a.name.split('.').pop() ?? 'file'}</span>
                {/if}
                <button type="button" class="btn btn--icon btn--quiet attach-x" data-testid="conv-attachment-remove"
                  aria-label="Remove {a.name}" title="Remove {a.name}" onclick={() => removeAttachment(a.id)}>×</button>
              </li>
            {/each}
          </ul>
        {/if}
        {#each attachErrors as msg (msg)}
          <p class="attach-error" role="status" data-testid="conv-attach-error">{msg}</p>
        {/each}
```

```svelte
          <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-attach-button"
            aria-label="Attach files" title={attachBlocked ?? 'Attach files'}
            aria-disabled={attachBlocked !== null} onclick={pickFiles}>⌾</button>
```

```svelte
        {#if dragDepth > 0}
          <div class="drop-veil" aria-hidden="true">Drop to attach</div>
        {/if}
```

and the handlers on the shell itself: `ondragenter`, `ondragover`, `ondragleave`, `ondrop`, plus `onpaste={onComposerPaste}` on the textarea.

- [ ] **Step 5: Style it**

```css
  .attach-strip {
    display: flex;
    flex-wrap: wrap;
    gap: var(--control-gap);
    margin: 0 0 2px;
    padding: 0 2px;
    list-style: none;
    /* Exactly two rows, then scroll: growth is quantised to 50px so
       preserveThread corrects by a clean integer. */
    max-height: 94px;
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  .attach {
    position: relative;
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    overflow: hidden;
  }
  .attach-img { width: 100%; height: 100%; object-fit: cover; display: block; }
  .attach-ext {
    font-family: var(--mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--control-fg-quiet);
  }
  /* 18px is the one deliberate exception to the 24px floor: the 44px tile is
     the primary target and Backspace on a focused tile removes it too, which
     is WCAG 2.5.8's equivalent-control path. It is bounded to this case. */
  .attach-x {
    position: absolute;
    top: 2px;
    right: 2px;
    width: 18px;
    height: 18px;
    padding: 0;
    background: color-mix(in srgb, var(--bg) 78%, transparent);
    color: var(--fg);
    font-size: 13px;
    opacity: 0;
  }
  .attach:hover .attach-x,
  .attach:focus-within .attach-x,
  .attach-x:focus-visible { opacity: 1; }
  @media (hover: none) { .attach-x { opacity: 1; } }
  .attach[data-state='reading'] { opacity: 0.6; }
  .attach[data-state='error'] {
    border-color: var(--usage-crit);
    box-shadow: inset 0 0 0 1px var(--usage-crit);
  }
  .attach-error {
    margin: 0 0 2px;
    padding: 0 2px;
    color: var(--usage-crit);
    font-size: var(--control-font-sm);
  }
  .composer-shell.is-dragging {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .drop-veil {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--bg) 82%, transparent);
    color: var(--accent);
    font-size: var(--control-font);
    font-weight: 600;
    /* Must not eat the drop event. */
    pointer-events: none;
  }
```

Add `class:is-dragging={dragDepth > 0}` to the shell.

- [ ] **Step 6: Run the full frontend suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(composer): attach files, with thumbnails in the box"
```

---

### Task 6b: A dropped file has a size

Added during execution. Task 6's drop path works, but Tauri's drag-drop event
carries **paths only** — nothing stats them — so a dropped attachment arrives
with `size: 0`. `MAX_BYTES` and `MAX_TOTAL` are therefore enforced for picked
files and silently skipped for dropped ones, which is the primary entry point.
`upload_attachments` enforces no byte budget either, so a dropped 2 GB file
would be attempted: `SshClient::upload_file` streams it under a 60 s per-file
timeout, on a link the user may not control.

The limits the spec states must apply to both origins, or they are not limits.

**Files:**
- Modify: `src-tauri/src/commands/upload.rs`, `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/backend/verdicts.rs`, `src/lib/hub.ts`
- Modify: `src/lib/ConversationPanel.svelte`, `src/lib/attachments.ts`
- Test: the Rust test module, `src/lib/attachments.test.ts`, `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `UploadAllowList`, `classify`, `PickedFile` — all existing.
- Produces: `#[tauri::command] attachment_describe(paths: Vec<String>, allow) -> Result<Vec<PickedFile>, IpcError>`.
  It is the measuring sibling of `attachment_preview`: allow-list gate first, then
  stat, then classify. It authorises nothing — a dropped path is already
  authorised by the Tauri drag-drop handler in `lib.rs`, which is what makes this
  safe to add without widening the threat model.

**Steps**

- [ ] Write the failing Rust tests: an allow-listed path is described with its
  real size and kind; a path that is not allow-listed is `E_FORBIDDEN` and is
  never stat'd; a batch with one unreadable file returns the rest.
- [ ] Run them, confirm they fail, record the output.
- [ ] Implement `attachment_describe`, reusing `classify`. Gate first, no
  filesystem access before the gate — the same ordering `preview_for` already
  follows and that its review verified.
- [ ] Register it in `generate_handler!` after `attachment_preview`; add its
  `LocalOnly` verdict row in that same position; `refuse_local_only` by name;
  `REGEN_HUB_VERDICTS=1`, `REGEN_DOCS=1`, and `REGEN_LOCAL_ONLY=1` if asked.
  Add its `REASONS` entry in `src/lib/hub.ts` — the composer reaches it, so it
  does not belong on the no-UI-caller allowlist.
- [ ] In the composer's drop path, call `attachment_describe` with the event's
  paths and feed the described files into `addFiles`, so a dropped file goes
  through exactly the same limits as a picked one.
- [ ] Remove `droppedFile`'s `size: 0` placeholder from `attachments.ts`, or
  keep it only for the case where describing fails and say so in a comment.
- [ ] Add a frontend test proving an oversized DROPPED file is rejected with the
  same message an oversized picked file gets.
- [ ] Full suite, clippy, fmt, then commit.

```bash
git commit -m "fix(upload): dropped attachments obey the same size limits as picked ones"
```

---

### Task 7: Sending, with the paths in the prompt

The files upload first; their absolute remote paths go into the prompt text. An upload failure **cancels the send and leaves the draft intact** — sending the prompt without its attachment would hand Claude an incomplete brief and look like it worked.

**Files:**
- Create: `src/lib/attach_prompt.ts`
- Create: `src/lib/attach_prompt.test.ts`
- Modify: `src/lib/ConversationPanel.svelte` (`send` / `sendText`)
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `upload_attachments` from Task 3; `Attachment` from Task 5.
- Produces: `withAttachments(draft: string, paths: string[]): string` and `PROMPT_MAX_BYTES = 120 * 1024`, `tooLong(text: string): boolean`.

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from 'vitest';
import { withAttachments, tooLong, PROMPT_MAX_BYTES } from './attach_prompt';

describe('withAttachments', () => {
  it('names the files under the prompt, one per line', () => {
    expect(withAttachments('look at this', ['/w/p/.claude-fleet-attachments/a.png'])).toBe(
      'look at this\n\nAttached files:\n/w/p/.claude-fleet-attachments/a.png',
    );
  });
  it('leaves a prompt without attachments alone', () => {
    expect(withAttachments('hello', [])).toBe('hello');
  });
  it('works when the draft is empty', () => {
    expect(withAttachments('', ['/w/a.png'])).toBe('Attached files:\n/w/a.png');
  });
});

describe('tooLong', () => {
  it('bounds the prompt below the 128 KiB argv ceiling', () => {
    expect(PROMPT_MAX_BYTES).toBeLessThan(128 * 1024);
    expect(tooLong('x'.repeat(100))).toBe(false);
    expect(tooLong('x'.repeat(PROMPT_MAX_BYTES + 1))).toBe(true);
  });
  it('measures bytes, not characters', () => {
    expect(tooLong('é'.repeat(PROMPT_MAX_BYTES - 10))).toBe(true);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/attach_prompt.test.ts`
Expected: FAIL — `Failed to resolve import "./attach_prompt"`

- [ ] **Step 3: Write the module**

```ts
/**
 * Putting attachment paths into a prompt.
 *
 * The prompt is delivered by `tmux send-keys -l` inside a single `bash -lc`
 * argv word, and Linux caps one argument at 128 KiB (MAX_ARG_STRLEN). Nothing
 * on the send path validates that today: an over-long prompt surfaces as a
 * raw "Argument list too long". Paths are small, but the bound belongs here.
 */

/** Below 128 KiB, with room for the tmux wrapper around the body. */
export const PROMPT_MAX_BYTES = 120 * 1024;

export function tooLong(text: string): boolean {
  return new TextEncoder().encode(text).length > PROMPT_MAX_BYTES;
}

export function withAttachments(draft: string, paths: string[]): string {
  if (paths.length === 0) return draft;
  const block = `Attached files:\n${paths.join('\n')}`;
  return draft.trim().length === 0 ? block : `${draft}\n\n${block}`;
}
```

- [ ] **Step 4: Write the failing component test**

```ts
  it('uploads before sending and puts the paths in the prompt', async () => {
    const invoked: Array<{ cmd: string; args: unknown }> = [];
    mockInvoke((cmd, args) => {
      invoked.push({ cmd, args });
      if (cmd === 'upload_attachments') return ['/w/p/.claude-fleet-attachments/a.png'];
      return null;
    });
    await renderPanel();
    await addAttachments([{ path: '/tmp/a.png', name: 'a.png', size: 1024, kind: 'image' }]);
    await fireEvent.input(screen.getByTestId('conv-composer-input'), { target: { value: 'look' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));

    const sent = invoked.find((i) => i.cmd === 'send_prompt');
    expect((sent!.args as { args: { prompt: string } }).args.prompt).toBe(
      'look\n\nAttached files:\n/w/p/.claude-fleet-attachments/a.png',
    );
    expect(invoked.findIndex((i) => i.cmd === 'upload_attachments'))
      .toBeLessThan(invoked.findIndex((i) => i.cmd === 'send_prompt'));
  });

  it('a failed upload cancels the send and keeps the draft', async () => {
    mockInvoke((cmd) => {
      if (cmd === 'upload_attachments') throw { code: 'E_UPLOAD', message: 'host unreachable' };
      return null;
    });
    await renderPanel();
    await addAttachments([{ path: '/tmp/a.png', name: 'a.png', size: 1024, kind: 'image' }]);
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'look' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));

    expect(box.value).toBe('look');
    expect(screen.getByTestId('conv-composer-error').textContent).toContain('host unreachable');
  });
```

Use the test file's existing invoke mock rather than adding a second shape; `mockInvoke` above stands for whatever it is called there.

- [ ] **Step 5: Wire it into `send`**

In `ConversationPanel.svelte`'s send path, before the existing `sendPrompt` call:

```ts
    let paths: string[] = [];
    if (attachments.length > 0) {
      const up = await invokeCmd<string[]>('upload_attachments', {
        args: {
          host_alias: session.host_alias,
          session_name: session.tmux_name,
          local_paths: attachments.map((a) => a.path),
        },
      });
      if (!up.ok) {
        // The draft stays: a prompt without its attachment is a worse
        // outcome than no prompt at all.
        sendError = up.error.message;
        sending = false;
        return;
      }
      paths = up.value;
    }
    const body = withAttachments(draft, paths);
    if (tooLong(body)) {
      sendError = 'That prompt is too long to send through tmux. Shorten it.';
      sending = false;
      return;
    }
```

then send `body` instead of `draft`, and clear `attachments` alongside `draft` only on success.

- [ ] **Step 6: Run everything**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/lib/attach_prompt.ts src/lib/attach_prompt.test.ts src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(composer): send a prompt with its attachments"
```

---

## Done when

- `scripts/ci-local.sh` passes in CI order.
- `cargo test --workspace` passes, including `every_command_has_a_verdict`, `every_commands_body_does_what_its_row_says` and `reference_is_current`.
- `git diff --stat` shows `src/lib/hub_verdicts.generated.json`, `docs/hub.md` and `docs/control-api-reference.md` regenerated, not hand-edited.
- Attaching a file in hub-client mode shows a reason, not a dead button.
- No command accepts a local path from the webview that the webview could have invented.
