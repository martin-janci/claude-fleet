# Asset catalog — install names and plugin updates

## Summary

Two follow-ups parked by the catalog reviews (PRs #93, #96, #98):

1. **Install names.** The importer slugifies host identifiers into kebab-case
   catalog names, so an imported asset can sync back under a different
   identifier than the one the host already has (`~/.claude/skills/foo_bar`
   becomes `skill/foo-bar`, which renders to `~/.claude/skills/foo-bar`). The
   original stays `unmanaged` next to a managed copy. Assets gain an optional
   `install_as` header field naming the identifier used on hosts; the importer
   sets it whenever the slug diverges from a path-safe original.
2. **Plugin updates.** The planner never produces `plugin_update`: a pinned
   plugin installed at another version is `blocked` because the CLI cannot pin
   versions. The manifest now records which catalog pin fleet last applied, and
   when the pin changes the planner schedules one `claude plugin update`; a
   host that still differs afterwards stays `blocked` as today. `latest` refs
   never update automatically.

## Install names

### Model

`Header` gains `install_as: Option<String>` (`skip_serializing_if` none, like
`source`). `Asset::install_name() -> &str` returns `install_as` when set, else
`name`. `validate` errors when `install_as` is set on a `hook` or `plugin_ref`
(those kinds derive host keys from other fields), is empty after trim,
contains a character outside `[A-Za-z0-9._-]`, or is `.` / `..`. Lint warns
when `install_as` equals `name`.

### Rendering and inventory

Every harness site that derives a host path or config key from
`header.name` uses `install_name()` instead: Claude skill directory, agent
file, `mcpServers.<key>`; Codex skill directory, `mcp_servers.<key>`.
Manifest keys, inventory rows, plan actions and the UI keep the catalog
`name`. `compute_states` builds the `unmanaged` list by subtracting the
catalog's *install names* per kind from `installed()`, so an installed
`foo_bar` matches the catalog's `foo-bar` when `install_as: foo_bar`.

### Importer

For skills, agents and MCP servers, when the slug differs from the original
identifier and the original is a valid install name, the importer sets
`install_as` to the original. When the original is not a valid install name
(a space, a slash, a leading dot), the asset is still created under the slug
without `install_as`, and the report's new `warnings: Vec<Problem>` lists it
(`<kind> <slug>: installs under a new name; <original> stays unmanaged`).
Hooks and plugin refs never get `install_as`.

### UI and docs

The editor shows an optional "Installs as" input (`editor-install-as`) for
skills, agents and MCP servers; the detail header shows "installs as
`<install_as>`" when set. The import dialog lists warnings under their own
heading. `docs/concepts.md` describes the field.

## Plugin updates

### Manifest

`plugin_entry` records the rendered plugin merge's `value_hash` (the hash of
`[{"version": v}]` for a pin, of `[{}]` for `latest`) instead of an empty
string, so the manifest says which catalog pin fleet last applied. The
applier takes the value from the action's plan (the planner attaches the
render plan to plugin actions).

### Planner

`plugin_op` receives the host's manifest entry (`Option<&ManifestEntry>`)
instead of `in_manifest: bool`:

- no installed record → `plugin_install` (unchanged);
- installed record satisfies the render → `noop` / `adopt` (unchanged);
- installed record does not satisfy a pin:
  - the entry is absent, has an empty `value_hash` (written by an earlier
    fleet), or its `value_hash` differs from the current render's →
    `plugin_update`, reason `catalog pin changed to <v> (installed <w>)`;
  - otherwise → `blocked`, reason unchanged (`installed <w>, catalog pins
    <v>; the CLI cannot pin versions`).

`latest` renders `[{}]`, which any record satisfies, so it never reaches the
update branch. After an update the applier already reports `installed <w>,
catalog pins <v>` when the CLI landed on another version; the manifest entry
then carries the new hash and the next plan reads `blocked`.

### Docs

`docs/concepts.md` sync paragraph: one sentence on when plugins update.
`docs/control-api.md`: the `plan_sync` bullet mentions `plugin_update`.

## Testing

- model: `install_as` round-trips YAML/JSON; validate table (kind, empty,
  bad char, dot-dot); lint warning.
- harness: skill/agent/MCP render paths use `install_as`; Codex likewise.
- inventory: `foo_bar` installed + catalog `foo-bar` with `install_as` →
  `in_sync`, no `unmanaged` row; without `install_as` → `missing` +
  `unmanaged`.
- importer: `foo_bar` skill dir → `install_as: foo_bar`; MCP key
  `claude_ai_Docs` → `install_as`; `My Skill` dir → slug, no `install_as`,
  one warning; kebab originals → no `install_as`.
- planner: pin unchanged + drift → `blocked`; pin changed → `plugin_update`;
  legacy empty hash → `plugin_update`; `latest` → `noop`/`adopt`.
- applier: manifest entry carries the render's `value_hash` after
  install/adopt/update.
- frontend: editor input round-trips `install_as`; detail shows it; import
  dialog renders warnings.

## Out of scope

Renaming assets; per-host install names; updating `latest` refs; Codex
plugin support.
