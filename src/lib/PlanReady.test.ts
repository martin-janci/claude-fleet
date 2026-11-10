import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import PlanReady from './PlanReady.svelte';
import type { SessionRow } from './sessions';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const session = { id: 7, tmux_name: 'cf-x', friendly_name: 'Retry loop', host_alias: 'mercury' } as unknown as SessionRow;
const PLAN = 'docs/plans/2026-11-10-retry.md';
const said = (at = 't1') => ({ text: `Plan ready: \`${PLAN}\`. Two open questions at the end.`, at });
const calls = () => vi.mocked(invoke).mock.calls.map((c) => c[0]);

describe('PlanReady', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'repo_changes') return [{ path: PLAN, status: 'untracked', staged: false, orig_path: null }];
      if (cmd === 'repo_file') return { path: 'x', content: '# Retry plan\n\nStep one.', truncated: false, binary: false, is_dir: false, size: 10 };
      if (cmd === 'queue_prompt') return { delivered: true };
      return null;
    });
  });

  it('shows nothing until the agent says its plan is ready, and reads nothing', async () => {
    render(PlanReady, { session, sKey: 7, status: 'idle', said: null });
    await Promise.resolve();
    expect(screen.queryByTestId('plan-ready')).toBeNull();
    expect(calls()).not.toContain('repo_changes');
  });

  it('does not read the changes while the agent works', async () => {
    render(PlanReady, { session, sKey: 7, status: 'working', said: said() });
    await Promise.resolve();
    expect(calls()).not.toContain('repo_changes');
  });

  it('shows nothing for a plan the agent did not name', async () => {
    render(PlanReady, { session, sKey: 7, status: 'idle', said: { text: 'Plan ready: docs/plans/other.md', at: 't1' } });
    await waitFor(() => expect(calls()).toContain('repo_changes'));
    await Promise.resolve();
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });

  it('names the plan, shows it, and approve sends one prompt', async () => {
    render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => expect(screen.getByTestId('plan-ready-path').textContent).toBe(PLAN));
    await fireEvent.click(screen.getByTestId('plan-ready-open'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-text').textContent).toContain('Retry plan'));
    await fireEvent.click(screen.getByTestId('plan-ready-approve'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-sent')).toBeTruthy());
    const sent = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'queue_prompt');
    expect(sent).toHaveLength(1);
    expect(JSON.stringify(sent[0][1])).toContain(`${PLAN} is approved`);
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });

  it('ask for changes fills the composer and sends nothing', async () => {
    const onAskChanges = vi.fn();
    render(PlanReady, { session, sKey: 7, status: 'idle', said: said(), onAskChanges });
    await waitFor(() => screen.getByTestId('plan-ready-changes'));
    await fireEvent.click(screen.getByTestId('plan-ready-changes'));
    expect(onAskChanges).toHaveBeenCalledWith(expect.stringContaining(PLAN));
    expect(calls()).not.toContain('queue_prompt');
  });

  it('a hidden card stays away for that announcement, and the next one asks again', async () => {
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said('t1') });
    await waitFor(() => screen.getByTestId('plan-ready-dismiss'));
    await fireEvent.click(screen.getByTestId('plan-ready-dismiss'));
    expect(screen.queryByTestId('plan-ready')).toBeNull();
    // The same words read again (the panel re-reads the conversation): still hidden.
    await rerender({ session, sKey: 7, status: 'idle', said: said('t1') });
    await Promise.resolve();
    expect(screen.queryByTestId('plan-ready')).toBeNull();
    // Asked for changes, and the agent says it again after that prompt.
    await rerender({ session, sKey: 7, status: 'idle', said: said('t2') });
    await waitFor(() => expect(screen.getByTestId('plan-ready')).toBeTruthy());
  });

  it('the next prompt takes the card away, whoever sent it', async () => {
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    // "Looks good, go ahead", typed by hand or on another device: a new turn.
    await rerender({ session, sKey: 7, status: 'working', said: null });
    await waitFor(() => expect(screen.queryByTestId('plan-ready')).toBeNull());
    expect(calls()).not.toContain('queue_prompt');
  });

  it('reads the changes once for the same words', async () => {
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await rerender({ session, sKey: 7, status: 'idle', said: said() });
    await Promise.resolve();
    expect(calls().filter((c) => c === 'repo_changes')).toHaveLength(1);
  });
});
