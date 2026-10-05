import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetInspector from './AssetInspector.svelte';
import type { AssetListing } from './assets';
import { cardViews, type ChangesetView } from './assets_workspace';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const listing: AssetListing = {
  head: 'h', loaded_at: 1, problems: [], unmanaged: [
    { host_alias: 'oci', harness: 'claude', kind: 'skill', name: 'gone', state: 'orphan', catalog_hash: null, host_hash: null, scanned_at: 1, managed: true },
  ],
  assets: [
    { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], catalog: 'personal',
      hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'ppt', version: '2', description: 'Org skill.', tags: [], catalog: 'papayapos',
      hosts: [{ host_alias: 'trn', harness: 'claude', state: 'in_sync' }] },
  ],
  identities: [
    { kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'abcdef1234' }], signature: 'oci', variants: 1, class: 'normal', reason: null },
  ],
};
const base = {
  listing, cards: [{ id: 7, kind: 'new' as const, summary: 'New on oci: skill/fresh → core', state: 'proposed' as const, created_at: 1, groups: { core: 1 } }],
  hosts: [], order: ['oci', 'trn'], readOnly: false, canOpen: (a: { catalog?: string }) => (a.catalog ?? 'personal') === 'personal',
  autoEditKey: '', editNonce: 0, onsync: vi.fn(), ondeleted: vi.fn(), onimport: vi.fn(),
  oncard: { apply: vi.fn(), dismiss: vi.fn(), undo: vi.fn(), synchost: vi.fn() }, onreject: vi.fn(), cardBusy: false,
};
const tabLabels = () => Array.from(document.querySelectorAll('[role="tab"]')).map((t) => t.textContent);

const view7: ChangesetView = { id: 7, kind: 'new', summary: 'New on oci: skill/fresh → core', state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
  { position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'fresh', action: 'import', params: {}, decider: 'rule', state: 'pending' },
] };

beforeEach(() => {
  cardViews.set({});
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'catalog_get_asset') return { asset: { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], body: '# b' }, previews: [], hosts: [] };
    if (cmd === 'catalog_asset_history') return [{ sha: 'abcdef1234567', at: 1, author: 'Martin', subject: 'edit w' }];
    throw { code: 'E_TEST', message: cmd };
  });
});

describe('AssetInspector', () => {
  it('says what to select when nothing is', () => {
    render(AssetInspector, { ...base, selectedKey: null });
    expect(screen.getByText('Select an asset.')).toBeTruthy();
  });

  it('says so when the selected row is no longer listed', () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/vanished' });
    expect(screen.getByTestId('inspector-empty').textContent).toContain('no longer listed');
  });

  it('a personal asset: Overview, Source, Hosts, History over one AssetDetail', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    expect(tabLabels()).toEqual(['Overview', 'Source', 'Hosts', 'History']);
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('edit w'));
    expect(invoke).toHaveBeenCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'w', catalog: null } });
    expect(screen.getByTestId('inspector-history').textContent).toContain('abcdef1');
    // Back to Overview: the same instance, no second read.
    await fireEvent.click(screen.getByTestId('inspector-tab-overview'));
    expect(screen.getByTestId('asset-detail-title')).toBeTruthy();
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_get_asset')).toHaveLength(1);
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_asset_history')).toHaveLength(1);
  });

  it('each tab shows its own section of the one detail', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    await screen.findByTestId('asset-detail-title');
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.queryByTestId('asset-detail-title')).toBeNull();
    await fireEvent.click(screen.getByTestId('inspector-tab-source'));
    expect(screen.queryByTestId('asset-detail-title')).toBeNull();
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_get_asset')).toHaveLength(1);
  });

  it('Left/Right move between tabs', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    const overview = screen.getByTestId('inspector-tab-overview');
    await fireEvent.keyDown(overview, { key: 'ArrowRight' });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(screen.getByTestId('inspector-tab-source'), { key: 'ArrowLeft' });
    expect(overview.getAttribute('aria-selected')).toBe('true');
  });

  it('an asset this window cannot open: a summary, never a read of the asset', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    expect(tabLabels()).toEqual(['Overview', 'Hosts', 'History']);
    expect(screen.getByTestId('inspector-summary').textContent).toContain('Org skill.');
    expect(screen.getByTestId('inspector-summary').textContent).toContain('papayapos');
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('inspector-hosts').textContent).toContain('trn: in sync');
    expect(invoke.mock.calls.some((c) => c[0] === 'catalog_get_asset')).toBe(false);
  });

  it('an org asset\'s History asks for that catalog', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('edit w'));
    expect(invoke).toHaveBeenCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'ppt', catalog: 'papayapos' } });
  });

  it('History: a client without a grant gets a friendly refusal', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_asset_history') throw { code: 'E_FORBIDDEN', message: 'catalog_admin asset_history needs a grant (fleet-hub client grant)' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('no grant on catalog papayapos'));
  });

  it('History: an older hub that does not know the action says to update it', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_asset_history') throw { code: 'E_INVALID', message: 'unknown variant `asset_history`, expected one of …' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('update the hub'));
  });

  it('History: an empty log says so', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_asset_history') return [];
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('No commits'));
  });

  it('read-only: no History, and Hosts says which side moved', async () => {
    render(AssetInspector, { ...base, readOnly: true, selectedKey: 'asset:personal:skill/w' });
    expect(tabLabels()).toEqual(['Overview', 'Hosts']);
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('inspector-hosts').textContent).toContain('oci: drifted — edited on host');
    expect(invoke.mock.calls.some((c) => c[0] === 'catalog_get_asset')).toBe(false);
  });

  it('an unmanaged identity: where it is, and Import', async () => {
    const onimport = vi.fn();
    render(AssetInspector, { ...base, onimport, selectedKey: 'identity:skill/fresh' });
    expect(tabLabels()).toEqual(['Overview', 'Hosts']);
    expect(screen.getByTestId('inspector-summary').textContent).toContain('oci');
    await fireEvent.click(screen.getByTestId('inspector-import'));
    expect(onimport).toHaveBeenCalledWith(expect.objectContaining({ name: 'fresh' }));
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('inspector-hosts').textContent).toContain('abcdef1');
  });

  it('read-only: an identity has no Import', () => {
    render(AssetInspector, { ...base, readOnly: true, selectedKey: 'identity:skill/fresh' });
    expect(screen.queryByTestId('inspector-import')).toBeNull();
  });

  it('an orphan: what will happen to it, Overview only', () => {
    render(AssetInspector, { ...base, selectedKey: 'orphan:skill/gone' });
    expect(tabLabels()).toEqual(['Overview']);
    expect(screen.getByTestId('inspector-summary').textContent).toContain('oci');
    // Final review minor 7: nothing automatic removes it, only a person's Sync.
    expect(screen.getByTestId('inspector-summary').textContent).toContain('Only your Sync of those hosts removes it');
  });

  it('a card: its sentence and groups, read only', () => {
    render(AssetInspector, { ...base, selectedKey: 'card:7' });
    const s = screen.getByTestId('inspector-summary');
    expect(s.textContent).toContain('New on oci: skill/fresh → core');
    expect(s.textContent).toContain('core');
    expect(s.querySelectorAll('button')).toHaveLength(0);
  });

  it('a selected card shows its items and the reject ✕', async () => {
    cardViews.set({ 7: view7 });
    const onreject = vi.fn();
    render(AssetInspector, { ...base, onreject, selectedKey: 'card:7' });
    expect(tabLabels()).toEqual(['Items']);
    expect(screen.getByTestId('card-item-7-0')).toHaveTextContent('skill/fresh');
    await fireEvent.click(screen.getByTestId('card-reject-7-0'));
    expect(onreject).toHaveBeenCalledWith(7, [0]);
  });

  it('a rollout card also has a Hosts tab with what each host held back', async () => {
    const rollout: ChangesetView = { id: 7, kind: 'rollout', summary: 'Roll out core to oci', state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
      { position: 0, grp: 'core', kind: 'host', name: 'oci', action: 'sync', params: {}, decider: 'person', state: 'skipped', outcome: { held: [{ kind: 'skill', name: 'w', why: 'edited' }] } },
    ] };
    cardViews.set({ 7: rollout });
    render(AssetInspector, { ...base, cards: [{ ...base.cards[0], kind: 'rollout' as const }], selectedKey: 'card:7' });
    expect(tabLabels()).toEqual(['Items', 'Hosts']);
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('inspector-hosts')).toHaveTextContent('oci');
    expect(screen.getByTestId('inspector-hosts')).toHaveTextContent('skill/w — edited on the host');
  });

  it('a card applied through the Inspector Undo runs the verb', async () => {
    cardViews.set({ 7: { ...view7, state: 'applied', undoable: true, commits: { personal: 'abcdef12' } } });
    const oncard = { apply: vi.fn(), dismiss: vi.fn(), undo: vi.fn(), synchost: vi.fn() };
    render(AssetInspector, { ...base, oncard, cards: [{ ...base.cards[0], state: 'applied' as const, undoable: true }], selectedKey: 'card:7' });
    await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(oncard.undo).toHaveBeenCalledWith(7);
  });

  it('an applied card (not an open one) is read when selected, so its commits and Undo show', async () => {
    const applied: ChangesetView = { ...view7, state: 'applied', undoable: true, commits: { personal: 'abcdef12' } };
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_get_changeset') return applied;
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetInspector, { ...base, cards: [{ ...base.cards[0], state: 'applied' as const, undoable: true }], selectedKey: 'card:7' });
    expect(await screen.findByTestId('card-commits-7')).toHaveTextContent('personal abcdef1');
    expect(invoke).toHaveBeenCalledWith('catalog_get_changeset', { args: { id: 7 } });
  });

  it('after Undo the Inspector re-reads the card and no longer offers Undo', async () => {
    const applied: ChangesetView = { ...view7, state: 'applied', undoable: true, commits: { personal: 'abcdef12' } };
    let current: ChangesetView = applied;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_get_changeset') return current;
      throw { code: 'E_TEST', message: cmd };
    });
    const oncard = { apply: vi.fn(), dismiss: vi.fn(), undo: vi.fn(), synchost: vi.fn() };
    const card = { ...base.cards[0], state: 'applied' as const, undoable: true, applied_at: 5 };
    const { rerender } = render(AssetInspector, { ...base, oncard, cards: [card], selectedKey: 'card:7' });
    await fireEvent.click(await screen.findByRole('button', { name: 'Undo' }));
    expect(oncard.undo).toHaveBeenCalledWith(7);
    // The workspace reloads the cards: the card is now undone.
    current = { ...applied, state: 'undone', undoable: false };
    await rerender({ ...base, oncard, cards: [{ ...card, state: 'undone' as const, undoable: false }], selectedKey: 'card:7' });
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull());
  });

  it('opens a just-created asset straight into Source, editing', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w', autoEditKey: 'asset:personal:skill/w' });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });

  it('a just-created asset the listing has not caught up with still opens', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/brand-new', autoEditKey: 'asset:personal:skill/brand-new' });
    expect(tabLabels()).toEqual(['Overview', 'Source', 'Hosts', 'History']);
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });

  it('selecting another asset after an edit starts on Overview, not editing', async () => {
    const { rerender } = render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w', autoEditKey: 'asset:personal:skill/w' });
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
    await rerender({ selectedKey: 'asset:papayapos:skill/ppt' });
    expect(screen.getByTestId('inspector-tab-overview').getAttribute('aria-selected')).toBe('true');
    expect(screen.queryByTestId('editor-save')).toBeNull();
  });

  it('the e key (a new nonce) opens the selected asset in Source, editing', async () => {
    const { rerender } = render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(screen.queryByTestId('editor-save')).toBeNull();
    await rerender({ editNonce: 1 });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });

  it('the e key on a row this window cannot edit does nothing', async () => {
    const { rerender } = render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    await rerender({ editNonce: 1 });
    expect(screen.getByTestId('inspector-tab-overview').getAttribute('aria-selected')).toBe('true');
    expect(screen.queryByTestId('editor-save')).toBeNull();
  });

  it('a tab the row loses (read-only turns on) falls back to Overview', async () => {
    const { rerender } = render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w' });
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await rerender({ readOnly: true });
    expect(tabLabels()).toEqual(['Overview', 'Hosts']);
    expect(screen.getByTestId('inspector-tab-overview').getAttribute('aria-selected')).toBe('true');
    expect(screen.getByTestId('inspector-summary')).toBeTruthy();
  });

  it('an unsaved draft survives a visit to Overview and back to Source', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w', autoEditKey: 'asset:personal:skill/w' });
    const desc = (await screen.findByTestId('editor-description')) as HTMLTextAreaElement;
    await fireEvent.input(desc, { target: { value: 'half-typed' } });
    await fireEvent.click(screen.getByTestId('inspector-tab-overview'));
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    await fireEvent.click(screen.getByTestId('inspector-tab-source'));
    const back = screen.getByTestId('editor-description') as HTMLTextAreaElement;
    expect(back).toBe(desc);
    expect(back.value).toBe('half-typed');
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_get_asset')).toHaveLength(1);
  });

  it('the editor is hidden, not shown, outside Source', async () => {
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w', autoEditKey: 'asset:personal:skill/w' });
    const desc = await screen.findByTestId('editor-description');
    await fireEvent.click(screen.getByTestId('inspector-tab-overview'));
    expect(desc.closest('[hidden]')).not.toBeNull();
    await fireEvent.click(screen.getByTestId('inspector-tab-source'));
    expect(desc.closest('[hidden]')).toBeNull();
  });

  it('History is read afresh on every entry: after a Save, and after an error', async () => {
    let fail = true;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_get_asset') return { asset: { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], body: '# b' }, previews: [], hosts: [] };
      if (cmd === 'catalog_lint_asset') return { errors: [], warnings: [] };
      if (cmd === 'catalog_update_asset') return { commit: 'f00dfeed1234', lint: { errors: [], warnings: [] } };
      if (cmd === 'catalog_asset_history') {
        if (fail) throw { code: 'E_CATALOG_GIT', message: 'git is busy' };
        return [{ sha: 'abcdef1234567', at: 1, author: 'Martin', subject: 'edit w' }];
      }
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetInspector, { ...base, selectedKey: 'asset:personal:skill/w', autoEditKey: 'asset:personal:skill/w' });
    const desc = await screen.findByTestId('editor-description');
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('git is busy'));
    fail = false;
    await fireEvent.click(screen.getByTestId('inspector-tab-source'));
    await fireEvent.input(desc, { target: { value: 'changed' } });
    await fireEvent.click(screen.getByTestId('editor-save'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_update_asset', expect.anything()));
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('edit w'));
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_asset_history')).toHaveLength(2);
  });

  it('a wrong canOpen never reads an org asset as personal', async () => {
    render(AssetInspector, { ...base, canOpen: () => true, selectedKey: 'asset:papayapos:skill/ppt' });
    expect(tabLabels()).toEqual(['Overview', 'Hosts', 'History']);
    expect(screen.getByTestId('inspector-summary').textContent).toContain('Org skill.');
    expect(invoke.mock.calls.some((c) => c[0] === 'catalog_get_asset')).toBe(false);
  });

  it('History: an unrelated E_INVALID keeps its own message', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_asset_history') throw { code: 'E_INVALID', message: 'unknown catalog nope' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetInspector, { ...base, selectedKey: 'asset:papayapos:skill/ppt' });
    await fireEvent.click(screen.getByTestId('inspector-tab-history'));
    await waitFor(() => expect(screen.getByTestId('inspector-history').textContent).toContain('unknown catalog nope'));
    expect(screen.getByTestId('inspector-history').textContent).not.toContain('update the hub');
  });
});
