// Asset catalog store (sub-project 1). Mirrors src-tauri/src/service/catalog.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export type AssetKind = 'skill' | 'agent' | 'hook' | 'mcp_server' | 'plugin_ref';
export const KIND_ORDER: AssetKind[] = ['skill', 'agent', 'hook', 'mcp_server', 'plugin_ref'];
export const KIND_LABEL: Record<AssetKind, string> = {
  skill: 'Skills', agent: 'Agents', hook: 'Hooks', mcp_server: 'MCP servers', plugin_ref: 'Plugins',
};
export type AssetState = 'in_sync' | 'drifted' | 'missing' | 'unmanaged' | 'unsupported';

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
export interface AssetDetail {
  // `tags` is optional here even though the backend now always serialises
  // the key: a stale build, a hand-crafted IPC mock in a test, or any other
  // producer of this shape may still omit it, and `AssetDetail.svelte` must
  // not crash reading `.length` off `undefined`.
  asset: { kind: string; name: string; version: string; description: string; tags?: string[]; body: string } & Record<string, unknown>;
  previews: Preview[];
  hosts: HostState[];
}
export interface HostScanResult { host: string; status: string; detail: string | null; rows: number }
export interface ImportReport { created: [string, string][]; problems: Problem[]; flagged_secrets: string[]; dry_run: boolean }

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
