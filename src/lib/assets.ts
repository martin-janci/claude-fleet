// Asset catalog store (sub-project 1). Mirrors src-tauri/src/service/catalog.
import { writable } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';
import type { SessionRow } from './sessions';

export type AssetKind = 'skill' | 'agent' | 'command' | 'hook' | 'mcp_server' | 'plugin_ref';
export const KIND_ORDER: AssetKind[] = ['skill', 'agent', 'command', 'hook', 'mcp_server', 'plugin_ref'];
export const KIND_LABEL: Record<AssetKind, string> = {
  skill: 'Skills', agent: 'Agents', command: 'Commands', hook: 'Hooks', mcp_server: 'MCP servers', plugin_ref: 'Plugins',
};
/** One of a kind, for a form's Kind select ("Skill", "MCP server"). */
export const KIND_ONE: Record<AssetKind, string> = {
  skill: 'Skill', agent: 'Agent', command: 'Command', hook: 'Hook', mcp_server: 'MCP server', plugin_ref: 'Plugin',
};

/** The harnesses an asset can target (`service/catalog/harness`), as the
 *  editor's Harness boxes name them (G2.6). A kind a harness cannot render
 *  is not offered for it. */
export const HARNESSES: { id: string; label: string; kinds: AssetKind[] }[] = [
  { id: 'claude', label: 'Claude Code', kinds: ['skill', 'agent', 'command', 'hook', 'mcp_server', 'plugin_ref'] },
  { id: 'codex', label: 'Codex', kinds: ['skill', 'agent', 'mcp_server'] },
];

type TargetsMap = Record<string, Record<string, unknown>>;

/** Whether `harness` receives the asset: its `targets.<harness>.enabled`,
 *  true when absent (`TargetOverride`'s default). */
export function harnessOn(a: Pick<EditableAsset, 'targets'>, harness: string): boolean {
  const t = (a.targets as TargetsMap | undefined)?.[harness];
  return t?.enabled !== false;
}

/** `targets` with `harness` turned on or off. A target left holding only
 *  `enabled: true` is dropped, so the YAML stays as it was. */
export function withHarness(targets: unknown, harness: string, on: boolean): TargetsMap | undefined {
  const next: TargetsMap = { ...((targets as TargetsMap | undefined) ?? {}) };
  const t = { ...(next[harness] ?? {}) };
  if (on) delete t.enabled;
  else t.enabled = false;
  const emptyExtra = (k: string, v: unknown) => k === 'extra' && !!v && typeof v === 'object' && Object.keys(v).length === 0;
  if (Object.entries(t).every(([k, v]) => emptyExtra(k, v))) delete next[harness];
  else next[harness] = t;
  return Object.keys(next).length === 0 ? undefined : next;
}

/** The Import form's What boxes (G2.6): every kind the importer reads, as
 *  `only` entries (`<kind>:*`). All ticked imports everything (`[]`). */
export function importOnlyFor(kinds: readonly AssetKind[]): string[] {
  return kinds.length === KIND_ORDER.length ? [] : kinds.map((k) => `${k}:*`);
}
export interface CatalogConfigRow {
  repo_path: string;
  remote_url: string | null;
  head_commit: string | null;
  last_loaded_at: number | null;
}
export interface CatalogSummary { head: string; loaded_at: number; asset_count: number; problem_count: number }
/** Assets M5 (R4): on a drifted managed copy, which side moved. Absent when
 *  the copy's manifest entry cannot say (written before M5). */
export type DriftSide = 'host' | 'catalog';
export interface AssetInventoryRow {
  host_alias: string; harness: string; kind: string; name: string; state: string;
  catalog_hash: string | null; host_hash: string | null; scanned_at: number;
  managed: boolean;
  /** Present from hubs with migration 087; absent on older ones. */
  secret_like?: boolean; fleet_owned?: boolean;
  /** Migration 091: the asset's catalog. */
  catalog_id?: number | null;
  /** Assets M5 (R4). Absent from an older hub. */
  drift_side?: DriftSide | null;
}
export interface HostState {
  host_alias: string; harness: string; state: string;
  /** Assets M5 (R4). Absent from an older hub. */
  drift_side?: DriftSide | null;
}
export interface Problem { path: string; message: string }
/** Assets S1b: who may receive an asset. */
export type AssetScope = 'private' | 'shared';
export interface AssetSummary {
  kind: string; name: string; version: string; description: string; tags: string[]; hosts: HostState[];
  /** The identifier the asset installs under, when it differs from `name`. */
  install_as?: string;
  /** Optional because an older hub (pre-M1) omits the key. */
  scope?: AssetScope;
  /** Assets M5 (R11): `personal` or an org catalog's name. A listing spans
   *  every catalog, so one name can appear twice; an older hub omits the
   *  key and listed personal only — read it through `catalogOf`. */
  catalog?: string;
}
/** The catalog an asset summary is in (absent = an older hub's personal). */
export const catalogOf = (a: AssetSummary): string => a.catalog ?? 'personal';
/** Assets S1a (Task 3/4): `unmanaged` rows grouped per (kind, name) and
 *  classified server-side. Optional because older hubs (pre-migration 087)
 *  omit the key — `identitiesOf` below falls back to grouping client-side. */
export type IdentityClass = 'normal' | 'fleet_internal' | 'harness_internal' | 'needs_person';
export interface IdentityHost { host_alias: string; harness: string; host_hash: string | null }
export interface AssetIdentity {
  kind: string;
  name: string;
  hosts: IdentityHost[];
  /** Sorted distinct host aliases joined by ',' — the host-set signature. */
  signature: string;
  /** Distinct known content hashes across copies (0 when none is known). */
  variants: number;
  class: IdentityClass;
  /** Why a `needs_person` identity needs one. */
  reason: string | null;
}

export interface AssetListing {
  head: string; loaded_at: number; assets: AssetSummary[]; unmanaged: AssetInventoryRow[]; problems: Problem[];
  identities?: AssetIdentity[];
}
export interface FileWrite { path: string; bytes: string }
export interface ConfigMerge { file: string; json_path: string[]; mode: 'set' | 'append_unique' | 'subset'; value: unknown }
export interface RenderPlan { files: FileWrite[]; merges: ConfigMerge[]; placeholders: string[]; warnings: string[] }
export interface Preview { harness: string; plan: RenderPlan | null; unsupported: string | null }

/** A skill/agent `resources/…` file: `bytes` is base64. Never decode/display
 *  its content — only its size (see `resourceSize` below). */
export interface ResourceRef { rel_path: string; bytes: string }

/**
 * The `Asset` wire shape (sub-project 3): a flat object mirroring the Rust
 * manual serializer in `service/catalog/model.rs` (`merge_header_and_spec`).
 * Header fields (`kind, name, version, description, tags, source?,
 * targets?`) sit next to the kind-specific fields flattened in by
 * `AssetSpec`'s internal tag, plus `body` and an optional `resources` (the
 * backend omits the key when empty, hence optional here too — treat an
 * absent array as `[]`, never as "unknown").
 *
 * `tags` stays optional for the same reason as `AssetDetail.asset` below: a
 * stale build or hand-crafted IPC mock may omit it.
 *
 * Kind-specific fields (`allowed_tools`, `tools`, `model`, `event`, `match`,
 * `action`, `transport`, `url`, `headers`, `command`, `args`, `env`,
 * `harness`, `marketplace`, `plugin`, …) are not enumerated field-by-field
 * here — the index signature covers them so a value round-trips untouched
 * through the editor even for fields this UI does not render a control for
 * (`source`, `targets`, per-harness extras). `AssetEditor` reads/writes the
 * specific ones it renders through `KIND_FIELDS`.
 */
export interface EditableAsset {
  kind: AssetKind;
  name: string;
  version: string;
  description: string;
  tags?: string[];
  /** The identifier a harness installs this asset under, when it differs
   *  from `name` (skill/agent/mcp_server only — `hook`/`plugin_ref` derive
   *  their host key from other fields and reject it). Absent/`null` means
   *  "use `name`"; the backend omits the key on the wire when unset and
   *  accepts either an absent key or an explicit `null` back (`Option<String>`
   *  with `#[serde(default, skip_serializing_if = "Option::is_none")]`). */
  install_as?: string | null;
  /** Assets S1b: who may receive this asset. Absent means `private` (the
   *  default); the index signature already round-trips it through
   *  `updateAsset` like any other field this UI has no dedicated control
   *  for — declared explicitly here only for the type. */
  scope?: AssetScope;
  /** Per-harness overrides (`targets.<harness>`); the Harness boxes (G2.6)
   *  write `enabled`, everything else round-trips untouched. */
  targets?: Record<string, Record<string, unknown>>;
  body: string;
  resources?: ResourceRef[];
  [key: string]: unknown;
}

export interface AssetDetail {
  // `tags` is optional here even though the backend now always serialises
  // the key: a stale build, a hand-crafted IPC mock in a test, or any other
  // producer of this shape may still omit it, and `AssetDetail.svelte` must
  // not crash reading `.length` off `undefined`.
  asset: EditableAsset;
  previews: Preview[];
  hosts: HostState[];
}
export interface HostScanResult { host: string; status: string; detail: string | null; rows: number }
export interface ImportReport { created: [string, string][]; problems: Problem[]; warnings?: Problem[]; flagged_secrets: string[]; dry_run: boolean }

export const catalogConfig = writable<CatalogConfigRow | null>(null);
export const catalog = writable<AssetListing | null>(null);
export const inventory = writable<AssetInventoryRow[]>([]);

export async function loadCatalogConfig(): Promise<Result<CatalogConfigRow | null>> {
  const r = await invokeCmd<CatalogConfigRow | null>('catalog_config');
  if (r.ok) catalogConfig.set(r.value);
  return r;
}

export async function configureCatalog(repoPath: string, remoteUrl: string): Promise<Result<CatalogConfigRow>> {
  const r = await invokeCmd<CatalogConfigRow>('catalog_configure', {
    args: { repo_path: repoPath, remote_url: remoteUrl.trim() === '' ? null : remoteUrl.trim() },
  });
  if (r.ok) catalogConfig.set(r.value);
  return r;
}

export async function loadCatalog(pull: boolean): Promise<Result<CatalogSummary>> {
  const r = await invokeCmd<CatalogSummary>('catalog_load', { args: { pull } });
  if (r.ok) {
    catalogConfig.update((c) => (c ? { ...c, head_commit: r.value.head, last_loaded_at: r.value.loaded_at } : c));
  }
  return r;
}

export async function loadAssets(): Promise<Result<AssetListing>> {
  const r = await invokeCmd<AssetListing>('catalog_list_assets');
  if (r.ok) catalog.set(r.value);
  return r;
}

/** Fill the `catalog` store once at launch, so the quick switcher (⌘K) lists
 *  assets before the Assets tab was ever opened — the panel is the only other
 *  thing that loads it. Silent: a failure (no catalog configured, an ungranted
 *  hub client's refusal) leaves the store as it was. A hub loaded its own
 *  catalog at boot, so a hub client only reads the listing; a standalone
 *  window first loads its configured catalog, as the panel's mount does
 *  (nothing else has by then). */
export async function primeCatalog(remote: boolean): Promise<void> {
  if (!remote) {
    const c = await loadCatalogConfig();
    if (!c.ok || !c.value) return;
    if (!(await loadCatalog(false)).ok) return;
  }
  await loadAssets();
}

export function getAsset(kind: string, name: string): Promise<Result<AssetDetail>> {
  return invokeCmd<AssetDetail>('catalog_get_asset', { args: { kind, name } });
}

export function importHost(hostAlias: string, dryRun: boolean, only: string[] = []): Promise<Result<ImportReport>> {
  return invokeCmd<ImportReport>('catalog_import_host', { args: { host_alias: hostAlias, dry_run: dryRun, only } });
}

export function scanHosts(hostAlias?: string): Promise<Result<HostScanResult[]>> {
  return invokeCmd<HostScanResult[]>('assets_scan_hosts', { args: { host_alias: hostAlias ?? null } });
}

export async function loadInventory(): Promise<Result<AssetInventoryRow[]>> {
  const r = await invokeCmd<AssetInventoryRow[]>('assets_inventory');
  if (r.ok) inventory.set(r.value);
  return r;
}

const key = (r: { host_alias: string; harness: string; kind: string; name: string }) =>
  [r.host_alias, r.harness, r.kind, r.name].join('::');

export function mergeInventoryRow(row: AssetInventoryRow): void {
  inventory.update((arr) => {
    const i = arr.findIndex((r) => key(r) === key(row));
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function clearInventoryFor(hostAlias: string, harness: string): void {
  inventory.update((arr) => arr.filter((r) => !(r.host_alias === hostAlias && r.harness === harness)));
}

export interface KindGroup { kind: AssetKind; label: string; assets: AssetSummary[] }

export function groupByKind(listing: AssetListing): KindGroup[] {
  return KIND_ORDER
    .map((kind) => ({ kind, label: KIND_LABEL[kind], assets: listing.assets.filter((a) => a.kind === kind) }))
    .filter((g) => g.assets.length > 0);
}

export function stateCounts(hosts: HostState[]): Record<'in_sync' | 'drifted' | 'missing' | 'unsupported', number> {
  const c = { in_sync: 0, drifted: 0, missing: 0, unsupported: 0 };
  for (const h of hosts) if (h.state in c) c[h.state as keyof typeof c] += 1;
  return c;
}

/** `listing.identities` when the hub sent it (Task 3, migration 087+),
 *  otherwise group the `unmanaged` rows client-side — same shape, just
 *  `normal`/`variants: 0` for every identity since drift classification
 *  needs the host-hash comparison the backend does. */
export function identitiesOf(listing: AssetListing): AssetIdentity[] {
  if (listing.identities) return listing.identities;
  const by = new Map<string, AssetIdentity>();
  for (const r of listing.unmanaged.filter((r) => r.state === 'unmanaged')) {
    const key = `${r.kind}\u0000${r.name}`;
    const id = by.get(key) ?? { kind: r.kind, name: r.name, hosts: [], signature: '', variants: 0, class: 'normal' as IdentityClass, reason: null };
    id.hosts.push({ host_alias: r.host_alias, harness: r.harness, host_hash: r.host_hash });
    by.set(key, id);
  }
  return [...by.values()]
    .map((id) => ({ ...id, signature: [...new Set(id.hosts.map((h) => h.host_alias))].sort().join(',') }))
    .sort((a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name));
}

/** Every host alias seen across `ids`, `local` first then alphabetical —
 *  the fixed dot order `HostStrip` renders in. */
export function hostOrder(ids: AssetIdentity[]): string[] {
  return hostOrderOf(ids.flatMap((i) => i.hosts.map((h) => h.host_alias)));
}

/** The same fixed order over bare aliases (the one implementation:
 *  `hostOrder` and the Inbox both use it). */
export function hostOrderOf(aliases: Iterable<string>): string[] {
  const all = new Set(aliases);
  const rest = [...all].filter((a) => a !== 'local').sort();
  return all.has('local') ? ['local', ...rest] : rest;
}

const COPIES_DIFFER = 'copies differ on ';
/** The hosts whose copy of a `needs_person` identity is the odd one out
 *  (the server words it `copies differ on a, b`); `[]` for any other reason.
 *  The one parser: `AssetList` and the Inbox's dots both read it. */
export function oddHosts(i: Pick<AssetIdentity, 'reason'>): string[] {
  return i.reason?.startsWith(COPIES_DIFFER) ? i.reason.slice(COPIES_DIFFER.length).split(', ') : [];
}

// ── Sync engine (sub-project 2): plan/apply, secrets, progress. Mirrors
// src-tauri/src/service/catalog/sync/*. ──

export type ActionOp =
  | 'create' | 'update' | 'overwrite' | 'adopt' | 'remove'
  | 'plugin_install' | 'plugin_update' | 'noop' | 'blocked';

export interface SyncAction {
  kind: string;
  name: string;
  op: ActionOp;
  reason: string | null;
  files: string[];
  merges: string[];
  backup: boolean;
  secrets: string[];
  missing_secrets: string[];
  /** The catalog the asset comes from (absent from an older hub). */
  catalog?: string | null;
  /** What the planner found of the host's copy of an `update`/`overwrite`. */
  host_copy?: 'unchanged' | 'edited' | 'unverified';
}

export interface HostPlan {
  host_alias: string;
  harness: string;
  status: string;
  detail: string | null;
  actions: SyncAction[];
}

export interface SyncPlan {
  id: string;
  computed_at: number;
  hosts: HostPlan[];
  counts: Record<string, number>;
}

export interface ActionResult {
  kind: string;
  name: string;
  op: ActionOp;
  outcome: string;
  detail: string | null;
  /** The catalog the planned action came from; absent from a hub before M6. */
  catalog?: string | null;
}

export interface HostSyncResult {
  host_alias: string;
  harness: string;
  status: string;
  detail: string | null;
  restart_required: boolean;
  actions: ActionResult[];
}

export interface SyncRunSummary {
  plan_id: string;
  started_at: number;
  finished_at: number;
  hosts: HostSyncResult[];
  /** M4 I3 / M5 R10: SB6's automatic run, not a person's. */
  auto?: boolean;
}

export interface SecretRow {
  name: string;
  host_alias: string | null;
  updated_at: number;
}

export interface SyncProgress {
  plan_id: string;
  host_alias: string;
  harness: string;
  done: number;
  total: number;
}

/** The most recently completed sync run, loaded on mount and refreshed
 *  after `applySync` completes. */
export const lastSyncRun = writable<SyncRunSummary | null>(null);

/** The latest `sync:progress` event forwarded by `App.svelte`'s row-event
 *  subscription, for `SyncPlanView` to render a progress line during an
 *  in-flight apply. */
export const syncProgress = writable<SyncProgress | null>(null);

export function planSync(f: { hostAlias?: string; kind?: string; name?: string; allowUnlayered?: boolean }): Promise<Result<SyncPlan>> {
  return invokeCmd<SyncPlan>('catalog_plan_sync', {
    args: {
      host_alias: f.hostAlias ?? null,
      kind: f.kind ?? null,
      name: f.name ?? null,
      allow_unlayered: f.allowUnlayered ?? false,
    },
  });
}

export function applySync(planId: string, forcePartial: boolean, signal?: AbortSignal): Promise<Result<SyncRunSummary>> {
  return invokeCmdAbortable<SyncRunSummary>(
    'catalog_apply_sync',
    { args: { plan_id: planId, force_partial: forcePartial } },
    signal,
  );
}

export async function lastSync(): Promise<Result<SyncRunSummary | null>> {
  const r = await invokeCmd<SyncRunSummary | null>('catalog_last_sync');
  if (r.ok) lastSyncRun.set(r.value);
  return r;
}

export function listSecrets(): Promise<Result<SecretRow[]>> {
  return invokeCmd<SecretRow[]>('catalog_list_secrets');
}

export function setSecret(name: string, value: string, hostAlias?: string): Promise<Result<null>> {
  return invokeCmd<null>('catalog_set_secret', { args: { name, host_alias: hostAlias ?? null, value } });
}

export function deleteSecret(name: string, hostAlias?: string): Promise<Result<boolean>> {
  return invokeCmd<boolean>('catalog_delete_secret', { args: { name, host_alias: hostAlias ?? null } });
}

/** Whether applying `plan` would overwrite a host-edited asset or remove a
 *  manifest entry — the cases `SyncPlanView` renders its Apply button red
 *  for. */
export function isDestructive(plan: SyncPlan): boolean {
  return plan.hosts.some((h) => h.actions.some((a) => a.op === 'overwrite' || a.op === 'remove'));
}

// ── Authoring (sub-project 3): templates, lint, editor writes, commit/push,
// delegating to a session. Mirrors src-tauri/src/service/catalog/author.rs
// and author_session.rs. ──

export interface Finding { field: string; message: string }
export interface LintReport { errors: Finding[]; warnings: Finding[] }
export interface AssetLint { kind: string; name: string; report: LintReport }
export interface LintAll { assets: AssetLint[]; problems: Problem[]; errors: number; warnings: number }
/** One uncommitted change, per asset (`repo::RepoChange`, G2.6). */
export interface RepoChange { path: string; status: 'A' | 'M' | 'D' | string }
export interface RepoStatus {
  head: string; dirty: number; ahead: number | null; behind: number | null; has_upstream: boolean;
  /** Absent from an older hub. */
  changes?: RepoChange[];
}

/** The commit message the Commit form starts from, by rule from the
 *  changes (mirrors `repo::commit_message_for`). */
export function commitMessageFor(changes: readonly RepoChange[]): string {
  if (changes.length === 0) return 'catalog: commit pending changes';
  const verb = changes.every((c) => c.status === 'A') ? 'add' : changes.every((c) => c.status === 'D') ? 'remove' : 'update';
  const named = changes.slice(0, 3).map((c) => c.path);
  const more = changes.length - named.length;
  return `catalog: ${verb} ${named.join(', ')}${more > 0 ? ` (+${more})` : ''}`;
}
export interface WriteResult { commit: string; lint: LintReport }

/** The neutral tool vocabulary (`service/catalog/model.rs::TOOLS`). An
 *  `mcp:<server>` tool reference is also valid but is not one of these
 *  fixed entries. */
export const TOOLS = ['read', 'edit', 'write', 'bash', 'grep', 'glob', 'web_search', 'web_fetch', 'browser', 'agent', '*'];
/** Neutral model tiers (`model.rs::TIERS`). */
export const TIERS = ['fast', 'default', 'strong'];
/** Neutral hook/agent-loop events (`model.rs::EVENTS`). */
export const EVENTS = ['session_start', 'prompt_submit', 'before_tool', 'after_tool', 'stop', 'subagent_stop'];

/** The kind-specific field names `AssetEditor` renders a control for, in
 *  display order. Drives which `editor-field-<field>` blocks appear per
 *  kind; each field's widget (checklist, select, text, nested group) is
 *  chosen in `AssetEditor.svelte` itself. */
export const KIND_FIELDS: Record<AssetKind, string[]> = {
  skill: ['allowed_tools', 'user_invocable', 'triggers'],
  agent: ['tools', 'model'],
  hook: ['event', 'action'],
  mcp_server: ['transport', 'url', 'command', 'args', 'env'],
  plugin_ref: ['harness', 'marketplace', 'plugin'],
  command: ['allowed_tools', 'argument_hint', 'model'],
};

/** The most recently loaded repo status (dirty/ahead/behind/head), refreshed
 *  after every authoring write and on `AssetsPanel` mount. */
export const repoStatusStore = writable<RepoStatus | null>(null);

export async function createAsset(kind: string, name: string, duplicateFrom?: string): Promise<Result<WriteResult>> {
  return invokeCmd<WriteResult>('catalog_create_asset', {
    args: { kind, name, duplicate_from: duplicateFrom ?? null },
  });
}

/** Save an edited asset. `resources` defaults to `[]` — the backend expects
 *  the key present (it decides on its own whether to omit it on disk). */
export async function updateAsset(asset: EditableAsset): Promise<Result<WriteResult>> {
  return invokeCmd<WriteResult>('catalog_update_asset', {
    args: { asset: { ...asset, resources: asset.resources ?? [] } },
  });
}

export function deleteAsset(kind: string, name: string): Promise<Result<string>> {
  return invokeCmd<string>('catalog_delete_asset', { args: { kind, name } });
}

export function addResource(kind: string, name: string, localPath: string, relPath?: string): Promise<Result<WriteResult>> {
  return invokeCmd<WriteResult>('catalog_add_resource', {
    args: { kind, name, local_path: localPath, rel_path: relPath ?? null },
  });
}

export function removeResource(kind: string, name: string, relPath: string): Promise<Result<WriteResult>> {
  return invokeCmd<WriteResult>('catalog_remove_resource', { args: { kind, name, rel_path: relPath } });
}

export function lintAsset(kind: string, name: string): Promise<Result<LintReport>> {
  return invokeCmd<LintReport>('catalog_lint_asset', { args: { kind, name } });
}

export function lintAll(): Promise<Result<LintAll>> {
  return invokeCmd<LintAll>('catalog_lint_all');
}

export function commitPending(message?: string): Promise<Result<string>> {
  return invokeCmd<string>('catalog_commit_pending', { args: { message: message ?? null } });
}

export async function pushCatalog(): Promise<Result<RepoStatus>> {
  const r = await invokeCmd<RepoStatus>('catalog_push');
  if (r.ok) repoStatusStore.set(r.value);
  return r;
}

export async function repoStatus(): Promise<Result<RepoStatus>> {
  const r = await invokeCmd<RepoStatus>('catalog_repo_status');
  if (r.ok) repoStatusStore.set(r.value);
  return r;
}

export function assetTemplate(kind: string, name: string): Promise<Result<EditableAsset>> {
  return invokeCmd<EditableAsset>('catalog_template', { args: { kind, name } });
}

/** Delegate an asset (or "create a new asset" when `kind`/`name` are
 *  omitted) to a freshly spawned interactive session in the catalog repo.
 *  Abortable like `applySync` — the wrapper injects a `call_id`. */
export function spawnAuthorSession(
  f: { kind?: string; name?: string; instructions: string; host?: string },
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  return invokeCmdAbortable<SessionRow>('catalog_spawn_author_session', {
    args: {
      kind: f.kind ?? null,
      name: f.name ?? null,
      instructions: f.instructions,
      // G2.6: the Host picker; `local` (the catalog's own checkout) is the
      // default and is not sent, so an older backend reads the same args.
      ...(f.host && f.host !== 'local' ? { host_alias: f.host } : {}),
    },
  }, signal);
}

/** Bytes of a base64 resource, for the "size" the editor shows (never the
 *  content). Invalid/empty base64 yields 0 rather than throwing. */
export function resourceSize(bytes: string): number {
  try {
    return atob(bytes).length;
  } catch {
    return 0;
  }
}
