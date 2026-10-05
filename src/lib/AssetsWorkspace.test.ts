import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { readFileSync } from 'node:fs';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsWorkspace from './AssetsWorkspace.svelte';
import { catalog, inventory, lastSyncRun, repoStatusStore, type AssetListing, type SyncRunSummary } from './assets';
import { catalogStatuses, changesetSummaries, layerListing, type CatalogStatus } from './assets_workspace';
import { hosts } from './hosts';
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
const handlers = () => ({ onscan: vi.fn(), onsync: vi.fn(), onimport: vi.fn(), onsecrets: vi.fn(), onnew: vi.fn(), onlintall: vi.fn() });

beforeEach(() => {
  invoke.mockReset();
  invoke.mockRejectedValue({ code: 'E_TEST', message: 'not in this test' });
  hubStatus.set(STANDALONE);
  catalog.set(listing); inventory.set([]); lastSyncRun.set(null); repoStatusStore.set(null);
  catalogStatuses.set(null); changesetSummaries.set(null); layerListing.set(null);
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

  it('a on an asset row and s on an identity row do nothing; i is not bound in M5', async () => {
    const h = handlers();
    render(AssetsWorkspace, h);
    const list = screen.getByTestId('assets-list');
    list.focus();
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 'a' });
    await fireEvent.keyDown(document.activeElement!, { key: 'i' });
    await fireEvent.keyDown(document.activeElement!, { key: 'j' });
    await fireEvent.keyDown(document.activeElement!, { key: 's' });
    await fireEvent.keyDown(document.activeElement!, { key: 'i' });
    expect(h.onimport).not.toHaveBeenCalled();
    expect(h.onsync).not.toHaveBeenCalled();
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
      expect(screen.queryByTestId('assets-query-completions')).toBeNull();
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
