// Assets M5: the Inbox (spec, Workspace shell → Inbox; Rulings R16) and the
// sentence header — pure functions over the listing and the cards.
import { hostOrderOf, identitiesOf, oddHosts, type AssetIdentity, type AssetInventoryRow, type AssetListing, type AssetSummary, type HostState } from './assets';
import type { DotState } from './assets_visual';
import { ago, heldBack, isOpenCard, keyOf, PERSONAL, type ChangesetSummary } from './assets_workspace';
import { isUndoBanner } from './assets_cards';
import { rowOfAsset, rowOfIdentity, rowOfOrphan, type ParsedQuery, type QueryRow } from './assets_query';

export type InboxSection = 'cards' | 'applied' | 'needs' | 'drifted' | 'behind' | 'fresh' | 'insync';
export const SECTION_LABEL: Record<InboxSection, string> = {
  cards: 'Proposed',
  applied: 'Recently applied',
  needs: 'Needs you',
  drifted: 'Drifted',
  behind: 'Behind the catalog',
  fresh: 'New on hosts',
  insync: 'In sync',
};

export interface InboxRow {
  key: string;
  kind: string;
  /** The asset's name, or a card's sentence. */
  name: string;
  why: string;
  dots: Record<string, DotState>;
  query: QueryRow;
  asset?: AssetSummary;
  identity?: AssetIdentity;
  card?: ChangesetSummary;
}

export interface Inbox {
  sections: Record<InboxSection, InboxRow[]>;
  /** Fleet internals (`fleet_internal`, `harness_internal`): counted, not listed. */
  hidden: number;
  /** Open cards + needs you + drifted: what the rail's Inbox count says
   *  (an applied card with an Undo banner needs nothing). */
  needCount: number;
}

export interface InboxInput {
  listing: AssetListing;
  cards: ChangesetSummary[] | null;
  order: string[];
  /** Hosts the hosts store says are unreachable: their dots are stale. */
  stale: ReadonlySet<string>;
  /** A catalog asset's personal layers, for `layer:`. */
  layersOf?: (a: AssetSummary) => string[];
  /** An asset a sync plan could not apply for want of a secret: a copy that
   *  differs on the catalog's side is then not "behind" (nothing moved in
   *  the catalog), it just differs, and the row says so. */
  blocked?: (a: AssetSummary) => boolean;
}

/** `local` first, then alphabetical — the fixed dot order (one
 *  implementation, in `assets.ts`). */
export { hostOrderOf };

const words = (hosts: string[]) => hosts.join(', ');
const hostsWhere = (a: AssetSummary, pred: (s: HostState) => boolean) => [...new Set(a.hosts.filter(pred).map((s) => s.host_alias))];

/** Which side moved on a drifted managed copy (Assets M5, R4), in the
 *  Inbox's words — for every view that lists a copy per host. `null` when
 *  the side is unknown (an entry from before M5, an older hub). */
export function driftSideWords(side?: string | null): string | null {
  return side === 'host' ? 'edited on host' : side === 'catalog' ? 'behind the catalog' : null;
}

/** What happens to a copy that is only behind the catalog (nobody edited it
 *  on the host): the catalog's auto-sync brings it up, or the next Sync. */
export function behindWords(auto: boolean): string {
  return `Behind the catalog — ${auto ? 'fleet updates it automatically' : 'your next Sync updates it'}`;
}

/** One catalog asset's dots: differs over missing over in sync; a host with
 *  no row is `na`; a row on an unreachable host is `stale`. */
export function assetDots(a: AssetSummary, order: string[], stale: ReadonlySet<string>): Record<string, DotState> {
  const out: Record<string, DotState> = {};
  for (const h of order) {
    const states = a.hosts.filter((s) => s.host_alias === h).map((s) => s.state);
    let d: DotState = 'na';
    if (states.includes('drifted')) d = 'differs';
    else if (states.includes('missing')) d = 'missing';
    else if (states.includes('in_sync')) d = 'in_sync';
    if (states.length > 0 && stale.has(h)) d = 'stale';
    out[h] = d;
  }
  return out;
}

function identityDots(id: AssetIdentity, order: string[], stale: ReadonlySet<string>): Record<string, DotState> {
  const present = new Set(id.hosts.map((h) => h.host_alias));
  const odd = new Set(oddHosts(id));
  const out: Record<string, DotState> = {};
  for (const h of order) out[h] = !present.has(h) ? 'absent' : stale.has(h) ? 'stale' : odd.has(h) ? 'differs' : 'present';
  return out;
}

export function buildInbox(input: InboxInput): Inbox {
  const { listing, order, stale } = input;
  const sections: Record<InboxSection, InboxRow[]> = { cards: [], applied: [], needs: [], drifted: [], behind: [], fresh: [], insync: [] };

  for (const c of input.cards ?? []) {
    // An applied card that held hosts back stays too (final review I1): its
    // held lines and Sync {host} buttons are what the person acts on. So
    // does what `catalog.auto` synced on its own (redesign 8.7): an
    // automatic write is always seen.
    const section = isOpenCard(c) ? sections.cards : isUndoBanner(c) || heldBack(c) || c.auto ? sections.applied : null;
    section?.push({
      key: keyOf({ type: 'card', id: c.id }), kind: c.kind, name: c.summary, why: c.error ?? '',
      dots: {}, query: { kind: c.kind, name: c.summary, hosts: [] }, card: c,
    });
  }

  let hidden = 0;
  for (const id of identitiesOf(listing)) {
    if (id.class === 'fleet_internal' || id.class === 'harness_internal') {
      hidden += 1;
      continue;
    }
    const hosts = [...new Set(id.hosts.map((h) => h.host_alias))];
    const row: InboxRow = {
      key: keyOf({ type: 'identity', kind: id.kind, name: id.name }), kind: id.kind, name: id.name,
      why: id.class === 'needs_person' ? (id.reason ?? 'needs a person') : `Found on ${words(hosts)}`,
      dots: identityDots(id, order, stale), query: rowOfIdentity(id), identity: id,
    };
    (id.class === 'needs_person' ? sections.needs : sections.fresh).push(row);
  }

  const orphans = new Map<string, AssetInventoryRow[]>();
  for (const r of listing.unmanaged.filter((r) => r.state === 'orphan')) {
    const k = `${r.kind}/${r.name}`;
    orphans.set(k, [...(orphans.get(k) ?? []), r]);
  }
  for (const rows of orphans.values()) {
    const { kind, name } = rows[0];
    const hosts = [...new Set(rows.map((r) => r.host_alias))];
    const dots: Record<string, DotState> = {};
    for (const h of order) dots[h] = hosts.includes(h) ? 'differs' : 'na';
    sections.needs.push({
      key: keyOf({ type: 'orphan', kind, name }), kind, name,
      why: `Left on ${words(hosts)} after the catalog dropped it`, dots, query: rowOfOrphan(rows),
    });
  }

  for (const a of listing.assets) {
    const edited = hostsWhere(a, (s) => s.state === 'drifted' && s.drift_side === 'host');
    const unknown = hostsWhere(a, (s) => s.state === 'drifted' && !s.drift_side);
    const behind = hostsWhere(a, (s) => s.state === 'drifted' && s.drift_side === 'catalog');
    const row: InboxRow = {
      key: keyOf({ type: 'asset', catalog: a.catalog ?? PERSONAL, kind: a.kind, name: a.name }),
      kind: a.kind, name: a.name, why: '', dots: assetDots(a, order, stale),
      query: rowOfAsset(a, input.layersOf?.(a) ?? []), asset: a,
    };
    // Computed whatever `catalog.auto` says: it is the only signal for a copy
    // behind the catalog when auto is off, the layer never rolled out, or the
    // rollout was rejected.
    const behindWhy = behind.length
      ? input.blocked?.(a) ? `Differs from the catalog on ${words(behind)}` : `Behind the catalog on ${words(behind)}`
      : '';
    if (edited.length || unknown.length) {
      row.why = [edited.length ? `Edited on ${words(edited)}` : '', unknown.length ? `Differs on ${words(unknown)}` : '', behindWhy]
        .filter(Boolean)
        .join(' · ');
      sections.drifted.push(row);
    } else if (behind.length) {
      row.why = behindWhy;
      sections.behind.push(row);
    } else {
      sections.insync.push(row);
    }
  }

  return { sections, hidden, needCount: sections.cards.length + sections.needs.length + sections.drifted.length };
}

/** Whether the query keeps a card (T7 ruling, M6 R2). A card has no host,
 *  scope, layer or state of its own, so only free words (against its
 *  sentence) and `catalog:` (against the catalogs its apply commits to) can
 *  hide it. A card that names no catalog cannot be excluded by one. */
export function keepCard(query: ParsedQuery, c: ChangesetSummary): boolean {
  const cats = (c.catalogs ?? []).map((x) => x.toLowerCase());
  if (cats.length) {
    for (const t of query.tokens) if (t.key === 'catalog' && !t.values.some((v) => cats.includes(v))) return false;
  }
  return !query.text || query.text.split(' ').every((w) => c.summary.toLowerCase().includes(w));
}

/** The newest scan the window knows of (Unix seconds), or null. */
export function lastScanOf(listing: AssetListing | null, inventory: AssetInventoryRow[]): number | null {
  let best: number | null = null;
  for (const r of [...inventory, ...(listing?.unmanaged ?? [])]) {
    if (best === null || r.scanned_at > best) best = r.scanned_at;
  }
  return best;
}

export interface Sentence { text: string; sub: string }

/** The sticky header (parent spec: "a sentence: `Fleet converged · 5/5
 *  hosts · last scan 3m`, or `9 need you`"). */
export function sentence(inbox: Inbox, ctx: { reachable: number; total: number; lastScan: number | null; now: number }): Sentence {
  const n = inbox.needCount;
  const behind = inbox.sections.behind.length;
  const fresh = inbox.sections.fresh.length;
  const scan = ctx.lastScan === null ? 'never scanned' : `scan ${ago(ctx.lastScan, ctx.now)}`;
  const parts: string[] = [];
  if (n) parts.push(`${n} need${n === 1 ? 's' : ''} you`);
  if (behind) parts.push(`${behind} behind the catalog`);
  if (fresh) parts.push(`${fresh} new on hosts`);
  return { text: parts.length ? parts.join(' · ') : 'Fleet converged', sub: `${ctx.reachable}/${ctx.total} hosts · ${scan}` };
}
