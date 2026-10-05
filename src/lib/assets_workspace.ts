// Assets M5: the workspace's own reads — the catalogs behind the footer's
// chips, the cards behind the Inbox's Proposed rows, the layers behind
// `layer:`, a catalog's repo status, an asset's History — and the small pure
// helpers the views share. Mirrors service/catalog/{catalogs, changesets/mod,
// repo}.rs and the commands of commands/assets.rs (Rulings R13).
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { AssetSummary, RepoStatus, SyncRunSummary } from './assets';
import type { BadgeTone } from './assets_visual';

export const PERSONAL = 'personal';

export interface CatalogStatus {
  id: number;
  name: string;
  org_id: number | null;
  org?: string | null;
  repo_path: string;
  remote_url: string | null;
  head_commit: string | null;
  last_loaded_at: number | null;
  state: 'loaded' | 'problem' | 'not_loaded';
  problem?: string | null;
  asset_count: number;
  admitted?: string[];
  granted?: string[];
}

export type CardKind = 'bootstrap' | 'new' | 'drift' | 'rollout';
export type CardState = 'proposed' | 'applied' | 'undone' | 'dismissed' | 'failed';
export interface ChangesetSummary {
  id: number;
  kind: CardKind;
  summary: string;
  state: CardState;
  created_at: number;
  /** Unix milliseconds. */
  applied_at?: number | null;
  error?: string | null;
  groups?: Record<string, number>;
  pending?: number;
  undoable?: boolean;
}

/** One card item, as `changesets { get }` returns it (the full card is M6). */
export interface ChangesetItem {
  changeset_id: number;
  position: number;
  grp: string;
  catalog_id?: number | null;
  kind: string;
  name: string;
  /** import | assign_layer | set_scope | hide | take_host | restore | sync */
  action: string;
  params?: string | null;
  /** rule | jev | haiku | person */
  decider: string;
  /** pending | applied | skipped | rejected */
  state: string;
  /** Unix **milliseconds** (migration 097); absent while pending and on items
   *  decided before it. */
  decided_at?: number;
}

export interface CommitEntry { sha: string; at: number; author: string; subject: string }

export interface LayerDef { name: string; axis: 'role' | 'context'; description?: string; extends?: string; members?: string[] }
export interface HostLayerRow { host_alias: string; catalog_id?: number; layer_name: string; axis: string; position: number; active: boolean }
export interface LayerListing { layers: LayerDef[]; hosts: HostLayerRow[] }

/** The rail's views in M5 (R15); Layers and Hosts join in M6. */
export type WorkspaceView = 'inbox' | 'library';
export const RAIL_VIEWS: readonly { id: WorkspaceView; label: string }[] = [
  { id: 'inbox', label: 'Inbox' },
  { id: 'library', label: 'Library' },
];

/** `null` until loaded, and again when the read was refused (an ungranted,
 *  readonly or org-bound client): the views read it as "not here". */
export const catalogStatuses = writable<CatalogStatus[] | null>(null);
export const changesetSummaries = writable<ChangesetSummary[] | null>(null);
export const layerListing = writable<LayerListing | null>(null);

export async function loadCatalogStatuses(): Promise<Result<CatalogStatus[]>> {
  const r = await invokeCmd<CatalogStatus[]>('catalog_list_catalogs');
  catalogStatuses.set(r.ok ? r.value : null);
  return r;
}

export async function loadChangesets(): Promise<Result<ChangesetSummary[]>> {
  const r = await invokeCmd<ChangesetSummary[]>('catalog_list_changesets');
  changesetSummaries.set(r.ok ? r.value : null);
  return r;
}

export async function loadLayers(): Promise<Result<LayerListing>> {
  const r = await invokeCmd<LayerListing>('catalog_list_layers');
  layerListing.set(r.ok ? r.value : null);
  return r;
}

export function repoStatusOf(catalog: string): Promise<Result<RepoStatus>> {
  return invokeCmd<RepoStatus>('catalog_repo_status_in', { args: { name: catalog } });
}

export function assetHistory(kind: string, name: string, catalog?: string | null): Promise<Result<CommitEntry[]>> {
  return invokeCmd<CommitEntry[]>('catalog_asset_history', {
    args: { kind, name, catalog: catalog && catalog !== PERSONAL ? catalog : null },
  });
}

/** R14: a card the Inbox shows — still to apply, or failed and retryable. */
export function isOpenCard(c: ChangesetSummary): boolean {
  return c.state === 'proposed' || c.state === 'failed';
}

/** What a list row is — one string per row (`data-row-key`), shared by the
 *  views, the keyboard and the Inspector. */
export type Selection =
  | { type: 'asset'; catalog: string; kind: string; name: string }
  | { type: 'identity'; kind: string; name: string }
  | { type: 'orphan'; kind: string; name: string }
  | { type: 'card'; id: number };

export function keyOf(s: Selection): string {
  switch (s.type) {
    case 'asset':
      return `asset:${s.catalog}:${s.kind}/${s.name}`;
    case 'identity':
    case 'orphan':
      return `${s.type}:${s.kind}/${s.name}`;
    case 'card':
      return `card:${s.id}`;
  }
}

export function parseKey(key: string): Selection | null {
  const i = key.indexOf(':');
  if (i < 0) return null;
  const type = key.slice(0, i);
  let body = key.slice(i + 1);
  if (type === 'card') {
    const id = Number(body);
    return body !== '' && Number.isInteger(id) ? { type, id } : null;
  }
  let catalog = PERSONAL;
  if (type === 'asset') {
    const j = body.indexOf(':');
    if (j < 0) return null;
    catalog = body.slice(0, j);
    body = body.slice(j + 1);
  }
  const s = body.indexOf('/');
  if (s <= 0) return null;
  const kind = body.slice(0, s);
  const name = body.slice(s + 1);
  if (type === 'asset') return { type, catalog, kind, name };
  if (type === 'identity' || type === 'orphan') return { type, kind, name };
  return null;
}

export interface BadgeSpec { label: string; tone: BadgeTone; dashed: boolean; title: string }

/** The scope/catalog badge of a catalog asset (spec, Inbox rows: "a `Badge`
 *  for scope/catalog"): the org catalog's name, else shared or private. */
export function scopeBadge(a: Pick<AssetSummary, 'catalog' | 'scope'>): BadgeSpec {
  const catalog = a.catalog ?? PERSONAL;
  if (catalog !== PERSONAL) {
    return { label: catalog, tone: 'accent', dashed: false, title: `In catalog ${catalog}: only hosts that accept it receive it` };
  }
  if (a.scope === 'shared') {
    return { label: 'shared', tone: 'neutral', dashed: false, title: 'Personal catalog, shared: hosts of an org may receive it' };
  }
  return { label: 'private', tone: 'neutral', dashed: true, title: 'Personal catalog, private: never installed on a host of an org' };
}

export interface WriteContext { readOnly: boolean; remote: boolean; clientName: string | null; statuses: CatalogStatus[] | null }

/** R20: whether this window may author in `catalog`. Read-only: never.
 *  Standalone, or personal on a granted hub client: yes. Another catalog on
 *  a hub client: when `list_catalogs` names this client among its grantees. */
export function canWrite(catalog: string | undefined, ctx: WriteContext): boolean {
  if (ctx.readOnly) return false;
  const c = catalog ?? PERSONAL;
  if (!ctx.remote || c === PERSONAL) return true;
  const row = ctx.statuses?.find((s) => s.name === c);
  return !!row && !!ctx.clientName && (row.granted ?? []).includes(ctx.clientName);
}

/** The footer's last-sync line. */
export function summarizeRun(run: SyncRunSummary): string {
  const counts: Record<string, number> = {};
  for (const h of run.hosts) counts[h.status] = (counts[h.status] ?? 0) + 1;
  const parts = Object.entries(counts).map(([k, n]) => `${n} ${k}`);
  return `${new Date(run.finished_at * 1000).toLocaleString()} — ${parts.join(', ') || 'no hosts'}`;
}

/** The assets the last sync run could not apply for want of a secret (the
 *  planner words that as `missing secrets: …`; any other blocked action —
 *  an unsupported kind, say — is not a secret problem). It is the producer
 *  for the Inbox's `InboxInput.blocked`: the frontend holds only the last
 *  run's results, not the last plan, so a secret set since is not reflected
 *  until the next sync. */
export function blockedSecretKeys(run: SyncRunSummary | null): string[] {
  const keys = new Set<string>();
  for (const h of run?.hosts ?? [])
    for (const x of h.actions)
      if (x.op === 'blocked' && x.detail?.startsWith('missing secrets')) keys.add(`${x.kind}/${x.name}`);
  return [...keys];
}

export function blockedOnSecrets(run: SyncRunSummary | null): (a: { kind: string; name: string }) => boolean {
  const keys = new Set(blockedSecretKeys(run));
  return (a) => keys.has(`${a.kind}/${a.name}`);
}

/** "3 min ago" from Unix seconds. */
export function ago(secs: number, now: number): string {
  const d = Math.max(0, now - secs);
  if (d < 60) return 'just now';
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
}
