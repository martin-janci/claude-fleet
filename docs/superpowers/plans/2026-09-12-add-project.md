# Add a project that is not checked out yet — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From the New-session flow, add a project by cloning a GitHub repo, browsing the account's repos, adopting an existing folder, or creating a new one — on any reachable host.

**Architecture:** One `add_project` command over a tagged `AddProjectSource` enum in a new `service/add_project.rs`, with a `_with(&dyn SshExec)` twin for tests; a read-only `list_github_repos`; a pure URL parser mirrored in Rust and TypeScript; and one `AddProjectDialog.svelte` reached from the sidebar's project picker.

**Tech Stack:** Rust (Tauri 2, tokio::process, `FakeSsh`), Svelte 5 runes, Vitest. New dependency: `tauri-plugin-dialog` (Rust + JS + capability).

Spec: `docs/superpowers/specs/2026-09-12-add-project-design.md` — read it before Task 1.

Conventions for every task: shell-quote every interpolated value with `crate::shell::quote`; never hold the `Store` mutex across an `.await`; Rust from `src-tauri/` (`cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`); frontend from the root with `npx vitest run`, `npx svelte-check --tsconfig ./tsconfig.json`, `pnpm run build` (the pnpm test/check binaries are not on PATH). Commit after each task; no attribution lines. A new Tauri command means `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`.

---

## File map

| File | Change |
|---|---|
| `src-tauri/src/repo_url.rs` | new: `parse_repo_url` + `clone_url_for` |
| `src-tauri/src/service/add_project.rs` | new: args/source types, `add_project(_with)`, `list_github_repos(_with)` |
| `src-tauri/src/service/mod.rs`, `src-tauri/src/lib.rs` | declare the module; register both commands |
| `src-tauri/src/commands/projects.rs` | thin wrappers |
| `src-tauri/Cargo.toml`, `package.json`, `src-tauri/capabilities/default.json` | `tauri-plugin-dialog` |
| `docs/control-api-reference.md` | regenerated |
| `src/lib/repo_url.ts` (+ test) | the TS mirror of the parser |
| `src/lib/projects.ts` | `addProject`, `listGithubRepos`, types |
| `src/lib/AddProjectDialog.svelte` (+ test) | the dialog |
| `src/lib/Sidebar.svelte`, `src/lib/Sidebar.test.ts` | the `Add project…` row and the hand-off to `NewSessionDialog` |

---

### Task 1: `parse_repo_url` in Rust

**Files:**
- Create: `src-tauri/src/repo_url.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod repo_url;` next to the other top-level modules)

- [ ] **Step 1: Write the failing test**

At the bottom of `src-tauri/src/repo_url.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_accepted_github_form() {
        for input in [
            "martin-janci/claude-fleet",
            "https://github.com/martin-janci/claude-fleet",
            "https://github.com/martin-janci/claude-fleet.git",
            "http://github.com/martin-janci/claude-fleet",
            "https://github.com/martin-janci/claude-fleet/",
            "git@github.com:martin-janci/claude-fleet.git",
            "git@github.com:martin-janci/claude-fleet",
            "ssh://git@github.com/martin-janci/claude-fleet.git",
            "  martin-janci/claude-fleet  ",
        ] {
            assert_eq!(
                parse_repo_url(input),
                Some(("martin-janci".to_string(), "claude-fleet".to_string())),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_what_is_not_a_github_repo() {
        for bad in [
            "",
            "   ",
            "claude-fleet",
            "martin-janci/",
            "/claude-fleet",
            "martin-janci/claude-fleet/extra",
            "https://gitlab.com/o/r",
            "https://github.com/",
            "https://github.com/only-owner",
            "../etc/passwd",
            "martin-janci/../escape",
            "o/r; rm -rf /",
        ] {
            assert_eq!(parse_repo_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn clone_url_is_always_github_ssh() {
        assert_eq!(
            clone_url_for("martin-janci", "claude-fleet"),
            "git@github.com:martin-janci/claude-fleet.git"
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd src-tauri && cargo test repo_url`
Expected: compile error, `parse_repo_url` not found.

- [ ] **Step 3: Implement**

```rust
//! Parsing the repo identifiers the Add-project dialog accepts. GitHub only:
//! the clone URL is always normalised to GitHub SSH, matching
//! `ensure_remote_project`.

/// `(owner, repo)` from `owner/repo`, an https URL, or an SSH URL. `None`
/// for anything else — including a host other than github.com and any
/// component that is not a safe path component, so a parsed pair is always
/// safe to interpolate into a path.
pub fn parse_repo_url(input: &str) -> Option<(String, String)> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let rest = if let Some(r) = s.strip_prefix("git@github.com:") {
        r
    } else if let Some(r) = s.strip_prefix("ssh://git@github.com/") {
        r
    } else if let Some(r) = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
    {
        r
    } else if s.contains("://") || s.contains('@') {
        // Some other host, or an SSH form we do not accept.
        return None;
    } else {
        s
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !is_component(owner) || !is_component(repo) {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// A safe single path component: non-empty, no `/`, not `.`/`..`, and only
/// characters GitHub allows in an owner or repo name.
fn is_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The URL fleet clones with, for a pair from [`parse_repo_url`].
pub fn clone_url_for(owner: &str, repo: &str) -> String {
    format!("git@github.com:{owner}/{repo}.git")
}
```

- [ ] **Step 4: Run the tests**

Run: `cd src-tauri && cargo test repo_url` — 3 passed. Then `cargo fmt` / `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/repo_url.rs src-tauri/src/lib.rs
git commit -m "feat(repo-url): parse the repo identifiers Add project accepts"
```

---

### Task 2: `add_project` — the `clone` source

**Files:**
- Create: `src-tauri/src/service/add_project.rs`
- Modify: `src-tauri/src/service/mod.rs` (`pub mod add_project;`)

This task implements only `AddProjectSource::Clone`; Tasks 3 and 4 add the other two. Define the whole enum now so the later tasks only add match arms.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    fn store_with_no_projects() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    #[tokio::test]
    async fn clone_on_a_remote_host_runs_git_clone_and_registers_the_row() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u")
            .on(Match::script_contains("git clone"), Reply::ok(""));

        let row = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone {
                    url: "https://github.com/o/r".into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();

        assert_eq!((row.project.owner.as_str(), row.project.repo.as_str()), ("o", "r"));
        assert!(row.worktrees.is_empty(), "a fresh clone has no scanned worktrees yet");
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(
            script.contains("git clone 'git@github.com:o/r.git' '/home/u/projects/github.com/o/r'"),
            "{script}"
        );
        // The row's base_path is the LOCAL path it would occupy, not the host's.
        let s = store.lock().unwrap();
        let p = s.list_projects().unwrap();
        assert_eq!(p.len(), 1);
        assert!(!p[0].base_path.starts_with("/home/u/"), "{}", p[0].base_path);
    }

    #[tokio::test]
    async fn a_bad_url_is_refused_before_any_ssh() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone { url: "https://gitlab.com/o/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(fake.calls().is_empty(), "nothing should run for an unparseable url");
    }

    #[tokio::test]
    async fn a_project_that_already_exists_is_refused() {
        let store = store_with_no_projects();
        store.lock().unwrap().upsert_project("o", "r", "/p/o/r").unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone { url: "o/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_EXISTS");
        assert!(err.message.contains("o/r"));
    }

    #[tokio::test]
    async fn a_failing_clone_leaves_no_project_row() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("git clone"),
            Reply::fail(128, "fatal: repository not found"),
        );
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone { url: "o/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_GIT_SETUP");
        assert!(err.message.contains("repository not found"));
        assert!(store.lock().unwrap().list_projects().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_clone_script_quotes_an_owner_with_a_space_and_a_quote() {
        // parse_repo_url rejects those characters, so drive the script
        // builder directly to prove the quoting.
        let script = clone_script("/ro ot/o'x/r", "git@github.com:o'x/r.git");
        assert!(script.contains(&crate::shell::quote("/ro ot/o'x/r")), "{script}");
        assert!(script.contains(&crate::shell::quote("git@github.com:o'x/r.git")), "{script}");
        assert!(script.contains("mkdir -p \"$(dirname --"), "{script}");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd src-tauri && cargo test add_project` — compile errors.

- [ ] **Step 3: Implement**

```rust
//! Adding a project fleet does not know yet: clone a GitHub repo, adopt an
//! existing checkout, or create a new one. The New-session flow's entry
//! point for "this repo is not on disk yet".

use crate::ipc_error::{codes, IpcError};
use crate::projects::Layout;
use crate::repo_url::{clone_url_for, parse_repo_url};
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{ProjectTreeRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Wall clock for a clone: big repos over a slow link. The frontend can
/// cancel sooner through `call_id`.
const CLONE_WALL_CLOCK: Duration = Duration::from_secs(600);

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddProjectSource {
    Clone {
        url: String,
    },
    Folder {
        path: String,
    },
    New {
        owner: String,
        repo: String,
        #[serde(default)]
        create_remote: bool,
        #[serde(default)]
        confirm: Option<String>,
    },
}

#[derive(Deserialize)]
pub struct AddProjectArgs {
    pub host_alias: String,
    pub source: AddProjectSource,
    /// Set by `invokeCmdAbortable` so the dialog's Cancel can abort a clone.
    #[serde(default)]
    pub call_id: Option<String>,
}

/// Production entry point.
pub async fn add_project(
    args: AddProjectArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ProjectTreeRow, IpcError> {
    add_project_with(args, store, &**ssh).await
}

pub async fn add_project_with(
    args: AddProjectArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<ProjectTreeRow, IpcError> {
    match &args.source {
        AddProjectSource::Clone { url } => clone_source(&args, url, store, ssh).await,
        // Tasks 3 and 4.
        AddProjectSource::Folder { .. } | AddProjectSource::New { .. } => Err(IpcError::new(
            codes::E_INVALID,
            "source not implemented yet",
        )),
    }
}

async fn clone_source(
    args: &AddProjectArgs,
    url: &str,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<ProjectTreeRow, IpcError> {
    let (owner, repo) = parse_repo_url(url).ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("{url:?} is not a GitHub repository (try owner/repo or its URL)"),
        )
    })?;
    refuse_existing_project(store, &owner, &repo)?;
    let (host_root, local_root, layout) = roots(store, &args.host_alias)?;
    let local_base = layout.project_dir(&local_root, &owner, &repo);
    let clone_url = clone_url_for(&owner, &repo);

    if args.host_alias == crate::service::projects::LOCAL_HOST {
        run_local_script(&clone_script(&local_base, &clone_url), CLONE_WALL_CLOCK).await?;
    } else {
        let home = ssh.remote_home(&args.host_alias).await?;
        let root = crate::service::projects::expand_home(&host_root, &home);
        let dest = layout.project_dir(&root, &owner, &repo);
        let script = clone_script(&dest, &clone_url);
        let out = ssh
            .run(&args.host_alias, &["bash", "-lc", &quote(&script)], CLONE_WALL_CLOCK)
            .await?;
        if !out.status.success() {
            return Err(git_error(&args.host_alias, &out));
        }
    }
    register(store, &owner, &repo, &local_base)
}

/// Clone `url` into `dest` unless `dest` is already a checkout. Every value
/// quoted; `set -e` so a failed `mkdir` does not reach `git clone`.
pub(crate) fn clone_script(dest: &str, clone_url: &str) -> String {
    let d = quote(dest);
    format!(
        "set -e\n\
         if git -C {d} rev-parse --git-dir >/dev/null 2>&1; then echo \"exists\" >&2; exit 3; fi\n\
         mkdir -p \"$(dirname -- {d})\"\n\
         git clone {url} {d}\n",
        url = quote(clone_url),
    )
}
```

Write the remaining helpers to match the codebase:

- `refuse_existing_project(store, owner, repo)` — `codes::E_EXISTS` (add it to the codes module in this task) with the message `"<owner>/<repo> is already a fleet project"` when `Store::list_projects` already holds the pair. Compare CASE-INSENSITIVELY: GitHub treats owner and repo names case-insensitively and the default macOS filesystem does too, so `Owner/Repo` and `owner/repo` would otherwise become two project rows pointing at one checkout. The parser deliberately does NOT lowercase (the sidebar shows the owner as typed, and `FrantisekSefcik` must not become `franteseksefcik`), so the de-duplication belongs here. Takes and drops the lock, no `.await` inside.
- `roots(store, host)` — under one lock, returns `(project_base_for(s, host), local_projects_root(s) as String, layout(s))`.
- `run_local_script(script, wall_clock)` — `tokio::process::Command::new("bash").arg("-lc").arg(script)` with the same wall-clock/kill handling `SshClient::run_child` uses; map a non-zero exit to `E_GIT_SETUP` with stderr, exit code 3 to `E_EXISTS`.
- `git_error(host, out)` — exit code 3 → `E_EXISTS` ("already cloned at that path; it should appear after a refresh"); otherwise `E_GIT_SETUP` with stderr, falling back to stdout, and `(no stderr)` when both are empty.
- `register(store, owner, repo, base_path)` — `upsert_project` (which emits `project:updated`) then read the row back as a `ProjectTreeRow` with its (empty) worktrees. Check how `service::projects::list_projects` builds a `ProjectTreeRow` and reuse that shape.

Wire `call_id` through the cancellable path exactly as `new_session` does (see `commands/mutate.rs` / `cancel.rs` for the registry) — if that plumbing lives in the command layer, do it in Task 5 and note it here.

- [ ] **Step 4: Run the tests**

Run: `cd src-tauri && cargo test add_project` — all pass. Then fmt + clippy.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/add_project.rs src-tauri/src/service/mod.rs
git commit -m "feat(projects): add a project by cloning a GitHub repo"
```

---

### Task 3: the `folder` source

**Files:** `src-tauri/src/service/add_project.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn folder_is_refused_for_a_remote_host() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Folder { path: "/srv/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("local"));
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn folder_adopts_a_real_checkout_and_reads_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my-repo");
        std::fs::create_dir_all(&path).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["remote", "add", "origin", "git@github.com:acme/widget.git"],
        ] {
            let ok = std::process::Command::new("git")
                .args(&args)
                .current_dir(&path)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        }
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder { path: path.to_string_lossy().into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!((row.project.owner.as_str(), row.project.repo.as_str()), ("acme", "widget"));
        // Registered where it is — nothing moved.
        assert_eq!(row.project.base_path, path.canonicalize().unwrap().to_string_lossy());
    }

    #[tokio::test]
    async fn folder_without_an_origin_falls_back_to_the_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loose-repo");
        std::fs::create_dir_all(&path).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&path)
            .output()
            .unwrap()
            .status
            .success());
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder { path: path.to_string_lossy().into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.repo, "loose-repo");
    }

    #[tokio::test]
    async fn folder_that_is_not_a_checkout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder { path: dir.path().to_string_lossy().into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("git"));
    }
```

Check whether `tempfile` is already a dev-dependency in `src-tauri/Cargo.toml`; if not, add it under `[dev-dependencies]`. If the repo already has a helper for a throwaway git repo in tests (search `git init` under `src-tauri/src`), use that instead of hand-rolling.

- [ ] **Step 2: Run to verify they fail.** `cd src-tauri && cargo test add_project::tests::folder`

- [ ] **Step 3: Implement**

Add the `Folder` arm:

```rust
async fn folder_source(
    args: &AddProjectArgs,
    path: &str,
    store: &Mutex<Store>,
) -> Result<ProjectTreeRow, IpcError> {
    if args.host_alias != crate::service::projects::LOCAL_HOST {
        return Err(IpcError::new(
            codes::E_INVALID,
            "adopting a folder works on the local host only; clone it on the remote host instead",
        ));
    }
    crate::validate::remote_abs_path("folder", path)?;
    // Off the async worker: std::fs + git are blocking.
    let p = path.to_string();
    let (base_path, owner, repo) = tokio::task::spawn_blocking(move || adopt(&p))
        .await
        .map_err(|e| IpcError::new("E_IO", format!("folder probe failed: {e}")))??;
    refuse_existing_project(store, &owner, &repo)?;
    register(store, &owner, &repo, &base_path)
}

/// The checkout at `path`: its canonical top level, and the `(owner, repo)`
/// its `origin` names — falling back to `("local", <basename>)` when there
/// is no origin or it is not a GitHub URL.
fn adopt(path: &str) -> Result<(String, String, String), IpcError> {
    let top = git_out(path, &["rev-parse", "--show-toplevel"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            IpcError::new(codes::E_INVALID, format!("{path} is not a git checkout"))
        })?;
    let origin = git_out(&top, &["remote", "get-url", "origin"]).unwrap_or_default();
    let (owner, repo) = parse_repo_url(&origin).unwrap_or_else(|| {
        let name = std::path::Path::new(&top)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        ("local".to_string(), name)
    });
    Ok((top, owner, repo))
}

/// `git -C <dir> <args>` trimmed stdout, or `None` when git fails.
fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
```

Route it from `add_project_with`'s match.

- [ ] **Step 4: Run the tests + fmt + clippy.**

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/add_project.rs src-tauri/Cargo.toml
git commit -m "feat(projects): adopt an existing checkout as a project"
```

---

### Task 4: the `new` source and `list_github_repos`

**Files:** `src-tauri/src/service/add_project.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn new_creates_a_repo_with_an_initial_commit() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(Match::Any, Reply::ok(""));
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: false,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.repo, "widget");
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(script.contains("git init -b main"), "{script}");
        assert!(script.contains("commit --allow-empty"), "{script}");
        assert!(!script.contains("gh repo create"), "{script}");
    }

    #[tokio::test]
    async fn create_remote_without_the_confirmation_is_refused() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(Match::Any, Reply::ok(""));
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: true,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_CONFIRM_REQUIRED);
        assert!(fake.calls_for("vps").iter().all(|c| !c.command().contains("gh repo create")));
    }

    #[tokio::test]
    async fn create_remote_with_the_confirmation_runs_gh_repo_create() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(Match::Any, Reply::ok(""));
        add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: true,
                    confirm: Some("acme/widget".into()),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(script.contains("gh repo create 'acme/widget' --private"), "{script}");
    }

    #[tokio::test]
    async fn list_github_repos_maps_the_json_and_surfaces_a_failure() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("gh repo list"),
            Reply::ok(r#"[{"nameWithOwner":"acme/widget","description":"w","isPrivate":true,"updatedAt":"2026-09-01T10:00:00Z"}]"#),
        );
        let repos = list_github_repos_with("vps", &store, &fake).await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name_with_owner, "acme/widget");
        assert!(repos[0].is_private);

        let bad = FakeSsh::new();
        bad.with_home("/home/u").on(
            Match::script_contains("gh repo list"),
            Reply::fail(4, "gh: To get started with GitHub CLI, please run: gh auth login"),
        );
        let err = list_github_repos_with("vps", &store, &bad).await.unwrap_err();
        assert!(err.message.contains("gh auth login"), "{}", err.message);
    }
```

Error codes this task introduces: `E_EXISTS` and `E_GH` do NOT exist yet —
add both to the `codes` module in `src-tauri/src/ipc_error.rs` alongside the
existing constants, with the one-line doc comments the neighbours have, and
use `codes::E_EXISTS` / `codes::E_GH` rather than string literals. The
confirmation code already exists as `codes::E_CONFIRM_REQUIRED`.

- [ ] **Step 2: Run to verify they fail.**

- [ ] **Step 3: Implement**

The `New` arm: validate `owner` and `repo` with `crate::validate::path_component`; refuse an existing project; when `create_remote` is set, require `confirm == Some("<owner>/<repo>")` and otherwise return `codes::E_CONFIRM_REQUIRED` with a message naming the repository; build the script

```rust
fn new_project_script(dest: &str, owner: &str, repo: &str, create_remote: bool) -> String {
    let d = quote(dest);
    let mut s = format!(
        "set -e\n\
         if [ -e {d} ]; then echo \"exists\" >&2; exit 3; fi\n\
         mkdir -p {d}\n\
         git -C {d} init -b main\n\
         git -C {d} commit --allow-empty -m 'Initial commit'\n",
    );
    if create_remote {
        s.push_str(&format!(
            "git -C {d} -c gh.prompt=disabled >/dev/null 2>&1 || true\n\
             (cd {d} && gh repo create {slug} --private --source . --remote origin --push)\n",
            slug = quote(&format!("{owner}/{repo}")),
        ));
    }
    s
}
```

(simplify the `gh.prompt` line away if it is not needed — the point is that `gh repo create` runs inside the new directory and only when asked). Run it locally or over SSH exactly as the clone arm does, then `register`.

`list_github_repos_with(host, store, ssh)`:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GithubRepo {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: Option<String>,
}
```

runs `gh repo list --limit 200 --json nameWithOwner,description,isPrivate,updatedAt` (locally or over SSH), maps a non-zero exit to `codes::E_GH` carrying stderr verbatim, and parses the JSON with serde (`#[serde(rename_all = "camelCase")]` on a private wire struct). A parse failure is `codes::E_GH` too, with the first 200 bytes of output.

- [ ] **Step 4: Run the tests + fmt + clippy.**

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/add_project.rs
git commit -m "feat(projects): create a new project and browse GitHub repos"
```

---

### Task 5: commands, registration, reference regen

**Files:** `src-tauri/src/commands/projects.rs`, `src-tauri/src/lib.rs`, `docs/control-api-reference.md`

- [ ] **Step 1: Add the wrappers**

In `src-tauri/src/commands/projects.rs`, mirroring the existing wrappers there:

```rust
/// Add a project fleet does not know yet: clone a GitHub repo, adopt an
/// existing checkout, or create a new one. Returns the new project row.
#[tauri::command]
pub async fn add_project(
    args: AddProjectArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ProjectTreeRow, IpcError> {
    add_project::add_project(args, &store, &ssh).await
}

/// The repositories `gh` can see on `host_alias`, for the Add-project
/// dialog's browse mode. Read-only.
#[tauri::command]
pub async fn list_github_repos(
    args: ListGithubReposArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<GithubRepo>, IpcError> {
    add_project::list_github_repos(args, &store, &ssh).await
}
```

Register both in `lib.rs` next to `commands::projects::refresh_projects`. Wire `call_id` cancellation for `add_project` the way `new_session` does (find it: `grep -rn "call_id" src-tauri/src/commands src-tauri/src/cancel.rs`).

**Cancellation trap — read before touching the clone call.** Registering the
`call_id` at the command layer is NOT enough: the `CancellationToken` has to
reach `add_project_with`, which must race it (`SshExec::run_cancellable` for
the remote branch, a `tokio::select!` in `run_local_script` for the local
one). Otherwise the dialog ships a Cancel button that does nothing.

And note the regression it would cause: Task 2 deliberately moved the clone
onto `SshExec::run_bounded` so the connect timeout (20s) and the wall clock
(600s) are independent. `run_cancellable` takes ONE timeout and derives its
bound as `default_wall_clock(timeout)`, i.e. 3x — swapping to it naively puts
back the 600s-connect-timeout bug Task 2 fixed. Add a `run_bounded_cancellable`
to the `SshExec` trait (or an optional token on `run_bounded`), implemented
for `SshClient`, `Arc<T>`, `LocalExec` and `FakeSsh` exactly as Task 2 did for
`run_bounded`, and use that.

- [ ] **Step 2: Regenerate and verify**

Run: `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current` then the same command without `REGEN_DOCS` — must pass.
Run: `cd src-tauri && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/commands/projects.rs src-tauri/src/lib.rs docs/control-api-reference.md
git commit -m "feat(commands): add_project and list_github_repos"
```

---

### Task 6: `tauri-plugin-dialog`

**Files:** `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json`, `package.json`

- [ ] **Step 1: Add the dependency**

`src-tauri/Cargo.toml`: `tauri-plugin-dialog = "2"` next to `tauri-plugin-opener`.
`src-tauri/src/lib.rs`: `.plugin(tauri_plugin_dialog::init())` next to the other `.plugin(...)` calls.
`src-tauri/capabilities/default.json`: add `"dialog:allow-open"` to `permissions`.
`package.json`: `"@tauri-apps/plugin-dialog": "^2"` in `dependencies`, then `pnpm install`.

- [ ] **Step 2: Verify**

Run: `cd src-tauri && cargo build --lib` — clean. Run `pnpm run build` — succeeds. Run `cargo deny --manifest-path src-tauri/Cargo.toml check` — the new crate must not introduce an advisory or a disallowed licence; if it does, stop and report.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lib.rs src-tauri/capabilities/default.json package.json pnpm-lock.yaml
git commit -m "chore(deps): tauri-plugin-dialog for the folder picker"
```

---

### Task 7: the TypeScript parser and the store wrappers

**Files:** Create `src/lib/repo_url.ts` + `src/lib/repo_url.test.ts`; modify `src/lib/projects.ts`

- [ ] **Step 1: Write the failing test**

`src/lib/repo_url.test.ts` mirrors Task 1's Rust cases exactly — the same accepted forms, the same rejected ones — against `parseRepoUrl`.

- [ ] **Step 2: Run to verify it fails.** `npx vitest run src/lib/repo_url.test.ts`

- [ ] **Step 3: Implement** `parseRepoUrl(input: string): { owner: string; repo: string } | null` as a direct port of the Rust function (same prefixes, same `.git`/trailing-slash trimming, same component rule), with a comment naming `src-tauri/src/repo_url.rs` as the twin that must stay in sync.

In `src/lib/projects.ts` add the wire types and wrappers:

```ts
export type AddProjectSource =
  | { kind: 'clone'; url: string }
  | { kind: 'folder'; path: string }
  | { kind: 'new'; owner: string; repo: string; create_remote: boolean; confirm?: string };

export interface GithubRepo {
  name_with_owner: string;
  description: string | null;
  is_private: boolean;
  updated_at: string | null;
}

export async function addProject(
  hostAlias: string,
  source: AddProjectSource,
  signal?: AbortSignal,
): Promise<Result<ProjectTreeRow>> {
  const r = await invokeCmdAbortable<ProjectTreeRow>(
    'add_project',
    { args: { host_alias: hostAlias, source } },
    signal,
  );
  if (r.ok) mergeProject(r.value);
  return r;
}

export async function listGithubRepos(hostAlias: string): Promise<Result<GithubRepo[]>> {
  return invokeCmd<GithubRepo[]>('list_github_repos', { args: { host_alias: hostAlias } });
}
```

`mergeProject` should reuse whatever this file already uses to patch a project row into the store (`mergeProjectRow` or similar — check the existing event handlers) so the sidebar shows the new project without a refetch.

- [ ] **Step 4: Run tests + svelte-check.**

- [ ] **Step 5: Commit**

```bash
git add src/lib/repo_url.ts src/lib/repo_url.test.ts src/lib/projects.ts
git commit -m "feat(projects): addProject and listGithubRepos wrappers"
```

---

### Task 8: `AddProjectDialog.svelte`

**Files:** Create `src/lib/AddProjectDialog.svelte` + `src/lib/AddProjectDialog.test.ts`

Build the dialog described in the spec's "`AddProjectDialog.svelte`" section. Follow `NewSessionDialog.svelte` for every shared pattern: the `Modal` wrapper, the host chips and their disabled rule, the `last-host` pref, the path preview from `fleet_settings`, `busy` + `AbortController` around the create call, inline `.err` text, and Enter-to-submit.

Structure it so the four modes are data, not four copies of the form: a `mode` state (`'clone' | 'github' | 'folder' | 'new'`), a segmented control, and one `{#if}` per mode's fields. Keep the file focused — if it passes ~400 lines, extract the GitHub browser into its own component rather than growing it.

- [ ] **Step 1: Write the failing tests** in `src/lib/AddProjectDialog.test.ts`, mocking `@tauri-apps/api/core`'s `invoke` the way `NewSessionDialog.test.ts` does, and mocking `@tauri-apps/plugin-dialog`'s `open` for the folder mode:

- clone mode: typing `https://github.com/o/r` enables Create and sends `{kind:'clone', url:'https://github.com/o/r'}` with the chosen `host_alias`;
- clone mode: `not a repo` disables Create and shows the parse error;
- github mode: a failing `list_github_repos` shows the stderr text, not an empty list;
- github mode: picking a row switches to clone mode prefilled with that repo;
- folder mode: non-local host chips are disabled;
- folder mode: the picker's chosen path is sent as `{kind:'folder', path}`;
- new mode: Create sends `create_remote: false` by default;
- new mode: ticking "create on GitHub" requires the confirmation dialog before Create sends `confirm`;
- a failed create shows the backend message and leaves the dialog open;
- Cancel during a create aborts (the `cancel_command` IPC fires).

- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement the dialog.**
- [ ] **Step 4:** `npx vitest run src/lib/AddProjectDialog.test.ts`, then the whole suite, `npx svelte-check --tsconfig ./tsconfig.json`, `pnpm run build`.
- [ ] **Step 5: Commit**

```bash
git add src/lib/AddProjectDialog.svelte src/lib/AddProjectDialog.test.ts
git commit -m "feat(projects): the Add project dialog"
```

---

### Task 9: the sidebar entry point

**Files:** `src/lib/Sidebar.svelte`, `src/lib/Sidebar.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
  it('the project picker offers Add project, which opens the dialog', async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    await fireEvent.click(screen.getByTestId('add-project-row'));
    await tick();
    expect(screen.getByTestId('add-project-dialog')).toBeInTheDocument();
  });
```

Plus: after `addProject` resolves, the dialog closes and `NewSessionDialog` opens on the returned project (assert by its testid and the project name in its heading).

- [ ] **Step 2: Run to verify it fails.**

- [ ] **Step 3: Implement.** In `Sidebar.svelte` add `let showAddProject = $state(false);`, render `＋ Add project…` as the first row of the `{#if showProjectPicker}` block (`data-testid="add-project-row"`) so it is reachable even when `allProjectsSorted` is empty, mount `<AddProjectDialog>` when `showAddProject`, and on its success callback set `dialogProject` to the returned row so the existing `NewSessionDialog` opens on it.

- [ ] **Step 4:** `npx vitest run src/lib/Sidebar.test.ts`, whole suite, svelte-check, build.

- [ ] **Step 5: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "feat(sidebar): reach Add project from the project picker"
```

---

### Task 10: regression test, CI mirror, version, build, push

- [ ] **Step 1: The remote-only regression test**

In `src-tauri/src/service/projects.rs`'s tests: a project row whose `base_path` does not exist on disk survives `refresh_projects` (it is not a duplicate, so the delete arm does not reach it). This is the invariant the whole feature rests on.

Run: `cd src-tauri && cargo test refresh_projects`

- [ ] **Step 2: Full CI mirror**

```bash
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
pnpm install --frozen-lockfile && npx svelte-check --tsconfig ./tsconfig.json && npx vitest run && pnpm run build
cargo deny --manifest-path src-tauri/Cargo.toml check
```

- [ ] **Step 3: Bump, build, install**

Bump `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` to the next patch; commit as `chore(release): bump version to X.Y.Z`; build with `CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet pnpm tauri build --bundles app`; install to `/Applications` and verify the version.

- [ ] **Step 4: Push**

`git fetch origin`, merge or rebase onto `origin/main` (main moves during long tasks — re-run the CI mirror after integrating), then `git push origin HEAD:main`. `gh pr create` does not work from this machine.

---

## Self-review

- Spec coverage: entry point (T9), clone (T2), github browse (T4), folder (T3), new + confirm-gated remote creation (T4), commands + reference (T5), plugin (T6), TS parser + wrappers (T7), dialog (T8), the base_path invariant (T10). Out-of-scope items untouched.
- Types: `AddProjectSource`'s Rust `#[serde(tag = "kind", rename_all = "snake_case")]` matches the TS union's `kind` values and snake_case fields (`create_remote`); `GithubRepo`'s snake_case fields match the TS interface; `add_project` returns `ProjectTreeRow` in both.
- Placeholders: none. Tasks 2-4 name the helpers to write rather than inlining every line, and each says what to grep for when the codebase's shape must decide.
