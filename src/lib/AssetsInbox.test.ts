import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import AssetsInbox from './AssetsInbox.svelte';
import { buildInbox } from './assets_inbox';
import { parseQuery } from './assets_query';
import type { AssetListing } from './assets';

const listing: AssetListing = {
  head: 'abc', loaded_at: 1, problems: [], unmanaged: [],
  assets: [
    { kind: 'skill', name: 'edited', version: '1', description: '', tags: [], catalog: 'personal', scope: 'shared',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'quiet', version: '1', description: '', tags: [], catalog: 'papayapos',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'in_sync' }] },
  ],
  identities: [
    { kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }], signature: 'oci', variants: 1, class: 'normal', reason: null },
  ],
};
const card = { id: 7, kind: 'bootstrap' as const, summary: 'Adopt 1 as 1 layers', state: 'failed' as const, created_at: 1, error: 'core: import failed', groups: { core: 1 } };
const inbox = buildInbox({ listing, cards: [card], order: ['local', 'oci'], stale: new Set() });
const props = (over = {}) => ({ inbox, order: ['local', 'oci'], selectedKey: null, query: parseQuery(''), onselect: vi.fn(), ...over });

describe('AssetsInbox', () => {
  it('shows proposed cards first, then the sections, with in sync folded to one line', async () => {
    render(AssetsInbox, props());
    const headers = Array.from(document.querySelectorAll('[data-testid^="inbox-section-"]')).map((h) => h.getAttribute('data-testid'));
    expect(headers).toEqual(['inbox-section-cards', 'inbox-section-drifted', 'inbox-section-fresh']);
    const card = screen.getByTestId('inbox-row-card:7');
    expect(card.textContent).toContain('Adopt 1 as 1 layers');
    expect(card.textContent).toContain('failed');
    expect(card.textContent).toContain('core: import failed');
    expect(card.querySelectorAll('button')).toHaveLength(0);
    expect(screen.queryByTestId('inbox-row-asset:papayapos:skill/quiet')).toBeNull();
    const fold = screen.getByTestId('inbox-insync-toggle');
    expect(fold.getAttribute('aria-expanded')).toBe('false');
    expect(fold.textContent).toContain('In sync');
    await fireEvent.click(fold);
    expect(screen.getByTestId('inbox-row-asset:papayapos:skill/quiet').textContent).toContain('papayapos');
  });

  it('says why, with the scope badge and one dot per host', () => {
    render(AssetsInbox, props());
    const row = screen.getByTestId('inbox-row-asset:personal:skill/edited');
    expect(row.textContent).toContain('Edited on oci');
    expect(row.textContent).toContain('shared');
    expect(row.querySelector('.strip')?.getAttribute('aria-label')).toBe('local: not here, oci: differs');
  });

  it('selects a row by its key, and marks the selected one', async () => {
    const onselect = vi.fn();
    render(AssetsInbox, props({ onselect, selectedKey: 'identity:skill/fresh' }));
    expect(screen.getByTestId('inbox-row-identity:skill/fresh').getAttribute('aria-current')).toBe('true');
    await fireEvent.click(screen.getByTestId('inbox-row-asset:personal:skill/edited'));
    expect(onselect).toHaveBeenCalledWith('asset:personal:skill/edited');
  });

  it('filters by the query; tokens leave the cards alone, free words filter them', () => {
    render(AssetsInbox, props({ query: parseQuery('kind:skill fresh') }));
    expect(screen.getByTestId('inbox-row-identity:skill/fresh')).toBeTruthy();
    expect(screen.queryByTestId('inbox-row-asset:personal:skill/edited')).toBeNull();
    expect(screen.queryByTestId('inbox-row-card:7')).toBeNull();
  });

  it('says "No matches." when the query filters everything out, not that nothing needs you', () => {
    render(AssetsInbox, props({ query: parseQuery('zzz-nothing') }));
    expect(screen.getByTestId('inbox-quiet').textContent).toBe('No matches.');
  });

  it('is quiet when nothing needs you', () => {
    const calm = buildInbox({ listing: { ...listing, assets: [listing.assets[1]], identities: [] }, cards: [], order: ['oci'], stale: new Set() });
    render(AssetsInbox, props({ inbox: calm }));
    expect(screen.getByTestId('inbox-quiet').textContent).toBe('Nothing needs you.');
  });
});

describe('AssetsInbox — cards are filtered by free words and catalog only (T7 ruling)', () => {
  it('host:, kind:, state:, layer: and scope: tokens never hide a card', () => {
    render(AssetsInbox, props({ query: parseQuery('host:nowhere kind:hook state:orphan layer:x scope:org') }));
    expect(screen.getByTestId('inbox-row-card:7')).toBeTruthy();
  });

  it('free words match the card sentence, case-insensitively', () => {
    const { unmount } = render(AssetsInbox, props({ query: parseQuery('ADOPT') }));
    expect(screen.getByTestId('inbox-row-card:7')).toBeTruthy();
    unmount();
    render(AssetsInbox, props({ query: parseQuery('rollout') }));
    expect(screen.queryByTestId('inbox-row-card:7')).toBeNull();
  });

  it('a catalog: token never hides a card either (the summary does not name its catalogs yet)', () => {
    const { unmount } = render(AssetsInbox, props({ query: parseQuery('catalog:personal') }));
    expect(screen.getByTestId('inbox-row-card:7')).toBeTruthy();
    unmount();
    render(AssetsInbox, props({ query: parseQuery('catalog:acme') }));
    expect(screen.getByTestId('inbox-row-card:7')).toBeTruthy();
  });

  it('a card is a read-only row: selectable and marked, no apply, undo or dismiss', async () => {
    const onselect = vi.fn();
    render(AssetsInbox, props({ onselect, selectedKey: 'card:7' }));
    const row = screen.getByTestId('inbox-row-card:7');
    expect(row.tagName).toBe('BUTTON');
    expect(row.getAttribute('aria-current')).toBe('true');
    expect(row.getAttribute('data-row-key')).toBe('card:7');
    await fireEvent.click(row);
    expect(onselect).toHaveBeenCalledWith('card:7');
    expect(screen.queryByText(/apply|undo|dismiss|skip/i)).toBeNull();
  });
});

describe('AssetsInbox — every section, and what each row offers', () => {
  const full: AssetListing = {
    head: 'abc', loaded_at: 1, problems: [],
    assets: [
      { kind: 'skill', name: 'lag', version: '1', description: '', tags: [], catalog: 'personal',
        hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'catalog' }] },
      { kind: 'skill', name: 'ok', version: '1', description: '', tags: [], catalog: 'personal',
        hosts: [{ host_alias: 'oci', harness: 'claude', state: 'in_sync' }] },
      { kind: 'skill', name: 'ok2', version: '1', description: '', tags: [], catalog: 'personal',
        hosts: [{ host_alias: 'oci', harness: 'claude', state: 'in_sync' }] },
    ],
    unmanaged: [
      { host_alias: 'oci', harness: 'claude', kind: 'agent', name: 'left', state: 'orphan', scanned_at: 1 } as never,
    ],
    identities: [
      { kind: 'skill', name: 'split', signature: 'a', variants: 2, class: 'needs_person', reason: 'copies differ on oci',
        hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'a' }, { host_alias: 'oci', harness: 'claude', host_hash: 'b' }] },
      { kind: 'skill', name: 'fresh', signature: 'a', variants: 1, class: 'normal', reason: null,
        hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }] },
      { kind: 'hook', name: 'internal', signature: 'a', variants: 1, class: 'fleet_internal', reason: null,
        hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }] },
    ],
  };
  const big = buildInbox({ listing: full, cards: [], order: ['local', 'oci'], stale: new Set() });
  const bigProps = (over = {}) => ({ inbox: big, order: ['local', 'oci'], selectedKey: null, query: parseQuery(''), onselect: vi.fn(), onimport: vi.fn(), ...over });

  it('lists Needs you, Behind the catalog and New on hosts in order, with in sync counted', () => {
    render(AssetsInbox, bigProps());
    const headers = Array.from(document.querySelectorAll('[data-testid^="inbox-section-"]')).map((h) => h.getAttribute('data-testid'));
    expect(headers).toEqual(['inbox-section-needs', 'inbox-section-behind', 'inbox-section-fresh']);
    expect(screen.getByTestId('inbox-row-asset:personal:skill/lag').textContent).toContain('Behind the catalog on oci');
    expect(screen.getByTestId('inbox-row-orphan:agent/left').textContent).toContain('Left on oci');
    expect(screen.getByTestId('inbox-insync-toggle').textContent).toContain('2');
    expect(screen.getByTestId('assets-inbox').textContent).toContain('1 fleet internal hidden');
    expect(screen.queryByTestId('inbox-quiet')).toBeNull();
  });

  it('an unmanaged identity reuses the S1a row: reason badge, dots, and Import beside the select button', async () => {
    const onimport = vi.fn();
    const onselect = vi.fn();
    render(AssetsInbox, bigProps({ onimport, onselect }));
    const split = screen.getByTestId('inbox-row-identity:skill/split');
    expect(split.querySelector('.badge.warn')?.textContent).toBe('copies differ on oci');
    expect(split.querySelector('.strip')?.getAttribute('aria-label')).toBe('local: present, oci: differs');
    const imports = screen.getAllByText('Import');
    expect(imports).toHaveLength(2);
    expect(screen.getByLabelText('Import split')).toBe(imports[0]);
    await fireEvent.click(imports[0]);
    expect(onimport).toHaveBeenCalledWith(expect.objectContaining({ name: 'split' }));
    expect(onselect).not.toHaveBeenCalled();
  });

  it('read-only (a hub client without the grant): no Import', () => {
    render(AssetsInbox, bigProps({ readonly: true }));
    expect(screen.queryByText('Import')).toBeNull();
    expect(screen.getByTestId('inbox-row-identity:skill/fresh')).toBeTruthy();
  });

  it('a row is reachable by keyboard: a native button, not removed from the tab order', () => {
    render(AssetsInbox, bigProps());
    const row = screen.getByTestId('inbox-row-asset:personal:skill/lag');
    expect(row.tagName).toBe('BUTTON');
    expect(row.getAttribute('tabindex')).toBeNull();
    const fold = screen.getByTestId('inbox-insync-toggle');
    expect(fold.tagName).toBe('BUTTON');
  });

  it('an in-sync match the query asks for is still folded until opened, and counted by the query', async () => {
    render(AssetsInbox, bigProps({ query: parseQuery('ok2') }));
    expect(screen.getByTestId('inbox-insync-toggle').textContent).toContain('1');
    expect(screen.queryByTestId('inbox-row-asset:personal:skill/ok2')).toBeNull();
    await fireEvent.click(screen.getByTestId('inbox-insync-toggle'));
    expect(within(screen.getByTestId('assets-inbox')).getByTestId('inbox-row-asset:personal:skill/ok2')).toBeTruthy();
  });
});
