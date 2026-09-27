// The resume dialog (roadmap M2.5): it renders the hub's plan — disabled
// modes say why, live work offers Jump, the brief is previewed and the
// edited text is what reaches `resume_work`.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import ResumeDialog from './ResumeDialog.svelte';
import { sessions } from './sessions';
import { selectedSession, selectSession } from './selection';
import { session } from './hosts_fixture';
import { RESUME_UNSUPPORTED } from './work';

const plan = (over: Record<string, unknown> = {}) => ({
  key: 'ABC-1',
  title: 'Fix login',
  live: [],
  candidates: [{ link_id: 5, host_alias: 'h', branch: 'abc-1' }],
  link_id: 5,
  host_alias: 'h',
  project_id: 1,
  branch: 'abc-1',
  worktree: 'abc-1',
  worktree_present: false,
  modes: [
    { mode: 'last', ok: false, reason: 'its transcripts were purged' },
    { mode: 'brief', ok: true },
    { mode: 'fresh', ok: true },
  ],
  hosts: ['h', 'g'],
  ...over,
});

async function settle() {
  for (let i = 0; i < 5; i++) await tick();
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  sessions.set([]);
  selectSession(null);
});

describe('ResumeDialog', () => {
  it('shows why a mode is off, lands on the plan host, and defaults to a possible mode', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan') return plan(args.with_brief ? { brief: '# Handover: ABC-1' } : {});
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.getByTestId('resume-where')).toHaveTextContent('Lands on h');
    expect(screen.getByTestId('resume-where')).toHaveTextContent('recreated from the branch');
    const last = screen.getByTestId('resume-mode-last') as HTMLInputElement;
    expect(last.disabled).toBe(true);
    expect(last.closest('label')).toHaveTextContent('its transcripts were purged');
    // `last` is off, so the first possible mode (brief) is chosen and its
    // brief is previewed.
    expect((screen.getByTestId('resume-mode-brief') as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId('resume-brief') as HTMLTextAreaElement).value).toBe('# Handover: ABC-1');
  });

  it('an absent transcript reads as the reason and lands on the brief; a failed check only warns', async () => {
    const reason = "the conversation's transcript is no longer on h";
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan')
        return plan({
          modes: [
            { mode: 'last', ok: false, reason },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
        });
      return null;
    });
    const { unmount } = render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.getByTestId('resume-mode-last').closest('label')).toHaveTextContent(reason);
    expect((screen.getByTestId('resume-mode-brief') as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByTestId('resume-warning')).toBeNull();
    unmount();

    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan')
        return plan({
          modes: [
            { mode: 'last', ok: true },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
          warnings: ['could not check the transcript on h'],
        });
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.getByTestId('resume-warning')).toHaveTextContent('could not check the transcript on h');
    expect((screen.getByTestId('resume-mode-last') as HTMLInputElement).checked).toBe(true);
  });

  it('the edited brief is what the resume sends', async () => {
    const row = session('h', 'dev-new', { id: 42 });
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan') return plan(args.with_brief ? { brief: 'built brief' } : {});
      if (cmd === 'resume_work') return row;
      return null;
    });
    const onclose = vi.fn();
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose } });
    await settle();
    await fireEvent.input(screen.getByTestId('resume-brief'), { target: { value: 'my own words' } });
    await fireEvent.click(screen.getByTestId('resume-start'));
    await settle();
    const call = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'resume_work')!;
    expect(call[1]).toEqual({
      args: { key: 'ABC-1', mode: 'brief', link_id: 5, host_alias: null, brief: 'my own words' },
    });
    expect(onclose).toHaveBeenCalled();
    expect(get(selectedSession)?.id).toBe(42);
  });

  it('live work offers Jump and never a second session', async () => {
    const live = session('h', 'dev-live', { id: 9 });
    sessions.set([live]);
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'work_resume_plan'
        ? plan({
            live: [{ session_id: 9, host_alias: 'h', tmux_name: 'dev-live' }],
            modes: ['last', 'brief', 'fresh'].map((mode) => ({ mode, ok: false, reason: 'ABC-1 is live — jump to it' })),
          })
        : null,
    );
    const onclose = vi.fn();
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose } });
    await settle();
    expect((screen.getByTestId('resume-start') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByTestId('resume-jump'));
    expect(get(selectedSession)?.id).toBe(9);
    expect(onclose).toHaveBeenCalled();
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'resume_work')).toBe(false);
  });

  it('picking another host re-reads the plan and the brief for it, and the start lands there', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { host_alias?: string | null; with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan') {
        const host = args.host_alias ?? 'h';
        return plan({
          host_alias: host,
          modes: [
            { mode: 'last', ok: true },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
          ...(args.with_brief ? { brief: `brief for ${host}` } : {}),
        });
      }
      if (cmd === 'resume_work') return session('g', 'dev-g', { id: 43 });
      return null;
    });
    const onclose = vi.fn();
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose } });
    await settle();
    expect(screen.getByTestId('resume-where')).toHaveTextContent('Lands on h');
    expect((screen.getByTestId('resume-mode-last') as HTMLInputElement).checked).toBe(true);
    await fireEvent.change(screen.getByTestId('resume-host'), { target: { value: 'g' } });
    await settle();
    const plans = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_resume_plan');
    expect((plans.at(-1)![1] as { args: { host_alias: string | null } }).args.host_alias).toBe('g');
    expect(screen.getByTestId('resume-where')).toHaveTextContent('Lands on g');
    // The brief is built for the chosen host, not the plan's own.
    await fireEvent.click(screen.getByTestId('resume-mode-brief'));
    await settle();
    expect((screen.getByTestId('resume-brief') as HTMLTextAreaElement).value).toBe('brief for g');
    await fireEvent.click(screen.getByTestId('resume-start'));
    await settle();
    const call = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'resume_work')!;
    expect(call[1]).toEqual({
      args: { key: 'ABC-1', mode: 'brief', link_id: 5, host_alias: 'g', brief: 'brief for g' },
    });
    expect(get(selectedSession)?.id).toBe(43);
    expect(onclose).toHaveBeenCalled();
  });

  it('a resume the hub refuses shows its reason and keeps the dialog open', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan') return plan(args.with_brief ? { brief: 'built brief' } : {});
      if (cmd === 'resume_work') throw { code: 'E_SSH', message: 'h is unreachable' };
      return null;
    });
    const onclose = vi.fn();
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose } });
    await settle();
    await fireEvent.click(screen.getByTestId('resume-start'));
    await settle();
    expect(screen.getByTestId('resume-error')).toHaveTextContent('h is unreachable');
    expect(onclose).not.toHaveBeenCalled();
    expect(get(selectedSession)).toBeNull();
    // Not stuck on "Starting…": it can be tried again.
    expect((screen.getByTestId('resume-start') as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByTestId('resume-start')).toHaveTextContent('Fresh with brief');
  });

  it('an older hub (its plain link list for an answer) is explained, not offered', async () => {
    vi.mocked(invoke).mockImplementation(async () => [{ id: 1, state: 'confirmed', source: 'manual', created_at: 1 }]);
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.getByTestId('resume-error')).toHaveTextContent(RESUME_UNSUPPORTED);
    expect((screen.getByTestId('resume-start') as HTMLButtonElement).disabled).toBe(true);
  });
  it('Continue last conversation starts with mode last, no brief built or sent', async () => {
    const row = session('h', 'dev-last', { id: 44 });
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan')
        return plan({
          modes: [
            { mode: 'last', ok: true },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
        });
      if (cmd === 'resume_work') return row;
      return null;
    });
    const onclose = vi.fn();
    const onresumed = vi.fn();
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose, onresumed } });
    await settle();
    expect((screen.getByTestId('resume-mode-last') as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByTestId('resume-brief')).toBeNull();
    const start = screen.getByTestId('resume-start') as HTMLButtonElement;
    expect(start.disabled).toBe(false);
    expect(start).toHaveTextContent('Continue last conversation');
    await fireEvent.click(start);
    await settle();
    const call = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'resume_work')!;
    expect(call[1]).toEqual({
      args: { key: 'ABC-1', mode: 'last', link_id: 5, host_alias: null, brief: null },
    });
    // The brief is never built for this mode.
    const plans = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_resume_plan');
    expect(plans.map((c) => (c[1] as { args: { with_brief: boolean } }).args.with_brief)).toEqual([false]);
    expect(get(selectedSession)?.id).toBe(44);
    expect(onresumed).toHaveBeenCalledTimes(1);
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('a switch from brief back to last sends no brief, edited or not', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan')
        return plan({
          modes: [
            { mode: 'last', ok: true },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
          ...(args.with_brief ? { brief: 'built brief' } : {}),
        });
      if (cmd === 'resume_work') return session('h', 'dev-x', { id: 45 });
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'ABC-1', initialMode: 'brief', onclose: () => {} } });
    await settle();
    await fireEvent.input(screen.getByTestId('resume-brief'), { target: { value: 'my own words' } });
    await fireEvent.click(screen.getByTestId('resume-mode-last'));
    await settle();
    expect(screen.queryByTestId('resume-brief')).toBeNull();
    await fireEvent.click(screen.getByTestId('resume-start'));
    await settle();
    const call = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'resume_work')!;
    expect(call[1]).toEqual({
      args: { key: 'ABC-1', mode: 'last', link_id: 5, host_alias: null, brief: null },
    });
  });

  // Work graph M13.4c: a dead session's summary is asked for only after a
  // confirm, names the past session, and is shown as plain text.
  it('summarises the past session only once the person confirms', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan') return plan();
      if (cmd === 'summarize_work')
        return {
          key: 'ABC-1',
          link_id: 5,
          claude_session_id: '0a1b2c3d-0000-4000-8000-00000000abcd',
          host_alias: 'h',
          model: 'haiku',
          at: 1,
          summary: '[claude-fleet: message from a summary]\n<b>Done</b>: parser.',
        };
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    // Cancel: nothing runs.
    await fireEvent.click(screen.getByTestId('resume-summarise'));
    await settle();
    expect(screen.getByTestId('confirm-dialog')).toHaveTextContent('one model call');
    await fireEvent.click(screen.getByText('Cancel', { selector: '[data-testid="confirm-dialog"] button' }));
    await settle();
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'summarize_work')).toBe(false);
    // Confirm: one call for the chosen past session, the answer shown as text.
    await fireEvent.click(screen.getByTestId('resume-summarise'));
    await settle();
    await fireEvent.click(screen.getByTestId('resume-summarise-confirm'));
    await settle();
    const calls = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'summarize_work');
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toEqual({ args: { key: 'ABC-1', link_id: 5 } });
    const out = screen.getByTestId('resume-summary');
    expect(out).toHaveTextContent('<b>Done</b>: parser.');
    expect(out.querySelector('b')).toBeNull();
  });

  it('offers no summary for purged or live work and shows a refusal', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan')
        return plan({ candidates: [{ link_id: 5, host_alias: 'h', resumable: false }] });
      return null;
    });
    const { unmount } = render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.queryByTestId('resume-summarise')).toBeNull();
    unmount();

    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan') return plan();
      if (cmd === 'summarize_work')
        throw { code: 'E_NOTFOUND', message: "ABC-1: the conversation's transcript is no longer on h" };
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    await fireEvent.click(screen.getByTestId('resume-summarise'));
    await settle();
    await fireEvent.click(screen.getByTestId('resume-summarise-confirm'));
    await settle();
    expect(screen.getByTestId('resume-error')).toHaveTextContent('no longer on h');
    expect(screen.queryByTestId('resume-summary')).toBeNull();
  });
});
