import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import AssetList from './AssetList.svelte';
import type { AssetListing } from './assets';

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
    expect(onselect).toHaveBeenCalledWith('skill', 's');
  });

  it('renders both rows read-only too; a row from an older hub (no catalog) is personal', () => {
    const listing = colliding();
    delete listing.assets[0].catalog;
    render(AssetList, { listing, selected: null, filter: '', readonly: true, onselect: () => {}, onimport: vi.fn() });
    expect(screen.getByTestId('asset-row-skill-s')).toBeTruthy();
    expect(screen.getByTestId('asset-row-acme-skill-s')).toBeTruthy();
  });
});
