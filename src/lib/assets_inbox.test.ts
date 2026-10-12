import { describe, it, expect } from 'vitest';
import { buildInbox, hostOrderOf, keepCard, lastScanOf, sentence } from './assets_inbox';
import { parseQuery } from './assets_query';
import type { AssetListing } from './assets';
import type { ChangesetSummary } from './assets_workspace';

const listing: AssetListing = {
  head: 'abc', loaded_at: 1, problems: [],
  assets: [
    { kind: 'skill', name: 'edited', version: '1', description: '', tags: [], catalog: 'personal', scope: 'shared',
      hosts: [{ host_alias: 'trn', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'behind', version: '1', description: '', tags: [], catalog: 'papayapos',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'catalog' }] },
    { kind: 'skill', name: 'unknown', version: '1', description: '', tags: [],
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted' }] },
    { kind: 'skill', name: 'fine', version: '1', description: '', tags: [],
      hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }, { host_alias: 'oci', harness: 'claude', state: 'missing' }] },
  ],
  unmanaged: [
    { host_alias: 'mefistos', harness: 'claude', kind: 'skill', name: 'ghost', state: 'orphan', catalog_hash: null, host_hash: null, scanned_at: 50, managed: true },
  ],
  identities: [
    { kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }], signature: 'oci', variants: 1, class: 'normal', reason: null },
    { kind: 'mcp_server', name: 'jira', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }], signature: 'local', variants: 1, class: 'needs_person', reason: 'carries a secret' },
    { kind: 'hook', name: 'stop', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }], signature: 'local', variants: 1, class: 'fleet_internal', reason: null },
  ],
};
const cards: ChangesetSummary[] = [
  { id: 4, kind: 'bootstrap', summary: 'Adopt 3 as 1 layers', state: 'proposed', created_at: 1, groups: { core: 3 }, pending: 3 },
  { id: 3, kind: 'new', summary: 'old', state: 'applied', created_at: 1 },
];
const order = hostOrderOf(['trn', 'local', 'oci', 'mefistos']);

describe('buildInbox', () => {
  const inbox = buildInbox({ listing, cards, order, stale: new Set(['trn']) });
  const names = (s: keyof typeof inbox.sections) => inbox.sections[s].map((r) => r.name);

  it('puts every row in the section that says what to do next', () => {
    expect(names('cards')).toEqual(['Adopt 3 as 1 layers']);
    expect(names('applied')).toEqual([]);
    expect(names('needs')).toEqual(['jira', 'ghost']);
    expect(names('drifted')).toEqual(['edited', 'unknown']);
    expect(names('behind')).toEqual(['behind']);
    expect(names('fresh')).toEqual(['fresh']);
    expect(names('insync')).toEqual(['fine']);
    expect(inbox.hidden).toBe(1);
    expect(inbox.needCount).toBe(1 + 2 + 2);
  });

  it('says why, with the side that moved', () => {
    const why = (s: keyof typeof inbox.sections, n: string) => inbox.sections[s].find((r) => r.name === n)?.why;
    expect(why('drifted', 'edited')).toBe('Edited on trn');
    expect(why('drifted', 'unknown')).toBe('Differs on oci');
    expect(why('behind', 'behind')).toBe('Behind the catalog on oci');
    expect(why('needs', 'ghost')).toBe('Left on mefistos after the catalog dropped it');
    expect(why('needs', 'jira')).toBe('carries a secret');
    expect(why('fresh', 'fresh')).toBe('Found on oci');
  });

  it('keys rows by what they are, and draws a dot per host in order', () => {
    expect(order).toEqual(['local', 'mefistos', 'oci', 'trn']);
    expect(inbox.sections.behind[0].key).toBe('asset:papayapos:skill/behind');
    expect(inbox.sections.fresh[0].key).toBe('identity:skill/fresh');
    expect(inbox.sections.cards[0].key).toBe('card:4');
    expect(inbox.sections.insync[0].dots).toEqual({ local: 'in_sync', mefistos: 'na', oci: 'missing', trn: 'na' });
    expect(inbox.sections.drifted[0].dots.trn).toBe('stale');
  });
});

describe('sentence', () => {
  it('is a sentence about the fleet', () => {
    const inbox = buildInbox({ listing, cards, order, stale: new Set() });
    expect(sentence(inbox, { reachable: 4, total: 5, lastScan: 50, now: 50 + 180 })).toEqual({
      text: '5 need you · 1 behind the catalog · 1 new on hosts',
      sub: '4/5 hosts · scan 3 min ago',
    });
    const quiet = buildInbox({ listing: { ...listing, assets: [listing.assets[3]], unmanaged: [], identities: [] }, cards: [], order, stale: new Set() });
    expect(sentence(quiet, { reachable: 5, total: 5, lastScan: null, now: 1 }).text).toBe('Fleet converged');
  });
  it('takes the newest scan of the inventory and the listing', () => {
    expect(lastScanOf(listing, [])).toBe(50);
    expect(lastScanOf(null, [])).toBeNull();
  });
});

describe('rows beyond the brief', () => {
  it('shows a behind-the-catalog copy whatever the layer rollout says, and words a blocked one as a difference', () => {
    const inbox = buildInbox({ listing, cards: [], order, stale: new Set() });
    expect(inbox.sections.behind.map((r) => r.name)).toEqual(['behind']);
    const blocked = buildInbox({ listing, cards: [], order, stale: new Set(), blocked: (a) => a.name === 'behind' });
    expect(blocked.sections.behind[0].why).toBe('Differs from the catalog on oci');
  });
  it('a mixed asset is Drifted, and still says where it is behind', () => {
    const mixed: AssetListing = { ...listing, assets: [{ ...listing.assets[0], hosts: [
      { host_alias: 'trn', harness: 'claude', state: 'drifted', drift_side: 'host' },
      { host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'catalog' },
    ] }], unmanaged: [], identities: [] };
    const inbox = buildInbox({ listing: mixed, cards: [], order, stale: new Set() });
    expect(inbox.sections.drifted[0].why).toBe('Edited on trn · Behind the catalog on oci');
    expect(inbox.sections.behind).toEqual([]);
  });
  it('draws an identity whose copies differ with the odd host marked, from the one shared parser', () => {
    const odd: AssetListing = { ...listing, assets: [], unmanaged: [], identities: [
      { kind: 'skill', name: 'x', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'a' }, { host_alias: 'oci', harness: 'claude', host_hash: 'b' }],
        signature: 'local,oci', variants: 2, class: 'needs_person', reason: 'copies differ on oci' },
    ] };
    const inbox = buildInbox({ listing: odd, cards: [], order, stale: new Set(['local']) });
    expect(inbox.sections.needs[0].dots).toEqual({ local: 'stale', mefistos: 'absent', oci: 'differs', trn: 'absent' });
    expect(inbox.sections.needs[0].why).toBe('copies differ on oci');
  });
  it('lists only open cards (proposed, failed) and carries a failure as the why', () => {
    const c: ChangesetSummary[] = [
      { id: 1, kind: 'rollout', summary: 'Roll out to core', state: 'failed', created_at: 1, error: 'git push rejected' },
      { id: 2, kind: 'new', summary: 'dismissed', state: 'dismissed', created_at: 1 },
      { id: 3, kind: 'drift', summary: 'undone', state: 'undone', created_at: 1 },
    ];
    const inbox = buildInbox({ listing: { ...listing, assets: [], unmanaged: [], identities: [] }, cards: c, order, stale: new Set() });
    expect(inbox.sections.cards.map((r) => [r.key, r.why])).toEqual([['card:1', 'git push rejected']]);
    expect(buildInbox({ listing, cards: null, order, stale: new Set() }).sections.cards).toEqual([]);
  });
  it('lists an applied, undoable card under applied, and counts it as nothing that needs you', () => {
    const c: ChangesetSummary[] = [
      { id: 5, kind: 'new', summary: 'New on oci', state: 'applied', undoable: true, created_at: 1, applied_at: 2 },
      { id: 6, kind: 'new', summary: 'older', state: 'applied', undoable: false, created_at: 1, applied_at: 2 },
      { id: 7, kind: 'rollout', summary: 'Roll out', state: 'proposed', created_at: 1 },
    ];
    const inbox = buildInbox({ listing: { ...listing, assets: [], unmanaged: [], identities: [] }, cards: c, order, stale: new Set() });
    expect(inbox.sections.applied.map((r) => r.key)).toEqual(['card:5']);
    expect(inbox.sections.cards.map((r) => r.key)).toEqual(['card:7']);
    expect(inbox.needCount).toBe(1);
  });
  it('keeps an applied card that held hosts back under applied, so its held lines and Sync buttons show', () => {
    const c: ChangesetSummary[] = [
      { id: 8, kind: 'rollout', summary: 'Roll out core to oci', state: 'applied', undoable: false, created_at: 1, applied_at: 2, held_hosts: ['oci'] },
      { id: 9, kind: 'rollout', summary: 'Roll out core to trn', state: 'applied', undoable: false, created_at: 1, applied_at: 2, held_hosts: [] },
    ];
    const inbox = buildInbox({ listing: { ...listing, assets: [], unmanaged: [], identities: [] }, cards: c, order, stale: new Set() });
    expect(inbox.sections.applied.map((r) => r.key)).toEqual(['card:8']);
    expect(inbox.needCount).toBe(0);
  });
  it('shows what catalog.auto synced on its own under applied, with its note, needing nothing (8.7)', () => {
    const note = 'Synced on its own: catalog.auto is on.';
    const c: ChangesetSummary[] = [
      { id: 10, kind: 'rollout', summary: 'Synced core to oci', state: 'applied', undoable: false, created_at: 1, applied_at: 2, error: note, auto: true },
      { id: 11, kind: 'rollout', summary: 'Roll out core to trn', state: 'applied', undoable: false, created_at: 1, applied_at: 2 },
    ];
    const inbox = buildInbox({ listing: { ...listing, assets: [], unmanaged: [], identities: [] }, cards: c, order, stale: new Set() });
    expect(inbox.sections.applied.map((r) => [r.key, r.name, r.why])).toEqual([['card:10', 'Synced core to oci', note]]);
    expect(inbox.needCount).toBe(0);
  });
  it('gives each row the query row the shared search reads, with the layers of its asset', () => {
    const inbox = buildInbox({ listing, cards: [], order, stale: new Set(), layersOf: (a) => (a.name === 'fine' ? ['core'] : []) });
    expect(inbox.sections.insync[0].query).toMatchObject({ kind: 'skill', name: 'fine', catalog: 'personal', layers: ['core'] });
    expect(inbox.sections.behind[0].query.catalog).toBe('papayapos');
  });
  it('takes the newest scan across the inventory and the listing', () => {
    expect(lastScanOf(listing, [{ ...listing.unmanaged[0], scanned_at: 90 }])).toBe(90);
  });
  it('words a never-scanned fleet and a lone need in the singular', () => {
    const one = buildInbox({ listing: { ...listing, assets: [], unmanaged: [], identities: [listing.identities![1]] }, cards: [], order, stale: new Set() });
    expect(sentence(one, { reachable: 1, total: 1, lastScan: null, now: 1 })).toEqual({ text: '1 needs you', sub: '1/1 hosts · never scanned' });
  });
});

describe('keepCard', () => {
  const c = (o: Partial<ChangesetSummary> = {}): ChangesetSummary => ({ id: 1, kind: 'new', summary: 'New on oci: skill/w', state: 'proposed', created_at: 1, catalogs: ['personal'], ...o });
  it('a catalog: token keeps a card of that catalog and hides one of another (R2)', () => {
    expect(keepCard(parseQuery('catalog:personal'), c())).toBe(true);
    expect(keepCard(parseQuery('catalog:acme'), c())).toBe(false);
    expect(keepCard(parseQuery('catalog:acme,personal'), c({ catalogs: ['personal', 'x'] }))).toBe(true);
  });
  it('a card that names no catalog cannot be excluded by one', () => {
    expect(keepCard(parseQuery('catalog:acme'), c({ catalogs: undefined }))).toBe(true);
    expect(keepCard(parseQuery('catalog:acme'), c({ catalogs: [] }))).toBe(true);
  });
  it('free words still match the sentence, and the other tokens never hide a card', () => {
    expect(keepCard(parseQuery('host:nowhere kind:hook state:orphan layer:x scope:org'), c())).toBe(true);
    expect(keepCard(parseQuery('ONCI'), c())).toBe(false);
    expect(keepCard(parseQuery('SKILL/W'), c())).toBe(true);
  });
});
