import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => { throw { code: 'E_TEST', message: 'no backend' }; }) }));
import { invoke } from '@tauri-apps/api/core';
import Toolkit from './Toolkit.svelte';
import { catalog, type AssetListing } from './assets';
import { assetsViewRequest } from './app_views';
import { toolkitTab } from './toolkit_skills';
import { expectAccessible } from './a11y_check';
import { hubStatus, STANDALONE } from './hub';

const listing: AssetListing = {
  head: 'h', loaded_at: 1, unmanaged: [], problems: [],
  assets: [
    { kind: 'skill', name: 'steward', version: '1.4', description: 'Drive a PR to green', tags: [], catalog: 'personal', hosts: [
      { host_alias: 'local', harness: 'claude', state: 'in_sync' },
      { host_alias: 'trn', harness: 'claude', state: 'drifted', drift_side: 'host' },
    ] },
    { kind: 'agent', name: 'helper', version: '1', description: '', tags: [], hosts: [] },
  ],
};

beforeEach(() => {
  toolkitTab.set('skills');
  catalog.set(listing);
});
afterEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async () => { throw { code: 'E_TEST', message: 'no backend' }; });
  catalog.set(null);
  assetsViewRequest.set(null);
  hubStatus.set({ ...STANDALONE });
});

describe('Toolkit (step 3.16)', () => {
  it('shows each skill per host, with a drifted host marked', () => {
    const { getByTestId } = render(Toolkit, { visible: true });
    expect(getByTestId('toolkit-skills-summary').textContent).toMatch(/1\s+skill · 1 out of sync/);
    const row = getByTestId('toolkit-skill-steward');
    const trn = row.querySelector('[data-host="trn"]')!;
    expect(trn.getAttribute('data-state')).toBe('edited');
    expect(trn.textContent).toBe('edited');
    expect(trn.getAttribute('title')).toBe('trn: edited on host');
    expect(row.querySelector('[data-host="local"]')!.textContent).toBe('✓ 1.4');
  });

  it('counts both tabs', () => {
    const { getByTestId } = render(Toolkit, { visible: true });
    const tabs = getByTestId('toolkit-tabs');
    expect(tabs.querySelector('[data-tab="skills"]')!.textContent).toContain('1');
    expect(tabs.querySelector('[data-tab="assets"]')!.textContent).toContain('2');
  });

  it('hands Sync and Edit to the Assets tab', async () => {
    // The Assets tab's panel takes each request as it mounts; record them.
    const asked: unknown[] = [];
    const stop = assetsViewRequest.subscribe((r) => r && asked.push(r));
    const { getByTestId, findByRole } = render(Toolkit, { visible: true });
    await fireEvent.click(getByTestId('toolkit-sync'));
    expect(get(toolkitTab)).toBe('assets');
    expect(getByTestId('toolkit').querySelector('[role="region"][aria-label="Assets catalog"]')).not.toBeNull();
    toolkitTab.set('skills');
    await fireEvent.click(await findByRole('button', { name: 'Edit steward' }));
    expect(get(toolkitTab)).toBe('assets');
    stop();
    expect(asked).toEqual([
      expect.objectContaining({ command: 'sync' }),
      expect.objectContaining({ select: 'asset:personal:skill/steward' }),
    ]);
  });

  it('+ Add skill opens New asset on that kind in the Assets tab (M15 G7.13)', async () => {
    const asked: unknown[] = [];
    const stop = assetsViewRequest.subscribe((r) => r && asked.push(r));
    toolkitTab.set('mcp');
    const { getByTestId } = render(Toolkit, { visible: true });
    expect(getByTestId('toolkit-add').textContent).toContain('+ Add MCP server');
    await fireEvent.click(getByTestId('toolkit-add'));
    expect(get(toolkitTab)).toBe('assets');
    stop();
    expect(asked).toEqual([expect.objectContaining({ newKind: 'mcp_server' })]);
    toolkitTab.set('skills');
  });

  it('says where to set up a catalog when none is loaded', async () => {
    catalog.set(null);
    const { getByTestId, getByRole } = render(Toolkit, { visible: true });
    expect(getByTestId('toolkit-skills-empty')).toBeTruthy();
    await fireEvent.click(getByRole('button', { name: 'Open the Assets catalog' }));
    expect(get(toolkitTab)).toBe('assets');
  });

  it('has its own nav: MCP servers and Hooks list their kind, and the rest links out (UX audit 2026-10-09, A2)', async () => {
    const { getByTestId, findByTestId } = render(Toolkit, { visible: true });
    const nav = getByTestId('toolkit-tabs');
    expect(Array.from(nav.querySelectorAll('button')).map((b) => b.textContent?.replace(/\d.*$/, '').trim())).toEqual([
      'Skills',
      'MCP servers',
      'Hooks',
      'Assets catalog',
      'Prompts & snippets',
      'Downloads…',
    ]);
    await fireEvent.click(nav.querySelector('[data-tab="mcp"]')!);
    expect(get(toolkitTab)).toBe('mcp');
    expect((await findByTestId('toolkit-mcp-summary')).textContent).toMatch(/0\s+MCP servers/);
    expect(nav.querySelector('[data-tab="mcp"]')!.getAttribute('aria-current')).toBe('page');
    toolkitTab.set('skills');
  });

  it('edits the prompt chips in its Prompts & snippets page', async () => {
    const { getByTestId, findByTestId } = render(Toolkit, { visible: true });
    await fireEvent.click(getByTestId('toolkit-tabs').querySelector('[data-tab="prompts"]')!);
    expect(get(toolkitTab)).toBe('prompts');
    expect(await findByTestId('toolkit-prompts')).toBeTruthy();
    expect(getByTestId('preset-add')).toBeTruthy();
    toolkitTab.set('skills');
  });

  it('says the device is read-only, and how to grant it, instead of offering buttons that do nothing', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_config') throw { code: 'E_FORBIDDEN', message: 'not granted' };
      if (cmd === 'catalog_list_assets') return listing;
      throw { code: 'E_TEST', message: 'no backend' };
    });
    const { findByTestId, getByTestId, getByRole } = render(Toolkit, { visible: true });
    expect((await findByTestId('toolkit-grant-cmd')).textContent).toMatch(/^fleet-hub client grant .+ assets$/);
    expect((getByTestId('toolkit-add') as HTMLButtonElement).disabled).toBe(true);
    expect((getByTestId('toolkit-sync') as HTMLButtonElement).disabled).toBe(true);
    expect(getByRole('button', { name: 'View steward' })).toBeTruthy();
  });

  it('keeps the unavailable notice when an older catalog check answers after it', async () => {
    // The first check is still waiting on the catalog when the hub goes
    // unavailable; its late answer must not take the notice back.
    let answer!: (v: null) => void;
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_config') return new Promise((r) => { answer = r; });
      if (cmd === 'catalog_list_assets') return listing;
      throw { code: 'E_TEST', message: 'no backend' };
    });
    const { findByTestId, getByTestId } = render(Toolkit, { visible: true });
    await vi.waitFor(() => expect(answer).toBeTypeOf('function'));
    hubStatus.set({ ...STANDALONE, unavailable: 'the hub is unreachable' });
    expect(await findByTestId('toolkit-unavailable')).toBeTruthy();
    answer(null);
    await new Promise((r) => setTimeout(r, 0));
    expect(getByTestId('toolkit-unavailable')).toBeTruthy();
    expect((getByTestId('toolkit-sync') as HTMLButtonElement).disabled).toBe(true);
  });

  it('quotes a device name with a space or an apostrophe in the grant command', async () => {
    const before = get(hubStatus);
    hubStatus.set({ ...before, client_name: "Martin's MacBook" });
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_config') throw { code: 'E_FORBIDDEN', message: 'not granted' };
      if (cmd === 'catalog_list_assets') return listing;
      throw { code: 'E_TEST', message: 'no backend' };
    });
    try {
      const { findByTestId } = render(Toolkit, { visible: true });
      expect((await findByTestId('toolkit-grant-cmd')).textContent).toBe(
        `fleet-hub client grant 'Martin'\\''s MacBook' assets`,
      );
    } finally {
      hubStatus.set(before);
    }
  });

  it('does not tell a read-only device to set up a catalog it cannot manage', async () => {
    catalog.set(null);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_config') throw { code: 'E_FORBIDDEN', message: 'not granted' };
      throw { code: 'E_TEST', message: 'no backend' };
    });
    const { findByTestId, queryByTestId } = render(Toolkit, { visible: true });
    expect(await findByTestId('toolkit-read-only')).toBeTruthy();
    expect(queryByTestId('toolkit-skills-empty')).toBeNull();
    expect(queryByTestId('toolkit-none')?.textContent).toBe('Nothing is listed yet.');
  });

  it('does not tell a device whose hub is unavailable to set up a catalog', async () => {
    catalog.set(null);
    const before = get(hubStatus);
    hubStatus.set({ ...before, unavailable: 'the hub is unreachable' });
    try {
      const { findByTestId, queryByTestId } = render(Toolkit, { visible: true });
      expect(await findByTestId('toolkit-unavailable')).toBeTruthy();
      expect(queryByTestId('toolkit-skills-empty')).toBeNull();
    } finally {
      hubStatus.set(before);
    }
  });

  it('explains the catalog and offers to set it up when there is none', async () => {
    catalog.set(null);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_config') return null;
      throw { code: 'E_CATALOG_NOT_CONFIGURED', message: 'no catalog' };
    });
    const { findByTestId, queryByTestId } = render(Toolkit, { visible: true });
    expect((await findByTestId('toolkit-setup')).textContent).toContain('git repository');
    expect(queryByTestId('toolkit-skills-empty')).toBeNull();
    await fireEvent.click(await findByTestId('toolkit-setup-open'));
    expect(get(toolkitTab)).toBe('assets');
  });

  it('lists the skills found on hosts but not in the catalog, with Import', async () => {
    const asked: unknown[] = [];
    const stop = assetsViewRequest.subscribe((r) => r && asked.push(r));
    catalog.set({
      ...listing,
      identities: [
        { kind: 'skill', name: 'pdf', hosts: [{ host_alias: 'trn', harness: 'claude', host_hash: 'a' }], signature: 'trn', variants: 1, class: 'normal', reason: null },
      ],
    });
    const { getByTestId, getByRole } = render(Toolkit, { visible: true });
    expect(getByTestId('toolkit-skills-summary').textContent).toMatch(/1\s+skill · 1 out of sync · 1 on hosts only/);
    const row = getByTestId('toolkit-skill-pdf');
    expect(row.querySelector('[data-host="trn"]')!.getAttribute('data-state')).toBe('unmanaged');
    await fireEvent.click(getByRole('button', { name: 'Import pdf' }));
    stop();
    expect(get(toolkitTab)).toBe('assets');
    expect(asked).toEqual([expect.objectContaining({ select: 'identity:skill/pdf' })]);
  });

  it('passes the axe and audit checks', async () => {
    const { container } = render(Toolkit, { visible: true });
    await expectAccessible(container);
  });
});
