# Asset catalog — sub-project 1: universal model, import, inventory

## Summary

claude-fleet gains a manager for agent assets: skills, subagents, hooks, MCP
servers, and plugin references. Assets live in a git repo in a harness-neutral
intermediate representation (IR). Fleet loads the repo, renders each asset for
a target harness (Claude Code first, Codex CLI as the portability check), scans
every host for what is actually installed, and shows drift. This sub-project is
read-only towards hosts: it imports, renders, previews and reports. Writing to
hosts (sync) and in-app authoring are separate sub-projects.

## Motivation

Today the controller's `~/.claude` holds ~91 skills, 18 agents, 6 plugins from
3 marketplaces, hooks and MCP entries. Some skills are symlinks into other
repos. None of it is version-controlled as a whole, nothing tells which host
has which version, and everything is written in Claude Code's own layout. The
same person already runs a Gemini skills directory on the same box, and wants
to be able to move to a different harness later without rewriting the estate.

## Design decisions (resolved during brainstorming)

- Lives inside claude-fleet (Rust service + IPC + MCP tools + UI tab), not a
  standalone tool.
- Source of truth is a dedicated git repo of assets in a fleet-defined IR
  (a fully abstract representation, not a mirror of any harness layout).
- Asset kinds in v1: skill, agent, hook, MCP server, plugin reference.
  Plugins are references only: the catalog records what to install; fleet
  never copies plugin contents.
- Targeting is "everything everywhere" at user scope. Tags are recorded but
  do not affect targeting.
- Bootstrap by importing from the controller host's Claude config.
- Assets on a host that are not in the catalog are left alone and reported as
  unmanaged. The catalog is never authoritative over unmanaged files.
- Secrets are `${NAME}` placeholders in the catalog, resolved at sync time
  (sub-project 2) from fleet's own store. Values never enter the repo.
- Full lifecycle is delivered as three sub-projects, in order: (1) catalog +
  inventory (this spec), (2) sync engine, (3) authoring.
- Second harness validated now is Codex CLI. The Codex adapter renders skills
  and MCP servers only and is marked experimental; no Codex host scanning.

## The universal model (IR)

### Repo layout

```
agent-assets/
  catalog.yaml                # schema_version: 1, name
  skills/<name>/asset.yaml    # metadata; body.md and resources/ alongside
  agents/<name>/asset.yaml    # metadata; prompt.md alongside
  hooks/<name>.yaml
  mcp/<name>.yaml
  plugins/<name>.yaml
  secrets.example.yaml        # list of ${PLACEHOLDER} names the catalog uses
```

`name` is the folder or file stem. Names are kebab-case `[a-z0-9-]+`, unique
within a kind.

### Common header

Every asset file starts with:

| Field | Type | Notes |
|---|---|---|
| `kind` | enum | `skill`, `agent`, `hook`, `mcp_server`, `plugin_ref` |
| `name` | string | must equal the folder/file stem |
| `version` | string | free-form; bumped by the author |
| `description` | string | required, non-empty |
| `tags` | string[] | optional, informational in v1 |
| `source` | object | optional; `{ imported_from: <host>, original_path, symlink_target }` written by the importer |
| `targets` | map | optional; per-harness block, see below |

`targets.<harness>` may contain `enabled: false`, any IR field to override for
that harness only, and `extra`, a free-form map that the renderer writes back
verbatim into the harness's native format. `extra` is how the importer stays
lossless when a harness has a field the IR has no slot for.

### Neutral vocabularies

Mapped per harness in Rust (`harness/<name>.rs`). The repo never contains
harness-specific names outside `targets`.

- Tools: `read`, `edit`, `write`, `bash`, `grep`, `glob`, `web_search`,
  `web_fetch`, `browser`, `agent`, `mcp:<server>`, and `*` for all.
- Model tiers: `fast`, `default`, `strong`. An explicit model id goes under
  `targets.<harness>.model`.
- Hook events: `session_start`, `prompt_submit`, `before_tool`, `after_tool`,
  `stop`, `subagent_stop`.

Claude mapping (v1): tools map to `Read`, `Edit`, `Write`, `Bash`, `Grep`,
`Glob`, `WebSearch`, `WebFetch`, `mcp__<server>__*`, `Agent`; tiers map to
`haiku`, `sonnet`, `opus`; events map to `SessionStart`, `UserPromptSubmit`,
`PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`.

Unknown tool names coming from an import are preserved under
`targets.claude.extra.tools` rather than rejected.

### Per kind

**skill** (`skills/<name>/asset.yaml` + `body.md` + optional `resources/`)

| Field | Notes |
|---|---|
| `allowed_tools` | neutral tool list, optional |
| `user_invocable` | bool, default true |
| `triggers` | string[], optional; folded into the description on harnesses without a trigger slot |

`body.md` is the instruction text. `resources/` is copied verbatim into the
rendered skill folder.

**agent** (`agents/<name>/asset.yaml` + `prompt.md`)

| Field | Notes |
|---|---|
| `tools` | neutral tool list, optional (means all) |
| `model` | tier, default `default` |

**hook** (`hooks/<name>.yaml`)

| Field | Notes |
|---|---|
| `event` | neutral event |
| `match` | optional `{ tool: <neutral tool or literal harness name> }` |
| `action.type` | `command` or `http` |
| `action.command` | for `command` |
| `action.url`, `action.headers` | for `http` |
| `action.timeout_s` | optional |

**mcp_server** (`mcp/<name>.yaml`)

| Field | Notes |
|---|---|
| `transport` | `http` or `stdio` |
| `url`, `headers` | for `http` |
| `command`, `args`, `env` | for `stdio` |

**plugin_ref** (`plugins/<name>.yaml`)

| Field | Notes |
|---|---|
| `harness` | which harness's plugin system; `claude` in v1 |
| `marketplace` | `{ name, source: github, repo }` |
| `plugin` | plugin name inside the marketplace |
| `version` | exact version string or `latest` |

Plugin refs are harness-specific by construction and have no neutral mapping.

### Secrets

Any string value may contain `${NAME}`. `secrets.example.yaml` lists the names
the catalog needs. Rendering in this sub-project keeps placeholders verbatim
and reports them in the preview. Substitution is sub-project 2.

### Codex paper check

| Kind | Codex rendering |
|---|---|
| skill | `~/.codex/skills/<name>/SKILL.md` + resources, 1:1 |
| mcp_server | `[mcp_servers.<name>]` table in `~/.codex/config.toml` |
| agent | unsupported; skipped with a warning unless `targets.codex.render_as: skill` |
| hook | unsupported; skipped with a warning |
| plugin_ref | not applicable (`harness: claude`) |

## Architecture

### Service module (`src-tauri/src/service/catalog/`)

```
model.rs      IR structs (serde YAML), validation, content hashing
repo.rs       locate / clone / pull the catalog repo on the controller, parse into Catalog
import.rs     host config → IR assets written into the repo working tree
inventory.rs  scan hosts, compute per-asset state, persist rows
harness/
  mod.rs      trait Harness + RenderPlan types + registry
  claude.rs   full: skill, agent, hook, mcp_server, plugin_ref
  codex.rs    skill + mcp_server; experimental
```

Service functions take `&Mutex<Store>` and `&Arc<SshClient>`, never Tauri
types, so IPC and MCP share one code path.

### Catalog loading (`repo.rs`)

The repo is cloned on the controller machine. Config is a local path plus an
optional remote URL. Fleet shells to local `git`: clone if the path has no
`.git`, `git pull --ff-only` on explicit pull, `git rev-parse HEAD` after
load. Parsing walks the kind directories, validates each file, and builds an
in-memory `Catalog { assets: Vec<Asset>, problems: Vec<Problem>, head: String,
loaded_at }`. A problem is `{ path, message }`; a problematic file is excluded
and the rest loads. The catalog is never stored in SQLite.

### Render plan (`harness/mod.rs`)

```rust
pub struct RenderPlan {
    pub files: Vec<FileWrite>,      // { path: String (with ~), bytes: Vec<u8> }
    pub merges: Vec<ConfigMerge>,   // { file: String, json_path: Vec<String>, value: serde_json::Value }
    pub placeholders: Vec<String>,  // unresolved ${NAME}s found while rendering
    pub warnings: Vec<String>,
}
pub trait Harness {
    fn id(&self) -> &'static str;
    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported>;
    fn scan_script(&self) -> Option<String>;               // bash, run over ssh; None = no scanning
    fn parse_scan(&self, stdout: &str) -> Result<HostSnapshot, IpcError>;
}
```

`codex.rs` returns `None` from `scan_script`, so inventory only runs for
Claude in this sub-project and Codex cells in the host matrix show
`unsupported` or the rendered preview only.

The catalog hash of an asset for a harness is the hash of its rendered plan
(files and merges), so drift is measured against what would actually be
written, not against the IR.

### Inventory scan (`inventory.rs`)

One SSH round-trip per host per harness. The scan script prints, in a
delimited block format, `sha256sum` of every file under the harness's skills
and agents directories, and the raw contents of the settings file, the MCP
config file and the installed-plugins file. `parse_scan` turns that into a
`HostSnapshot`: per skill/agent a map of relative path to hash, the parsed
hook entries, MCP entries and plugin list.

State per (host, harness, kind, name):

| State | Meaning |
|---|---|
| `in_sync` | catalog asset present on host with identical rendered hash |
| `drifted` | present with a different hash |
| `missing` | in catalog, not on host |
| `unmanaged` | on host, not in catalog |
| `unsupported` | harness cannot render this kind |

Managed-ness will come from a manifest file the sync engine writes
(sub-project 2). In this sub-project a present-and-matching asset reads as
`in_sync` regardless of who installed it.

Hosts that are hidden are skipped. Unreachable hosts produce a `skipped` entry
in the per-host result list; a scan error produces `failed` with the message.
Other hosts complete. This mirrors `provision_hosts`.

### Importer (`import.rs`)

Reads a host's Claude config (the controller in v1; the same scan script
provides the inputs, with file contents fetched for the assets being imported):

- Skills: each folder under `~/.claude/skills` becomes `skills/<name>/`.
  SKILL.md frontmatter maps to the header and skill fields; unknown keys go to
  `targets.claude.extra`; the markdown body becomes `body.md`; other files go
  to `resources/`. Symlinked folders are dereferenced and copied; `source`
  records the link and its target. Broken symlinks are reported and skipped.
- Agents: `~/.claude/agents/*.md` → `agents/<name>/`, tools mapped to neutral
  names where known, otherwise preserved in `extra`; model mapped to a tier
  where recognisable, otherwise preserved in `targets.claude.model`.
- Hooks: each entry in `settings.json` `hooks` → one `hooks/<name>.yaml`, name
  derived from event and matcher, de-duplicated with a numeric suffix.
  Bearer tokens and other header values matching the fleet's own MCP token
  are replaced by `${FLEET_MCP_TOKEN}`; other literal secrets are left as-is
  and flagged in the dry-run report.
- MCP servers: `~/.claude.json` `mcpServers` → `mcp/<name>.yaml`, same
  placeholder treatment.
- Plugins: `installed_plugins.json` + `known_marketplaces.json` →
  `plugins/<name>.yaml` with the exact installed version.

A `<name>` above that is not already valid kebab-case
(`[a-z0-9][a-z0-9-]*`) — a skill/agent folder name, an MCP server key, or a
plugin id — is slugified (lowercased, every run of other characters
collapsed to a single `-`, leading/trailing `-` trimmed) rather than
rejected; two different original names that slugify to the same value
produce a collision problem row for the second one instead of one silently
overwriting the other.

Import never overwrites an existing catalog asset; a collision becomes a
problem row. Dry-run returns the list of assets that would be created plus
problems and flagged secrets, without touching disk. Import does not commit;
the working tree is left for the user or the authoring flow.

### Persisted state

Migration `018_asset_catalog.sql`:

```sql
CREATE TABLE catalog_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  repo_path TEXT NOT NULL,
  remote_url TEXT,
  head_commit TEXT,
  last_loaded_at INTEGER
);
CREATE TABLE asset_inventory (
  host_alias TEXT NOT NULL,
  harness TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  state TEXT NOT NULL,
  catalog_hash TEXT,
  host_hash TEXT,
  scanned_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, harness, kind, name)
);
INSERT OR IGNORE INTO schema_version (version) VALUES (18);
```

Store helpers: get/set catalog config; upsert inventory rows for one host in
a single transaction, deleting rows for that host that were not seen; list
inventory. Upserts emit `asset_inventory:updated` row events from inside the
store, and a `catalog:loaded` event carries the summary (counts, head,
problems count) after a load.

### Commands and tools

Tauri IPC (`commands/assets.rs`, thin wrappers):

| Command | Purpose |
|---|---|
| `catalog_configure(repo_path, remote_url?)` | store config, clone if needed |
| `catalog_load(pull: bool)` | optional pull, parse, return summary |
| `catalog_list_assets()` | assets with per-host state summaries |
| `catalog_get_asset(kind, name)` | asset + rendered preview per harness |
| `catalog_import_host(host_alias, dry_run)` | importer |
| `assets_scan_hosts(host_alias?)` | scan all or one host |
| `assets_inventory()` | cached inventory rows |

MCP tools, three, added in one release: `list_assets`, `scan_assets`,
`import_assets`. Each audits non-secret args and returns `ok_json`. Rendered
previews are not exposed over MCP in v1.

### Error codes

| Code | When |
|---|---|
| `E_CATALOG_NOT_CONFIGURED` | any catalog call before `catalog_configure` |
| `E_CATALOG_GIT` | clone/pull/rev-parse failed; details carry git stderr; previous in-memory catalog is kept |
| `E_CATALOG_PARSE` | `catalog.yaml` itself is unreadable or has an unknown schema version |
| `E_ASSET_UNSUPPORTED` | a harness cannot render the requested kind |
| `E_ASSET_EXISTS` | import collision (also surfaced as a problem row in bulk import) |

Per-asset parse failures are problem rows, not errors.

## UI

A third view tab, "Assets", next to Terminal and Files in `App.svelte`,
using the same state-plus-slot mechanism as the Files tab; the terminal view
stays mounted underneath.

`src/lib/assets.ts` holds writable stores `catalog`, `inventory` and
`catalogConfig`, with merge/remove helpers keyed on the inventory row
identity, and an `onAssetInventoryUpdated` handler registered in
`subscribeToRowEvents`. Mutations do the optimistic patch from their return
value.

Layout inside the tab:

- **Toolbar**: repo path and head commit, "Pull", "Scan hosts" with last-scan
  time, "Import from host" (dialog: pick host, dry-run report, confirm), and a
  problems badge that opens the problems list.
- **Asset list** (left): grouped by kind with counts, text filter, per-row
  chip with in-sync / drifted / missing host counts. A separate bottom group
  "On hosts, not in catalog" lists unmanaged assets with a per-row Import
  action.
- **Detail pane** (right): header fields, a host matrix (rows: hosts;
  columns: harnesses; cells: state), and a rendered preview with a harness
  switcher showing the exact files and config merges, unresolved
  placeholders, or the unsupported note. Read-only in this sub-project; the
  sub-project 3 editor slots in here.

States: not configured shows a setup card (path or remote URL). Loading and
scanning are inline spinners on the toolbar buttons. Unreachable hosts are a
grey column with a "skipped" tooltip.

## Testing

- `model.rs`: serde round-trip per kind, validation failures (bad name,
  missing description, name/stem mismatch), hash stability across field
  order.
- `harness/claude.rs`, `harness/codex.rs`: golden-file tests from IR fixture
  to exact rendered output; unsupported kinds return the typed error;
  `targets.<harness>.extra` passthrough.
- `import.rs`: fixture directory mimicking a Claude config with a symlinked
  skill, an agent with unknown frontmatter, hooks with a fleet token, MCP
  entries and an installed-plugins file. Assert the IR output, the `extra`
  passthrough, placeholder substitution, and that import followed by render
  reproduces the fixture for every supported field.
- `inventory.rs`: pure tests for `parse_scan` and state computation. No SSH
  in unit tests.
- `store.rs`: migration 018 under the existing idempotency test; schema
  version assertion bumped to 18; upsert-and-prune behaviour.
- `mcp`: `reference_is_current` regenerated for the three tools.
- Frontend: Vitest for store merge/remove and grouping/filter logic;
  component tests for the tab switch and the setup card.

## File-by-file change list

- `src-tauri/migrations/018_asset_catalog.sql` — new.
- `src-tauri/src/store.rs` — migrate arm, row structs, helpers, bus calls.
- `src-tauri/src/events.rs` — `asset_inventory_updated`, `catalog_loaded` on
  the trait and all three bus impls.
- `src-tauri/src/service/mod.rs` — `pub mod catalog;`.
- `src-tauri/src/service/catalog/{mod,model,repo,import,inventory}.rs` — new.
- `src-tauri/src/service/catalog/harness/{mod,claude,codex}.rs` — new.
- `src-tauri/src/commands/assets.rs` — new; registered in `lib.rs`
  `generate_handler!`.
- `src-tauri/src/ipc_error.rs` — new codes.
- `src-tauri/src/mcp/tools.rs` — three tools; `docs/control-api-reference.md`
  regenerated.
- `src/lib/assets.ts`, `src/lib/events.ts` — store and event wiring.
- `src/App.svelte`, `src/lib/components/AssetsPanel.svelte` and children —
  tab and panel.
- `docs/concepts.md`, `docs/control-api.md` — short sections on the catalog.

## Out of scope (later sub-projects)

- Writing anything to hosts: sync, managed manifest, secret substitution,
  plugin install via the harness CLI, drift repair. Sub-project 2.
- Editing, templates, lint, delegating authoring to a session. Sub-project 3.
- Codex host scanning and importing from Codex.
- Project-scoped assets; only user scope.
- Profiles or tags affecting targeting.
