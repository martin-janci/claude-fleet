import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => { throw { code: 'E_TEST', message: 'no backend' }; }) }));
import Toolkit from './Toolkit.svelte';
import { catalog, type AssetListing } from './assets';
import { assetsViewRequest } from './app_views';
import { toolkitTab } from './toolkit_skills';
import { expectAccessible } from './a11y_check';

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
  catalog.set(null);
  assetsViewRequest.set(null);
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

  it('passes the axe and audit checks', async () => {
    const { container } = render(Toolkit, { visible: true });
    await expectAccessible(container);
  });
});
