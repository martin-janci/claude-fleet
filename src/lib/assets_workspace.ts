// Assets M5: the workspace's own reads — the catalogs behind the footer's
// chips, the cards behind the Inbox's Proposed rows, the layers behind
// `layer:`, a catalog's repo status, an asset's History — and the small pure
// helpers the views share. Mirrors service/catalog/{catalogs, changesets/mod,
// repo}.rs and the commands of commands/assets.rs (Rulings R13).
import { get, writable } from 'svelte/store';
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

export type CardKind = 'bootstrap' | 'new' | 'drift' | 'rollout' | 'layer';
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
  /** The catalogs the card's apply commits to (M6). */
  catalogs?: string[];
  /** A drift card whose copy has since changed or gone (M6). */
  withdrawn?: boolean;
  /** The hosts whose copies its apply held back (final review I1); absent from an older hub. */
  held_hosts?: string[];
  /** `catalog.auto` synced it on its own (redesign 8.7); absent from an older hub. */
  auto?: boolean;
}

/** Why a host's copy was held back by a card's apply (R1). */
export type HeldWhy = 'edited' | 'unverified' | 'differs';
export interface HeldLine { kind: string; name: string; why: HeldWhy }
/** What an applied item left behind. It is per HOST: two items on one host share the same held lines. */
export interface ItemOutcome { held?: HeldLine[]; note?: string | null }
export interface ItemParams {
  from_host?: string; layer?: string; member?: string; host?: string; axis?: string;
  scope?: string; hash?: string; reason?: string; assets?: string[]; harness?: string;
  to?: string; members?: string[]; description?: string;
}

/** One card item, as `changesets { list, id }` answers it. */
export interface ItemView {
  position: number;
  grp: string;
  catalog?: string | null;
  kind: string;
  name: string;
  action: 'import' | 'assign_layer' | 'set_scope' | 'hide' | 'take_host' | 'restore' | 'sync'
    | 'create_layer' | 'rename_layer' | 'move_member';
  params: ItemParams;
  decider: 'rule' | 'jev' | 'haiku' | 'person';
  state: 'pending' | 'applied' | 'skipped' | 'rejected';
  outcome?: ItemOutcome | null;
}

export interface ChangesetView {
  id: number;
  kind: CardKind;
  summary: string;
  state: CardState;
  created_at: number;
  /** Unix milliseconds. */
  applied_at?: number | null;
  error?: string | null;
  commits: Record<string, string>;
  undoable: boolean;
  catalogs?: string[];
  withdrawn?: boolean;
  items: ItemView[];
}

export type LayerChange =
  | { op: 'create'; catalog?: string; layer: string; axis?: 'role' | 'context'; description?: string; members?: string[] }
  | { op: 'rename'; catalog?: string; layer: string; to: string }
  | { op: 'move'; catalog?: string; member: string; layer: string; to: string };

export interface Provenance { introduced_by: string; overridden_by?: string[]; catalog: string }
export interface ResolutionView {
  provenance: Record<string, Provenance>;
  excluded: Record<string, string>;
  refused: { kind: string; name: string; reason: string; catalog?: string | null }[];
  /** `[kind, name]` pairs. */
  withheld: [string, string][];
  /** Catalog → why it was held back. */
  held_back: Record<string, string>;
  assets: { kind: string; name: string; version: string }[];
}

/** `path` keeps the planner's `~/…` form. A file holding a `${SECRET}` placeholder is never read, so both texts are absent. */
export interface DriftFile {
  path: string;
  catalog?: string | null;
  host?: string | null;
  binary?: boolean;
  truncated?: boolean;
  secret?: boolean;
}
export interface DriftDiff { host_alias: string; harness: string; files: DriftFile[]; merges_only?: boolean }

export interface CommitEntry { sha: string; at: number; author: string; subject: string }

export interface LayerDef { name: string; axis: 'role' | 'context'; description?: string; extends?: string; members?: string[] }
export interface HostLayerRow { host_alias: string; catalog_id?: number; layer_name: string; axis: string; position: number; active: boolean }
export interface LayerListing { layers: LayerDef[]; hosts: HostLayerRow[] }

/** The rail's views (R15); Layers and Hosts join in M6. */
export type WorkspaceView = 'inbox' | 'layers' | 'hosts' | 'library';
export const RAIL_VIEWS: readonly { id: WorkspaceView; label: string }[] = [
  { id: 'inbox', label: 'Inbox' },
  { id: 'layers', label: 'Layers' },
  { id: 'hosts', label: 'Hosts' },
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
  void loadOpenCardViews(r.ok ? r.value : null);
  return r;
}

export async function loadLayers(): Promise<Result<LayerListing>> {
  const r = await invokeCmd<LayerListing>('catalog_list_layers');
  layerListing.set(r.ok ? r.value : null);
  return r;
}

export const cardViews = writable<Record<number, ChangesetView>>({});
export const layersByCatalog = writable<Record<string, LayerListing> | null>(null);

export function getChangeset(id: number) { return invokeCmd<ChangesetView>('catalog_get_changeset', { args: { id } }); }
export function applyChangeset(id: number, positions?: number[] | null) {
  return invokeCmd<ChangesetView>('catalog_apply_changeset', { args: positions ? { id, positions } : { id } });
}
export function undoChangeset(id: number) { return invokeCmd<ChangesetView>('catalog_undo_changeset', { args: { id } }); }
export function dismissChangeset(id: number) { return invokeCmd<ChangesetView>('catalog_dismiss_changeset', { args: { id } }); }
export function rejectItems(id: number, positions: number[]) {
  return invokeCmd<ChangesetView>('catalog_reject_changeset_items', { args: { id, positions } });
}
export function proposeChangesets() { return invokeCmd<ChangesetSummary[]>('catalog_propose_changesets'); }
export function proposeLayerChange(change: LayerChange) {
  return invokeCmd<ChangesetView>('catalog_propose_layer_change', { args: { change } });
}
export function admitCatalog(host_alias: string, catalog: string) {
  return invokeCmd<string[]>('catalog_admit_catalog', { args: { host_alias, catalog } });
}
export function unadmitCatalog(host_alias: string, catalog: string) {
  return invokeCmd<string[]>('catalog_unadmit_catalog', { args: { host_alias, catalog } });
}
export function listLayersIn(name: string) { return invokeCmd<LayerListing>('catalog_list_layers_in', { args: { name } }); }
export function hostProvenance(host_alias: string) {
  return invokeCmd<ResolutionView>('catalog_host_provenance', { args: { host_alias } });
}
export function driftDiff(a: { host_alias: string; kind: string; name: string; harness?: string | null; catalog?: string | null }) {
  return invokeCmd<DriftDiff>('catalog_drift_diff', {
    args: { ...a, harness: a.harness ?? null, catalog: a.catalog && a.catalog !== PERSONAL ? a.catalog : null },
  });
}

let cardViewsGen = 0;

/** R12: every open card in full (cards are few; the views need items), and
 *  every applied card that held hosts back (final review I1: the Inbox shows
 *  its held lines and Sync buttons). Overlapping loads can finish out of
 *  order, so only the newest run's result stands; a card whose fetch failed
 *  keeps its previous view. */
export async function loadOpenCardViews(cards: ChangesetSummary[] | null): Promise<void> {
  const gen = ++cardViewsGen;
  const open = (cards ?? []).filter((c) => isOpenCard(c) || heldBack(c));
  const got = await Promise.all(open.map((c) => getChangeset(c.id)));
  if (gen !== cardViewsGen) return;
  const prev = get(cardViews);
  const next: Record<number, ChangesetView> = {};
  got.forEach((r, i) => {
    const id = open[i].id;
    if (r.ok) next[id] = r.value;
    else if (prev[id]) next[id] = prev[id];
  });
  cardViews.set(next);
}

/** R17: each loaded catalog's layers, by name. */
export async function loadAllLayers(statuses: CatalogStatus[] | null): Promise<void> {
  const loaded = (statuses ?? []).filter((s) => s.state === 'loaded');
  const got = await Promise.all(loaded.map((s) => listLayersIn(s.name)));
  const next: Record<string, LayerListing> = {};
  got.forEach((r, i) => { if (r.ok) next[loaded[i].name] = r.value; });
  layersByCatalog.set(statuses ? next : null);
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

/** An applied card whose apply held some hosts' copies back (final review I1). */
export function heldBack(c: ChangesetSummary): boolean {
  return c.state === 'applied' && (c.held_hosts?.length ?? 0) > 0;
}

/** What a list row is — one string per row (`data-row-key`), shared by the
 *  views, the keyboard and the Inspector. */
export type Selection =
  | { type: 'asset'; catalog: string; kind: string; name: string }
  | { type: 'identity'; kind: string; name: string }
  | { type: 'orphan'; kind: string; name: string }
  | { type: 'card'; id: number }
  | { type: 'layer'; catalog: string; name: string }
  | { type: 'host'; alias: string };

export function keyOf(s: Selection): string {
  switch (s.type) {
    case 'asset':
      return `asset:${s.catalog}:${s.kind}/${s.name}`;
    case 'identity':
    case 'orphan':
      return `${s.type}:${s.kind}/${s.name}`;
    case 'card':
      return `card:${s.id}`;
    case 'layer':
      return `layer:${s.catalog}:${s.name}`;
    case 'host':
      return `host:${s.alias}`;
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
  if (type === 'host') return body ? { type, alias: body } : null;
  if (type === 'layer') {
    const j = body.indexOf(':');
    return j > 0 && j < body.length - 1 ? { type, catalog: body.slice(0, j), name: body.slice(j + 1) } : null;
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

/** The assets the last sync run could not apply for want of a secret, as
 *  `catalog:kind/name` (a result without a catalog, from a hub before M6, is
 *  personal's). The planner words that as `missing secrets: …`; any other
 *  blocked action — an unsupported kind, say — is not a secret problem. It
 *  is the producer for the Inbox's `InboxInput.blocked`: the frontend holds
 *  only the last run's results, not the last plan, so a secret set since is
 *  not reflected until the next sync. */
export function blockedSecretKeys(run: SyncRunSummary | null): string[] {
  const keys = new Set<string>();
  for (const h of run?.hosts ?? [])
    for (const x of h.actions)
      if (x.op === 'blocked' && x.detail?.startsWith('missing secrets')) keys.add(`${x.catalog ?? PERSONAL}:${x.kind}/${x.name}`);
  return [...keys];
}

export function blockedOnSecrets(run: SyncRunSummary | null): (a: { kind: string; name: string; catalog?: string | null }) => boolean {
  const keys = new Set(blockedSecretKeys(run));
  return (a) => keys.has(`${a.catalog ?? PERSONAL}:${a.kind}/${a.name}`);
}

/** "3 min ago" from Unix seconds. */
export function ago(secs: number, now: number): string {
  const d = Math.max(0, now - secs);
  if (d < 60) return 'just now';
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
}
