import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import PlanReady from './PlanReady.svelte';
import { markPlanFirst } from './plan_first';
import type { SessionRow } from './sessions';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const session = { id: 7, tmux_name: 'cf-x', friendly_name: 'Retry loop', host_alias: 'mercury' } as unknown as SessionRow;
const calls = () => vi.mocked(invoke).mock.calls.map((c) => c[0]);

describe('PlanReady', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'repo_changes') return [{ path: 'docs/plans/2026-11-10-retry.md', status: 'untracked', staged: false, orig_path: null }];
      if (cmd === 'repo_file') return { path: 'x', content: '# Retry plan\n\nStep one.', truncated: false, binary: false, is_dir: false, size: 10 };
      if (cmd === 'queue_prompt') return { delivered: true };
      return null;
    });
  });

  it('shows nothing for a session not started with Plan first, and reads nothing', async () => {
    render(PlanReady, { session, sKey: 7, status: 'idle' });
    await Promise.resolve();
    expect(screen.queryByTestId('plan-ready')).toBeNull();
    expect(calls()).not.toContain('repo_changes');
  });

  it('does not read the changes while the agent works', async () => {
    markPlanFirst(7);
    render(PlanReady, { session, sKey: 7, status: 'working' });
    await Promise.resolve();
    expect(calls()).not.toContain('repo_changes');
  });

  it('names the plan, shows it, and approve sends one prompt', async () => {
    markPlanFirst(7);
    render(PlanReady, { session, sKey: 7, status: 'idle' });
    await waitFor(() => expect(screen.getByTestId('plan-ready-path').textContent).toBe('docs/plans/2026-11-10-retry.md'));
    await fireEvent.click(screen.getByTestId('plan-ready-open'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-text').textContent).toContain('Retry plan'));
    await fireEvent.click(screen.getByTestId('plan-ready-approve'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-sent')).toBeTruthy());
    const sent = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'queue_prompt');
    expect(sent).toHaveLength(1);
    expect(JSON.stringify(sent[0][1])).toContain('docs/plans/2026-11-10-retry.md is approved');
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });

  it('ask for changes fills the composer and sends nothing', async () => {
    markPlanFirst(7);
    const onAskChanges = vi.fn();
    render(PlanReady, { session, sKey: 7, status: 'idle', onAskChanges });
    await waitFor(() => screen.getByTestId('plan-ready-changes'));
    await fireEvent.click(screen.getByTestId('plan-ready-changes'));
    expect(onAskChanges).toHaveBeenCalledWith(expect.stringContaining('docs/plans/2026-11-10-retry.md'));
    expect(calls()).not.toContain('queue_prompt');
  });

  it('a dismissed card stays away for that plan', async () => {
    markPlanFirst(7);
    render(PlanReady, { session, sKey: 7, status: 'idle' });
    await waitFor(() => screen.getByTestId('plan-ready-dismiss'));
    await fireEvent.click(screen.getByTestId('plan-ready-dismiss'));
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });
});
