import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import PlanReady from './PlanReady.svelte';
import { errorText } from './error_copy';
import { STANDALONE, hubStatus } from './hub';
import { hubConnection } from './hub_connection';
import type { SessionRow } from './sessions';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const session = { id: 7, tmux_name: 'cf-x', friendly_name: 'Retry loop', host_alias: 'mercury' } as unknown as SessionRow;
const PLAN = 'docs/plans/2026-11-10-retry.md';
const said = (at = 't1') => ({ text: `Plan ready: \`${PLAN}\`. Two open questions at the end.`, at });
const calls = () => vi.mocked(invoke).mock.calls.map((c) => c[0]);

describe('PlanReady', () => {
  beforeEach(() => {
    localStorage.clear();
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
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
    await waitFor(() => expect(screen.getByTestId('plan-ready-sent').textContent).toBe('Approved: Claude is starting.'));
    const sent = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'queue_prompt');
    expect(sent).toHaveLength(1);
    expect(JSON.stringify(sent[0][1])).toContain(`${PLAN} is approved`);
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });

  it('a long plan says it was cut short, and one that cannot be read says why', async () => {
    const file = { path: 'x', content: '# Retry plan', truncated: true, binary: false, is_dir: false, size: 10 };
    const answers = vi.mocked(invoke).getMockImplementation()!;
    let read: () => unknown = () => file;
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => (cmd === 'repo_file' ? read() : answers(cmd, args as never)));
    const first = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await fireEvent.click(screen.getByTestId('plan-ready-open'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-text').textContent).toContain('cut short'));
    first.unmount();

    const refusal = { code: 'E_SSH', message: 'The host did not answer.' };
    read = () => {
      throw refusal;
    };
    const second = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await fireEvent.click(screen.getByTestId('plan-ready-open'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-error').textContent).toBe(errorText(refusal)));
    // The card and its answers stay: the plan can be approved unread.
    expect((screen.getByTestId('plan-ready-approve') as HTMLButtonElement).disabled).toBe(false);
    second.unmount();

    read = () => ({ ...file, content: '', binary: true });
    render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await fireEvent.click(screen.getByTestId('plan-ready-open'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-error').textContent).toBe('The plan file cannot be shown here.'));
  });

  it('names the agent that wrote the plan', async () => {
    render(PlanReady, { session: { ...session, agent: 'codex' } as SessionRow, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => expect(screen.getByTestId('plan-ready').textContent).toContain('Codex wrote a plan and is waiting.'));
    expect((screen.getByTestId('plan-ready-approve') as HTMLButtonElement).title).toBe('Tells Codex the plan is approved and to start');
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
    // It stays away once that turn ends with no other announcement.
    await rerender({ session, sKey: 7, status: 'idle', said: null });
    await tick();
    expect(screen.queryByTestId('plan-ready')).toBeNull();
    expect(calls()).not.toContain('queue_prompt');
  });

  it('an approval that fails leaves the card up with the reason, to send again', async () => {
    const refusal = { code: 'E_SESSION_GONE', message: 'The session is not running.' };
    const answers = vi.mocked(invoke).getMockImplementation()!;
    let refuse = true;
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd !== 'queue_prompt') return answers(cmd, args as never);
      if (refuse) throw refusal;
      // The second try lands behind a turn that is running.
      return { delivered: false };
    });
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await fireEvent.click(screen.getByTestId('plan-ready-approve'));
    await waitFor(() => expect(screen.getByTestId('plan-ready-error').textContent).toBe(errorText(refusal)));
    expect(screen.queryByTestId('plan-ready-sent')).toBeNull();
    // Nothing was answered: the same words read again still ask.
    await rerender({ session, sKey: 7, status: 'idle', said: said() });
    const approve = screen.getByTestId('plan-ready-approve') as HTMLButtonElement;
    expect(approve.disabled).toBe(false);
    refuse = false;
    await fireEvent.click(approve);
    await waitFor(() => expect(screen.getByTestId('plan-ready-sent').textContent).toBe('Approved: Claude starts when its turn ends.'));
    expect(screen.queryByTestId('plan-ready-error')).toBeNull();
    expect(calls().filter((c) => c === 'queue_prompt')).toHaveLength(2);
  });

  it('with the hub away the card stays and its answers are off', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    hubConnection.set({ state: 'offline', attempt: 2, retry_in_secs: 5, reason: 'socket closed' });
    render(PlanReady, { session, sKey: 7, status: 'idle', said: said(), onAskChanges: vi.fn() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    const approve = screen.getByTestId('plan-ready-approve') as HTMLButtonElement;
    expect(approve.disabled).toBe(true);
    expect(approve.title).not.toContain('approved');
    expect((screen.getByTestId('plan-ready-changes') as HTMLButtonElement).disabled).toBe(true);
    // Reading the plan is not an answer.
    expect((screen.getByTestId('plan-ready-open') as HTMLButtonElement).disabled).toBe(false);
  });

  it('steps aside while the agent works on a turn no prompt opened, and is back when it stops', async () => {
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    // A notification: the same announcement, and the agent is busy.
    await rerender({ session, sKey: 7, status: 'working', said: said() });
    expect(screen.queryByTestId('plan-ready')).toBeNull();
    await rerender({ session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => expect(screen.getByTestId('plan-ready-path').textContent).toBe(PLAN));
    expect(calls().filter((c) => c === 'repo_changes')).toHaveLength(2);
  });

  it('a look that fails keeps the card it had', async () => {
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await rerender({ session, sKey: 7, status: 'working', said: said() });
    // The host does not answer the look after that turn: not "no plan".
    vi.mocked(invoke).mockRejectedValue({ code: 'E_SSH', message: 'The host did not answer.' });
    await rerender({ session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => expect(calls().filter((c) => c === 'repo_changes')).toHaveLength(2));
    await tick();
    expect(screen.getByTestId('plan-ready-path').textContent).toBe(PLAN);
  });

  it('a slow read is dropped once another session is open', async () => {
    const answers: ((changes: unknown) => void)[] = [];
    vi.mocked(invoke).mockImplementation((cmd: string) => (cmd === 'repo_changes' ? new Promise((answer) => answers.push(answer)) : Promise.resolve(null)));
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => expect(answers).toHaveLength(1));
    // The next session's agent said nothing about a plan.
    await rerender({ session: { ...session, id: 8 }, sKey: 8, status: 'idle', said: null });
    answers[0]([{ path: PLAN, status: 'untracked', staged: false, orig_path: null }]);
    await tick();
    await tick();
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });

  it("a session on another hub gets no card, and this hub's changes are not read for it", async () => {
    render(PlanReady, { session: { ...session, fleet: 'work' } as SessionRow, sKey: 7, status: 'idle', said: said() });
    await tick();
    await tick();
    expect(calls()).not.toContain('repo_changes');
    expect(screen.queryByTestId('plan-ready')).toBeNull();
  });

  it('reads the changes once for the same words', async () => {
    const { rerender } = render(PlanReady, { session, sKey: 7, status: 'idle', said: said() });
    await waitFor(() => screen.getByTestId('plan-ready'));
    await rerender({ session, sKey: 7, status: 'idle', said: said() });
    await Promise.resolve();
    expect(calls().filter((c) => c === 'repo_changes')).toHaveLength(1);
  });
});
