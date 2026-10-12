import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import IdentityRow from './IdentityRow.svelte';
import type { AssetIdentity } from './assets';

const id = (over: Partial<AssetIdentity> = {}): AssetIdentity => ({
  kind: 'skill', name: 'worktree', signature: 'local,oci', variants: 2, class: 'needs_person',
  reason: 'copies differ on oci',
  hosts: [
    { host_alias: 'local', harness: 'claude', host_hash: 'h1' },
    { host_alias: 'oci', harness: 'claude', host_hash: 'h2' },
  ],
  ...over,
});

describe('IdentityRow — the one unmanaged-identity row (S1a), shared by the list and the Inbox', () => {
  it('plain: name, kind, reason badge, host strip, and an Import action', async () => {
    const onimport = vi.fn();
    const identity = id();
    render(IdentityRow, { identity, order: ['local', 'oci'], testid: 'identity-row-skill-worktree', onimport });
    const row = screen.getByTestId('identity-row-skill-worktree');
    expect(row.textContent).toContain('worktree');
    expect(row.querySelector('.badge.warn')?.textContent).toBe('copies differ on oci');
    expect(row.querySelector('.strip')?.getAttribute('aria-label')).toBe('local: present, oci: differs');
    expect(within(row).getByText('Import').getAttribute('aria-label')).toBeNull();
    await fireEvent.click(within(row).getByText('Import'));
    expect(onimport).toHaveBeenCalledWith(identity);
  });

  it('no Import when read-only or for a fleet internal', () => {
    const { unmount } = render(IdentityRow, { identity: id(), order: ['local'], testid: 't', readonly: true, onimport: vi.fn() });
    expect(within(screen.getByTestId('t')).queryByText('Import')).toBeNull();
    unmount();
    render(IdentityRow, { identity: id({ class: 'fleet_internal' }), order: ['local'], testid: 't', onimport: vi.fn() });
    expect(within(screen.getByTestId('t')).queryByText('Import')).toBeNull();
  });

  it('selectable: the row is one button carrying the key, with Import beside it, not inside it', async () => {
    const onselect = vi.fn();
    const onimport = vi.fn();
    render(IdentityRow, {
      identity: id({ class: 'normal', reason: null }), order: ['local', 'oci'], testid: 'unused', onimport,
      why: 'Found on local, oci', states: { local: 'present', oci: 'stale' },
      select: { key: 'identity:skill/worktree', testid: 'inbox-row-identity:skill/worktree', selected: true, onselect },
    });
    const pick = screen.getByTestId('inbox-row-identity:skill/worktree');
    expect(pick.tagName).toBe('BUTTON');
    expect(pick.getAttribute('data-row-key')).toBe('identity:skill/worktree');
    expect(pick.getAttribute('aria-current')).toBe('true');
    expect(pick.textContent).toContain('Found on local, oci');
    expect(pick.querySelector('.strip')?.getAttribute('aria-label')).toBe('local: present, oci: stale scan');
    expect(pick.querySelector('button')).toBeNull();
    await fireEvent.click(pick);
    expect(onselect).toHaveBeenCalledTimes(1);
    await fireEvent.click(screen.getByLabelText('Import worktree'));
    expect(onimport).toHaveBeenCalledTimes(1);
    expect(onselect).toHaveBeenCalledTimes(1);
  });
});
