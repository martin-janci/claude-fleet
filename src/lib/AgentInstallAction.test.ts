import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('./agent_install', async () => {
  const actual = await vi.importActual<typeof import('./agent_install')>('./agent_install');
  return { ...actual, agentInstalls: vi.fn(), installAgent: vi.fn() };
});

import AgentInstallAction from './AgentInstallAction.svelte';
import { agentInstalls, installAgent, INSTALL_POLL_MS, type AgentInstall } from './agent_install';

const mockedList = agentInstalls as unknown as ReturnType<typeof vi.fn>;
const mockedInstall = installAgent as unknown as ReturnType<typeof vi.fn>;

function job(over: Partial<AgentInstall> = {}): AgentInstall {
  return {
    id: 7,
    host_alias: 'mercury',
    version: '0.6.2',
    state: 'running',
    step: 'download',
    detail: null,
    started_at: Math.floor(Date.now() / 1000),
    finished_at: null,
    ...over,
  };
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve();
    await tick();
  }
}

describe('AgentInstallAction (review w14-4)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockedList.mockReset();
    mockedInstall.mockReset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('a failed poll keeps the step on screen and polls again, then finishes', async () => {
    const ondone = vi.fn();
    mockedList.mockResolvedValueOnce({ ok: true, value: [job()] });
    render(AgentInstallAction, { props: { alias: 'mercury', ondone } });
    await flush();
    expect(screen.getByTestId('agent-install-step').textContent).toContain('Downloading');

    // The hub link blips once.
    mockedList.mockResolvedValueOnce({ ok: false, error: { code: 'E_HUB_OFFLINE', message: 'down' } });
    await vi.advanceTimersByTimeAsync(INSTALL_POLL_MS);
    await flush();
    expect(screen.getByTestId('agent-install-step').textContent).toContain('Downloading');

    // It comes back and the job is done.
    mockedList.mockResolvedValue({ ok: true, value: [job({ state: 'done', step: 'done' })] });
    await vi.advanceTimersByTimeAsync(INSTALL_POLL_MS * 2);
    await flush();
    expect(ondone).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('agent-install-live')).toBeNull();
  });

  it('on mount, a job that failed lately shows its line beside Install', async () => {
    mockedList.mockResolvedValueOnce({
      ok: true,
      value: [job({ state: 'failed', detail: 'could not reach the host', finished_at: Math.floor(Date.now() / 1000) - 60 })],
    });
    render(AgentInstallAction, { props: { alias: 'mercury' } });
    await flush();
    expect(screen.getByTestId('agent-install-failed').textContent).toContain('could not reach the host');
    expect(screen.getByTestId('agent-install')).toBeInTheDocument();
  });
});
