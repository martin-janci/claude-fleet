// Context help: what the desktop sends `context_help`, the model the setting
// names, and the history a prompt line hands over.
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  askBlocked,
  askContextHelp,
  commandLine,
  helpModel,
  recentHistory,
  screenHistory,
  HELP_HISTORY_MAX,
} from './context_help';
import { SETTING_DEFAULTS } from './fleet_settings';

describe('context_help', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('sends the target, the context and the model, the history capped', async () => {
    vi.mocked(invoke).mockResolvedValue({ answer: 'ok', model: 'haiku', host_alias: 'mercury', history_items: 1, at: 1 });
    const history = Array.from({ length: HELP_HISTORY_MAX + 5 }, (_, i) => `p${i}`);
    const r = await askContextHelp(
      { host_alias: 'mercury', session_name: 'dev-1', profile: '' },
      { surface: 'composer', line: '/pl', history, commands: ['/plan #KEY'] },
      '  how?  ',
      'sonnet',
    );
    expect(r.ok).toBe(true);
    const [cmd, payload] = vi.mocked(invoke).mock.calls[0];
    expect(cmd).toBe('context_help');
    const args = (payload as { args: Record<string, unknown> }).args;
    expect(args).toMatchObject({
      surface: 'composer',
      host_alias: 'mercury',
      session_name: 'dev-1',
      terminal: null,
      profile: null,
      model: 'sonnet',
      question: 'how?',
      line: '/pl',
      commands: ['/plan #KEY'],
    });
    expect(args.history).toHaveLength(HELP_HISTORY_MAX);
    expect((args.history as string[]).at(-1)).toBe(`p${HELP_HISTORY_MAX + 4}`);
  });

  it('reads the model from work.help_model, Haiku unless it names one we know', () => {
    expect(helpModel(SETTING_DEFAULTS)).toBe('haiku');
    expect(helpModel({ 'work.help_model': 'opus' })).toBe('opus');
    expect(helpModel({ 'work.help_model': 'gpt' })).toBe('haiku');
  });

  it('reads a slash command with its usage and description', () => {
    expect(commandLine({ name: 'plan', description: 'Plan subtasks', usage: '#KEY or a goal' })).toBe(
      '/plan #KEY or a goal — Plan subtasks',
    );
    expect(commandLine({ name: 'clear', description: '' })).toBe('/clear');
  });

  it('keeps the newest non-blank history and a screen without its blank tail', () => {
    expect(recentHistory(['a', ' ', 'b', 'c'], 2)).toEqual(['b', 'c']);
    expect(screenHistory('$ ls  \nsrc\n\n\n')).toEqual(['$ ls', 'src']);
  });

  it('needs a question or a line', () => {
    expect(askBlocked(' ', '\n')).not.toBeNull();
    expect(askBlocked('why?', '')).toBeNull();
    expect(askBlocked('', 'git push')).toBeNull();
  });
});
