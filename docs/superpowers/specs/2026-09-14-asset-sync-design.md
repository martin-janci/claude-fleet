# Asset catalog — sub-project 2: the sync engine

## Summary

Sub-project 1 (`2026-09-14-asset-catalog-design.md`, merged as PR #93) made the
catalog visible: it renders assets per harness, imports from the controller and
scans hosts for drift, but never writes to a host. This sub-project adds the
writes. A sync is planned first and applied only after confirmation. Every
write is guarded against concurrent edits, backed up when it replaces something
a person may have edited, and recorded in a per-harness managed manifest on the
host so fleet only ever removes what it installed. Secrets referenced as
`${NAME}` in the catalog are resolved from the fleet database at apply time
and never leave the controller except inside the written files.

## Decisions (resolved during brainstorming)

- Trigger: manual, plan first. Sync computes a plan, shows it, applies on
  confirmation. Same shape for the MCP tools.
- Drift on a managed asset: catalog wins; the host copy is backed up as
  `<path>.fleet-bak-<unix time>` first, and the plan marks the row `overwrite`.
- Orphans (in the manifest, no longer in the catalog): removed, with backup.
  Only assets fleet installed are ever removed; unmanaged files are never
  touched.
- Secrets: fleet SQLite, one global value per name with optional per-host
  override. Built-ins `FLEET_MCP_TOKEN` (that host's existing token) and
  `FLEET_MCP_PORT`.
- Plugins: installed and updated through the `claude plugin` CLI on the host,
  non-interactively. The CLI cannot pin a version; a pinned version that does
  not match after install is reported, never forced.
- Harnesses: Claude Code and Codex. Codex therefore gains a host scan and a
  TOML config merge in this sub-project.
- Mechanism: controller-computed writes, one script per host per batch, with a
  compare-and-swap on every file against the hash seen at scan time.

## Managed manifest

One file per harness that fleet owns: `~/.claude/.fleet-assets.json` and
`~/.codex/.fleet-assets.json`.

```json
{
  "version": 1,
  "updated_at": 1789400000,
  "assets": {
    "skill/worktree": {
      "hash": "<render hash>",
      "files": ["~/.claude/skills/worktree/SKILL.md", "…"],
      "merges": [{ "file": "~/.claude/settings.json", "json_path": ["hooks", "Stop"], "mode": "append_unique", "value_hash": "…" }],
      "synced_at": 1789400000
    }
  }
}
```

The manifest path is added to the harness's scanned config files, so it
arrives with the existing scan round trip. It is the source of managed-ness
for inventory and of the file and merge lists used to remove an orphan.

## Inventory changes

- `asset_inventory` gains `managed INTEGER NOT NULL DEFAULT 0` (migration 031)
  and one new state, `orphan`: the manifest names it, the catalog does not.
- The five existing states keep their meaning. A catalog asset that is present
  and matching but absent from the manifest is `in_sync` with `managed = 0`;
  the plan offers `adopt`.
- `unmanaged` rows are unchanged: on the host, not in the catalog, not in the
  manifest.

## The plan

Planning always begins with a fresh scan of the selected hosts. It then
produces one `HostPlan` per (host, harness) with a list of actions.

| Action | Condition |
|---|---|
| `create` | in catalog, not on host |
| `update` | managed, host hash equals manifest hash, catalog hash differs |
| `overwrite` | managed, host hash differs from manifest hash (edited on host); backup |
| `adopt` | present and matching, not in manifest; only the manifest changes |
| `remove` | orphan; backup, then delete files and config entries |
| `plugin_install` | plugin ref, not installed |
| `plugin_update` | plugin ref installed, ref is `latest`, and a newer version may exist; or pinned and installed version differs (reported as `blocked` if the CLI cannot reach the pin) |
| `noop` | managed and identical |
| `blocked` | secret name unresolvable for this host, kind unsupported on this harness, host unreachable, config file unparsable, `claude` CLI absent (plugin actions only) |

Each action carries the asset key, the file paths and config merges involved,
whether a backup will be taken, and the secret names it needs. Values of
secrets never appear in a plan.

The plan is kept in memory in a registry keyed by a random id with a ten
minute TTL. Apply takes the id. A missing or expired id is
`E_SYNC_PLAN_STALE`. Apply re-verifies every write with compare-and-swap; a
file whose hash changed between plan and apply becomes a `conflict` result for
that action and nothing is written to it.

## Secrets

Tables (migration 031):

```sql
CREATE TABLE IF NOT EXISTS catalog_secrets (name TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS catalog_secrets_host (host_alias TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (host_alias, name));
```

Resolution for a host: host override, then global, then built-in. Built-ins:
`FLEET_MCP_TOKEN` from `host_tokens` for that host (the master token is never
used for a host), `FLEET_MCP_PORT` from the `mcp.port` setting. Substitution
replaces `${NAME}` in every file body and merge value string of a render plan.
A plan action whose names cannot all be resolved is `blocked` with the missing
names listed. Secrets never appear in previews, plans, logs, audit lines, MCP
output or events. The UI shows names and which hosts override them.

## Applying a plan on a host

For each host, for each harness, in this order:

1. **Substitute** secrets into the render plans of every action. Files or
   merge values whose bytes changed are secret-bearing.
2. **Plain files** are written by bash scripts chunked at about 512 KB of
   base64. Each write is guarded: the script computes the current sha256 (or
   `absent`) and writes only if it equals the hash the plan recorded;
   otherwise it prints `CONFLICT <path>`. `overwrite` and `remove` first copy
   the existing file to `<path>.fleet-bak-<unix time>`. Scripts contain no
   single quotes and go through `shell::quote` as one `bash -lc` word, like
   the scan script.
3. **Secret-bearing files** go one at a time through the existing 0600 path
   (`write_host_file_secret`: umask 077, touch, chmod 600, stream the bytes),
   after the same compare-and-swap check, so values never appear in an
   argument list.
4. **Config merges** are applied on the controller: the harness parses the
   scanned config, applies every merge for that file (set, append unique,
   subset), serialises, and the whole file is written under compare-and-swap
   on the scanned hash. Ten hooks into one settings file are one guarded
   write. A config file that fails to parse is never written and all its
   merges are `blocked`. Claude merges JSON; Codex merges TOML.
5. **Plugins**: `claude plugin marketplace add <repo>` when the marketplace is
   unknown, then `claude plugin install <name>@<marketplace> --scope user
   --json -y` or `claude plugin update <name> --scope user --json -y`. The JSON
   result line is parsed; the installed version is then read back from the
   plugins file. A pinned version that still differs is reported in the
   result. A missing `claude` binary blocks plugin actions only.
6. **Manifest** is written last, and only if every non-conflict action on
   that harness succeeded; removals drop their entries. A half-applied host
   therefore never claims to be managed.
7. **Re-scan** the host and recompute inventory, emitting the usual inventory
   row events, so the matrix shows reality rather than intent.

A host result carries `status` (`applied`, `partial`, `skipped`, `failed`),
per-action outcomes (`done`, `conflict`, `failed` with message, `blocked`),
and a `restart_required` flag when any MCP server, hook or plugin changed.

Cancellation binds to the cancellation registry as other long commands do.
A cancel stops before the next host; the current host finishes its manifest
step or is reported `partial`.

## Codex

`harness/codex.rs` gains `scan_script()` (hashes under `~/.codex/skills`,
`config.toml` shipped as base64 with a `##CONFIG` block, and the Codex
manifest), `parse_scan()` (TOML parsed into `serde_json::Value` for the
snapshot), `installed()` (skills by folder, MCP servers by `mcp_servers`
keys), and `merge_config()` for TOML. The renderer stays limited to skills
and MCP servers.

## Harness trait addition

```rust
fn merge_config(&self, file: &str, existing: &str, merges: &[ConfigMerge]) -> Result<String, IpcError>;
fn manifest_path(&self) -> &'static str;
```

The applier stays generic over harnesses.

## Service, store, commands, MCP

Module `service/catalog/sync/`:

```
mod.rs       plan_sync(), apply_sync(), plan registry, HostSyncResult, ActionResult
plan.rs      SyncPlan, HostPlan, Action, ActionOp; compute from catalog + snapshot + manifest + secrets
manifest.rs  Manifest struct, parse from snapshot, serialise, diff
secrets.rs   resolve(host) and substitute(RenderPlan)
apply.rs     per-host applier: scripts, CAS, backups, secret uploads, merges, plugins, manifest
```

Store (migration 031): the `managed` column, the two secrets tables, and
`sync_runs(id, started_at, finished_at, summary_json)` so the last result
survives a restart. Helpers: secrets CRUD, `record_sync_run`, `last_sync_run`.

Commands: `catalog_plan_sync(host_alias?, kind?, name?)`,
`catalog_apply_sync(plan_id, call_id?)`, `catalog_last_sync`,
`catalog_list_secrets`, `catalog_set_secret(name, value, host_alias?)`,
`catalog_delete_secret(name, host_alias?)`.

MCP tools, three: `plan_sync` (mutating classification: it is the gate to
apply and refreshes inventory), `apply_sync` (in `CONFIRM_TOOLS`, needs a
confirm nonce; master token only when no host filter is given, like
`provision_hosts`), `set_secret` (master token only; the value is never
audited). No tool returns a secret value; `list_secrets` is UI only.

Events: apply emits the existing `asset_inventory:updated` rows as each host
is re-scanned, plus `sync:progress { plan_id, host_alias, harness, done,
total }`.

Errors: `E_SYNC_PLAN_STALE`, `E_SECRET_MISSING` (apply requested for a plan
with blocked actions and `force_partial` false), `E_SYNC_CANCELLED` maps to
the existing `E_CANCELLED`. Host-level failures are per-host results.

## UI

Assets tab additions:

- Toolbar: **Sync** button. Opens the plan dialog for all hosts. `busy` gains
  `'plan'` and `'apply'`.
- Asset detail: **Sync this asset** button beside the host matrix, and a per
  cell **Sync** action on cells that are `missing`, `drifted` or `orphan`.
- **Plan dialog** (`SyncPlanDialog.svelte`): grouped by host and harness, one
  row per action with op, asset, backup flag, secret names, and reason for
  `blocked`. Summary counts at the top. Confirm applies; the button is red
  when any action is `overwrite` or `remove`. While applying, rows update to
  `done`, `conflict`, `failed`; a progress line follows `sync:progress`.
  Afterwards a per-host result strip like the scan result strip, including
  "restart Claude on <host>" notes.
- **Secrets panel** (`SecretsPanel.svelte`) reachable from the toolbar: list
  of names from `secrets.example.yaml` plus any stored, a masked value input,
  per-host override rows, delete. Values are write-only; the list shows only
  whether a value is set.

## Testing

- `plan.rs`: table-driven tests over (catalog, snapshot, manifest, secrets)
  fixtures producing every action kind; adopt versus create versus update
  versus overwrite decided by manifest hash; orphan detection; blocked
  reasons.
- `secrets.rs`: resolution order, built-ins, missing names, substitution
  leaves unknown placeholders untouched.
- `manifest.rs`: round trip, diff, parse tolerance.
- `apply.rs`: script generation golden tests (no single quotes, CAS guard
  present, backup before overwrite, chunk boundaries); conflict parsing from
  script output; merge application per harness (JSON and TOML) as pure
  functions; plugin JSON result parsing. An end-to-end test against `local`
  with a temp HOME: create, update, overwrite with backup, remove orphan,
  manifest written last, conflict when the file changes after planning.
- Codex: scan script smoke test under bash with a temp HOME; TOML merge golden.
- Store: migration 031 idempotent; secrets CRUD; sync run record.
- MCP: guard classification tests (`CONFIRM_TOOLS` count bumped), reference
  regenerated.
- Frontend: store tests for plan and apply wrappers; component tests for the
  plan dialog (grouping, blocked rows, confirm colour, progress updates) and
  the secrets panel (masked input, override rows).

## Out of scope

- Automatic sync on the reconcile tick.
- Per-asset targeting profiles; everything still goes to every host.
- Project-scoped assets.
- Codex import and Codex plugin support.
- Rotating or exporting secrets; a secret value is never readable back.
- Sub-project 3: editing, templates, lint, authoring sessions.
