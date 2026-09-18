// Asset catalog store (sub-project 1). Mirrors src-tauri/src/service/catalog.
import { writable } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';
import type { SessionRow } from './sessions';

export type AssetKind = 'skill' | 'agent' | 'hook' | 'mcp_server' | 'plugin_ref';
export const KIND_ORDER: AssetKind[] = ['skill', 'agent', 'hook', 'mcp_server', 'plugin_ref'];
export const KIND_LABEL: Record<AssetKind, string> = {
  skill: 'Skills', agent: 'Agents', hook: 'Hooks', mcp_server: 'MCP servers', plugin_ref: 'Plugins',
};
export interface CatalogConfigRow {
  repo_path: string;
  remote_url: string | null;
  head_commit: string | null;
  last_loaded_at: number | null;
}
export interface CatalogSummary { head: string; loaded_at: number; asset_count: number; problem_count: number }
export interface AssetInventoryRow {
  host_alias: string; harness: string; kind: string; name: string; state: string;
  catalog_hash: string | null; host_hash: string | null; scanned_at: number;
  managed: boolean;
}
export interface HostState { host_alias: string; harness: string; state: string }
export interface Problem { path: string; message: string }
export interface AssetSummary {
  kind: string; name: string; version: string; description: string; tags: string[]; hosts: HostState[];
}
export interface AssetListing {
  head: string; loaded_at: number; assets: AssetSummary[]; unmanaged: AssetInventoryRow[]; problems: Problem[];
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

export function getAsset(kind: string, name: string): Promise<Result<AssetDetail>> {
  return invokeCmd<AssetDetail>('catalog_get_asset', { args: { kind, name } });
}

export function importHost(hostAlias: string, dryRun: boolean): Promise<Result<ImportReport>> {
  return invokeCmd<ImportReport>('catalog_import_host', { args: { host_alias: hostAlias, dry_run: dryRun } });
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
 *  subscription, for `SyncPlanDialog` to render a progress line during an
 *  in-flight apply. */
export const syncProgress = writable<SyncProgress | null>(null);

export function planSync(f: { hostAlias?: string; kind?: string; name?: string }): Promise<Result<SyncPlan>> {
  return invokeCmd<SyncPlan>('catalog_plan_sync', {
    args: { host_alias: f.hostAlias ?? null, kind: f.kind ?? null, name: f.name ?? null },
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
 *  manifest entry — the cases `SyncPlanDialog` renders its Apply button red
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
export interface RepoStatus { head: string; dirty: number; ahead: number | null; behind: number | null; has_upstream: boolean }
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
  f: { kind?: string; name?: string; instructions: string },
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  return invokeCmdAbortable<SessionRow>('catalog_spawn_author_session', {
    args: { kind: f.kind ?? null, name: f.name ?? null, instructions: f.instructions },
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
