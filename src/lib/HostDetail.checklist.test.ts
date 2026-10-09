// Orbit Fleet 4.7: the host detail's health checklist, Re-provision and
// New session here, against a mocked backend.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import HostDetail from './HostDetail.svelte';
import { hostChecks } from './host_check';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { NOW, host } from './hosts_fixture';
import { uiLayout } from './prefs';
import { expectAccessible } from './a11y_check';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;
let guardInstalled = false;

beforeEach(() => {
  guardInstalled = false;
  uiLayout.set('classic');
  hostChecks.set(new Map());
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, payload?: { args?: { alias?: string }; host?: string }) => {
    switch (cmd) {
      case 'check_host':
        return {
          alias: payload?.args?.alias,
          checked_at: NOW,
          error: null,
          tmux_version: '3.5a',
          agents_on_path: ['claude'],
          fleet_hooks: true,
          guard_hook: guardInstalled,
        };
      case 'provision_hosts':
        guardInstalled = true;
        return [{ host: payload?.host, status: 'provisioned', detail: null }];
      default:
        return null;
    }
  });
});

function mount(over: Record<string, unknown> = {}) {
  const props = {
    host: host('mercury'),
    account: null,
    snapshot: null,
    sharedWith: [],
    hostSessions: [],
    token: null,
    tokensLoaded: true,
    hook: { state: 'seen' as const, lastAt: NOW - 300 },
    attention: null,
    now: NOW,
    locale: 'en-GB',
    timeZone: 'UTC',
    editingNickname: false,
    oneditstart: vi.fn(),
    oneditdone: vi.fn(),
    onreprobe: vi.fn(),
    onrefreshusage: vi.fn(),
    onnewsession: vi.fn(),
    ...over,
  };
  render(HostDetail, { props });
  return props;
}

const checkRow = (key: string) =>
  screen.getAllByTestId('detail-check-row').find((r) => r.dataset.key === key)!;

describe('HostDetail: health checklist', () => {
  it('runs only on demand, then shows a failing guard hook and advises a re-provision', async () => {
    mount();
    expect(inv.mock.calls.some((c) => c[0] === 'check_host')).toBe(false);
    expect(checkRow('guard').dataset.state).toBe('unknown');
    await fireEvent.click(screen.getByTestId('detail-run-checks'));
    await waitFor(() => expect(checkRow('guard').dataset.state).toBe('fail'));
    expect(within(checkRow('guard')).getByText(/not installed/)).toBeTruthy();
    expect(checkRow('hooks').dataset.state).toBe('ok');
    expect(screen.getByTestId('detail-reprovision').classList.contains('advised')).toBe(true);
  });

  it('Re-provision this host provisions only it, then checks again', async () => {
    mount();
    await fireEvent.click(screen.getByTestId('detail-run-checks'));
    await waitFor(() => expect(checkRow('guard').dataset.state).toBe('fail'));
    await fireEvent.click(screen.getByTestId('detail-reprovision'));
    await waitFor(() => expect(checkRow('guard').dataset.state).toBe('ok'));
    const prov = inv.mock.calls.find((c) => c[0] === 'provision_hosts')!;
    expect(prov[1]).toEqual({ rotate: false, host: 'mercury' });
  });

  it('New session here starts on this host', async () => {
    const props = mount();
    await fireEvent.click(screen.getByTestId('detail-new-session-here'));
    expect(props.onnewsession).toHaveBeenCalledTimes(1);
  });

  it('a paired client cannot run the checks or re-provision, and says why', () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    mount();
    const run = screen.getByTestId('detail-run-checks') as HTMLButtonElement;
    const prov = screen.getByTestId('detail-reprovision') as HTMLButtonElement;
    expect(run.disabled && prov.disabled).toBe(true);
    expect(run.title).toMatch(/paired client/);
  });

  it('New shows the Hex field with its own step text while checking and re-provisioning', async () => {
    uiLayout.set('new');
    const gates: Array<() => void> = [];
    const real = inv.getMockImplementation() as (cmd: string, payload?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, payload?: unknown) => {
      if (cmd === 'check_host' || cmd === 'provision_hosts') await new Promise<void>((r) => gates.push(r));
      return real(cmd, payload);
    });
    mount();
    expect(screen.queryByTestId('detail-check-live')).toBeNull();
    await fireEvent.click(screen.getByTestId('detail-run-checks'));
    const live = await screen.findByTestId('detail-check-live');
    expect(within(live).getByTestId('detail-check-live-step').textContent).toBe(
      'Checking mercury: tmux, hooks, agents and the guard…',
    );
    expect(live.querySelector('[data-testid^="detail-check-loader"]')).toBeTruthy();
    gates.shift()!();
    await waitFor(() => expect(screen.queryByTestId('detail-check-live')).toBeNull());
    await fireEvent.click(screen.getByTestId('detail-reprovision'));
    expect((await screen.findByTestId('detail-check-live-step')).textContent).toMatch(/^Re-provisioning mercury: /);
    gates.shift()!();
    // The re-check after a re-provision shows the checking text again.
    await waitFor(() => expect(screen.getByTestId('detail-check-live-step').textContent).toMatch(/^Checking mercury/));
    gates.shift()!();
    await waitFor(() => expect(screen.queryByTestId('detail-check-live')).toBeNull());
  });

  it('Classic shows no loader while checking', async () => {
    let release!: () => void;
    const real = inv.getMockImplementation() as (cmd: string, payload?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, payload?: unknown) => {
      if (cmd === 'check_host') await new Promise<void>((r) => (release = r));
      return real(cmd, payload);
    });
    mount();
    await fireEvent.click(screen.getByTestId('detail-run-checks'));
    await waitFor(() => expect(screen.getByTestId('detail-run-checks').textContent).toBe('checking…'));
    expect(screen.queryByTestId('detail-check-live')).toBeNull();
    release();
  });
});

describe('HostDetail: accessibility', () => {
  it('the host detail, checked, is accessible in New', async () => {
    uiLayout.set('new');
    try {
      mount();
      await fireEvent.click(screen.getByTestId('detail-run-checks'));
      await waitFor(() => expect(checkRow('guard').dataset.state).toBe('fail'));
      await expectAccessible(document.body);
    } finally {
      uiLayout.set('classic');
    }
  });
});
