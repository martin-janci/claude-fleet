// Gap plan G2.7, the FormsSession board's "Rename and label": the name and
// the label (the session's tags) in one sheet, one Save, Undo puts both back.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class {} }));

import { invoke } from '@tauri-apps/api/core';
import RenameLabelSheet from './RenameLabelSheet.svelte';
import { session } from './hosts_fixture';
import { sessions, type SessionRow } from './sessions';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests } from './access';
import { clearToasts, toasts } from './toasts';

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
const row = session('mefistos', 'dev-foo', { id: 7, friendly_name: 'Old name', tags: ['wip'] } as Partial<SessionRow>);

function calls(cmd: string) {
  return inv.mock.calls.filter((c) => c[0] === cmd).map((c) => (c[1] as { args: unknown }).args);
}
async function settle() {
  for (let i = 0; i < 4; i++) {
    await tick();
    await Promise.resolve();
  }
}

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, a?: { args: Record<string, unknown> }) => {
    if (cmd === 'set_session_friendly_name') return { ...row, friendly_name: a?.args.friendly_name || null };
    if (cmd === 'set_session_tags') return { ...row, tags: a?.args.tags };
    return null;
  });
  sessions.set([row]);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  clearToasts();
});

describe('RenameLabelSheet', () => {
  it('opens on the name and the label the session has', () => {
    render(RenameLabelSheet, { props: { session: row, onclose: () => {} } });
    expect((screen.getByTestId('rename-label-name') as HTMLInputElement).value).toBe('Old name');
    expect((screen.getByTestId('rename-label-label') as HTMLInputElement).value).toBe('wip');
    // Nothing changed yet: Save is off and says why.
    expect((screen.getByTestId('rename-label-save') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('sheet-why').textContent).toBe('Nothing changed.');
  });

  it('saves the label as the session tags and the name, and Undo puts both back', async () => {
    const onclose = vi.fn();
    render(RenameLabelSheet, { props: { session: row, onclose } });
    await fireEvent.input(screen.getByTestId('rename-label-name'), { target: { value: 'Release prep' } });
    await fireEvent.input(screen.getByTestId('rename-label-label'), { target: { value: 'release, wip' } });
    await fireEvent.click(screen.getByTestId('rename-label-save'));
    await settle();
    expect(calls('set_session_friendly_name')).toEqual([
      { host_alias: 'mefistos', tmux_name: 'dev-foo', friendly_name: 'Release prep' },
    ]);
    expect(calls('set_session_tags')).toEqual([{ session_id: 7, tags: ['release', 'wip'] }]);
    expect(onclose).toHaveBeenCalled();
    const toast = get(toasts).at(-1)!;
    expect(toast.message).toBe('Name and label saved');
    toast.action!.run();
    await settle();
    expect(calls('set_session_friendly_name').at(-1)).toEqual({ host_alias: 'mefistos', tmux_name: 'dev-foo', friendly_name: 'Old name' });
    expect(calls('set_session_tags').at(-1)).toEqual({ session_id: 7, tags: ['wip'] });
  });

  it('writes only what changed', async () => {
    render(RenameLabelSheet, { props: { session: row, onclose: () => {} } });
    await fireEvent.input(screen.getByTestId('rename-label-label'), { target: { value: '' } });
    await fireEvent.click(screen.getByTestId('rename-label-save'));
    await settle();
    expect(calls('set_session_tags')).toEqual([{ session_id: 7, tags: [] }]);
    expect(calls('set_session_friendly_name')).toEqual([]);
  });

  it('checks the label on blur and keeps Save off until it is fixed', async () => {
    render(RenameLabelSheet, { props: { session: row, onclose: () => {} } });
    const label = screen.getByTestId('rename-label-label');
    await fireEvent.input(label, { target: { value: 'fix;now' } });
    expect(screen.queryByTestId('rename-label-problem')).toBeNull();
    await fireEvent.blur(label);
    expect(screen.getByTestId('rename-label-problem').textContent).toContain('may only use letters');
    expect((screen.getByTestId('rename-label-save') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('sheet-why').textContent).toBe('Fix the label first.');
  });

  it('a refused save keeps the input and says so at the top', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'set_session_tags') throw { code: 'E_FORBIDDEN', message: 'not your session' };
      return null;
    });
    const onclose = vi.fn();
    render(RenameLabelSheet, { props: { session: row, onclose } });
    await fireEvent.input(screen.getByTestId('rename-label-label'), { target: { value: 'release' } });
    await fireEvent.click(screen.getByTestId('rename-label-save'));
    await settle();
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByTestId('rename-label-error').dataset.kind).toBe('refused');
    expect((screen.getByTestId('rename-label-label') as HTMLInputElement).value).toBe('release');
  });
});
