import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import AssetList from './AssetList.svelte';
import type { AssetListing } from './assets';
import type { QueryRow } from './assets_query';

const baseListing = (): AssetListing => ({
  head: 'abc', loaded_at: 1, problems: [], assets: [], unmanaged: [],
});

describe('AssetList — identity badge and host-strip differ contract', () => {
  it('renders a needs_person badge with its reason, and marks exactly the hosts named in "copies differ on ..." as differing', () => {
    const listing: AssetListing = {
      ...baseListing(),
      identities: [
        {
          kind: 'skill',
          name: 'worktree',
          signature: 'local,oci,trn',
          variants: 2,
          class: 'needs_person',
          reason: 'copies differ on oci, trn',
          hosts: [
            { host_alias: 'local', harness: 'claude', host_hash: 'h1' },
            { host_alias: 'oci', harness: 'claude', host_hash: 'h2' },
            { host_alias: 'trn', harness: 'claude', host_hash: 'h3' },
          ],
        },
      ],
    };

    render(AssetList, {
      listing,
      selected: null,
      filter: '',
      onselect: () => {},
      onimport: vi.fn(),
    });

    const row = screen.getByTestId('identity-row-skill-worktree');
    expect(row.textContent).toContain('worktree');
    const badge = row.querySelector('.badge.warn');
    expect(badge).toBeTruthy();
    expect(badge?.textContent).toBe('copies differ on oci, trn');
    expect(badge?.getAttribute('title')).toBe('copies differ on oci, trn');

    // HostStrip: local is not named in the reason, so it must read
    // "present", while oci and trn — sliced out of the reason string via
    // the `"copies differ on "` (17-char) prefix and `", "` split — must
    // both read "differs". This pins the Rust <-> TS contract: the backend
    // reason sentence's exact prefix/separator is what `AssetList`'s
    // `oddHosts` parses.
    const strip = row.querySelector('.strip');
    expect(strip?.getAttribute('aria-label')).toBe('local: present, oci: differs, trn: differs');
  });

  it('an identity without a reason shows no badge and no differing host', () => {
    const listing: AssetListing = {
      ...baseListing(),
      identities: [
        {
          kind: 'skill', name: 'plain', signature: 'local', variants: 1, class: 'normal', reason: null,
          hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h1' }],
        },
      ],
    };

    render(AssetList, { listing, selected: null, filter: '', onselect: () => {}, onimport: vi.fn() });

    const row = screen.getByTestId('identity-row-skill-plain');
    expect(row.querySelector('.badge.warn')).toBeNull();
    const strip = row.querySelector('.strip');
    expect(strip?.getAttribute('aria-label')).toBe('local: present');
  });
});

describe('AssetList — one name in two catalogs (Assets M5, PF9)', () => {
  const colliding = (): AssetListing => ({
    ...baseListing(),
    assets: [
      { kind: 'skill', name: 's', version: '1', description: 'd', tags: [], hosts: [], catalog: 'personal' },
      { kind: 'skill', name: 's', version: '2', description: 'd', tags: [], hosts: [], catalog: 'acme' },
    ],
  });

  it('renders both rows, keyed by (catalog, name); only the personal one opens the detail', async () => {
    const onselect = vi.fn();
    render(AssetList, { listing: colliding(), selected: { kind: 'skill', name: 's' }, filter: '', onselect, onimport: vi.fn() });

    const mine = screen.getByTestId('asset-row-skill-s');
    const theirs = screen.getByTestId('asset-row-acme-skill-s');
    expect(mine.classList.contains('selected')).toBe(true);
    expect(theirs.classList.contains('selected')).toBe(false);
    expect(theirs.textContent).toContain('acme');

    theirs.click();
    expect(onselect).not.toHaveBeenCalled();
    mine.click();
    expect(onselect).toHaveBeenCalledWith('skill', 's', 'personal');
  });

  it('renders both rows read-only too; a row from an older hub (no catalog) is personal', () => {
    const listing = colliding();
    delete listing.assets[0].catalog;
    render(AssetList, { listing, selected: null, filter: '', readonly: true, onselect: () => {}, onimport: vi.fn() });
    expect(screen.getByTestId('asset-row-skill-s')).toBeTruthy();
    expect(screen.getByTestId('asset-row-acme-skill-s')).toBeTruthy();
  });
});

describe('AssetList — state counts, needs-person and orphan are Badges (Assets M5, R26)', () => {
  it('says each state count in words in a toned Badge, and an orphan in a warn Badge', () => {
    const listing: AssetListing = {
      ...baseListing(),
      assets: [
        {
          kind: 'skill', name: 's', version: '1', description: 'd', tags: [],
          hosts: [
            { host_alias: 'local', harness: 'claude', state: 'in_sync' },
            { host_alias: 'oci', harness: 'claude', state: 'drifted' },
            { host_alias: 'trn', harness: 'claude', state: 'missing' },
          ],
        },
      ],
      unmanaged: [
        {
          host_alias: 'oci', harness: 'claude', kind: 'skill', name: 'o', state: 'orphan',
          catalog_hash: null, host_hash: 'h', scanned_at: 1, managed: true,
        },
      ],
    };
    render(AssetList, { listing, selected: null, filter: '', onselect: () => {}, onimport: vi.fn() });

    const row = screen.getByTestId('asset-row-skill-s');
    const badges = Array.from(row.querySelectorAll('.chips .badge')).map((b) => [b.textContent, b.className]);
    expect(badges).toEqual([
      ['●1 in sync', expect.stringContaining('ok')],
      ['◐1 drifted', expect.stringContaining('warn')],
      ['○1 missing', expect.stringContaining('muted')],
    ]);
    const orphan = screen.getByTestId('orphan-badge-oci-claude-skill-o');
    expect(orphan.className).toContain('badge');
    expect(orphan.className).toContain('warn');
    expect(orphan.textContent).toBe('orphan');
  });
});

describe('AssetList — the Library (Assets M5)', () => {
  const two: AssetListing = {
    ...baseListing(),
    assets: [
      { kind: 'skill', name: 'w', version: '1', description: 'd', tags: [], hosts: [], catalog: 'personal' },
      { kind: 'skill', name: 'w', version: '1', description: 'd', tags: [], hosts: [], catalog: 'papayapos' },
      { kind: 'skill', name: 'other', version: '1', description: 'd', tags: [], hosts: [], catalog: 'personal', scope: 'shared' },
    ],
  };
  const keysOf = (c: ParentNode) => Array.from(c.querySelectorAll('[data-row-key]')).map((r) => r.getAttribute('data-row-key'));

  it('lists an asset once per catalog, with its scope or catalog badge and a row key', () => {
    const { container } = render(AssetList, { listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn() });
    expect(keysOf(container)).toEqual(['asset:personal:skill/w', 'asset:papayapos:skill/w', 'asset:personal:skill/other']);
    expect(container.querySelector('[data-row-key="asset:papayapos:skill/w"]')?.textContent).toContain('papayapos');
    expect(container.querySelector('[data-row-key="asset:personal:skill/w"]')?.textContent).toContain('private');
    expect(container.querySelector('[data-row-key="asset:personal:skill/other"]')?.textContent).toContain('shared');
  });

  it('keeps only the rows the query keeps', () => {
    const { container } = render(AssetList, {
      listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn(),
      keep: (r: { catalog?: string | null }) => r.catalog === 'papayapos',
    });
    expect(keysOf(container)).toEqual(['asset:papayapos:skill/w']);
  });

  it('shows an asset this window cannot write as managed and static, in its own group, selectable once the inspector opens it', async () => {
    const onselect = vi.fn();
    render(AssetList, {
      listing: two, selected: null, filter: '', onselect, onimport: vi.fn(), openStatic: true,
      canWrite: (a: { catalog?: string }) => a.catalog !== 'papayapos',
    });
    const row = document.querySelector('[data-row-key="asset:papayapos:skill/w"]') as HTMLElement;
    expect(row.tagName).toBe('DIV');
    expect(row.textContent).toContain('managed');
    expect(row.closest('[data-testid="library-managed"]')).toBeTruthy();
    expect(document.querySelector('[data-row-key="asset:personal:skill/w"]')?.tagName).toBe('BUTTON');
    expect(screen.getByTestId('library-managed-header').textContent).toMatch(/Managed, read-only\s*1/);
    expect(row.getAttribute('aria-disabled')).toBeNull();
    expect(row.classList.contains('inert')).toBe(false);
    await fireEvent.click(row);
    expect(onselect).toHaveBeenCalledWith('skill', 'w', 'papayapos');
    await fireEvent.keyDown(row, { key: 'Enter' });
    expect(onselect).toHaveBeenCalledTimes(2);
  });

  it('keeps a static row inert until the inspector can open it (openStatic off)', async () => {
    const onselect = vi.fn();
    render(AssetList, {
      listing: two, selected: null, filter: '', onselect, onimport: vi.fn(),
      canWrite: (a: { catalog?: string }) => a.catalog !== 'papayapos',
    });
    const row = document.querySelector('[data-row-key="asset:papayapos:skill/w"]') as HTMLElement;
    // Said in words, and no hover that promises a click.
    expect(row.getAttribute('aria-disabled')).toBe('true');
    expect(row.classList.contains('inert')).toBe(true);
    await fireEvent.click(row);
    await fireEvent.keyDown(row, { key: ' ' });
    expect(onselect).not.toHaveBeenCalled();
  });

  it('has no managed group when everything can be written; none per row when the whole window is read-only', () => {
    const w = render(AssetList, { listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn(), canWrite: () => true });
    expect(screen.queryByTestId('library-managed')).toBeNull();
    w.unmount();
    render(AssetList, { listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn(), readonly: true, canWrite: () => false });
    expect(screen.queryByTestId('library-managed')).toBeNull();
    expect(screen.getByTestId('asset-row-skill-other').textContent).not.toContain('managed');
  });

  it('draws one dot per host on each asset row, in words for assistive tech', () => {
    const listing: AssetListing = {
      ...baseListing(),
      assets: [{
        kind: 'skill', name: 'd', version: '1', description: '', tags: [], catalog: 'personal',
        hosts: [
          { host_alias: 'local', harness: 'claude', state: 'in_sync' },
          { host_alias: 'oci', harness: 'claude', state: 'drifted' },
        ],
      }],
    };
    render(AssetList, { listing, selected: null, filter: '', onselect: () => {}, onimport: vi.fn() });
    const strip = screen.getByTestId('asset-row-skill-d').querySelector('.strip');
    expect(strip?.getAttribute('aria-label')).toBe('local: in sync, oci: differs');
  });

  it('marks the selected asset by (catalog, name) and by key', () => {
    const { container } = render(AssetList, { listing: two, selected: { kind: 'skill', name: 'w', catalog: 'papayapos' }, filter: '', onselect: () => {}, onimport: vi.fn() });
    expect(container.querySelector('[data-row-key="asset:papayapos:skill/w"]')?.classList.contains('selected')).toBe(true);
    expect(container.querySelector('[data-row-key="asset:personal:skill/w"]')?.classList.contains('selected')).toBe(false);
  });

  it('gives identity and orphan rows a key, and picks them', async () => {
    const onpick = vi.fn();
    const listing: AssetListing = {
      ...baseListing(),
      identities: [{
        kind: 'skill', name: 'found', signature: 'local', variants: 1, class: 'normal', reason: null,
        hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }],
      }],
      unmanaged: [
        { host_alias: 'oci', harness: 'claude', kind: 'agent', name: 'gone', state: 'orphan', catalog_hash: null, host_hash: 'h', scanned_at: 1, managed: true },
        { host_alias: 'trn', harness: 'claude', kind: 'agent', name: 'gone', state: 'orphan', catalog_hash: null, host_hash: 'h', scanned_at: 1, managed: true },
      ],
    };
    const { container } = render(AssetList, { listing, selected: null, selectedKey: 'orphan:agent/gone', filter: '', onselect: () => {}, onimport: vi.fn(), onpick });
    expect(keysOf(container)).toEqual(['identity:skill/found', 'orphan:agent/gone']);
    await fireEvent.click(container.querySelector('[data-row-key="identity:skill/found"]')!);
    expect(onpick).toHaveBeenLastCalledWith('identity:skill/found');
    const orphan = container.querySelector('[data-row-key="orphan:agent/gone"]') as HTMLElement;
    expect(orphan.classList.contains('selected')).toBe(true);
    expect(orphan.querySelector('.strip')?.getAttribute('aria-label')).toBe('local: not here, oci: differs, trn: differs');
    await fireEvent.keyDown(orphan, { key: 'Enter' });
    expect(onpick).toHaveBeenLastCalledWith('orphan:agent/gone');
  });

  it('Import beside an identity row imports without picking it', async () => {
    const onimport = vi.fn();
    const onpick = vi.fn();
    const listing: AssetListing = {
      ...baseListing(),
      identities: [{
        kind: 'skill', name: 'found', signature: 'local', variants: 1, class: 'normal', reason: null,
        hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }],
      }],
    };
    render(AssetList, { listing, selected: null, filter: '', onselect: () => {}, onimport, onpick });
    await fireEvent.click(screen.getByRole('button', { name: 'Import found' }));
    expect(onimport).toHaveBeenCalledOnce();
    expect(onpick).not.toHaveBeenCalled();
  });

  it('without onpick the S1a rows still carry their key, and are plain rows', () => {
    const listing: AssetListing = {
      ...baseListing(),
      identities: [{
        kind: 'skill', name: 'found', signature: 'local', variants: 1, class: 'normal', reason: null,
        hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }],
      }],
      unmanaged: [{ host_alias: 'oci', harness: 'claude', kind: 'agent', name: 'gone', state: 'orphan', catalog_hash: null, host_hash: 'h', scanned_at: 1, managed: true }],
    };
    const { container } = render(AssetList, { listing, selected: null, filter: '', onselect: () => {}, onimport: vi.fn() });
    expect(keysOf(container)).toEqual(['identity:skill/found', 'orphan:agent/gone']);
    expect(container.querySelector('[data-row-key="orphan:agent/gone"]')?.getAttribute('role')).toBeNull();
  });

  it('applies keep to identities and orphans too (one search behaviour)', () => {
    const listing: AssetListing = {
      ...baseListing(),
      identities: [{
        kind: 'skill', name: 'found', signature: 'local', variants: 1, class: 'normal', reason: null,
        hosts: [{ host_alias: 'local', harness: 'claude', host_hash: 'h' }],
      }],
      unmanaged: [{ host_alias: 'oci', harness: 'claude', kind: 'agent', name: 'gone', state: 'orphan', catalog_hash: null, host_hash: 'h', scanned_at: 1, managed: true }],
    };
    const { container } = render(AssetList, { listing, selected: null, filter: '', onselect: () => {}, onimport: vi.fn(), keep: (r: { kind: string }) => r.kind === 'agent' });
    expect(keysOf(container)).toEqual(['orphan:agent/gone']);
  });

  it('hands keep the row the query reads: the catalog, scope, layers and managed flag', () => {
    const seen: QueryRow[] = [];
    render(AssetList, {
      listing: two, selected: null, filter: '', onselect: () => {}, onimport: vi.fn(),
      keep: (r: QueryRow) => (seen.push(r), true),
      layersOf: (a: { name: string }) => (a.name === 'other' ? ['core'] : []),
      canWrite: (a: { catalog?: string }) => a.catalog !== 'papayapos',
    });
    const o = seen.find((r) => r.name === 'other')!;
    expect(o).toMatchObject({ catalog: 'personal', scope: 'shared', layers: ['core'], managedElsewhere: false });
    expect(seen.find((r) => r.catalog === 'papayapos')).toMatchObject({ managedElsewhere: true });
  });
});
