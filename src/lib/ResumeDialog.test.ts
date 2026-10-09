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

  it('lists the earlier sessions and re-plans for the one picked (step 5.10)', async () => {
    const asked: (number | null)[] = [];
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { link_id: number | null } }).args;
      if (cmd === 'work_resume_plan') {
        asked.push(args.link_id);
        const link = args.link_id ?? 5;
        return plan({
          link_id: link,
          host_alias: link === 5 ? 'h' : 'g',
          candidates: [
            { link_id: 5, name: 'Receipt totals rounding', host_alias: 'h', conversations: 2, ended_at: Math.floor(Date.now() / 1000) - 3 * 86400 },
            { link_id: 4, name: 'First attempt', host_alias: 'g', resumable: false },
          ],
          modes: [
            { mode: 'last', ok: link === 5, reason: 'its conversation is gone' },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
        });
      }
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'PD-2412', onclose: () => {} } });
    await settle();
    expect(screen.getByText('This work has 2 earlier sessions. Pick one to continue.')).toBeTruthy();
    expect((screen.getByTestId('resume-candidate-5') as HTMLInputElement).checked).toBe(true);
    expect(screen.getByTestId('resume-candidate-5').closest('label')).toHaveTextContent('h · 3 d ago · 2 conversations');
    expect(screen.getByTestId('resume-candidate-4').closest('label')).toHaveTextContent('conversation gone');
    await fireEvent.click(screen.getByTestId('resume-candidate-4'));
    await settle();
    // The re-plan for link 4, then the brief it now defaults to, also for 4.
    expect(asked[0]).toBeNull();
    expect(asked.slice(1).length).toBeGreaterThan(0);
    expect(asked.slice(1).every((l) => l === 4)).toBe(true);
    expect(screen.getByTestId('resume-where')).toHaveTextContent('Lands on g');
    expect((screen.getByTestId('resume-mode-brief') as HTMLInputElement).checked).toBe(true);
  });

  it('Start fresh instead switches to a fresh start', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_resume_plan')
        return plan({ modes: [{ mode: 'last', ok: true }, { mode: 'brief', ok: true }, { mode: 'fresh', ok: true }] });
      return null;
    });
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    await fireEvent.click(screen.getByTestId('resume-fresh-instead'));
    await settle();
    expect((screen.getByTestId('resume-mode-fresh') as HTMLInputElement).checked).toBe(true);
    expect(screen.getByTestId('resume-start')).toHaveTextContent('Fresh');
    expect(screen.queryByTestId('resume-fresh-instead')).toBeNull();
  });
});

// Step 5.12: "What changed" on Resume is the past session's summary, written
// on its own host and shown as a draft; the brief is rebuilt to include it.
describe('ResumeDialog: What changed (new layout)', () => {
  it('drafts the summary, says where it came from, and Clear empties it', async () => {
    const { uiLayout } = await import('./prefs');
    uiLayout.set('new');
    let briefs = 0;
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan') {
        if (args.with_brief) briefs++;
        return plan(args.with_brief ? { brief: `brief ${briefs}` } : {});
      }
      if (cmd === 'summarize_past_work')
        return {
          key: 'ABC-1',
          link_id: 5,
          host_alias: 'h',
          claude_session_id: 'x',
          model: 'haiku',
          journal_id: 1,
          at: 1,
          summary: 'Fixed the login redirect; tests still red.',
        };
      return null;
    });
    try {
      render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
      await settle();
      expect(briefs).toBe(1);
      await fireEvent.click(screen.getByTestId('resume-changed-run'));
      await settle();
      expect(invoke).toHaveBeenCalledWith('summarize_past_work', { args: { key: 'ABC-1', link_id: 5 } });
      const input = screen.getByTestId('resume-changed-draft-input') as HTMLTextAreaElement;
      expect(input.value).toBe('Fixed the login redirect; tests still red.');
      expect(screen.getByTestId('resume-changed-draft-meta')).toHaveTextContent(
        'by haiku on h · from its last conversation',
      );
      expect(briefs).toBe(2);
      expect((screen.getByTestId('resume-brief') as HTMLTextAreaElement).value).toBe('brief 2');
      await fireEvent.click(screen.getByTestId('resume-changed-draft-clear'));
      await settle();
      expect(screen.getByTestId('resume-changed-run')).toBeInTheDocument();
    } finally {
      uiLayout.set('classic');
    }
  });

  // Review r15 F22: the field is the person's, and the brief follows it.
  it('an edit to What changed reaches the brief, and Clear takes it out', async () => {
    const { uiLayout } = await import('./prefs');
    const { SUMMARY_HEADER } = await import('./resume_brief');
    uiLayout.set('new');
    const summary = 'Fixed the login redirect; tests still red.';
    let summarized = false;
    const built = () =>
      summarized ? ['# Handover', `${SUMMARY_HEADER} (now):`, summary, 'Title: Fix login'].join('\n') : '# Handover';
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: { with_brief?: boolean } }).args;
      if (cmd === 'work_resume_plan') return plan(args.with_brief ? { brief: built() } : {});
      if (cmd === 'summarize_past_work') {
        summarized = true;
        return { key: 'ABC-1', link_id: 5, host_alias: 'h', claude_session_id: 'x', model: 'haiku', journal_id: 1, at: 1, summary };
      }
      return null;
    });
    try {
      render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
      await settle();
      await fireEvent.click(screen.getByTestId('resume-changed-run'));
      await settle();
      const field = screen.getByTestId('resume-changed-draft-input') as HTMLTextAreaElement;
      await fireEvent.input(field, { target: { value: 'Tests are green now.' } });
      await settle();
      const briefBox = () => (screen.getByTestId('resume-brief') as HTMLTextAreaElement).value;
      expect(briefBox()).toBe(['# Handover', `${SUMMARY_HEADER} (now):`, 'Tests are green now.', 'Title: Fix login'].join('\n'));
      await fireEvent.click(screen.getByTestId('resume-changed-draft-clear'));
      await settle();
      expect(briefBox()).toBe(['# Handover', 'Title: Fix login'].join('\n'));
      expect(screen.queryByTestId('resume-changed-stale')).toBeNull();
    } finally {
      uiLayout.set('classic');
    }
  });

  it('the classic layout has no What changed', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_resume_plan' ? plan() : null));
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.queryByTestId('resume-changed')).toBeNull();
  });
});
