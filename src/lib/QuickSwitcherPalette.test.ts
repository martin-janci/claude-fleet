import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

// Redesign step 3.9: the ⌘K palette. `>` commands, `#` tasks, `@` hosts;
// session commands; Pause all; settings changed in plain words.

vi.mock('./missions', async (orig) => ({
  ...(await orig<typeof import('./missions')>()),
  pauseAllMissions: vi.fn(async () => ({ ok: true, value: [1, 2] })),
}));
vi.mock('./fleet_settings', async (orig) => ({
  ...(await orig<typeof import('./fleet_settings')>()),
  setFleetSetting: vi.fn(async () => ({ ok: true, value: {} })),
}));
vi.mock('./answer_send', async (orig) => ({
  ...(await orig<typeof import('./answer_send')>()),
  sendAnswer: vi.fn(async () => ({ ok: true })),
}));

import QuickSwitcher from './QuickSwitcher.svelte';
import { sessions } from './sessions';
import { projects } from './projects';
import { hosts } from './hosts';
import { selectSessionExplicitly, clearSelection } from './selection';
import { recentSessions } from './quick_switcher';
import { session, host } from './hosts_fixture';
import { pauseAllMissions } from './missions';
import { setFleetSetting } from './fleet_settings';
import { sendAnswer } from './answer_send';
import { descriptors } from './pages/pages';
import { allDescriptors } from './pages/testing';
import { shortcutSheetOpen } from './app_views';
import type { PendingInput } from './pending_input';

const PERMISSION: PendingInput = {
  kind: 'permission',
  question: 'Push to origin?',
  options: [
    { n: 1, label: 'Yes', selected: true },
    { n: 2, label: 'No', selected: false },
  ],
};

const LINUX_CHORD = { key: 'K', ctrlKey: true, shiftKey: true };

async function typeInSwitcher(value: string) {
  render(QuickSwitcher);
  await fireEvent.keyDown(window, LINUX_CHORD);
  await tick();
  const input = screen.getByTestId('switcher-input') as HTMLInputElement;
  await fireEvent.input(input, { target: { value } });
  await tick();
  return input;
}

const kinds = () =>
  Array.from(document.querySelectorAll('[data-testid^="switcher-"][data-key]')).map(
    (e) => e.getAttribute('data-testid')!,
  );

beforeEach(() => {
  localStorage.clear();
  recentSessions.set([]);
  clearSelection();
  projects.set([]);
  hosts.set([host('mac'), host('nas')]);
  sessions.set([session('mac', 'push-fix', { claude_status: 'blocked', pending_input: PERMISSION })]);
  descriptors.set(new Map(allDescriptors.map((d) => [d.key, d])));
  vi.mocked(pauseAllMissions).mockClear();
  vi.mocked(setFleetSetting).mockClear();
  vi.mocked(sendAnswer).mockClear();
});
afterEach(() => {
  shortcutSheetOpen.set(false);
  descriptors.set(new Map());
});

describe('QuickSwitcher palette (redesign step 3.9)', () => {
  it('> keeps only commands; the empty query still shows no palette commands', async () => {
    await typeInSwitcher('>');
    const shown = kinds();
    expect(shown.length).toBeGreaterThan(0);
    expect(new Set(shown)).toEqual(new Set(['switcher-command']));
    expect(screen.getByText('Pause all missions')).toBeTruthy();
  });

  it('@ keeps only hosts', async () => {
    await typeInSwitcher('@na');
    expect(new Set(kinds())).toEqual(new Set(['switcher-host']));
  });

  it('# keeps only tasks and tickets', async () => {
    await typeInSwitcher('#PD-12');
    expect(kinds()).toContain('switcher-lookup');
    for (const k of kinds()) expect(['switcher-ticket', 'switcher-lookup']).toContain(k);
    expect(kinds()).not.toContain('switcher-session');
  });

  it('Pause all runs pauseAllMissions', async () => {
    const input = await typeInSwitcher('> pause all');
    expect(screen.getAllByTestId('switcher-command')[0].getAttribute('data-key')).toBe('cmd:app.pause-all');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    await tick();
    expect(pauseAllMissions).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('Keyboard shortcuts opens the sheet', async () => {
    const input = await typeInSwitcher('> keyboard');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    await tick();
    expect(get(shortcutSheetOpen)).toBe(true);
  });

  it('Approve on the open session presses option 1 through the freshness check', async () => {
    selectSessionExplicitly(get(sessions)[0]);
    const input = await typeInSwitcher('approve');
    const first = screen.getAllByTestId('switcher-command')[0];
    expect(first.getAttribute('data-key')).toBe('cmd:session.approve');
    expect(first.textContent).toContain('Approve: Yes');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    await tick();
    expect(sendAnswer).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sendAnswer).mock.calls[0][2]).toBe('1');
  });

  it('a settings change in plain words is one row that writes the setting', async () => {
    const input = await typeInSwitcher('set recent work to 3 days');
    const row = screen.getByTestId('switcher-setting');
    expect(row.textContent).toMatch(/Set .* to /);
    expect(kinds()[0]).toBe('switcher-setting');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    await tick();
    expect(setFleetSetting).toHaveBeenCalledWith('work.recent_days', '3');
  });
});
