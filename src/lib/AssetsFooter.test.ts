import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsFooter from './AssetsFooter.svelte';
import { lastSyncRun, repoStatusStore, syncProgress, catalogConfig } from './assets';
import { catalogStatuses } from './assets_workspace';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const props = { readOnly: false, listing: null, busy: '', onpull: vi.fn(), oncommit: vi.fn(), onpush: vi.fn() };
const status = (name: string, org_id: number | null, head: string, extra: Record<string, unknown> = {}) => ({
  id: org_id ?? 1, name, org_id, repo_path: '/r', remote_url: null, head_commit: head, last_loaded_at: 1, state: 'loaded' as const, asset_count: 1, ...extra,
});
const run = (auto: boolean, over: Record<string, unknown> = {}) => ({
  plan_id: 'p', started_at: 1, finished_at: 2, auto,
  hosts: [{ host_alias: 'oci', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] }],
  ...over,
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockRejectedValue({ code: 'E_FORBIDDEN', message: 'no grant' });
  lastSyncRun.set(null); repoStatusStore.set(null); syncProgress.set(null); catalogConfig.set(null); catalogStatuses.set(null);
  fleetSettings.set({ ...SETTING_DEFAULTS });
});

describe('AssetsFooter', () => {
  it('a chip per catalog, auto, and the last sync (an SB6 run marked auto)', () => {
    catalogStatuses.set([status('personal', null, 'a1b2c3d9'), status('papayapos', 7, '9f0e1d2a')]);
    lastSyncRun.set(run(true));
    render(AssetsFooter, props);
    expect(screen.getByTestId('catalog-chip-personal')).toBeTruthy();
    expect(screen.getByTestId('catalog-chip-papayapos').textContent).toContain('@9f0e1d2');
    expect(screen.getByTestId('assets-auto').textContent).toBe('auto: on');
    const last = screen.getByTestId('assets-last-sync');
    expect(last.textContent).toContain('1 applied');
    expect(last.textContent).toContain('auto');
  });
  it('a person run is not marked auto', () => {
    lastSyncRun.set(run(false));
    render(AssetsFooter, props);
    const last = screen.getByTestId('assets-last-sync');
    expect(last.textContent).toContain('1 applied');
    expect(last.querySelector('[title*="SB6"]')).toBeNull();
  });
  it('auto: off follows catalog.auto', () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'catalog.auto': 'false' });
    render(AssetsFooter, props);
    expect(screen.getByTestId('assets-auto').textContent).toBe('auto: off');
  });
  it('with no catalog listing (refused), one personal chip from the repo status', () => {
    repoStatusStore.set({ head: 'abcdef1234', dirty: 0, ahead: 1, behind: 0, has_upstream: true });
    render(AssetsFooter, props);
    expect(screen.getByTestId('assets-head').textContent).toBe('personal @abcdef1 ↑1');
  });
  it('an org catalog that failed to load shows it on its chip', () => {
    catalogStatuses.set([status('personal', null, 'a1b2c3d9'), status('papayapos', 7, '', { state: 'problem', problem: 'schema_version 99', head_commit: null })]);
    render(AssetsFooter, props);
    const chip = screen.getByTestId('catalog-chip-papayapos');
    expect(chip.textContent).toContain('⚠');
    // Where it lives, then why it failed.
    expect(chip.getAttribute('title')).toBe('/r\nschema_version 99');
  });
  it('reads an org catalog\'s ahead and dirty from its own repo status', async () => {
    catalogStatuses.set([status('personal', null, 'a1b2c3d9'), status('papayapos', 7, '9f0e1d2a')]);
    invoke.mockImplementation(async (cmd: string, a: { args?: { name?: string } }) => {
      if (cmd === 'catalog_repo_status_in' && a?.args?.name === 'papayapos') return { head: '9f0e1d2a', dirty: 0, ahead: 4, behind: 0, has_upstream: true };
      throw { code: 'E_FORBIDDEN', message: 'no grant' };
    });
    render(AssetsFooter, props);
    await waitFor(() => expect(screen.getByTestId('catalog-chip-papayapos').textContent).toContain('↑4'));
  });
  it('re-reads an org catalog\'s status with each listing the panel loads, keeping an open popover open', async () => {
    catalogStatuses.set([status('personal', null, 'a1b2c3d9'), status('papayapos', 7, '9f0e1d2a')]);
    repoStatusStore.set({ head: 'a1b2c3d9', dirty: 0, ahead: 0, behind: 0, has_upstream: true });
    let ahead = 1;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_repo_status_in') return { head: '9f0e1d2a', dirty: 0, ahead, behind: 0, has_upstream: true };
      throw { code: 'E_FORBIDDEN', message: 'no grant' };
    });
    const listing = (loaded_at: number) => ({ head: 'a1b2c3d9', loaded_at, assets: [], unmanaged: [], problems: [] });
    const { rerender } = render(AssetsFooter, { ...props, listing: listing(1) });
    await waitFor(() => expect(screen.getByTestId('catalog-chip-papayapos').textContent).toContain('↑1'));
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('assets-pull')).toBeTruthy();

    ahead = 0;
    await rerender({ ...props, listing: listing(2) });
    await waitFor(() => expect(screen.getByTestId('catalog-chip-papayapos').textContent).not.toContain('↑'));
    expect(screen.getByTestId('assets-pull')).toBeTruthy();
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_repo_status_in')).toHaveLength(2);
  });
  it('each chip says where its catalog lives: an org one from its listing row, the personal fallback from the config', async () => {
    catalogStatuses.set([status('personal', null, 'a1b2c3d9', { repo_path: '/home/me/agent-assets', remote_url: 'git@x:me/a.git' }), status('papayapos', 7, '9f0e1d2a', { repo_path: '/srv/papayapos' })]);
    render(AssetsFooter, props);
    await fireEvent.click(screen.getByTestId('catalog-chip-papayapos'));
    expect(screen.getByTestId('catalog-where-papayapos').textContent).toContain('/srv/papayapos');
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('catalog-where-personal').textContent).toContain('/home/me/agent-assets');
    expect(screen.getByTestId('catalog-where-personal').textContent).toContain('git@x:me/a.git');
  });
  it('the personal fallback chip (no catalog listing) takes path and remote from the config', async () => {
    catalogConfig.set({ repo_path: '/cfg/assets', remote_url: 'git@y:me/b.git', head_commit: 'c0ffee12', last_loaded_at: 1 });
    render(AssetsFooter, props);
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('catalog-where-personal').textContent).toContain('/cfg/assets');
    expect(screen.getByTestId('catalog-where-personal').textContent).toContain('git@y:me/b.git');
  });
  it('shows the work in progress instead of the last sync', () => {
    syncProgress.set({ plan_id: 'p', host_alias: 'oci', harness: 'claude', done: 2, total: 5 });
    render(AssetsFooter, { ...props, busy: 'apply' });
    expect(screen.getByTestId('assets-job').textContent).toContain('Syncing 2/5');
    expect(screen.queryByTestId('assets-last-sync')).toBeNull();
  });
  it('words a scan in progress without a count', () => {
    render(AssetsFooter, { ...props, busy: 'scan' });
    expect(screen.getByTestId('assets-job').textContent).toContain('Scanning hosts');
    expect(screen.queryByRole('progressbar')).toBeNull();
  });
  it('says which assets the last sync could not apply for want of a secret', () => {
    lastSyncRun.set(run(false, {
      hosts: [{
        host_alias: 'oci', harness: 'claude', status: 'partial', detail: null, restart_required: false,
        actions: [{ kind: 'mcp', name: 'fleet', op: 'blocked', outcome: 'blocked', detail: 'missing secrets: FLEET_MCP_TOKEN' }],
      }],
    }));
    render(AssetsFooter, props);
    expect(screen.getByTestId('assets-blocked').textContent).toContain('1 blocked on a secret');
  });
  it('read-only: the chips are information, with no popover actions', async () => {
    render(AssetsFooter, { ...props, readOnly: true, listing: { head: 'feedface99', loaded_at: 1, assets: [], unmanaged: [], problems: [] } });
    expect(screen.getByTestId('assets-head').textContent).toBe('personal @feedfac');
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    for (const id of ['assets-pull', 'assets-commit-pending', 'assets-push']) expect(screen.queryByTestId(id)).toBeNull();
  });
  it('wires the personal popover to the callbacks', async () => {
    const onpush = vi.fn();
    repoStatusStore.set({ head: 'abcdef1234', dirty: 0, ahead: 1, behind: 0, has_upstream: true });
    render(AssetsFooter, { ...props, onpush });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    await fireEvent.click(screen.getByTestId('assets-push'));
    expect(onpush).toHaveBeenCalled();
  });
  describe('the live region', () => {
    const live = () => screen.getByTestId('assets-live');
    it('is always there, a polite status, empty when idle', () => {
      render(AssetsFooter, props);
      expect(live().getAttribute('role')).toBe('status');
      expect(live().getAttribute('aria-live')).toBe('polite');
      expect(live().textContent).toBe('');
    });
    it('names the job and its progress, says Finished. when it ends, and clears on the next job', async () => {
      const { rerender } = render(AssetsFooter, { ...props, busy: 'scan' });
      expect(live().textContent).toBe('Scanning hosts…');
      await rerender({ ...props, busy: 'apply' });
      expect(live().textContent).toBe('Syncing…');
      syncProgress.set({ plan_id: 'p', host_alias: 'oci', harness: 'claude', done: 2, total: 5 });
      await waitFor(() => expect(live().textContent).toBe('Syncing 2 of 5 hosts…'));
      await rerender({ ...props, busy: '' });
      await waitFor(() => expect(live().textContent).toBe('Finished.'));
      await rerender({ ...props, busy: 'pull' });
      await waitFor(() => expect(live().textContent).toBe('Pulling…'));
    });
    it('stays the same element across a job, so a screen reader announces its changes', async () => {
      const { rerender } = render(AssetsFooter, props);
      const el = live();
      await rerender({ ...props, busy: 'push' });
      await rerender({ ...props, busy: '' });
      expect(live()).toBe(el);
    });
    it('is not what the chip announces: the chip is visual only', () => {
      render(AssetsFooter, { ...props, busy: 'scan' });
      const chip = screen.getByTestId('assets-job');
      expect(chip.getAttribute('role')).toBeNull();
      expect(chip.getAttribute('aria-live')).toBeNull();
      expect(document.querySelectorAll('[role="status"]')).toHaveLength(1);
    });
  });
});
