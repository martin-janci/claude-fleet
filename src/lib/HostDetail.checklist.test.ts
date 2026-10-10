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
import { expectAccessible } from './a11y_check';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;
let guardInstalled = false;

beforeEach(() => {
  guardInstalled = false;
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

});

describe('HostDetail: fleet-agent install (4.9)', () => {
  let jobs: Record<string, unknown>[];
  beforeEach(() => {
    jobs = [];
    const real = inv.getMockImplementation() as (cmd: string, payload?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, payload?: { args?: { alias?: string; version?: string } }) => {
      if (cmd === 'install_agent') {
        const j = { id: 3, host_alias: payload?.args?.alias, version: payload?.args?.version, state: 'running', step: 'target', started_at: NOW };
        jobs = [j];
        return j;
      }
      if (cmd === 'agent_installs') return jobs;
      return real(cmd, payload);
    });
  });

  it('standalone: an SSH host needs no agent, and nothing offers one', () => {
    mount({ hubVersion: '0.5.4' });
    expect(checkRow('agent').dataset.state).toBe('na');
    expect(screen.queryByTestId('detail-agent-install')).toBeNull();
  });

  it('paired: "not installed · Install 0.5.4"; the click starts the job and its steps show beside the Hex field', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
      const props = mount({ hubVersion: '0.5.4' });
      expect(checkRow('agent').dataset.state).toBe('warn');
      expect(within(checkRow('agent')).getByText(/not installed/)).toBeTruthy();
      const btn = screen.getByTestId('detail-agent-install');
      expect(btn.textContent?.trim()).toBe('Install 0.5.4');
      expect(inv.mock.calls.some((c) => c[0] === 'install_agent')).toBe(false);
      await fireEvent.click(btn);
      expect(inv.mock.calls.find((c) => c[0] === 'install_agent')?.[1]).toEqual({
        args: { alias: 'mercury', version: '0.5.4' },
      });
      await waitFor(() => expect(screen.getByTestId('detail-agent-install-step').textContent).toContain('Reading which build fits'));
      jobs = [{ ...jobs[0], step: 'download' }];
      await vi.advanceTimersByTimeAsync(2100);
      await waitFor(() => expect(screen.getByTestId('detail-agent-install-step').textContent).toContain('SHA256SUMS'));
      // The Hex field, past its 400 ms delay.
      expect(screen.getByTestId('detail-agent-install-loader').dataset.loader).toBe('hex-field');
      jobs = [{ ...jobs[0], state: 'done', step: 'done', finished_at: NOW }];
      await vi.advanceTimersByTimeAsync(2100);
      await waitFor(() => expect(props.onreprobe).toHaveBeenCalledTimes(1));
    } finally {
      vi.useRealTimers();
    }
  });

  it('paired: a failed job says why and offers the button again', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    jobs = [{ id: 2, host_alias: 'mercury', version: '0.5.4', state: 'failed', step: 'download', detail: 'curl is not installed', started_at: NOW }];
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'install_agent') return jobs[0];
      if (cmd === 'agent_installs') return jobs;
      return null;
    });
    mount({ hubVersion: '0.5.4' });
    await fireEvent.click(screen.getByTestId('detail-agent-install'));
    expect((await screen.findByTestId('detail-agent-install-failed')).textContent).toBe('Failed · curl is not installed');
    expect(screen.getByTestId('detail-agent-install')).toBeTruthy();
  });
});

describe('HostDetail: accessibility', () => {
  it('the host detail, checked, is accessible in New', async () => {
    mount();
    await fireEvent.click(screen.getByTestId('detail-run-checks'));
    await waitFor(() => expect(checkRow('guard').dataset.state).toBe('fail'));
    await expectAccessible(document.body);
  });
});
