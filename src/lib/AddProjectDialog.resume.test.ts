import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import AddProjectDialog from './AddProjectDialog.svelte';
import { hosts } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const mockedInvoke = invoke as ReturnType<typeof vi.fn>;

const row = {
  project: { id: 7, owner: 'o', repo: 'r', base_path: '/p/o/r', last_session_at: null, adopted: false, system: false },
  worktrees: [],
};

const kept = {
  kind: 'add_project',
  key: '',
  label: 'acme/api',
  step: 1,
  answers: { mode: 'clone', host: 'mefistos', url: 'acme/api' },
  device: "Ada's Pixel",
  created_at: 100,
  updated_at: Math.floor(Date.now() / 1000) - 300,
};

function route(saved: unknown) {
  mockedInvoke.mockImplementation(async (cmd: string, args?: any) => {
    if (cmd === 'wizard_state') {
      const a = args.args;
      if (a.action === 'get') return saved;
      if (a.action === 'clear') return { removed: true };
      if (a.action === 'save') return { ...kept, ...a };
    }
    if (cmd === 'add_project') return row;
    return null;
  });
}

const wizardCalls = (action: string) =>
  mockedInvoke.mock.calls.filter((c) => c[0] === 'wizard_state' && c[1].args.action === action).map((c) => c[1].args);

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

function mount(props: Record<string, unknown> = {}) {
  const onCreated = vi.fn();
  render(AddProjectDialog, { props: { onCreated, onCancel: vi.fn(), saveDelayMs: 0, ...props } });
  return { onCreated };
}

beforeEach(() => {
  mockedInvoke.mockReset();
  hubStatus.set({ ...STANDALONE });
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  fleetSettings.set({ ...SETTING_DEFAULTS });
  localStorage.clear();
});

describe('AddProjectDialog resume (G7.2)', () => {
  it('offers what another device kept, and Resume fills it in', async () => {
    route(kept);
    mount();
    await flush();
    const banner = screen.getByTestId('add-resume');
    expect(banner.textContent).toContain('acme/api');
    expect(banner.textContent).toContain("on Ada's Pixel");
    await fireEvent.click(screen.getByTestId('add-resume-go'));
    await flush();
    expect(screen.queryByTestId('add-resume')).toBeNull();
    expect((screen.getByTestId('clone-url') as HTMLInputElement).value).toBe('acme/api');
  });

  it('saves nothing while the offer is unanswered', async () => {
    route(kept);
    mount();
    await flush();
    await fireEvent.click(screen.getByTestId('add-mode-clone'));
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'other/repo' } });
    await new Promise((r) => setTimeout(r, 5));
    expect(wizardCalls('save')).toHaveLength(0);
  });

  it('Start over drops the draft', async () => {
    route(kept);
    mount();
    await flush();
    await fireEvent.click(screen.getByTestId('add-resume-over'));
    await flush();
    expect(screen.queryByTestId('add-resume')).toBeNull();
    expect(wizardCalls('clear')).toEqual([{ action: 'clear', kind: 'add_project' }]);
  });

  it('keeps what is typed, and clears it once the project is added', async () => {
    route(null);
    const { onCreated } = mount({ initialMode: undefined });
    await flush();
    await fireEvent.click(screen.getByTestId('add-mode-clone'));
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'o/r' } });
    await flush();
    await new Promise((r) => setTimeout(r, 5));
    const saves = wizardCalls('save');
    expect(saves.length).toBeGreaterThan(0);
    expect(saves.at(-1)).toMatchObject({ kind: 'add_project', step: 1, label: 'o/r', answers: { mode: 'clone', url: 'o/r' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(wizardCalls('clear')).toHaveLength(1);
    expect(onCreated).toHaveBeenCalled();
  });

  it('a prefilled dialog neither asks nor offers', async () => {
    route(kept);
    mount({ initialCloneUrl: 'acme/widgets' });
    await flush();
    expect(wizardCalls('get')).toHaveLength(0);
    expect(screen.queryByTestId('add-resume')).toBeNull();
  });
});
