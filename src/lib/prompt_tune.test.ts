// The prompt tuner's calls: the composer's goes to session_prompt_tune with
// the history capped, a routine's to routines { tune_prompt } with its draft.
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { tuneBlocked, tuneComposerPrompt, tuneRoutinePrompt } from './prompt_tune';
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

  it('an empty draft is blocked', () => {
    expect(tuneBlocked(' \n')).toMatch(/draft/);
    expect(tuneBlocked('x')).toBeNull();
  });
});
