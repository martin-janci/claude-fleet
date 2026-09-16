# Asset Authoring (sub-project 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the catalog editable from the app: create from templates, edit in a form with a body text editor, manage skill resources, lint before saving, auto-commit every save, push explicitly, delete, and hand an asset to an interactive Claude session in the catalog repo.

**Architecture:** `catalog/repo.rs` gains path-addressed git helpers (`stage`, `commit`, `push`, `status`, `remove_asset`) and resource pruning on overwrite. A new `catalog/author.rs` holds templates, lint, and the create/update/delete/resources/commit-pending/push operations, each ending with `catalog::load(false)`. Session delegation lives in `catalog/author_session.rs`, mirroring `spawn_review` with cwd = the repo, after adopting the repo as a fleet project. Twelve thin Tauri commands; no MCP changes. Frontend: `AssetEditor.svelte`, `NewAssetDialog.svelte`, `AuthorSessionDialog.svelte`, `LintAllDialog.svelte`, wiring in `AssetDetail`/`AssetsPanel`.

**Tech Stack:** Rust (tauri 2, serde, serde_yaml, rusqlite), Svelte 5 + TypeScript (+ `@tauri-apps/plugin-dialog` for the file picker), Vitest.

**Spec:** `docs/superpowers/specs/2026-09-15-asset-authoring-design.md` (binding). Earlier specs: `2026-09-14-asset-catalog-design.md`, `2026-09-14-asset-sync-design.md`.

## Global Constraints

- Service functions take `&Mutex<Store>` / `&Arc<SshClient>`; never hold the `Store` mutex or `CATALOG` lock across an `.await`; no Tauri types in `service/`.
- Every write to the catalog repo ends with `catalog::load(false)` (so `CATALOG` and the UI refresh via `catalog:loaded`). Every save auto-commits `catalog: create|update|delete <kind>/<name>`; resource changes commit `catalog: update <kind>/<name> resources`.
- Git identity fallback: when `git config user.email` is unset in the repo, commits pass `-c user.name=claude-fleet -c user.email=fleet@localhost`. Push requires an upstream; failures are `E_CATALOG_GIT` with stderr in details.
- Input validation at the command boundary: asset names via `model::is_valid_name`; resource relative paths match `[A-Za-z0-9._/-]+`, contain no `..` or empty segment, and start with `resources/`; local file paths for `add_resource` must be absolute and exist.
- Lint: errors block save (`E_LINT`, details = the report); warnings never block. Templates must lint clean except the plugin_ref `TODO` fields.
- No `eprintln!`; codes in `ipc_error::codes` (`E_LINT` new). No new MCP tools; `docs/control-api-reference.md` regenerated for the command list (`REGEN_DOCS=1 cargo test --manifest-path Cargo.toml reference_is_current`).
- Build/verify on this host after `cd src-tauri && source /home/dev/.local/tauri-sysroot/env.sh && export RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu`; run cargo in the foreground. Base: 1436 Rust tests, 1219 frontend tests. Frontend: `CI=true pnpm install --frozen-lockfile` once, then `pnpm run check && pnpm run test && pnpm run build`.
- Commits: Conventional Commits ending with `Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS`. Never touch version fields or CHANGELOG.md.

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/service/catalog/repo.rs` | + `git_status`, `stage_paths`, `commit`, `push`, `remove_asset`, `has_identity`; `write_asset` prunes stale resources on overwrite |
| `src-tauri/src/service/catalog/author.rs` | templates, lint, `create`/`update`/`delete_asset`/`add_resource`/`remove_resource`/`commit_pending`/`push`/`repo_status` |
| `src-tauri/src/service/catalog/author_session.rs` | `build_author_prompt`, `ensure_catalog_project`, `spawn_author_session` |
| `src-tauri/src/service/catalog/mod.rs` | `pub mod author; pub mod author_session;` |
| `src-tauri/src/ipc_error.rs` | `codes::E_LINT` |
| `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs` | twelve commands + registration |
| `src/lib/assets.ts` | types + wrappers |
| `src/lib/AssetEditor.svelte`, `NewAssetDialog.svelte`, `AuthorSessionDialog.svelte`, `LintAllDialog.svelte` | new UI |
| `src/lib/AssetDetail.svelte`, `src/lib/AssetsPanel.svelte`, `src/App.svelte` | wiring, tab-focus reload |
| `docs/concepts.md`, `docs/control-api.md` | docs |

---

### Task 1: Path-addressed git helpers and resource pruning in `repo.rs`

**Files:** Modify `src-tauri/src/service/catalog/repo.rs`.

**Interfaces (Produces):**
```rust
#[derive(Debug, Clone, Serialize)]
pub struct RepoStatus { pub head: String, pub dirty: usize, pub ahead: Option<u64>, pub behind: Option<u64>, pub has_upstream: bool }
pub fn git_status(root: &Path) -> Result<RepoStatus, IpcError>   // porcelain count; rev-list --left-right --count @{u}...HEAD when upstream exists
pub fn has_identity(root: &Path) -> bool                          // git config user.email succeeds
pub fn stage_paths(root: &Path, rel_paths: &[String]) -> Result<(), IpcError>  // git add -A -- <paths>; empty list = git add -A
pub fn commit(root: &Path, message: &str) -> Result<String, IpcError>          // returns the new HEAD; identity fallback; E_CATALOG_GIT("nothing to commit") when clean
pub fn push(root: &Path) -> Result<(), IpcError>                                // requires upstream
pub fn remove_asset(root: &Path, kind: Kind, name: &str) -> Result<Vec<String>, IpcError> // deletes folder/file, returns removed rel paths; E_ASSET_NOT_FOUND
pub fn asset_rel_dir(kind: Kind, name: &str) -> String                         // "skills/<name>" or "hooks/<name>.yaml"
```
`write_asset(root, asset, overwrite=true)` deletes files under `<dir>/resources/` that are not in `asset.resources`, and removes empty directories it leaves behind.

- [ ] Tests first (temp repos under `std::env::temp_dir()` with `git init -q -b main` and local `user.email` only where the test says so): `commit_uses_fallback_identity_when_unset` (repo with no identity commits; author email `fleet@localhost`); `commit_keeps_configured_identity`; `git_status_counts_dirty_and_ahead` (no upstream → `has_upstream=false`, ahead None; with a bare remote + push -u → ahead 1 after a commit); `remove_asset_deletes_folder_and_file_kinds`; `write_asset_overwrite_prunes_stale_resources` (write with two resources, rewrite with one → the other file gone, empty dirs removed, `load_dir` reflects it); `stage_paths_then_commit_returns_head`.
- [ ] Implement; `cargo test service::catalog::repo`; fmt; clippy; full test; commit `feat(catalog): path-addressed git helpers, remove_asset, resource pruning`.

---

### Task 2: Templates, lint, and the authoring operations

**Files:** Create `src-tauri/src/service/catalog/author.rs`; modify `catalog/mod.rs` (`pub mod author;`), `src-tauri/src/ipc_error.rs` (`E_LINT`).

**Interfaces (Produces):**
```rust
#[derive(Debug, Clone, Serialize)] pub struct Finding { pub field: String, pub message: String }
#[derive(Debug, Clone, Default, Serialize)] pub struct LintReport { pub errors: Vec<Finding>, pub warnings: Vec<Finding> }
#[derive(Debug, Clone, Serialize)] pub struct AssetLint { pub kind: String, pub name: String, pub report: LintReport }
#[derive(Debug, Clone, Serialize)] pub struct LintAll { pub assets: Vec<AssetLint>, pub problems: Vec<Problem>, pub errors: usize, pub warnings: usize }
pub fn template(kind: Kind, name: &str) -> Asset
pub fn secrets_example_names(root: &Path) -> Vec<String>        // parses secrets.example.yaml: a list or a map's keys; empty when missing
pub fn lint(asset: &Asset, catalog: &Catalog, secrets_example: &[String], secrets_example_exists: bool) -> LintReport
pub fn lint_all(catalog: &Catalog, root: &Path) -> LintAll
#[derive(Deserialize)] pub struct CreateArgs { pub kind: Kind, pub name: String, pub duplicate_from: Option<String> }
#[derive(Deserialize)] pub struct UpdateArgs { pub asset: Asset }        // Asset deserializes from the UI (manual impl includes body + resources)
#[derive(Deserialize)] pub struct AssetRef { pub kind: Kind, pub name: String }
#[derive(Deserialize)] pub struct AddResourceArgs { pub kind: Kind, pub name: String, pub local_path: String, pub rel_path: Option<String> } // default rel = "resources/<file name>"
#[derive(Deserialize)] pub struct RemoveResourceArgs { pub kind: Kind, pub name: String, pub rel_path: String }
#[derive(Deserialize)] pub struct CommitPendingArgs { pub message: Option<String> }
#[derive(Serialize)] pub struct WriteResult { pub commit: String, pub lint: LintReport }
pub fn create(args: CreateArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError>
pub fn update(args: UpdateArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError>
pub fn delete_asset(args: AssetRef, store: &Mutex<Store>) -> Result<String /*commit*/, IpcError>
pub fn add_resource(args: AddResourceArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError>
pub fn remove_resource(args: RemoveResourceArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError>
pub fn commit_pending(args: CommitPendingArgs, store: &Mutex<Store>) -> Result<String, IpcError>   // E_CATALOG_GIT when clean
pub fn push(store: &Mutex<Store>) -> Result<RepoStatus, IpcError>
pub fn repo_status(store: &Mutex<Store>) -> Result<RepoStatus, IpcError>
pub fn lint_asset(args: AssetRef, store: &Mutex<Store>) -> Result<LintReport, IpcError>
pub fn lint_everything(store: &Mutex<Store>) -> Result<LintAll, IpcError>
```
Rules: the lint error/warning list is exactly the spec's. `create` with `duplicate_from` copies header/spec/body/resources from the existing asset of the same kind, renames, clears `source`; templates per the spec. `update` validates the name, lints (errors → `E_LINT` with `details: report`), writes with overwrite (pruning), stages the asset dir/file, commits, reloads. `delete_asset` uses `repo::remove_asset`, stages, commits `catalog: delete …`, reloads. Resource ops read the asset from `CATALOG`, mutate `resources`, write, commit `… resources`, reload. Every op resolves the repo root from `catalog::config` (`E_CATALOG_NOT_CONFIGURED` when absent).

- [ ] Tests first: templates validate (all but plugin_ref clean; plugin_ref has exactly the TODO errors); lint table (one test per error rule and per warning rule, plus a clean asset yields empty report); `create_update_delete_round_trip` on a temp repo with a configured store (files exist, `git log` shows the three `catalog:` commits in order, `CATALOG` reloaded each time — guard with `CATALOG_TEST_LOCK`); `update_refuses_on_lint_errors_and_writes_nothing`; `resources_add_and_remove_commit_and_prune`; `commit_pending_commits_a_dirty_tree_and_errors_when_clean`; `push_to_a_bare_remote_updates_ahead_count`; `duplicate_from_copies_body_and_resources`.
- [ ] Implement; verify; commit `feat(catalog): templates, lint, and authoring operations with auto-commit`.

---

### Task 3: Delegate to a session

**Files:** Create `src-tauri/src/service/catalog/author_session.rs`; modify `catalog/mod.rs`.

**Interfaces (Produces):**
```rust
#[derive(Deserialize)] pub struct SpawnAuthorArgs { pub kind: Option<Kind>, pub name: Option<String>, pub instructions: String, pub call_id: Option<u64> }
pub fn build_author_prompt(repo_path: &str, target: Option<(Kind, &str)>, instructions: &str) -> String
pub async fn ensure_catalog_project(store: &Mutex<Store>, ssh: &Arc<SshClient>, reg: &Arc<CancellationRegistry>, repo_path: &str) -> Result<i64 /*project id*/, IpcError>
pub async fn spawn_author_session(args: SpawnAuthorArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>, reg: &Arc<CancellationRegistry>) -> Result<SessionRow, IpcError>
```
`ensure_catalog_project`: find a `ProjectRow` whose `base_path` equals the repo path (canonicalised both sides); else `add_project(AddProjectArgs { host_alias: "local", source: Folder { path }, call_id: None })` and use the returned id. `spawn_author_session`: `new_session(NewSessionArgs { host_alias: "local", project_id, worktree_id: None, name: "catalog-<kind>-<name>" or "catalog-new-<hex>", kind: Some("work"), friendly_name: Some("author <kind>/<name>"), call_id, .. })`, then `wait_for_repl_ready(ssh, "local", &row.tmux_name)` and `send_prompt(SendPromptArgs { host_alias, tmux_name, prompt, submit: true })` soft-failing with `tracing::warn!` (mirror `spawn_review`), return the row. The prompt (pure builder, golden-tested) contains: repo path and layout, the target asset path (`skills/<name>/asset.yaml` + `body.md` …) or "create a new asset", the IR rules paragraph (kebab-case names; description required; neutral tools `read edit write bash grep glob web_search web_fetch browser agent mcp:<server> *`; tiers `fast default strong`; events `session_start prompt_submit before_tool after_tool stop subagent_stop`; `${NAME}` placeholders; body.md / prompt.md), the instructions, and "commit with a `catalog: …` message when done".

- [ ] Tests first: prompt golden (contains the path, the vocabulary, the instructions, the commit rule); `ensure_catalog_project_is_idempotent` with a store + temp repo (first call adopts, second returns the same id; no SSH needed for the local Folder source — check `add_project`'s folder path is store-only); a `spawn_author_session` test is limited to argument construction (session name/friendly name) via a small pure helper `session_name_for(kind, name)`.
- [ ] Implement; verify; commit `feat(catalog): open an authoring session in the catalog repo`.

---

### Task 4: Commands and reference

**Files:** Modify `src-tauri/src/commands/assets.rs`, `src-tauri/src/lib.rs`; regenerate `docs/control-api-reference.md`.

Twelve thin commands: `catalog_create_asset(CreateArgs)`, `catalog_update_asset(UpdateArgs)`, `catalog_delete_asset(AssetRef)`, `catalog_add_resource(AddResourceArgs)`, `catalog_remove_resource(RemoveResourceArgs)`, `catalog_lint_asset(AssetRef)`, `catalog_lint_all()`, `catalog_commit_pending(CommitPendingArgs)`, `catalog_push()`, `catalog_repo_status()`, `catalog_template(AssetRef) -> Asset`, `catalog_spawn_author_session(SpawnAuthorArgs)` (async, needs `reg`). Validate names (`is_valid_name`), resource paths (rule in Global Constraints), and `local_path` (absolute, exists) at the boundary with `E_INVALID`.

- [ ] Implement; `REGEN_DOCS=1 cargo test reference_is_current`; fmt/clippy/full test; commit `feat(catalog): authoring commands`.

---

### Task 5: Frontend

**Files:** Modify `src/lib/assets.ts` (+ test), `src/lib/AssetDetail.svelte`, `src/lib/AssetsPanel.svelte` (+ test), `src/App.svelte`; create `src/lib/AssetEditor.svelte` (+ test), `src/lib/NewAssetDialog.svelte` (+ test), `src/lib/AuthorSessionDialog.svelte` (+ test), `src/lib/LintAllDialog.svelte`.

**assets.ts**: types `Finding`, `LintReport`, `AssetLint`, `LintAll`, `RepoStatus`, `WriteResult`, `EditableAsset` (the `Asset` wire shape: `kind, name, version, description, tags, source?, targets?, body, resources: { rel_path, bytes }[]` plus the kind fields flattened — mirror the Rust manual serializer exactly; read `catalog_get_asset` output to confirm); wrappers `createAsset`, `updateAsset`, `deleteAsset`, `addResource`, `removeResource`, `lintAsset`, `lintAll`, `commitPending`, `pushCatalog`, `repoStatus`, `assetTemplate`, `spawnAuthorSession` (abortable); store `repoStatusStore`; `KIND_FIELDS` describing per-kind form fields; constants `TOOLS`, `TIERS`, `EVENTS` mirroring Rust.

**AssetEditor** (`props: { asset: EditableAsset; onsaved(result); oncancel() }`): header inputs (`editor-name` read-only for existing assets, `editor-description`, `editor-version`, `editor-tags`), kind fields (`editor-field-<field>`; tool checklists, selects), body textarea (`editor-body`), resources list (`editor-resource-<rel>` rows with size and `editor-resource-remove-<rel>`; `editor-resource-add` uses `open({ multiple: false })` from `@tauri-apps/plugin-dialog`), live lint via a debounced `lintAsset`-style local call (`catalog_lint_asset` works on the stored asset; for unsaved edits run `validate`-equivalent checks client-side: required description, name pattern, and show server lint after save — keep it simple: the panel shows the last server report plus client-side required-field errors), **Save** (`editor-save`, disabled when client errors or unchanged), **Cancel** (`editor-cancel`). On save → `updateAsset`; on `E_LINT` show the report's errors inline.

**NewAssetDialog**: kind select, name input (kebab validation), duplicate-from select (assets of that kind), **Create** (`new-asset-create`) → `createAsset` → `onsaved(kind, name)` which selects the asset and opens the editor.

**AuthorSessionDialog**: instructions textarea prefilled with `Improve this <kind> "<name>": ` or `Create a new <kind> that …`, **Open session** (`author-open`, abortable) → `spawnAuthorSession` → `selectSession(row)`; sets a module flag `authorSessionOpened = true`.

**LintAllDialog**: counts, per-asset findings grouped, link to select an asset.

**AssetDetail**: buttons `asset-edit`, `asset-delete` (ConfirmDialog with `danger`), `asset-lint` (shows report inline, `asset-lint-report`), `asset-open-session`; edit mode swaps the preview for `AssetEditor`; after save re-fetch via a `reloadKey`.

**AssetsPanel**: toolbar `assets-new`, `assets-commit-pending` (visible when `repoStatus.dirty > 0`; prompts for a message with default `catalog: commit pending changes`), `assets-push` (badge `↑<ahead>`, disabled when `!has_upstream`), `assets-lint-all`; status strip `assets-repo-status`; `repoStatus()` refreshed after every write and on mount; when the panel becomes visible again (a `visible` prop from `App.svelte`, like `ConversationPanel`) and `authorSessionOpened` is set, call `reload(false)` and clear the flag.

- [ ] Tests first for each component and wrapper as listed; implement; `pnpm run check && pnpm run test && pnpm run build`; commit `feat(ui): asset editor, templates, lint, commit/push and authoring sessions in the Assets tab`.

---

### Task 6: Docs

- `docs/concepts.md` Asset catalog section: two sentences on authoring (edit/create/delete in the app, auto-commit, push, open in session).
- `docs/control-api.md`: note that authoring is desktop-only (no MCP tools) and that sessions may edit the repo directly with `catalog:` commits.
- Commit `docs: describe catalog authoring`.

---

## Plan self-review notes

- Spec coverage: git helpers + pruning (T1), templates/lint/ops (T2), delegate (T3), commands (T4), UI (T5), docs (T6). Errors `E_LINT`, `E_ASSET_EXISTS`, `E_ASSET_NOT_FOUND`, `E_CATALOG_GIT`, `E_INVALID` covered.
- Deliberate simplification: live lint in the editor is client-side required-field checks plus the server report after save; full server lint runs on the stored asset via the Lint button (the spec's "live lint panel" is satisfied for errors that block save; warnings appear after save).
- Type consistency: `RepoStatus` (T1) returned by T2's `repo_status`/`push`, typed in T5; `LintReport`/`WriteResult` (T2) used by T4/T5; `SpawnAuthorArgs` (T3) by T4/T5; `EditableAsset` mirrors the manual `Asset` serde impl from sub-project 1.
