// The prompt tuner's calls: the composer's goes to session_prompt_tune with
// the history capped, a routine's to routines { tune_prompt } with its draft.
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  TUNE_DRAFT_MAX_CHARS,
  TUNE_NEEDS_NEWER_HUB,
  tuneBlocked,
  tuneComposerPrompt,
  tuneRoutinePrompt,
} from './prompt_tune';
import { HELP_HISTORY_MAX } from './context_help';
import type { RoutineInput } from './routines';

const reply = { prompt: 'p', notes: '', unchanged: false, model: 'haiku', host_alias: 'h', at: 1 };

describe('prompt_tune', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('the composer sends the draft, the ask and the newest history', async () => {
    vi.mocked(invoke).mockResolvedValue(reply);
    const history = Array.from({ length: HELP_HISTORY_MAX + 3 }, (_, i) => `p${i}`);
    const r = await tuneComposerPrompt(7, { draft: 'fix it', history, commands: ['/plan'] }, '  shorter ');
    expect(r.ok).toBe(true);
    const [cmd, payload] = vi.mocked(invoke).mock.calls[0];
    expect(cmd).toBe('session_prompt_tune');
    const args = (payload as { args: Record<string, unknown> }).args;
    expect(args).toMatchObject({ session_id: 7, draft: 'fix it', ask: 'shorter', commands: ['/plan'] });
    expect((args.history as string[]).length).toBe(HELP_HISTORY_MAX);
    expect((args.history as string[])[0]).toBe('p3');
  });

  it('a routine sends its whole draft to routines tune_prompt', async () => {
    vi.mocked(invoke).mockResolvedValue(reply);
    const routine = { name: '', trigger: 'cron', host_alias: 'h', project_id: 2, prompt: 'check ci' } as RoutineInput;
    await tuneRoutinePrompt(routine, '');
    const [cmd, payload] = vi.mocked(invoke).mock.calls[0];
    expect(cmd).toBe('routines');
    expect((payload as { args: Record<string, unknown> }).args).toEqual({ action: 'tune_prompt', routine, ask: '' });
  });

  // Review w07-3 / w07-8.
  it('a routine tunes the rewrite in place of its prompt, and an older hub reads as needing an update', async () => {
    const routine = { name: '', trigger: 'cron', host_alias: 'h', project_id: 2, prompt: 'check ci' } as RoutineInput;
    vi.mocked(invoke).mockResolvedValue(reply);
    await tuneRoutinePrompt(routine, 'shorter', 'the rewrite');
    const args = (vi.mocked(invoke).mock.calls[0][1] as { args: { routine: RoutineInput } }).args;
    expect(args.routine).toEqual({ ...routine, prompt: 'the rewrite' });

    vi.mocked(invoke).mockRejectedValueOnce({
      code: 'E_INVALID',
      message: 'action must be list | get | dry_run, got "tune_prompt"',
    });
    const r = await tuneRoutinePrompt(routine, '');
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.message).toBe(TUNE_NEEDS_NEWER_HUB);
  });

  it('an empty draft is blocked', () => {
    expect(tuneBlocked(' \n')).toMatch(/draft/);
    expect(tuneBlocked('x')).toBeNull();
    expect(tuneBlocked('a'.repeat(TUNE_DRAFT_MAX_CHARS))).toBeNull();
    expect(tuneBlocked('a'.repeat(TUNE_DRAFT_MAX_CHARS + 1))).toMatch(/too long/);
  });
});
