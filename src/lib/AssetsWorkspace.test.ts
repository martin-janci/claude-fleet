import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { readFileSync } from 'node:fs';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsWorkspace from './AssetsWorkspace.svelte';
import { catalog, inventory, lastSyncRun, repoStatusStore, type AssetListing, type SyncPlan, type SyncRunSummary } from './assets';
import { cardViews, catalogStatuses, changesetSummaries, layerListing, layersByCatalog, type CatalogStatus, type ChangesetSummary, type ChangesetView, type LayerListing } from './assets_workspace';
import { toasts } from './toasts';
import { hosts } from './hosts';
import { orgs, type OrgDetail } from './orgs';
import { hubStatus, STANDALONE } from './hub';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const listing: AssetListing = {
  head: 'abcdef1234567890', loaded_at: 1, problems: [{ path: 'hooks/bad.yaml', message: 'name' }],
  unmanaged: [{ host_alias: 'oci', harness: 'claude', kind: 'skill', name: 'fresh', state: 'unmanaged', catalog_hash: null, host_hash: 'h', scanned_at: 100, managed: false }],
  assets: [
    { kind: 'skill', name: 'edited', version: '1', description: '', tags: [], catalog: 'personal', hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'host' }] },
    { kind: 'skill', name: 'fine', version: '1', description: '', tags: [], catalog: 'personal', hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }] },
  ],
  identities: [{ kind: 'skill', name: 'fresh', hosts: [{ host_alias: 'oci', harness: 'claude', host_hash: 'h' }], signature: 'oci', variants: 1, class: 'normal', reason: null }],
};
const NEW_CARD: ChangesetSummary = { id: 7, kind: 'new', summary: 'New on oci: skill/fresh → core', state: 'proposed', created_at: 1, groups: { core: 1 }, catalogs: ['personal'] };
const NEW_VIEW: ChangesetView = { id: 7, kind: 'new', summary: NEW_CARD.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal'], items: [
  { position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'fresh', action: 'import', params: {}, decider: 'rule', state: 'pending' },
  { position: 1, grp: 'core', catalog: 'personal', kind: 'skill', name: 'second', action: 'import', params: {}, decider: 'rule', state: 'pending' },
  { position: 2, grp: 'core', catalog: 'personal', kind: 'skill', name: 'done', action: 'import', params: {}, decider: 'rule', state: 'rejected' },
] };
const DRIFT_CARD: ChangesetSummary = { id: 8, kind: 'drift', summary: 'oci edited skill/edited', state: 'proposed', created_at: 1 };
const DRIFT_VIEW: ChangesetView = { id: 8, kind: 'drift', summary: DRIFT_CARD.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
  { position: 0, grp: 'oci', kind: 'skill', name: 'edited', action: 'take_host', params: { host: 'oci' }, decider: 'person', state: 'pending' },
] };
const withCards = (...cards: [ChangesetSummary, ChangesetView][]) => {
  changesetSummaries.set(cards.map(([c]) => c));
  cardViews.set(Object.fromEntries(cards.map(([c, v]) => [c.id, v])));
};
const cardCalls = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd);
/** The hub answers every card verb with the applied card, and the reload with the cards as they were. */
function answerCards(cards: [ChangesetSummary, ChangesetView][]) {
  invoke.mockImplementation(async (cmd: string, a?: { args?: { id?: number } }) => {
    if (cmd === 'catalog_list_changesets') return cards.map(([c]) => c);
    if (cmd === 'catalog_get_changeset') return cards.find(([c]) => c.id === a?.args?.id)?.[1];
    if (cmd.startsWith('catalog_') && cmd.endsWith('_changeset')) return { ...NEW_VIEW, state: 'applied', undoable: true, commits: { personal: 'abc1234' } };
    if (cmd === 'catalog_reject_changeset_items') return { ...NEW_VIEW };
    throw { code: 'E_TEST', message: cmd };
  });
}
const LAYERS: LayerListing = {
  layers: [{ name: 'core', axis: 'context', members: ['skill/fine'] }, { name: 'server', axis: 'role', extends: 'core' }],
  hosts: [{ host_alias: 'oci', catalog_id: 1, layer_name: 'server', axis: 'role', position: 0, active: true }],
};
const handlers = () => ({ onscan: vi.fn(), onsync: vi.fn(), onimport: vi.fn(), onsecrets: vi.fn(), onnew: vi.fn(), onlintall: vi.fn() });

beforeEach(() => {
  invoke.mockReset();
  invoke.mockRejectedValue({ code: 'E_TEST', message: 'not in this test' });
  hubStatus.set(STANDALONE);
  catalog.set(listing); inventory.set([]); lastSyncRun.set(null); repoStatusStore.set(null);
  catalogStatuses.set(null); changesetSummaries.set(null); layerListing.set(null); layersByCatalog.set(null); cardViews.set({}); toasts.set([]);
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
    { alias: 'oci', ssh_alias: 'oci', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
  ]);
});

describe('AssetsWorkspace', () => {
  it('opens on the Inbox with a sentence, the rail, the list, the Inspector and the footer', () => {
    render(AssetsWorkspace, handlers());
    expect(screen.getByTestId('assets-sentence').textContent).toBe('1 needs you · 1 new on hosts');
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('aria-current')).toBe('page');
    expect(screen.getByTestId('assets-rail-inbox').textContent).toContain('1');
    expect(screen.getByTestId('assets-inbox')).toBeTruthy();
    expect(screen.getByTestId('inspector-empty')).toBeTruthy();
    expect(screen.getByTestId('assets-footer')).toBeTruthy();
    expect(screen.getByTestId('assets-problems').textContent).toContain('1 problems');
  });

  it('has one primary button, Sync fleet, and quiet controls otherwise', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const primaries = document.querySelectorAll('.btn--primary');
    expect(primaries).toHaveLength(1);
    expect(primaries[0].textContent).toContain('Sync fleet');
    await fireEvent.click(screen.getByTestId('assets-sync'));
    expect(h.onsync).toHaveBeenCalledWith({});
    await fireEvent.click(screen.getByTestId('assets-scan'));
    expect(h.onscan).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('assets-secrets'));
    expect(h.onsecrets).toHaveBeenCalled();
  });

  it('one primary per region: the header Sync with nothing selected, the selected card’s verb otherwise (R-C)', async () => {
    withCards([NEW_CARD, NEW_VIEW], [DRIFT_CARD, DRIFT_VIEW]);
    render(AssetsWorkspace, handlers());
    const primaries = (region: string) => Array.from(document.querySelectorAll(`${region} .btn--primary`));
    // Inbox, nothing selected: the header's Sync, and no card's verb.
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['assets-sync']);
    expect(primaries('.insp')).toHaveLength(0);
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(1);
    // A card selected: its verb is the main column's one primary; Sync goes plain.
    await fireEvent.click(screen.getByTestId('card-7'));
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['card-primary-7']);
    expect(screen.getByTestId('assets-sync')).toHaveClass('btn');
    expect(screen.getByTestId('card-primary-8')).not.toHaveClass('btn--primary');
    expect(primaries('.insp').length).toBeLessThanOrEqual(1);
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(1);
    // Selecting an asset gives Sync its primary back.
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['assets-sync']);
    // Layers: with its list showing, Sync is still the main column's primary...
    layersByCatalog.set({ personal: LAYERS });
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['assets-sync']);
    // ...and with the New layer form open its Propose is, and Sync goes plain.
    await fireEvent.click(screen.getByTestId('layers-new'));
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['layer-form-submit']);
    expect(screen.getByTestId('assets-sync')).toHaveClass('btn');
    expect(screen.getByTestId('assets-sync')).not.toHaveClass('btn--primary');
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(1);
    // A layer's own rename form is the Inspector's one primary: one per region.
    await fireEvent.click(screen.getByTestId('layer-row-personal-core'));
    await fireEvent.click(screen.getByTestId('layer-rename'));
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['layer-form-submit']);
    expect(primaries('.insp').map((b) => b.getAttribute('data-testid'))).toEqual(['layer-form-submit']);
    // Cancelling the main form gives Sync its primary back.
    await fireEvent.click(within(screen.getByTestId('assets-list')).getByRole('button', { name: 'Cancel' }));
    expect(primaries('.main').map((b) => b.getAttribute('data-testid'))).toEqual(['assets-sync']);
    expect(primaries('.insp')).toHaveLength(1);
  });

  it('a selected card whose verb is not on screen leaves Sync the primary, and ⌘↵ runs Sync', async () => {
    const APPLIED: ChangesetSummary = { id: 9, kind: 'new', summary: 'Applied one', state: 'applied', undoable: true, created_at: 1, catalogs: ['personal'] };
    withCards([NEW_CARD, NEW_VIEW], [APPLIED, { ...NEW_VIEW, id: 9, state: 'applied', undoable: true }]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    const h = handlers();
    render(AssetsWorkspace, h);
    const mainPrimaries = () => Array.from(document.querySelectorAll('.main .btn--primary')).map((b) => b.getAttribute('data-testid'));
    // An applied banner has no verb: Sync stays the primary.
    await fireEvent.click(screen.getByTestId('card-9'));
    expect(mainPrimaries()).toEqual(['assets-sync']);
    // An open card in the Library (not on screen): Sync is the primary and ⌘↵ syncs.
    await fireEvent.click(screen.getByTestId('card-7'));
    expect(mainPrimaries()).toEqual(['card-primary-7']);
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    expect(mainPrimaries()).toEqual(['assets-sync']);
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Enter', metaKey: true });
    expect(h.onsync).toHaveBeenCalledWith({});
    expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
  });

  it('a query that hides the selected card gives Sync its primary and ⌘↵ back', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    const h = handlers();
    render(AssetsWorkspace, h);
    await fireEvent.click(screen.getByTestId('card-7'));
    await fireEvent.input(screen.getByTestId('assets-query'), { target: { value: 'zzz-nothing' } });
    expect(screen.queryByTestId('card-7')).toBeNull();
    expect(Array.from(document.querySelectorAll('.main .btn--primary')).map((b) => b.getAttribute('data-testid'))).toEqual(['assets-sync']);
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Enter', metaKey: true });
    expect(h.onsync).toHaveBeenCalledWith({});
    expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
  });

  it('a hub client granted personal only: a card on acme shows no verbs and i does nothing on it', async () => {
    const ACME: ChangesetSummary = { ...NEW_CARD, catalogs: ['acme'] };
    withCards([ACME, { ...NEW_VIEW, catalogs: ['acme'] }]);
    answerCards([[ACME, NEW_VIEW]]);
    hubStatus.set({ ...STANDALONE, remote: true, client_name: 'desk', client_mode: 'full' });
    catalogStatuses.set([
      { id: 1, name: 'personal', org_id: null, repo_path: '/p', remote_url: null, head_commit: 'a', last_loaded_at: 1, state: 'loaded', asset_count: 0, granted: ['desk'] },
      { id: 2, name: 'acme', org_id: 1, repo_path: '/a', remote_url: null, head_commit: 'a', last_loaded_at: 1, state: 'loaded', asset_count: 0, granted: ['someone-else'] },
    ]);
    const h = handlers();
    render(AssetsWorkspace, h);
    expect(screen.getByTestId('card-7')).toBeTruthy();
    expect(screen.queryByTestId('card-primary-7')).toBeNull();
    expect(screen.queryByTestId('card-dismiss-7')).toBeNull();
    await fireEvent.click(screen.getByTestId('card-7'));
    expect(screen.queryByTestId('card-skip-7-core')).toBeNull();
    expect(screen.queryByTestId('card-reject-7-0')).toBeNull();
    const list = screen.getByTestId('assets-list');
    await fireEvent.keyDown(list, { key: 'i' });
    await fireEvent.keyDown(list, { key: 'Enter', metaKey: true });
    expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(0);
    expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
    // The identity the card covers: neither i nor a acts through it.
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    await fireEvent.keyDown(list, { key: 'i' });
    await fireEvent.keyDown(list, { key: 'a' });
    expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(0);
    expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
  });

  it('the Library holds the authoring controls and every asset once', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    expect(screen.queryByTestId('assets-new')).toBeNull();
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    expect(screen.getByTestId('assets-rail-library').getAttribute('aria-current')).toBe('page');
    expect(screen.getByTestId('asset-row-skill-edited')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('assets-new'));
    expect(h.onnew).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('assets-import'));
    expect(h.onimport).toHaveBeenCalledWith(null);
    await fireEvent.click(screen.getByTestId('assets-lint-all'));
    expect(h.onlintall).toHaveBeenCalled();
  });

  it('Secrets on the rail is disabled while the panel is busy', () => {
    render(AssetsWorkspace, { ...handlers(), busy: 'scan' });
    expect(screen.getByTestId('assets-secrets')).toBeDisabled();
  });

  it('selecting a row shows it in the Inspector', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    expect(screen.getByTestId('inspector').textContent).toContain('fresh');
  });

  it('read-only: one scope chip, no mutating control, the grant command on demand', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true, readOnlyClient: 'desk', onrefresh: vi.fn() });
    const chip = screen.getByTestId('assets-readonly');
    expect(chip.textContent).toBe('read-only · ask the operator to grant assets on personal');
    for (const id of ['assets-sync', 'assets-secrets', 'assets-new', 'assets-import', 'assets-lint-all']) {
      expect(screen.queryByTestId(id), id).toBeNull();
    }
    expect(screen.getByTestId('assets-scan')).toBeTruthy();
    expect(screen.getByTestId('assets-hub-refresh')).toBeTruthy();
    await fireEvent.click(chip);
    expect(screen.getByTestId('assets-grant-cmd').textContent).toBe('fleet-hub client grant desk assets');
  });

  it('read-only chip: controls the note it reveals; without a client name it says how to find it', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true, readOnlyClient: null });
    const chip = screen.getByTestId('assets-readonly');
    await fireEvent.click(chip);
    const note = document.getElementById(chip.getAttribute('aria-controls') ?? '');
    expect(note?.getAttribute('role')).toBe('note');
    expect(note?.contains(screen.getByTestId('assets-grant-cmd'))).toBe(true);
    expect(screen.getByTestId('assets-grant-cmd').textContent).toBe("fleet-hub client grant <this client's name> assets");
    expect(note?.textContent).toContain('fleet-hub client list');
  });

  it('read-only chip with a client name has no name-lookup hint', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true, readOnlyClient: 'desk' });
    await fireEvent.click(screen.getByTestId('assets-readonly'));
    expect(document.querySelector('[role="note"]')?.textContent).not.toContain('fleet-hub client list');
  });

  it('read-only: the Inbox and the Library rows offer no Import', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true, readOnlyClient: 'desk' });
    // The Import sits beside a picker row, on its line.
    expect(screen.getByTestId('inbox-row-identity:skill/fresh').parentElement!.textContent).not.toContain('Import');
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    expect(screen.getByTestId('identity-row-skill-fresh').parentElement!.textContent).not.toContain('Import');
  });

  it('the Inbox’s Import on an identity row reaches the panel', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const row = screen.getByTestId('inbox-row-identity:skill/fresh');
    const imp = Array.from(row.parentElement!.querySelectorAll('button')).find((b) => b.textContent?.trim() === 'Import');
    expect(imp).toBeTruthy();
    await fireEvent.click(imp!);
    expect(h.onimport).toHaveBeenCalledWith(expect.objectContaining({ kind: 'skill', name: 'fresh' }));
  });

  it('the Inbox words a copy blocked on a secret as differing, not behind (blockedOnSecrets)', () => {
    catalog.set({
      ...listing,
      assets: [{ kind: 'skill', name: 'needs-key', version: '1', description: '', tags: [], catalog: 'personal', hosts: [{ host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'catalog' }] }],
    });
    const run: SyncRunSummary = {
      plan_id: 'p', started_at: 1, finished_at: 2,
      hosts: [{ host_alias: 'oci', harness: 'claude', status: 'applied', detail: null, restart_required: false,
        actions: [{ kind: 'skill', name: 'needs-key', op: 'blocked', outcome: 'skipped', detail: 'missing secrets: TOKEN' }] }],
    } as SyncRunSummary;
    lastSyncRun.set(run);
    render(AssetsWorkspace, handlers());
    expect(screen.getByTestId('inbox-row-asset:personal:skill/needs-key').textContent).toContain('Differs from the catalog on oci');
  });

  it('the Library filters with the whole token query (PF11): tokens and free words alike', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    const q = screen.getByTestId('assets-query');
    await fireEvent.input(q, { target: { value: 'state:edited' } });
    expect(screen.getByTestId('asset-row-skill-edited')).toBeTruthy();
    expect(screen.queryByTestId('asset-row-skill-fine')).toBeNull();
    await fireEvent.input(q, { target: { value: 'FINE' } });
    expect(screen.queryByTestId('asset-row-skill-edited')).toBeNull();
    expect(screen.getByTestId('asset-row-skill-fine')).toBeTruthy();
  });

  it('an org catalog row opens as a summary in the Inspector, never through catalog_get_asset', async () => {
    catalog.set({
      ...listing,
      assets: [...listing.assets, { kind: 'skill', name: 'shared-one', version: '2', description: 'org', tags: [], catalog: 'acme', hosts: [] }],
    });
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    const row = screen.getByTestId('asset-row-acme-skill-shared-one');
    expect(row.getAttribute('aria-disabled')).toBeNull();
    await fireEvent.click(row);
    expect(screen.getByTestId('inspector').textContent).toContain('shared-one');
    expect(screen.getByTestId('inspector-summary')).toBeTruthy();
    expect(invoke.mock.calls.some((c) => c[0] === 'catalog_get_asset')).toBe(false);
  });

  it('a personal row opens the full detail (Overview · Source · Hosts · History)', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    await fireEvent.click(screen.getByTestId('asset-row-skill-fine'));
    expect(screen.getByTestId('inspector-tab-source')).toBeTruthy();
    expect(screen.getByTestId('inspector-tab-history')).toBeTruthy();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_get_asset', { args: { kind: 'skill', name: 'fine' } }));
  });

  it('the footer re-reads an org catalog’s status when the listing reloads, without closing a popover', async () => {
    const acme: CatalogStatus = {
      id: 2, name: 'acme', org_id: 1, repo_path: '/a', remote_url: null, head_commit: 'aaaaaaa', last_loaded_at: 1, state: 'loaded', asset_count: 0,
    };
    catalogStatuses.set([{ ...acme, id: 1, name: 'personal', org_id: null }, acme]);
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_repo_status_in') return { head: 'aaaaaaa', dirty: 0, ahead: 0, behind: 0, has_upstream: true };
      throw { code: 'E_TEST', message: cmd };
    });
    repoStatusStore.set({ head: 'abcdef1', dirty: 0, ahead: 0, behind: 0, has_upstream: true });
    render(AssetsWorkspace, handlers());
    const statusReads = () => invoke.mock.calls.filter((c) => c[0] === 'catalog_repo_status_in').length;
    await waitFor(() => expect(statusReads()).toBe(1));
    // A reload while the personal popover is open leaves it open.
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('assets-pull')).toBeTruthy();
    catalog.set({ ...listing, loaded_at: 2 });
    await waitFor(() => expect(statusReads()).toBe(2));
    expect(screen.getByTestId('assets-pull')).toBeTruthy();
  });

  it('the asset detail scrolls inside the Inspector only (no second scroller)', () => {
    const css = readFileSync('src/lib/AssetDetail.svelte', 'utf8').match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';
    const rule = /\.detail\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).not.toMatch(/overflow/);
    expect(rule).not.toMatch(/height/);
  });
});

const ROLLOUT_CARD: ChangesetSummary = { id: 11, kind: 'rollout', summary: 'Roll out core to 2 hosts', state: 'proposed', created_at: 1, catalogs: ['personal'] };
const ROLLOUT_VIEW: ChangesetView = { id: 11, kind: 'rollout', summary: ROLLOUT_CARD.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal'], items: [
  { position: 0, grp: 'core', kind: 'host', name: 'oci', action: 'sync', params: { assets: ['skill/edited', 'skill/fine'] }, decider: 'person', state: 'pending' },
  { position: 1, grp: 'core', kind: 'host', name: 'htz', action: 'sync', params: { assets: ['skill/fine'] }, decider: 'person', state: 'pending' },
  { position: 2, grp: 'core', kind: 'host', name: 'old', action: 'sync', params: { assets: ['skill/fine'] }, decider: 'person', state: 'applied' },
] };
const PLAN: SyncPlan = {
  id: 'plan-1', computed_at: 1, counts: { create: 1 },
  hosts: [{ host_alias: 'oci', harness: 'claude', status: 'planned', detail: null, actions: [
    { kind: 'skill', name: 'fine', op: 'create', reason: null, files: [], merges: [], backup: false, secrets: [], missing_secrets: [], catalog: 'personal' },
  ] }],
};
const primaries = (region: string) => Array.from(document.querySelectorAll(`${region} .btn--primary`)).map((b) => b.getAttribute('data-testid'));

describe('AssetsWorkspace plan view (R15, R16)', () => {
  it('while a plan is open it replaces the list; Back closes it', async () => {
    const onplanclose = vi.fn();
    render(AssetsWorkspace, { ...handlers(), plan: PLAN, onplanclose });
    expect(screen.getByTestId('sync-plan-view')).toBeTruthy();
    expect(screen.getByTestId('assets-workspace').contains(screen.getByTestId('sync-plan-view'))).toBe(true);
    expect(screen.queryByTestId('assets-inbox')).toBeNull();
    await fireEvent.click(screen.getByTestId('plan-back'));
    expect(onplanclose).toHaveBeenCalled();
  });

  it('Esc closes only the plan: the event is stopped before the App’s window listener', async () => {
    const onplanclose = vi.fn();
    const appEsc = vi.fn();
    window.addEventListener('keydown', appEsc);
    try {
      render(AssetsWorkspace, { ...handlers(), plan: PLAN, onplanclose });
      const back = screen.getByTestId('plan-back');
      // The view takes focus when it opens, so Esc starts inside the workspace.
      expect(document.activeElement).toBe(back);
      const ev = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
      back.dispatchEvent(ev);
      expect(onplanclose).toHaveBeenCalledTimes(1);
      expect(ev.defaultPrevented).toBe(true);
      expect(appEsc).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener('keydown', appEsc);
    }
  });

  it('Esc from body (focus fell out of the view) still closes only the plan', async () => {
    const onplanclose = vi.fn();
    const appEsc = vi.fn();
    window.addEventListener('keydown', appEsc);
    try {
      render(AssetsWorkspace, { ...handlers(), plan: PLAN, onplanclose });
      (document.activeElement as HTMLElement | null)?.blur();
      expect(document.activeElement).toBe(document.body);
      const ev = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
      document.body.dispatchEvent(ev);
      expect(onplanclose).toHaveBeenCalledTimes(1);
      expect(ev.defaultPrevented).toBe(true);
      expect(appEsc).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener('keydown', appEsc);
    }
  });

  it('Esc from body after a "Plan anyway" and after an apply keeps the overlay and closes the plan', async () => {
    const unlayered: SyncPlan = { ...PLAN, hosts: [{ host_alias: 'oci', harness: 'claude', status: 'skipped', detail: 'no layers assigned: x', actions: [] }] };
    const planned: SyncPlan = { ...PLAN, id: 'plan-2' };
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_plan_sync') return planned;
      if (cmd === 'catalog_apply_sync') return { plan_id: 'plan-2', started_at: 1, finished_at: 2, hosts: [] };
      throw { code: 'E_TEST', message: cmd };
    });
    const appEsc = vi.fn();
    window.addEventListener('keydown', appEsc);
    try {
      const onplanclose = vi.fn();
      const { rerender } = render(AssetsWorkspace, { ...handlers(), plan: unlayered, onplanclose });
      const props = (p: SyncPlan) => ({ ...handlers(), plan: p, onplanclose, onreplanned: (n: SyncPlan) => void rerender(props(n)) });
      await rerender(props(unlayered));
      screen.getByTestId('plan-anyway-oci-claude').focus();
      await fireEvent.click(screen.getByTestId('plan-anyway-oci-claude'));
      await waitFor(() => expect(screen.queryByTestId('plan-anyway-oci-claude')).toBeNull());
      // Focus is back inside the view, and Esc from body is taken too.
      await waitFor(() => expect(screen.getByTestId('sync-plan-view').contains(document.activeElement)).toBe(true));
      await fireEvent.click(screen.getByTestId('plan-apply'));
      await waitFor(() => expect(screen.getByTestId('plan-apply')).toBeDisabled());
      await waitFor(() => expect(screen.getByTestId('sync-plan-view').contains(document.activeElement)).toBe(true));
      document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
      expect(onplanclose).toHaveBeenCalledTimes(1);
      expect(appEsc).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener('keydown', appEsc);
    }
  });

  it('with no plan open Esc is left to the App', async () => {
    const appEsc = vi.fn();
    window.addEventListener('keydown', appEsc);
    try {
      render(AssetsWorkspace, handlers());
      await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Escape' });
      expect(appEsc).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener('keydown', appEsc);
    }
  });

  it('Esc does not close a plan while it is applying, but still does not close the overlay', async () => {
    const onplanclose = vi.fn();
    const appEsc = vi.fn();
    window.addEventListener('keydown', appEsc);
    try {
      render(AssetsWorkspace, { ...handlers(), plan: PLAN, onplanclose, busy: 'apply' });
      await fireEvent.keyDown(screen.getByTestId('assets-workspace'), { key: 'Escape' });
      expect(onplanclose).not.toHaveBeenCalled();
      expect(appEsc).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener('keydown', appEsc);
    }
  });

  it('a field and a dialog keep their Esc', async () => {
    const onplanclose = vi.fn();
    render(AssetsWorkspace, { ...handlers(), plan: PLAN, onplanclose });
    await fireEvent.keyDown(screen.getByTestId('assets-query'), { key: 'Escape' });
    expect(onplanclose).not.toHaveBeenCalled();
  });

  it('the hidden list takes no keys while a plan is open', async () => {
    const h = handlers();
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    render(AssetsWorkspace, { ...h, plan: PLAN, selectedKey: 'identity:skill/fresh' });
    const root = screen.getByTestId('assets-workspace');
    await fireEvent.keyDown(root, { key: 'a' });
    await fireEvent.keyDown(root, { key: 'i' });
    await fireEvent.keyDown(root, { key: 'Enter', metaKey: true });
    expect(h.onimport).not.toHaveBeenCalled();
    expect(h.onsync).not.toHaveBeenCalled();
    expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
    expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(0);
  });

  it('one primary per region with a plan open: the plan’s Apply, never the header Sync (nor a card verb)', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    const { rerender } = render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:7' });
    expect(primaries('.main')).toEqual(['card-primary-7']);
    await rerender({ ...handlers(), selectedKey: 'card:7', plan: PLAN });
    expect(primaries('.main')).toEqual(['plan-apply']);
    expect(primaries('.insp')).toHaveLength(0);
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(1);
    expect(screen.getByTestId('assets-sync')).toHaveClass('btn');
    expect(screen.getByTestId('assets-sync').textContent).not.toContain('⌘↵');
    // Closed again: the selected card's verb is back.
    await rerender({ ...handlers(), selectedKey: 'card:7', plan: null });
    expect(primaries('.main')).toEqual(['card-primary-7']);
  });

  it('a plan with nothing selected: Apply is the one primary, header Sync plain', () => {
    render(AssetsWorkspace, { ...handlers(), plan: PLAN });
    expect(primaries('.main')).toEqual(['plan-apply']);
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(1);
  });

  it('Applying a plan reports it and the plan’s secrets link opens the secrets panel', async () => {
    const h = handlers();
    const p: SyncPlan = { ...PLAN, hosts: [{ ...PLAN.hosts[0], actions: [{ ...PLAN.hosts[0].actions[0], op: 'blocked', reason: 'missing', missing_secrets: ['TOKEN'] }] }] };
    render(AssetsWorkspace, { ...h, plan: p });
    await fireEvent.click(screen.getByTestId('plan-action-secrets-oci-claude-skill-fine'));
    expect(h.onsecrets).toHaveBeenCalled();
  });

  describe('Review plan on a Rollout card', () => {
    const answerPlans = (fail?: string) =>
      invoke.mockImplementation(async (cmd: string, a?: { args?: { host_alias?: string } }) => {
        if (cmd !== 'catalog_plan_sync') throw { code: 'E_TEST', message: cmd };
        const host = a?.args?.host_alias ?? '';
        if (host === fail) throw { code: 'E_TEST', message: `no plan for ${host}` };
        return {
          id: `p-${host}`, computed_at: 1, counts: { update: 1 },
          hosts: [{ host_alias: host, harness: 'claude', status: 'planned', detail: null, actions: [
            { kind: 'skill', name: 'fine', op: 'update', reason: null, files: [], merges: [], backup: false, secrets: [], missing_secrets: [], catalog: 'personal', host_copy: host === 'oci' ? 'unverified' : 'unchanged' },
            { kind: 'skill', name: 'not-ours', op: 'create', reason: null, files: [], merges: [], backup: false, secrets: [], missing_secrets: [], catalog: 'personal' },
          ] }],
        };
      });

    it('plans each pending host host-scoped and shows them in review mode', async () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      answerPlans();
      render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:11' });
      await fireEvent.click(screen.getByTestId('card-review-11'));
      await screen.findByTestId('sync-plan-view');
      const planned = cardCalls('catalog_plan_sync').map((c) => (c[1] as { args: { host_alias: string; allow_unlayered: boolean } }).args);
      expect(planned.map((a) => a.host_alias).sort()).toEqual(['htz', 'oci']);
      expect(planned.every((a) => a.allow_unlayered === false)).toBe(true);
      // Review mode: no Apply; the card's own actions only; held ones marked.
      expect(screen.queryByTestId('plan-apply')).toBeNull();
      expect(screen.getByRole('heading', { name: 'Roll-out review' })).toBeTruthy();
      expect(screen.queryByTestId('plan-action-oci-claude-skill-not-ours')).toBeNull();
      expect(screen.getByTestId('plan-held-oci-claude-skill-fine')).toHaveTextContent('held — sync it yourself');
      expect(screen.getByTestId('plan-action-htz-claude-skill-fine')).toBeTruthy();
      expect(screen.getByTestId('plan-review-note')).toBeTruthy();
      // The list is replaced; only the header Sync is a primary.
      expect(screen.queryByTestId('assets-inbox')).toBeNull();
      expect(primaries('.main')).toEqual(['assets-sync']);
      expect(primaries('.insp')).toHaveLength(0);
    });

    it('Back and Esc close only the review', async () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      answerPlans();
      const appEsc = vi.fn();
      window.addEventListener('keydown', appEsc);
      try {
        render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:11' });
        await fireEvent.click(screen.getByTestId('card-review-11'));
        await screen.findByTestId('sync-plan-view');
        await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
        await waitFor(() => expect(screen.queryByTestId('sync-plan-view')).toBeNull());
        expect(appEsc).not.toHaveBeenCalled();
        expect(screen.getByTestId('assets-inbox')).toBeTruthy();
        await fireEvent.click(screen.getByTestId('card-review-11'));
        await fireEvent.click(await screen.findByTestId('plan-back'));
        expect(screen.queryByTestId('sync-plan-view')).toBeNull();
      } finally {
        window.removeEventListener('keydown', appEsc);
      }
    });

    it('an asset selected from outside (the quick switcher) leaves the review and shows the Library', async () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      answerPlans();
      const { rerender } = render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:11' });
      await fireEvent.click(screen.getByTestId('card-review-11'));
      await screen.findByTestId('sync-plan-view');
      await rerender({ ...handlers(), selectedKey: 'asset:personal:skill/edited', view: 'library' });
      await waitFor(() => expect(screen.queryByTestId('sync-plan-view')).toBeNull());
      expect(screen.getByTestId('asset-row-skill-edited')).toBeTruthy();
    });

    it('an asset selected from outside leaves the review alone while a card verb runs', async () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      answerPlans();
      const { rerender } = render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:11' });
      await fireEvent.click(screen.getByTestId('card-review-11'));
      await screen.findByTestId('sync-plan-view');
      await rerender({ ...handlers(), selectedKey: 'card:11', cardBusy: 'card' });
      await rerender({ ...handlers(), selectedKey: 'asset:personal:skill/edited', view: 'library', cardBusy: 'card' });
      await Promise.resolve();
      expect(screen.getByTestId('sync-plan-view')).toBeTruthy();
    });

    it('a host whose plan failed is reported and the rest are still shown', async () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      answerPlans('htz');
      render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:11' });
      await fireEvent.click(screen.getByTestId('card-review-11'));
      await screen.findByTestId('sync-plan-view');
      expect(screen.getByTestId('plan-host-oci-claude')).toBeTruthy();
      expect(screen.queryByTestId('plan-host-htz-claude')).toBeNull();
    });

    it('the Hosts tab offers it too', async () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      answerPlans();
      render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:11' });
      await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
      await fireEvent.click(screen.getByTestId('card-review-11'));
      expect(await screen.findByTestId('sync-plan-view')).toBeTruthy();
    });

    it('a read-only window has no Review plan', () => {
      withCards([ROLLOUT_CARD, ROLLOUT_VIEW]);
      render(AssetsWorkspace, { ...handlers(), readOnly: true, selectedKey: 'card:11' });
      expect(screen.queryByTestId('card-review-11')).toBeNull();
    });
  });
});

describe('AssetsWorkspace keyboard', () => {
  const rowKey = () => (document.activeElement as HTMLElement | null)?.getAttribute('data-row-key');

  it('j/k move focus through the rows in display order; Enter selects', async () => {
    render(AssetsWorkspace, handlers());
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    expect(rowKey()).toBe('identity:skill/fresh');
    await fireEvent.keyDown(document.activeElement!, { key: 'k' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
    await fireEvent.click(document.activeElement!);
    expect(screen.getByTestId('inbox-row-asset:personal:skill/edited').getAttribute('aria-current')).toBe('true');
  });

  it('s syncs the focused asset, a adopts the focused identity, ⌘↵ runs the primary', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 's' });
    expect(h.onsync).toHaveBeenCalledWith({ kind: 'skill', name: 'edited' });
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'a' });
    expect(h.onimport).toHaveBeenCalledWith(expect.objectContaining({ kind: 'skill', name: 'fresh' }));
    await fireEvent.keyDown(document.activeElement!, { key: 'Enter', metaKey: true });
    expect(h.onsync).toHaveBeenLastCalledWith({});
  });

  it('/ focuses the query; keys typed there are the field’s', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: '/' });
    const q = screen.getByTestId('assets-query');
    expect(document.activeElement).toBe(q);
    await fireEvent.keyDown(q, { key: 's' });
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('read-only: moving and selecting only', async () => {
    const h = handlers();
    render(AssetsWorkspace, { ...h, readOnly: true });
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
    await fireEvent.keyDown(document.activeElement!, { key: 's' });
    await fireEvent.keyDown(document.activeElement!, { key: 'Enter', ctrlKey: true });
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('read-only: a and e do nothing', async () => {
    const h = handlers();
    render(AssetsWorkspace, { ...h, readOnly: true });
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'e' });
    expect(screen.queryByTestId('inspector-tab-source')).toBeNull();
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'a' });
    expect(h.onimport).not.toHaveBeenCalled();
  });

  it('k from the list goes to the last row; j/k stop at the ends', async () => {
    render(AssetsWorkspace, handlers());
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'k' });
    expect(rowKey()).toBe('identity:skill/fresh');
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(rowKey()).toBe('identity:skill/fresh');
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowUp' });
    await fireEvent.keyDown(document.activeElement!, { key: 'ArrowUp' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
  });

  it('j starts after the selected row when focus is on the list', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('inbox-row-asset:personal:skill/edited'));
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    expect(rowKey()).toBe('identity:skill/fresh');
  });

  it('j/k bring the row into view in the Inbox and in the Library', async () => {
    const seen: string[] = [];
    const spy = vi.fn(function (this: HTMLElement) {
      seen.push(this.getAttribute('data-row-key') ?? '');
    });
    const had = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
    Element.prototype.scrollIntoView = spy as unknown as Element['scrollIntoView'];
    try {
      render(AssetsWorkspace, handlers());
      const list = screen.getByTestId('assets-list');
      list.focus();
      await fireEvent.keyDown(list, { key: 'j' });
      expect(seen).toEqual(['asset:personal:skill/edited']);
      expect(spy).toHaveBeenLastCalledWith({ block: 'nearest' });
      await fireEvent.click(screen.getByTestId('assets-rail-library'));
      list.focus();
      await fireEvent.keyDown(list, { key: 'j' });
      expect(rowKey()).toBe('asset:personal:skill/edited');
      await fireEvent.keyDown(document.activeElement!, { key: 'j' });
      expect(rowKey()).toBe('asset:personal:skill/fine');
      expect(seen.slice(1)).toEqual(['asset:personal:skill/edited', 'asset:personal:skill/fine']);
    } finally {
      if (had) Object.defineProperty(Element.prototype, 'scrollIntoView', had);
      else delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it('the Library has one scroller, the list body (j/k scroll the same element in both views)', () => {
    const css = readFileSync('src/lib/AssetList.svelte', 'utf8').match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';
    const rule = /\.asset-list\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).not.toMatch(/overflow/);
    expect(rule).not.toMatch(/height/);
  });

  it('e opens the focused personal asset in Source, editing', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_get_asset') return { asset: { kind: 'skill', name: 'edited', version: '1', description: 'd', tags: [], body: '# b' }, previews: [], hosts: [] };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'e' });
    expect(screen.getByTestId('inbox-row-asset:personal:skill/edited').getAttribute('aria-current')).toBe('true');
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });

  it('e on the already-selected asset opens it in Source too', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_get_asset') return { asset: { kind: 'skill', name: 'edited', version: '1', description: 'd', tags: [], body: '# b' }, previews: [], hosts: [] };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    const row = screen.getByTestId('inbox-row-asset:personal:skill/edited');
    await fireEvent.click(row);
    expect(screen.getByTestId('inspector-tab-overview').getAttribute('aria-selected')).toBe('true');
    row.focus();
    await fireEvent.keyDown(row, { key: 'e' });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });

  it('e on the asset already being edited keeps the unsaved draft and only shows Source (final review I2)', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_get_asset') return { asset: { kind: 'skill', name: 'edited', version: '1', description: 'd', tags: [], body: '# b' }, previews: [], hosts: [] };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    const row = screen.getByTestId('inbox-row-asset:personal:skill/edited');
    await fireEvent.click(row);
    row.focus();
    await fireEvent.keyDown(row, { key: 'e' });
    const description = (await screen.findByTestId('editor-description')) as HTMLTextAreaElement;
    await fireEvent.input(description, { target: { value: 'unsaved words' } });
    await fireEvent.click(screen.getByTestId('inspector-tab-overview'));
    row.focus();
    await fireEvent.keyDown(row, { key: 'e' });
    expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).toBe('true');
    const after = (await screen.findByTestId('editor-description')) as HTMLTextAreaElement;
    expect(after).toBe(description);
    expect(after.value).toBe('unsaved words');
  });

  it('e and s leave an org catalog asset and an identity alone', async () => {
    catalog.set({
      ...listing,
      assets: [...listing.assets, { kind: 'skill', name: 'shared-one', version: '2', description: 'org', tags: [], catalog: 'acme', hosts: [] }],
    });
    const h = handlers();
    render(AssetsWorkspace, h);
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    const org = screen.getByTestId('asset-row-acme-skill-shared-one');
    org.focus();
    await fireEvent.keyDown(org, { key: 'e' });
    await fireEvent.keyDown(org, { key: 's' });
    expect(screen.queryByTestId('inspector-tab-source')).toBeNull();
    expect(h.onsync).not.toHaveBeenCalled();
    const id = screen.getByTestId('identity-row-skill-fresh');
    id.focus();
    await fireEvent.keyDown(id, { key: 'e' });
    await fireEvent.keyDown(id, { key: 's' });
    expect(screen.queryByTestId('inspector-tab-source')).toBeNull();
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('a on an asset row and s on an identity row do nothing', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'a' });
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 's' });
    expect(h.onimport).not.toHaveBeenCalled();
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('i on a card rejects its pending items', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('card-7'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'i' });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_reject_changeset_items', { args: { id: 7, positions: [0, 1] } }));
  });

  it('i on an identity a New card covers rejects that card', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'i' });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_reject_changeset_items', { args: { id: 7, positions: [0, 1] } }));
  });

  it('i on anything else does nothing', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    render(AssetsWorkspace, handlers());
    const list = screen.getByTestId('assets-list');
    await fireEvent.click(screen.getByTestId('inbox-row-asset:personal:skill/edited'));
    await fireEvent.keyDown(list, { key: 'i' });
    // An identity no card covers.
    catalog.set({ ...listing, identities: [{ ...listing.identities![0], name: 'other' }] });
    await fireEvent.click(await screen.findByTestId('inbox-row-identity:skill/other'));
    await fireEvent.keyDown(list, { key: 'i' });
    expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(0);
  });

  it('i does nothing for a read-only client or a card with nothing pending', async () => {
    withCards([NEW_CARD, { ...NEW_VIEW, items: NEW_VIEW.items.map((i) => ({ ...i, state: 'rejected' as const })) }]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    const { unmount } = render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('card-7'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'i' });
    unmount();
    withCards([NEW_CARD, NEW_VIEW]);
    render(AssetsWorkspace, { ...handlers(), readOnly: true });
    await fireEvent.click(screen.getByTestId('card-7'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'i' });
    expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(0);
  });

  it('a on an identity a New card covers applies that card instead of opening Import', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    const h = handlers();
    render(AssetsWorkspace, h);
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'a' });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_apply_changeset', { args: { id: 7 } }));
    expect(h.onimport).not.toHaveBeenCalled();
  });

  it('a on an identity a "needs a look" or a Hide card covers selects that card, never applies it', async () => {
    for (const first of [
      { ...NEW_VIEW.items[0], grp: 'needs a look', params: { reason: 'carries a secret' } },
      { ...NEW_VIEW.items[0], grp: 'hidden', action: 'hide' as const },
    ]) {
      const v = { ...NEW_VIEW, items: [first] };
      withCards([NEW_CARD, v]);
      answerCards([[NEW_CARD, v]]);
      const h = handlers();
      const { unmount } = render(AssetsWorkspace, h);
      await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
      await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'a' });
      await waitFor(() => expect(screen.getByTestId('card-7').getAttribute('aria-current')).toBe('true'));
      expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
      expect(h.onimport).not.toHaveBeenCalled();
      unmount();
    }
  });

  it('⌘↵ with a card selected runs its primary, not Sync', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    const h = handlers();
    render(AssetsWorkspace, h);
    await fireEvent.click(screen.getByTestId('card-7'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Enter', metaKey: true });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_apply_changeset', { args: { id: 7 } }));
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('⌘↵ with a drift card selected does nothing (its primary only selects)', async () => {
    withCards([DRIFT_CARD, DRIFT_VIEW]);
    answerCards([[DRIFT_CARD, DRIFT_VIEW]]);
    const h = handlers();
    render(AssetsWorkspace, h);
    await fireEvent.click(screen.getByTestId('card-8'));
    await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Enter', metaKey: true });
    expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('a card’s verbs reload the cards and the panel after they run', async () => {
    withCards([NEW_CARD, NEW_VIEW]);
    answerCards([[NEW_CARD, NEW_VIEW]]);
    const onrefresh = vi.fn();
    render(AssetsWorkspace, { ...handlers(), onrefresh });
    await fireEvent.click(screen.getByTestId('card-dismiss-7'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_dismiss_changeset', { args: { id: 7 } }));
    await waitFor(() => expect(onrefresh).toHaveBeenCalled());
    expect(cardCalls('catalog_list_changesets').length).toBeGreaterThan(0);
  });

  it('the arrows scroll the Inspector natively; j there still walks the list', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('inbox-row-identity:skill/fresh'));
    const panel = screen.getByTestId('inspector').querySelector<HTMLElement>('[role="tabpanel"]')!;
    panel.focus();
    const ev = new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true });
    panel.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(panel);
    await fireEvent.keyDown(panel, { key: 'k' });
    expect(rowKey()).toBe('asset:personal:skill/edited');
  });

  it('keys with a modifier other than ⌘↵ are not taken', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    for (const mod of ['altKey', 'metaKey', 'ctrlKey']) {
      const ev = new KeyboardEvent('keydown', { key: 'j', bubbles: true, cancelable: true, [mod]: true });
      list.dispatchEvent(ev);
      expect(ev.defaultPrevented, mod).toBe(false);
    }
    expect(document.activeElement).toBe(list);
  });

  it('keys inside a dialog (the catalog chip popover) stay the dialog’s', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    await fireEvent.click(screen.getByTestId('inbox-row-asset:personal:skill/edited'));
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    const inPopover = screen.getByTestId('assets-pull');
    expect(inPopover.closest('[role="dialog"]')).toBeTruthy();
    await fireEvent.keyDown(inPopover, { key: 's' });
    await fireEvent.keyDown(inPopover, { key: 'Enter', metaKey: true });
    await fireEvent.keyDown(inPopover, { key: 'j' });
    expect(h.onsync).not.toHaveBeenCalled();
    expect(rowKey()).not.toBe('asset:personal:skill/edited');
    // The same s on the list acts on the selection.
    const list = screen.getByTestId('assets-list');
    await fireEvent.keyDown(list, { key: 's' });
    expect(h.onsync).toHaveBeenCalledWith({ kind: 'skill', name: 'edited' });
  });

  it('⌘↵ waits while the panel is busy', async () => {
    const h = handlers();
    render(AssetsWorkspace, { ...h, busy: 'scan' });
    const list = screen.getByTestId('assets-list');
    await fireEvent.keyDown(list, { key: 'Enter', metaKey: true });
    await fireEvent.keyDown(list, { key: 'Enter', ctrlKey: true });
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('a hidden panel takes no keys', async () => {
    const h = handlers();
    render(AssetsWorkspace, { ...h, visible: false });
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(list, { key: 'Enter', metaKey: true });
    expect(document.activeElement).toBe(list);
    expect(h.onsync).not.toHaveBeenCalled();
  });

  it('a visible panel puts focus on the list, so j works at once', () => {
    render(AssetsWorkspace, handlers());
    expect(document.activeElement).toBe(screen.getByTestId('assets-list'));
  });

  it('Esc in the query: clears, then gives the list its focus back; Esc never leaves the field', async () => {
    render(AssetsWorkspace, handlers());
    const list = screen.getByTestId('assets-list');
    await fireEvent.keyDown(list, { key: '/' });
    const q = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(q, { target: { value: 'kind:' } });
    expect(screen.getByTestId('assets-query-completions')).toBeTruthy();
    const outside = vi.fn();
    document.addEventListener('keydown', outside);
    try {
      await fireEvent.keyDown(q, { key: 'Escape' }); // closes the completions
      expect(screen.getByTestId('assets-query-completions').hidden).toBe(true);
      expect(q.value).toBe('kind:');
      await fireEvent.keyDown(q, { key: 'Escape' }); // clears
      expect(q.value).toBe('');
      expect(document.activeElement).toBe(q);
      await fireEvent.keyDown(q, { key: 'Escape' }); // back to the list
      expect(document.activeElement).toBe(list);
      expect(outside).not.toHaveBeenCalled();
    } finally {
      document.removeEventListener('keydown', outside);
    }
  });

  it('the list shows a focus ring when it has the keyboard', () => {
    const css = readFileSync('src/lib/AssetsWorkspace.svelte', 'utf8').match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';
    expect(css).toMatch(/\.body:focus-visible\s*\{[^}]*outline:\s*var\(--ring-w\) solid var\(--ring\)/);
  });
});

const LAYER_CARD: ChangesetSummary = { id: 21, kind: 'layer', summary: 'New layer servers in personal', state: 'proposed', created_at: 1, catalogs: ['personal'] };
const LAYER_VIEW: ChangesetView = { id: 21, kind: 'layer', summary: LAYER_CARD.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal'], items: [
  { position: 0, grp: 'personal', catalog: 'personal', kind: 'layer', name: 'servers', action: 'create_layer', params: { layer: 'servers' }, decider: 'person', state: 'pending' },
] };
async function proposeServersLayer() {
  await fireEvent.click(screen.getByTestId('assets-rail-layers'));
  await fireEvent.click(screen.getByTestId('layers-new'));
  await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'servers' } });
  await fireEvent.click(screen.getByTestId('layer-form-submit'));
}

describe('AssetsWorkspace layers (R17, R5, R6)', () => {
  beforeEach(() => layersByCatalog.set({ personal: LAYERS }));

  it('the Layers rail entry shows the Layers view, grouped by catalog, in place of the Library', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    expect(screen.getByTestId('layers-view')).toBeTruthy();
    expect(screen.getByTestId('layers-catalog-personal')).toBeTruthy();
    expect(screen.getByTestId('layer-row-personal-core')).toHaveTextContent('1');
    expect(screen.queryByTestId('assets-new')).toBeNull();
  });

  it('a selected layer opens in the Inspector; its Hosts tab says why it is on a host', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    await fireEvent.click(screen.getByTestId('layer-row-personal-core'));
    expect(screen.getByTestId('layer-inspector')).toBeTruthy();
    expect(screen.getByTestId('layer-member-skill/fine')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('layer-why-oci')).toHaveTextContent('role server → extends core');
  });

  it('the personal listing stands in until every catalog’s layers have loaded', async () => {
    layersByCatalog.set(null);
    layerListing.set(LAYERS);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    expect(screen.getByTestId('layer-row-personal-core')).toBeTruthy();
  });

  it('read-only: the Layers view without New layer, Propose again, rename or move', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true });
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    expect(screen.queryByTestId('layers-new')).toBeNull();
    expect(screen.queryByTestId('layers-propose')).toBeNull();
    await fireEvent.click(screen.getByTestId('layer-row-personal-core'));
    expect(screen.getByTestId('layer-member-skill/fine')).toBeTruthy();
    expect(screen.queryByTestId('layer-rename')).toBeNull();
    expect(screen.queryByTestId('layer-move-skill/fine')).toBeNull();
  });

  it('proposing a layer change selects its card in the Inbox and offers Apply now', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_layer_change') return LAYER_VIEW;
      if (cmd === 'catalog_list_changesets') return [LAYER_CARD];
      if (cmd === 'catalog_get_changeset') return LAYER_VIEW;
      if (cmd === 'catalog_apply_changeset') return { ...LAYER_VIEW, state: 'applied', undoable: true, commits: { personal: 'abc1234' } };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await proposeServersLayer();
    expect(cardCalls('catalog_propose_layer_change')[0][1]).toEqual({ args: { change: { op: 'create', catalog: 'personal', layer: 'servers', axis: 'context' } } });
    await waitFor(() => expect(screen.getByTestId('assets-inbox')).toBeTruthy());
    await waitFor(() => expect(document.querySelector('[data-row-key="card:21"]')?.getAttribute('aria-current')).toBe('true'));
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('aria-current')).toBe('page');
    expect(screen.queryByTestId('layer-form')).toBeNull();
    const t = get(toasts).find((x) => x.message === 'Card ready: New layer servers in personal');
    expect(t?.action?.label).toBe('Apply now');
    t!.action!.run();
    await waitFor(() => expect(cardCalls('catalog_apply_changeset')).toEqual([['catalog_apply_changeset', { args: { id: 21 } }]]));
  });

  it('renaming from the Inspector proposes the change for that layer', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_layer_change') return { ...LAYER_VIEW, summary: 'Rename core → base' };
      if (cmd === 'catalog_list_changesets') return [LAYER_CARD];
      if (cmd === 'catalog_get_changeset') return LAYER_VIEW;
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    await fireEvent.click(screen.getByTestId('layer-row-personal-core'));
    await fireEvent.click(screen.getByTestId('layer-rename'));
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'base' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    await waitFor(() => expect(screen.getByTestId('assets-inbox')).toBeTruthy());
    expect(cardCalls('catalog_propose_layer_change')[0][1]).toEqual({ args: { change: { op: 'rename', catalog: 'personal', layer: 'core', to: 'base' } } });
  });

  it('an older hub refusing propose_layer is worded, and the form stays for another try', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_layer_change') throw { code: 'E_INVALID', message: 'unknown changesets action: propose_layer' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await proposeServersLayer();
    await waitFor(() => expect(get(toasts).some((t) => /^The hub is older/.test(t.message))).toBe(true));
    expect(screen.getByTestId('layers-view')).toBeTruthy();
    expect(screen.getByTestId('layer-form')).toBeTruthy();
  });

  it('another refusal is shown as an error toast', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_layer_change') throw { code: 'E_INVALID', message: 'layer exists' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await proposeServersLayer();
    await waitFor(() => expect(get(toasts).some((t) => t.kind === 'error' && t.message.includes('layer exists'))).toBe(true));
  });

  it('Propose again re-proposes, reloads the cards and says how many are open', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_changesets') return [NEW_CARD];
      if (cmd === 'catalog_list_changesets') return [NEW_CARD, LAYER_CARD];
      if (cmd === 'catalog_get_changeset') return NEW_VIEW;
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    await fireEvent.click(screen.getByTestId('layers-propose'));
    await waitFor(() => expect(get(toasts).some((t) => t.message === 'Proposed again: 2 open cards')).toBe(true));
    expect(cardCalls('catalog_list_changesets')).toHaveLength(1);
  });

  it('leaving the Layers view closes the New layer form', async () => {
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    await fireEvent.click(screen.getByTestId('layers-new'));
    expect(screen.getByTestId('layer-form')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('assets-rail-inbox'));
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    expect(screen.queryByTestId('layer-form')).toBeNull();
  });

  it('the form offers only the catalogs this window may write', async () => {
    const st = (name: string, o: Partial<CatalogStatus> = {}) => ({ name, org_id: null, state: 'loaded', ...o }) as CatalogStatus;
    catalogStatuses.set([st('personal'), st('acme', { org_id: 5, granted: [] }), st('broken', { state: 'problem' })]);
    hubStatus.set({ ...STANDALONE, remote: true, client_name: 'mac' });
    layersByCatalog.set({ personal: LAYERS, acme: LAYERS });
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-layers'));
    await fireEvent.click(screen.getByTestId('layers-new'));
    const opts = Array.from(screen.getByTestId('layer-form-catalog').querySelectorAll('option')).map((o) => o.value);
    expect(opts).toEqual(['personal']);
    // A catalog it may not write opens its layers read-only.
    await fireEvent.click(screen.getByTestId('layer-row-acme-core'));
    expect(screen.queryByTestId('layer-rename')).toBeNull();
  });
});

describe('AssetsWorkspace hosts (R18)', () => {
  const st = (name: string, o: Partial<CatalogStatus> = {}) => ({ id: 1, name, org_id: null, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded', asset_count: 0, admitted: [], ...o }) as CatalogStatus;
  const PAPAYA = st('papayapos', { id: 2, org_id: 7, admitted: ['local'] });
  const provenance = { provenance: { 'skill/w': { introduced_by: 'core', catalog: 'personal' } }, excluded: {}, refused: [], withheld: [], held_back: {}, assets: [] };
  beforeEach(() => {
    catalogStatuses.set([st('personal'), PAPAYA]);
    layersByCatalog.set({ personal: LAYERS });
    orgs.set([{ id: 7, name: 'papayapos' } as OrgDetail]);
    hosts.update((h) => [...h, { ...h[0], alias: 'trn', ssh_alias: 'trn', org_id: 7 }]);
  });
  const hubAnswers = (after: CatalogStatus[] = [st('personal'), PAPAYA]) =>
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_host_provenance') return provenance;
      if (cmd === 'catalog_admit_catalog' || cmd === 'catalog_unadmit_catalog') return ['oci'];
      if (cmd === 'catalog_list_catalogs') return after;
      if (cmd === 'catalog_list_layers_in') return LAYERS;
      throw { code: 'E_TEST', message: cmd };
    });

  it('the Hosts rail entry shows the Hosts view and a host selection shows its provenance', async () => {
    hubAnswers();
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    expect(screen.getByTestId('hosts-view')).toBeTruthy();
    expect(screen.queryByTestId('assets-new')).toBeNull();
    expect(screen.getByTestId('host-org-trn')).toHaveTextContent('papayapos');
    expect(screen.getByTestId('host-role-oci-personal')).toHaveTextContent('role server');
    await fireEvent.click(screen.getByTestId('host-row-oci'));
    expect(await screen.findByTestId('host-inspector')).toBeTruthy();
    expect(await screen.findByTestId('host-prov-line-skill/w')).toHaveTextContent('skill/w — via layer core from personal');
    expect(cardCalls('catalog_host_provenance')[0][1]).toEqual({ args: { host_alias: 'oci' } });
    // The Hosts view adds no primary: Sync fleet stays the one.
    expect(Array.from(document.querySelectorAll('.btn--primary')).map((b) => b.getAttribute('data-testid'))).toEqual(['assets-sync']);
  });

  it('toggling an admission admits, reloads the catalogs, and says so', async () => {
    hubAnswers([st('personal'), { ...PAPAYA, admitted: ['local', 'oci'] }]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    await waitFor(() => expect(get(toasts).some((t) => t.message === 'oci now receives papayapos')).toBe(true));
    expect(cardCalls('catalog_admit_catalog')[0][1]).toEqual({ args: { host_alias: 'oci', catalog: 'papayapos' } });
    expect(cardCalls('catalog_list_catalogs')).toHaveLength(1);
    await waitFor(() => expect(screen.getByTestId('host-accept-oci-papayapos')).toHaveAttribute('aria-pressed', 'true'));
  });

  it('toggling an admission for the selected host re-reads its provenance', async () => {
    hubAnswers([st('personal'), { ...PAPAYA, admitted: ['local', 'oci'] }]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    await fireEvent.click(screen.getByTestId('host-row-oci'));
    await screen.findByTestId('host-prov-line-skill/w');
    expect(cardCalls('catalog_host_provenance')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    await waitFor(() => expect(get(toasts).some((t) => t.message === 'oci now receives papayapos')).toBe(true));
    await waitFor(() => expect(cardCalls('catalog_host_provenance')).toHaveLength(2));
    expect(cardCalls('catalog_host_provenance')[1][1]).toEqual({ args: { host_alias: 'oci' } });
  });

  it('unadmitting says the host keeps what it has', async () => {
    hubAnswers([st('personal'), { ...PAPAYA, admitted: [] }]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    await fireEvent.click(screen.getByTestId('host-accept-local-papayapos'));
    await waitFor(() =>
      expect(get(toasts).some((t) => t.message === 'local no longer receives papayapos; what is installed stays until you remove it')).toBe(true),
    );
    expect(cardCalls('catalog_unadmit_catalog')[0][1]).toEqual({ args: { host_alias: 'local', catalog: 'papayapos' } });
  });

  it('a refused admission is an error toast and changes nothing', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_admit_catalog') throw { code: 'E_INVALID', message: 'oci has an org' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    await waitFor(() => expect(get(toasts).some((t) => t.kind === 'error')).toBe(true));
    expect(cardCalls('catalog_list_catalogs')).toHaveLength(0);
    expect(screen.getByTestId('host-accept-oci-papayapos')).not.toBeDisabled();
  });

  it('read-only: the Hosts view without admission toggles', async () => {
    render(AssetsWorkspace, { ...handlers(), readOnly: true });
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    expect(screen.getByTestId('host-accept-oci-papayapos')).toBeDisabled();
  });

  it('a hub client granted only personal cannot toggle papayapos: disabled, with the reason, and no call', async () => {
    hubAnswers();
    hubStatus.set({ ...STANDALONE, remote: true, client_name: 'desk', client_mode: 'full' });
    catalogStatuses.set([st('personal', { granted: ['desk'] }), { ...PAPAYA, granted: ['someone-else'] }]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    const t = screen.getByTestId('host-accept-oci-papayapos');
    expect(t).toBeDisabled();
    expect(t.getAttribute('title')).toBe('Needs a grant on papayapos: ask the operator');
    await fireEvent.click(t);
    await fireEvent.click(screen.getByTestId('host-accept-local-papayapos'));
    expect(cardCalls('catalog_admit_catalog')).toHaveLength(0);
    expect(cardCalls('catalog_unadmit_catalog')).toHaveLength(0);
  });

  it('a hub client granted papayapos can toggle it', async () => {
    hubAnswers([st('personal', { granted: ['desk'] }), { ...PAPAYA, granted: ['desk'], admitted: ['local', 'oci'] }]);
    hubStatus.set({ ...STANDALONE, remote: true, client_name: 'desk', client_mode: 'full' });
    catalogStatuses.set([st('personal', { granted: ['desk'] }), { ...PAPAYA, granted: ['desk'] }]);
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    await waitFor(() => expect(cardCalls('catalog_admit_catalog')).toHaveLength(1));
  });

  it('a failed catalog reload after a toggle is reported, beside the success', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_admit_catalog') return ['oci'];
      if (cmd === 'catalog_list_catalogs') throw { code: 'E_HUB_UNREACHABLE', message: 'hub did not answer' };
      throw { code: 'E_TEST', message: cmd };
    });
    render(AssetsWorkspace, handlers());
    await fireEvent.click(screen.getByTestId('assets-rail-hosts'));
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    await waitFor(() => expect(get(toasts).some((t) => t.kind === 'error' && t.message.startsWith("Reload catalogs: Couldn't reach the hub"))).toBe(true));
    expect(get(toasts).some((t) => t.message === 'oci now receives papayapos')).toBe(true);
  });
});

describe('AssetsWorkspace carries (Task 15)', () => {
  const css = readFileSync('src/lib/AssetsWorkspace.svelte', 'utf8').match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';
  const media = (q: string) => new RegExp(`@media \\(max-width: ${q}\\)\\s*\\{([\\s\\S]*?)\\n  \\}`).exec(css)?.[1] ?? '';

  it('the layout narrows: the rail to 56px under 1100px, the Inspector under the list under 860px', () => {
    expect(media('1100px')).toMatch(/grid-template-columns:\s*56px minmax\(0, 1fr\) minmax\(var\(--inspector-min\), var\(--list-w\)\)/);
    const narrow = media('860px');
    expect(narrow).toMatch(/grid-template-columns:\s*56px minmax\(0, 1fr\);/);
    expect(narrow).toContain("grid-template-areas: 'rail main' 'rail insp' 'foot foot'");
    expect(narrow).toMatch(/\.insp\s*\{[^}]*border-top/);
  });

  describe('a, s, e and i act only from the list or the Inspector', () => {
    const chip = () => screen.getByTestId('catalog-chip-personal');
    const SEL = 'asset:personal:skill/edited';

    it('s from the footer chip does nothing; from the list row it syncs', async () => {
      const h = handlers();
      render(AssetsWorkspace, { ...h, selectedKey: SEL });
      chip().focus();
      expect(document.activeElement).toBe(chip());
      await fireEvent.keyDown(chip(), { key: 's' });
      expect(h.onsync).not.toHaveBeenCalled();
      const row = screen.getByTestId(`inbox-row-${SEL}`);
      row.focus();
      await fireEvent.keyDown(row, { key: 's' });
      expect(h.onsync).toHaveBeenCalledWith({ kind: 'skill', name: 'edited' });
    });

    it('s from the Inspector syncs the selected asset', async () => {
      const h = handlers();
      render(AssetsWorkspace, { ...h, selectedKey: SEL });
      const insp = document.querySelector('.insp') as HTMLElement;
      await fireEvent.keyDown(insp, { key: 's' });
      expect(h.onsync).toHaveBeenCalledWith({ kind: 'skill', name: 'edited' });
    });

    it('e from the footer chip does not open the editor', async () => {
      render(AssetsWorkspace, { ...handlers(), selectedKey: SEL });
      await fireEvent.keyDown(chip(), { key: 'e' });
      expect(screen.queryByTestId('editor-save')).toBeNull();
      expect(screen.getByTestId('inspector-tab-source').getAttribute('aria-selected')).not.toBe('true');
    });

    it('a from the footer chip does not adopt', async () => {
      const h = handlers();
      render(AssetsWorkspace, { ...h, selectedKey: 'identity:skill/fresh' });
      await fireEvent.keyDown(chip(), { key: 'a' });
      expect(h.onimport).not.toHaveBeenCalled();
      await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'a' });
      expect(h.onimport).toHaveBeenCalledWith(expect.objectContaining({ kind: 'skill', name: 'fresh' }));
    });

    it('i from the footer chip does not reject; from the list it does', async () => {
      withCards([NEW_CARD, NEW_VIEW]);
      answerCards([[NEW_CARD, NEW_VIEW]]);
      render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:7' });
      await fireEvent.keyDown(chip(), { key: 'i' });
      expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(0);
      await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'i' });
      await waitFor(() => expect(cardCalls('catalog_reject_changeset_items')).toHaveLength(1));
    });

    it('j and / are not scoped: the list still moves from the footer', async () => {
      render(AssetsWorkspace, handlers());
      await fireEvent.keyDown(chip(), { key: 'j' });
      expect((document.activeElement as HTMLElement | null)?.getAttribute('data-row-key')).toBe('asset:personal:skill/edited');
    });
  });

  describe('the selected card is the primary only while the Inbox is on screen', () => {
    it('with the listing not loaded, ⌘↵ syncs and the header Sync keeps the primary', async () => {
      withCards([NEW_CARD, NEW_VIEW]);
      answerCards([[NEW_CARD, NEW_VIEW]]);
      catalog.set(null);
      const h = handlers();
      render(AssetsWorkspace, { ...h, selectedKey: 'card:7' });
      expect(screen.queryByTestId('card-7')).toBeNull();
      expect(primaries('.main')).toEqual(['assets-sync']);
      await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Enter', metaKey: true });
      expect(h.onsync).toHaveBeenCalledWith({});
      expect(cardCalls('catalog_apply_changeset')).toHaveLength(0);
    });

    it('with the Inbox shown, ⌘↵ applies the card (the contrast)', async () => {
      withCards([NEW_CARD, NEW_VIEW]);
      answerCards([[NEW_CARD, NEW_VIEW]]);
      render(AssetsWorkspace, { ...handlers(), selectedKey: 'card:7' });
      expect(primaries('.main')).toEqual(['card-primary-7']);
      await fireEvent.keyDown(screen.getByTestId('assets-list'), { key: 'Enter', metaKey: true });
      await waitFor(() => expect(cardCalls('catalog_apply_changeset')).toHaveLength(1));
    });
  });
});
