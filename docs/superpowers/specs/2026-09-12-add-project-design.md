# Add a project that is not checked out yet

Date: 2026-09-12. Status: approved in conversation.

## Problem

A project exists in fleet only if it is already cloned under the local
projects root: `refresh_projects` scans the filesystem and `upsert_project`s
what it finds. There is no way to start work on a repo that is not on disk
yet, to adopt a checkout that lives outside the projects root, or to create a
new project. The user asked for all four entry points, reachable from the
New-session flow.

Two constraints from the existing code shape the design:

- `projects` is `(owner, repo UNIQUE, base_path NOT NULL, last_session_at)`.
  `base_path` is the LOCAL path; every remote path is derived from
  `owner`/`repo` plus that host's root (`Layout::project_dir`). So a project
  that exists only on a remote host still needs a `base_path` value: store
  the path it WOULD occupy locally.
- `refresh_projects` deletes a project row only when it is a duplicate whose
  `base_path` is a checkout another project now owns. A row that simply is
  not rediscovered survives. A remote-only project is therefore safe to
  register, and needs a regression test saying so.

## Entry point

`Sidebar.svelte`'s project picker (opened by the footer's "+ New session",
`showProjectPicker`) gains a pinned first row, `＋ Add project…`
(`data-testid="add-project-row"`), above `allProjectsSorted`. It opens
`AddProjectDialog.svelte`. On success the dialog closes, the new project's
row is merged into the `projects` store from the command's return value, and
`NewSessionDialog` opens on it — the user wanted a project in order to start
a session in it.

## `AddProjectDialog.svelte`

A `Modal` with a host picker (the same chips and rules as `NewSessionDialog`:
visible, reachable or `local`, remembered via the `last-host` pref) and four
modes selected by a segmented control (`data-testid="add-mode-<mode>"`).
Every mode previews the destination path before it acts, using the chosen
host's root (`PROJECTS_RESOLVED_KEY` from `fleet_settings`) and layout.

### 1. `clone` — clone by URL

One text field accepting `owner/repo`, `https://github.com/owner/repo(.git)`,
`http://…`, `git@github.com:owner/repo.git`, or `ssh://git@github.com/…`.
Parsed live by a pure `parseRepoUrl(input): { owner, repo } | null` in
`src/lib/repo_url.ts`; an unparseable value disables Create and shows why.
The clone URL sent to the backend is always normalised to
`git@github.com:<owner>/<repo>.git`, matching `ensure_remote_project`.

### 2. `github` — browse the account's repos

Runs `gh repo list --limit 200 --json nameWithOwner,description,isPrivate,updatedAt`
on the chosen host through a read-only `list_github_repos` command, rendered
in the existing `PickerList` with a client-side filter box. Picking a row
fills mode 1 and switches to it. `gh` missing, unauthenticated, or exiting
non-zero surfaces its stderr as an inline error with the remedy
(`gh auth login` on that host) — never an empty list pretending there are no
repos.

### 3. `folder` — adopt an existing checkout

`local` only (the chips for other hosts are disabled in this mode with a
title explaining why). A native folder picker via `tauri-plugin-dialog`
(new dependency: `tauri-plugin-dialog` in `Cargo.toml`, `@tauri-apps/plugin-dialog`
in `package.json`, and `dialog:allow-open` in `src-tauri/capabilities/default.json`).
The backend verifies the path is a git checkout (`git -C <path> rev-parse
--show-toplevel` equals the path's realpath, the same inode test the host
scan uses), reads `origin` for owner/repo, and falls back to
`(<current user or "local">, <basename>)` when there is no origin. The folder
is registered where it is; nothing is moved or copied.

### 4. `new` — create a new project

Owner and repo name, both validated with `crate::validate::path_component`.
Creates `<root>/<layout path>`, runs `git init -b main` and an empty initial
commit (`git commit --allow-empty -m "Initial commit"`) so the repo has a
branch that `git worktree add` can fork from. A separate, default-OFF
checkbox "also create the repository on GitHub" runs
`gh repo create <owner>/<repo> --private --source . --remote origin --push`.
Because that publishes to GitHub, the dialog requires a second explicit
confirmation (a `ConfirmDialog`) naming the exact repository before it runs,
and the command refuses the flag unless the frontend passes the confirmation
nonce, matching the existing confirm-gated MCP pattern.

## Backend

New `service/add_project.rs`:

```rust
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddProjectSource {
    Clone { url: String },
    Folder { path: String },
    New { owner: String, repo: String, create_remote: bool, confirm: Option<String> },
}

#[derive(Deserialize)]
pub struct AddProjectArgs {
    pub host_alias: String,
    pub source: AddProjectSource,
    pub call_id: Option<String>,
}

pub async fn add_project(
    args: AddProjectArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>,
) -> Result<ProjectTreeRow, IpcError>
```

and a testable `add_project_with(args, store, ssh: &dyn SshExec)` alongside,
so `FakeSsh` drives every remote path in tests, matching
`list_host_worktrees`.

Rules:

- The destination is `Layout::project_dir(project_base_for(store, host), owner, repo)`,
  expanded against the remote `$HOME` for a remote host
  (`repair::resolve_remote_paths`'s expansion, already `pub(crate)`).
- `local` runs git through `tokio::process`; a remote host runs one
  `bash -lc` script over SSH with every value through `crate::shell::quote`.
- A clone refuses when the destination already exists and is a git checkout
  (`E_EXISTS`, "already cloned at <path>; it should appear after a refresh"),
  and when the owner/repo pair already names a project row.
- `git clone` is long-running: it goes through `run_cancellable` with the
  frontend's `call_id`, so the dialog's Cancel aborts it exactly as
  `NewSessionDialog`'s does.
- `folder` is rejected for a non-`local` host with `E_INVALID`.
- Every success ends with `Store::upsert_project(owner, repo, base_path)`,
  which already emits `project:updated`, followed by reading the row back as
  a `ProjectTreeRow` (worktrees empty for a fresh clone until the next
  refresh). `base_path` is the LOCAL path the project would occupy, even when
  the clone landed on a remote host.
- Nothing is deleted or overwritten on any failure path: a clone that fails
  leaves no project row, and a partially-cloned directory is reported in the
  error rather than removed (matching `ensure_remote_project`'s documented
  behaviour on cancel).

`list_github_repos(host_alias, store, ssh) -> Vec<GithubRepo>` is a separate
read-only command; `GithubRepo { name_with_owner, description, is_private, updated_at }`.

Both commands are registered in `lib.rs`, which regenerates
`docs/control-api-reference.md` (`REGEN_DOCS=1`). Neither is an MCP tool.

## Tests

- Rust: `parse` of every accepted URL form and rejection of the rest (a pure
  helper mirrored in TS); the destination path for both layouts and both host
  kinds; the clone script's quoting for an owner/repo containing a quote and
  a space; `E_EXISTS` on a second clone; `folder` refused for a remote host;
  `folder` accepting a real temporary git repo and reading its origin;
  `folder` falling back to the basename with no origin; `new` creating a repo
  with an initial commit; `create_remote` refused without the confirmation;
  `list_github_repos` mapping `gh` JSON and surfacing a non-zero exit;
  a remote-only project row surviving `refresh_projects`.
- Frontend: `parseRepoUrl` unit tests; the picker's `Add project…` row opens
  the dialog; each mode's Create sends the right `source`; an unparseable URL
  disables Create; the folder mode disables non-local hosts; the GitHub mode
  shows an error instead of an empty list when `gh` fails; the create-remote
  checkbox requires the confirmation; success closes the dialog and opens
  `NewSessionDialog` on the new project.

## Out of scope

- Non-GitHub remotes (GitLab, self-hosted). The URL parser accepts only
  GitHub forms, and the clone URL is normalised to GitHub SSH.
- Removing or un-registering a project (the existing purge flow is separate).
- Cloning the same project onto several hosts at once; the user picks one
  host and repeats if they want more.
