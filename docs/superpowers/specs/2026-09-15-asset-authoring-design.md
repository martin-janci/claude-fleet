# Asset catalog — sub-project 3: authoring

## Summary

Sub-projects 1 and 2 (`2026-09-14-asset-catalog-design.md`, PR #93;
`2026-09-14-asset-sync-design.md`, PR #96) made the catalog visible and
synced it to hosts. This sub-project makes the catalog editable from the app:
create assets from templates, edit them in a form with a text editor for the
body, manage a skill's resource files, lint before saving, auto-commit every
save, push explicitly, delete, and hand an asset to a Claude session that
works in the catalog repo. No new MCP tools: sessions already edit the repo
directly, and the sync engine is unchanged.

## Decisions (resolved during brainstorming)

- Commit model: every save auto-commits with a generated message
  (`catalog: create|update|delete <kind>/<name>`). Push is a separate explicit
  button. Imports (which leave a dirty tree) are committed by a "Commit
  pending" button.
- Editor: a form for header and kind-specific fields plus a plain text editor
  for `body.md` / `prompt.md`; `asset.yaml` is never edited raw in v1.
- Resources (skill `resources/…`): listed with sizes; remove with
  confirmation; add by picking local files. No in-app editing of resource
  contents.
- Delegate: an interactive fleet session with cwd = the catalog repo, seeded
  with a prompt naming the asset and the IR rules; the app selects it. A
  background mode is out of scope.
- Lint: static only; errors block save, warnings do not.
- No new MCP tools; the tool surface stays at 63.

## Writes to the catalog repo

All writes go through `service/catalog/author.rs` and end with
`catalog::load(false)` so the in-memory catalog and the UI refresh through
the existing `catalog:loaded` event.

| Operation | Effect |
|---|---|
| `create(kind, name, template_from?)` | `write_asset` of a template (or a copy of an existing asset with a new name), lint, commit `catalog: create <kind>/<name>` |
| `update(asset)` | lint (errors refuse), `write_asset(overwrite)` pruning resources no longer listed, commit `catalog: update <kind>/<name>` |
| `delete(kind, name)` | remove the folder or file, `git rm`, commit `catalog: delete <kind>/<name>` |
| `add_resource(kind, name, local_path)` / `remove_resource(kind, name, rel_path)` | copy in / delete, commit `catalog: update <kind>/<name> resources` |
| `commit_pending(message?)` | `git add -A && git commit` when the tree is dirty (imports) |
| `push()` | `git push`; `E_CATALOG_GIT` with stderr on failure; requires an upstream |
| `status()` | dirty file count, ahead/behind counts versus the upstream (0 when none), HEAD |

Git identity: if `git config user.email` is unset in the repo, commits use
`-c user.name=claude-fleet -c user.email=fleet@localhost` so a fresh clone can
commit without global configuration. Path arguments never come from the
frontend unvalidated: asset names pass `is_valid_name`, resource paths are
relative, `[A-Za-z0-9._/-]`, no `..`, under `resources/`.

`repo.rs` gains the path-addressed git helpers (`stage`, `commit`, `push`,
`status`, `remove_asset`) and `write_asset` learns to prune stale resources on
overwrite. Deleted assets that were synced become `orphan` on hosts and are
removed by the next sync with backups, as sub-project 2 specified.

## Templates

`author::template(kind, name) -> Asset`, one per kind:

- skill: description "Describe when to use this skill.", body
  `# <name>\n\n## When to use\n\n## Steps\n`, `user_invocable: true`.
- agent: description "Describe what this agent does.", tools `[read, grep, glob]`,
  model `default`, prompt `You are …\n`.
- hook: event `stop`, action `{ type: command, command: "echo hook" }`.
- mcp_server: transport `http`, url `http://127.0.0.1:${FLEET_MCP_PORT}/mcp`.
- plugin_ref: harness `claude`, marketplace `{ name, source: github, repo }`
  left as `TODO` strings the lint flags, version `latest`.

"Duplicate from" copies an existing asset (header, spec, body, resources)
under the new name with `source` cleared.

## Lint

`author::lint(asset, catalog, secrets_example: &[String]) -> LintReport`
where `LintReport { errors: Vec<Finding>, warnings: Vec<Finding> }` and
`Finding { field: String, message: String }`.

Errors: everything `Asset::validate` reports; a folder-kind asset with an
empty body; a `TODO` placeholder left in any string field; a hook `command`
containing a newline.

Warnings: description shorter than 20 or longer than 1024 characters;
`${NAME}` placeholders not listed in `secrets.example.yaml` (or the file
missing while placeholders exist); an http hook or MCP url that is neither
`https://` nor a loopback address; an agent with an empty tool list; a
`targets.<harness>` key that is not a known harness; a skill body without a
heading.

`author::lint_all(catalog, secrets_example)` returns one report per asset
plus the catalog `problems`; the Lint-all button shows error and warning
counts and lists them.

## Delegating to a session

`author::spawn_author_session(args, store, ssh)`:

1. Ensure the catalog repo is a fleet project: look up `projects` by
   `base_path == repo_path`; if absent, adopt it through
   `add_project` with `AddProjectSource::Folder { path }`.
2. `new_session` on `local` in that project with name
   `catalog-<kind>-<name>` (or `catalog-new-<slug>`), kind `work`.
3. Wait for the REPL, then send the seeded prompt (soft-fail like
   `spawn_review`: the session survives a failed seed).
4. Return the session row; the UI selects it.

The seeded prompt is built by a pure function and states: the repo path and
layout, the asset's path, the IR field rules in one paragraph (kebab-case
names, required description, neutral tool/tier/event vocabularies,
`${NAME}` placeholders, `body.md`/`prompt.md`), the instruction text from the
dialog, and that the session must commit its changes with a
`catalog: …` message. When the Assets tab regains focus after such a session
was opened, the panel reloads the catalog.

## Commands

`catalog_create_asset`, `catalog_update_asset`, `catalog_delete_asset`,
`catalog_add_resource`, `catalog_remove_resource`, `catalog_lint_asset`,
`catalog_lint_all`, `catalog_commit_pending`, `catalog_push`,
`catalog_repo_status`, `catalog_template`, `catalog_spawn_author_session`.
`docs/control-api-reference.md` is regenerated for the command list. No MCP
tool changes.

Errors: `E_LINT` (save refused; details carry the report), `E_ASSET_EXISTS`,
`E_ASSET_NOT_FOUND`, `E_CATALOG_GIT`, `E_INVALID` for bad names or paths.

## UI

- Detail pane header gains **Edit**, **Delete** (confirm dialog, danger),
  **Lint**, and **Open in session**.
- **Edit mode** replaces the preview with `AssetEditor.svelte`: form
  controls per kind (text inputs, tag chips, selects for tier/event/transport,
  tool checklists from the neutral vocabulary), a textarea for the body, a
  resources list (size, remove, "Add file…" via the dialog plugin), a live
  lint panel (errors red, warnings amber), **Save** (disabled while errors
  exist or nothing changed) and **Cancel**. Save shows the commit hash and
  returns to the detail view.
- Toolbar gains **New asset** (dialog: kind, name, optional duplicate-from;
  creates and opens the editor), **Commit pending** (visible when the repo is
  dirty; message prefilled), **Push** (ahead count badge; disabled when no
  upstream), **Lint all** (report dialog).
- **Open in session** opens a small dialog with an instruction textarea
  (prefilled "Improve this <kind>: …"), spawns the session, and selects it.
- Repo status (dirty, ahead, behind, head) shows in the toolbar and refreshes
  after every write.

## Testing

- `repo.rs`: git helpers against temp repos (commit with and without
  identity, status counts with and without upstream, remove_asset, prune on
  overwrite).
- `author.rs`: templates validate clean except the plugin TODO; lint table
  (each error and warning rule); create/update/delete/resources end-to-end on
  a temp repo asserting files, commits and the reloaded `CATALOG`;
  `commit_pending` and `push` to a bare temp remote; prompt builder golden.
- `spawn_author_session`: project adoption idempotence and name/prompt
  construction tested with the store; the tmux/REPL part reuses the
  `spawn_review` fake-SSH tests where possible.
- Frontend: editor form validation, save disabled on errors, resources
  add/remove wrappers, new-asset dialog, delete confirmation, lint-all dialog,
  push/commit-pending buttons, open-in-session dialog, tab-focus reload.

## Out of scope

- Raw YAML editing; editing resource contents; renaming assets (delete +
  create).
- Background authoring sessions; MCP authoring tools.
- Branching or pull requests for the catalog repo; conflict resolution beyond
  `git pull --ff-only`.
- Project-scoped assets; per-asset targeting.
