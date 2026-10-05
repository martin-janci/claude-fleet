// Assets M6: what a card says and offers — its primary verb (R13), its note,
// why a copy was held (R1), and the Rollout review's mirror of the
// backend's Additive filter (R15: `action_allowed` in changesets/apply.rs).
import type { ChangesetSummary, ChangesetView, HeldWhy } from './assets_workspace';
import { isOpenCard } from './assets_workspace';
import type { IpcError } from './result';
import type { SyncAction, SyncPlan } from './assets';

export const NEEDS_A_LOOK = 'needs a look';
const CATALOG_ACTIONS = new Set(['import', 'take_host', 'create_layer', 'rename_layer', 'move_member']);

export interface Verb { label: string; apply: boolean }

export function primaryVerb(card: ChangesetSummary, view: ChangesetView | null | undefined): Verb | null {
  if (!isOpenCard(card)) return null;
  switch (card.kind) {
    case 'bootstrap': {
      if (!view) return { label: 'Adopt', apply: true };
      const imports = view.items.filter((i) => i.action === 'import' && i.grp !== NEEDS_A_LOOK && i.state === 'pending');
      const layers = new Set(imports.map((i) => i.grp));
      return { label: `Adopt ${imports.length} as ${layers.size} layer${layers.size === 1 ? '' : 's'}`, apply: true };
    }
    case 'new': {
      const first = view?.items[0];
      if (!first) return { label: 'Adopt', apply: true };
      if (first.grp === NEEDS_A_LOOK) return { label: 'Review', apply: false };
      if (first.action === 'hide') return { label: 'Hide', apply: true };
      return { label: `Adopt into ${first.grp}`, apply: true };
    }
    case 'rollout': {
      const hosts = new Set((view?.items ?? []).filter((i) => i.state === 'pending').map((i) => i.name));
      return { label: view ? `Roll out to ${hosts.size} host${hosts.size === 1 ? '' : 's'}` : 'Roll out', apply: true };
    }
    case 'drift':
      return { label: 'Review diff', apply: false };
    case 'layer':
      return { label: 'Apply', apply: true };
  }
}

export function cardNote(view: ChangesetView): string {
  if (view.kind === 'rollout') return 'Additive only: creates and adopts, updates a copy fleet wrote. Nothing is overwritten or removed.';
  if (view.kind === 'drift') return 'Taking makes a commit you can undo. Restoring keeps a .fleet-bak copy on the host.';
  const cats = view.catalogs ?? [];
  const n = cats.length;
  if (n === 0) return 'Records verdicts only. No host is touched.';
  return `${n} commit${n === 1 ? '' : 's'}: ${cats.join(', ')}. One Undo. No host is touched.`;
}

export function heldWords(why: HeldWhy): string {
  switch (why) {
    case 'edited': return 'edited on the host';
    case 'unverified': return 'synced before fleet recorded file hashes';
    case 'differs': return 'differs from the catalog';
  }
}

/** A hub before M6 refuses a new action with E_INVALID naming it unknown (Global Constraints, contract). */
export function olderHubWords(e: IpcError, what: string): string | null {
  if (e.code !== 'E_INVALID') return null;
  if (!/unknown (variant|changesets action)/.test(e.message)) return null;
  return `The hub is older than this desktop and cannot ${what} yet — update the hub.`;
}

export function cardOwns(a: SyncAction, assets: Set<string>, catalogs: Set<string>): boolean {
  return assets.has(`${a.kind}/${a.name}`) && !!a.catalog && catalogs.has(a.catalog);
}

/** R15: `action_allowed(OpFilter::Additive, a)`. A moved asset's Create over an
 *  unverified old copy is held by the backend but not visible here; the
 *  card's outcome after apply is authoritative. */
export function cardMayApply(a: SyncAction): boolean {
  if (a.op === 'create' || a.op === 'adopt') return true;
  if (a.op === 'update') return a.host_copy === 'unchanged';
  return false;
}

export function coveringNewCard(
  cards: ChangesetSummary[] | null,
  views: Record<number, ChangesetView>,
  kind: string,
  name: string,
): ChangesetSummary | null {
  for (const c of cards ?? []) {
    if (c.kind !== 'new' || !isOpenCard(c)) continue;
    const v = views[c.id];
    if (v?.items.some((i) => i.kind === kind && i.name === name && i.state === 'pending')) return c;
  }
  return null;
}

/** R15: host plans joined for a read-only review (its id is never applied). */
export function mergePlans(plans: SyncPlan[]): SyncPlan {
  const counts: Record<string, number> = {};
  for (const p of plans) for (const [k, n] of Object.entries(p.counts)) counts[k] = (counts[k] ?? 0) + n;
  return { id: 'review', computed_at: Math.max(0, ...plans.map((p) => p.computed_at)), hosts: plans.flatMap((p) => p.hosts), counts };
}

export function isUndoBanner(c: ChangesetSummary): boolean {
  return c.state === 'applied' && !!c.undoable;
}

/** Whether applying the card writes a catalog (a commit), as opposed to recording verdicts or syncing hosts. */
export function catalogChanging(view: ChangesetView): boolean {
  return view.items.some((i) => CATALOG_ACTIONS.has(i.action));
}
