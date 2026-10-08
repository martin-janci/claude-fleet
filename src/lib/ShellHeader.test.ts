import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ShellHeader from './ShellHeader.svelte';
import { accounts } from './accounts';
import { accountUsage } from './account_usage_store';
import { accountsPageRequest } from './account_pill';
import { destination } from './destination';
import { switcherRequest } from './switcher_request';
import { ADMIN, GMAIL, SPARE, WORK, snapshot } from './hosts_fixture';
import { expectAccessible } from './a11y_check';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const mission = (id: number, state: string) => ({ id, name: `m${id}`, goal: '', mode: 'finite', state });

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'work_missions') return [mission(1, 'active'), mission(2, 'active'), mission(3, 'paused')];
    if (cmd === 'pause_all_missions') return [1, 2];
    throw { code: 'E_TEST', message: `unexpected ${cmd}` };
  });
  accounts.set([ADMIN]);
  accountUsage.set({ [ADMIN.uuid]: snapshot(ADMIN.uuid, { fetched_at: Math.floor(Date.now() / 1000) - 60, usage: null }) });
  switcherRequest.set(null);
  accountsPageRequest.set(null);
  destination.set('session');
});

describe('ShellHeader (3.17)', () => {
  it('reads as the Main board: mark and name, ⌘K, accounts, then Automation', async () => {
    render(ShellHeader, { mac: true });
    const h = screen.getByTestId('shell-header');
    expect(h.querySelector('.brand')!.textContent).toBe('Orbit Fleet');
    expect(h.querySelector('.of-mark')).toBeTruthy();
    const order = Array.from(h.querySelectorAll('[data-testid], .command')).map(
      (e) => e.getAttribute('data-testid') ?? 'command',
    );
    expect(order).toEqual(['command', 'header-account', 'header-automation', 'header-pause-all']);
    await waitFor(() => expect(screen.getByTestId('header-automation').textContent).toContain('2 active'));
  });

  it('the command field opens the switcher', async () => {
    render(ShellHeader, { mac: true });
    await fireEvent.click(screen.getByRole('button', { name: /Search or run a command/ }));
    expect(get(switcherRequest)).toEqual({ mode: 'switch' });
  });

  it('an account pill opens that account; past three, +N opens the page', async () => {
    accounts.set([ADMIN, GMAIL, SPARE, WORK]);
    render(ShellHeader, { mac: true });
    expect(screen.getAllByTestId('header-account')).toHaveLength(3);
    await fireEvent.click(screen.getAllByTestId('header-account')[0]);
    expect(get(destination)).toBe('accounts');
    expect(get(accountsPageRequest)).not.toBeNull();
    expect(screen.getByTestId('header-accounts-more').textContent).toBe('+1');
  });

  it('Pause all pauses every active mission', async () => {
    render(ShellHeader, { mac: true });
    const btn = screen.getByTestId('header-pause-all') as HTMLButtonElement;
    await waitFor(() => expect(btn.disabled).toBe(false));
    await fireEvent.click(btn);
    expect(invoke).toHaveBeenCalledWith('pause_all_missions', { args: {} });
  });

  it('passes the axe and audit checks', async () => {
    const { container } = render(ShellHeader, { mac: true });
    await waitFor(() => expect(screen.getByTestId('header-automation').textContent).toContain('active'));
    await expectAccessible(container);
  });
});
