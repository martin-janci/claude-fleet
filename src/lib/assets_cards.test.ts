import { describe, it, expect } from 'vitest';
import { primaryVerb, cardOwns, cardMayApply, heldWords, olderHubWords, cardNote, coveringNewCard, mergePlans, isUndoBanner } from './assets_cards';
import type { ChangesetSummary, ChangesetView, ItemView } from './assets_workspace';
import type { SyncAction, SyncPlan } from './assets';

const item = (over: Partial<ItemView>): ItemView => ({
  position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'w', action: 'import',
  params: {}, decider: 'rule', state: 'pending', ...over,
});
const view = (over: Partial<ChangesetView>): ChangesetView => ({
  id: 1, kind: 'new', summary: 's', state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [], ...over,
});
const summary = (over: Partial<ChangesetSummary>): ChangesetSummary => ({ id: 1, kind: 'new', summary: 's', state: 'proposed', created_at: 1, ...over });

describe('primaryVerb', () => {
  it('bootstrap: adopt n as m layers, skipping needs a look', () => {
    const v = view({ kind: 'bootstrap', items: [
      item({ position: 0, grp: 'core' }), item({ position: 1, grp: 'core', name: 'x' }),
      item({ position: 2, grp: 'authoring', name: 'y' }), item({ position: 3, grp: 'needs a look', name: 'z' }),
      item({ position: 4, grp: 'core', action: 'assign_layer', kind: 'layer', name: 'core' }),
    ] });
    expect(primaryVerb(summary({ kind: 'bootstrap' }), v)).toEqual({ label: 'Adopt 3 as 2 layers', apply: true });
  });
  it('new into a layer, or review for a look card', () => {
    expect(primaryVerb(summary({}), view({ items: [item({})] }))).toEqual({ label: 'Adopt into core', apply: true });
    expect(primaryVerb(summary({}), view({ items: [item({ grp: 'needs a look' })] }))).toEqual({ label: 'Review', apply: false });
  });
  it('rollout to n hosts; drift reviews; layer applies', () => {
    const r = view({ kind: 'rollout', items: [item({ action: 'sync', kind: 'host', name: 'oci' }), item({ position: 1, action: 'sync', kind: 'host', name: 'htz' })] });
    expect(primaryVerb(summary({ kind: 'rollout' }), r)).toEqual({ label: 'Roll out to 2 hosts', apply: true });
    expect(primaryVerb(summary({ kind: 'drift' }), view({ kind: 'drift' }))).toEqual({ label: 'Review diff', apply: false });
    expect(primaryVerb(summary({ kind: 'layer' }), view({ kind: 'layer' }))).toEqual({ label: 'Apply', apply: true });
  });
  it('a closed card has no primary', () => {
    expect(primaryVerb(summary({ state: 'applied' }), null)).toBeNull();
  });
  it('without the full card, a card whose verb depends on its items applies nothing blind', () => {
    expect(primaryVerb(summary({ kind: 'bootstrap', groups: { core: 4, 'needs a look': 2 } }), null)).toEqual({ label: 'Adopt', apply: false });
    expect(primaryVerb(summary({ kind: 'new' }), null)).toEqual({ label: 'Adopt', apply: false });
    expect(primaryVerb(summary({ kind: 'rollout' }), null)).toEqual({ label: 'Roll out', apply: false });
    expect(primaryVerb(summary({ kind: 'layer' }), null)).toEqual({ label: 'Apply', apply: true });
  });
});

describe('cardMayApply mirrors the backend Additive filter', () => {
  const a = (op: SyncAction['op'], host_copy?: 'unchanged' | 'edited' | 'unverified'): SyncAction => ({
    kind: 'skill', name: 'w', op, reason: null, files: [], merges: [], backup: false, secrets: [], missing_secrets: [], catalog: 'personal', host_copy,
  });
  it('creates and adopts; updates only a verified copy; never overwrites or removes', () => {
    expect(cardMayApply(a('create'))).toBe(true);
    expect(cardMayApply(a('adopt'))).toBe(true);
    expect(cardMayApply(a('update', 'unchanged'))).toBe(true);
    expect(cardMayApply(a('update', 'unverified'))).toBe(false);
    expect(cardMayApply(a('update'))).toBe(false);
    expect(cardMayApply(a('overwrite', 'edited'))).toBe(false);
    expect(cardMayApply(a('remove'))).toBe(false);
  });
  it('owns only the card assets from its catalogs', () => {
    expect(cardOwns(a('create'), new Set(['skill/w']), new Set(['personal']))).toBe(true);
    expect(cardOwns(a('create'), new Set(['skill/x']), new Set(['personal']))).toBe(false);
    expect(cardOwns({ ...a('create'), catalog: 'acme' }, new Set(['skill/w']), new Set(['personal']))).toBe(false);
  });
});

describe('words', () => {
  it('says why a copy was held', () => {
    expect(heldWords('edited')).toBe('edited on the host');
    expect(heldWords('unverified')).toBe('synced before fleet recorded file hashes');
    expect(heldWords('differs')).toBe('differs from the catalog');
  });
  it('names an older hub only for an unknown action', () => {
    expect(olderHubWords({ code: 'E_INVALID', message: 'unknown variant `drift_diff`' }, 'show this diff')).toBe(
      'The hub is older than this desktop and cannot show this diff yet — update the hub.',
    );
    expect(olderHubWords({ code: 'E_INVALID', message: 'unknown changesets action propose_layer: list|…' }, 'propose layer changes')).toMatch(/^The hub is older/);
    expect(olderHubWords({ code: 'E_INVALID', message: 'bad scope' }, 'x')).toBeNull();
  });
  it('notes what applying a catalog card does', () => {
    expect(cardNote(view({ kind: 'bootstrap', catalogs: ['personal', 'papayapos'] }))).toBe('2 commits: personal, papayapos. One Undo. No host is touched.');
    expect(cardNote(view({ kind: 'rollout' }))).toBe('Additive only: creates and adopts, updates a copy fleet wrote. Nothing is overwritten or removed.');
  });
});

describe('coveringNewCard', () => {
  it('finds the open New card that imports an identity', () => {
    const views = { 4: view({ id: 4, items: [item({ kind: 'skill', name: 'fresh' })] }) };
    const cards = [summary({ id: 4 })];
    expect(coveringNewCard(cards, views, 'skill', 'fresh')?.id).toBe(4);
    expect(coveringNewCard(cards, views, 'skill', 'other')).toBeNull();
  });
});

describe('mergePlans', () => {
  it('joins host plans for a read-only review', () => {
    const p = (host: string): SyncPlan => ({ id: host, computed_at: 1, hosts: [{ host_alias: host, harness: 'claude', status: 'planned', detail: null, actions: [] }], counts: { create: 1 } });
    const m = mergePlans([p('oci'), p('htz')]);
    expect(m.hosts.map((h) => h.host_alias)).toEqual(['oci', 'htz']);
    expect(m.counts).toEqual({ create: 2 });
  });
});

describe('isUndoBanner', () => {
  it('is an applied, undoable card', () => {
    expect(isUndoBanner(summary({ state: 'applied', undoable: true }))).toBe(true);
    expect(isUndoBanner(summary({ state: 'applied', undoable: false }))).toBe(false);
  });
});
