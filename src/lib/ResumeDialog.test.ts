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

  it('an older hub (its plain link list for an answer) is explained, not offered', async () => {
    vi.mocked(invoke).mockImplementation(async () => [{ id: 1, state: 'confirmed', source: 'manual', created_at: 1 }]);
    render(ResumeDialog, { props: { workKey: 'ABC-1', onclose: () => {} } });
    await settle();
    expect(screen.getByTestId('resume-error')).toHaveTextContent(RESUME_UNSUPPORTED);
    expect((screen.getByTestId('resume-start') as HTMLButtonElement).disabled).toBe(true);
  });
});
