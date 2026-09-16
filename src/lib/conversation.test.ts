import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  sessionConversation,
  sameConversation,
  isPinned,
  emptyStateText,
  relativeTime,
  groupItems,
  toolName,
  toolGroupLabel,
  isLongPrompt,
  transcriptCarries,
  composerStatus,
  matchSlashCommands,
  completeSlashCommand,
  SLASH_COMMANDS,
  PROMPT_CLAMP_LINES,
  PIN_THRESHOLD_PX,
  type Conversation,
} from './conversation';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

function conv(over: Partial<Conversation> = {}): Conversation {
  return {
    truncated: false,
    turns: [
      {
        prompt: 'fix the bug',
        at: '2026-09-13T10:00:00Z',
        items: [{ kind: 'text', text: 'looking into it' }],
      },
    ],
    ...over,
  };
}

describe('sessionConversation', () => {
  it('invokes session_conversation with the wrapped session_id', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue({ turns: [], truncated: false });
    const r = await sessionConversation(5);
    expect(mockedInvoke).toHaveBeenCalledWith('session_conversation', { args: { session_id: 5 } });
    expect(r.ok).toBe(true);
  });
});

describe('sameConversation', () => {
  it('is true for deep-equal conversations', () => {
    expect(sameConversation(conv(), conv())).toBe(true);
  });

  it('is false when an item differs', () => {
    const a = conv();
    const b = conv({ turns: [{ ...a.turns[0], items: [{ kind: 'text', text: 'different' }] }] });
    expect(sameConversation(a, b)).toBe(false);
  });

  it('is false when the first argument is null', () => {
    expect(sameConversation(null, conv())).toBe(false);
  });
});

describe('isPinned', () => {
  it('is true exactly at the bottom', () => {
    expect(isPinned(100, 50, 150)).toBe(true);
  });

  it('is true at the threshold (40px from bottom)', () => {
    // scrollHeight - scrollTop - clientHeight === 40
    expect(isPinned(60, 50, 150)).toBe(true);
    expect(150 - 60 - 50).toBe(PIN_THRESHOLD_PX);
  });

  it('is false one pixel beyond the threshold (41px from bottom)', () => {
    expect(isPinned(59, 50, 150)).toBe(false);
    expect(150 - 59 - 50).toBe(41);
  });
});

describe('emptyStateText', () => {
  it('reports missing Claude session id regardless of error code', () => {
    expect(emptyStateText(null, false)).toBe('No Claude session id yet');
    expect(emptyStateText('E_NO_TRANSCRIPT', false)).toBe('No Claude session id yet');
  });

  it('reports no-conversation-yet for E_NO_TRANSCRIPT when an id is present', () => {
    expect(emptyStateText('E_NO_TRANSCRIPT', true)).toBe('No conversation yet');
  });

  it('returns null otherwise', () => {
    expect(emptyStateText(null, true)).toBeNull();
    expect(emptyStateText('E_INVALID_STATE', true)).toBeNull();
  });
});

describe('relativeTime', () => {
  const now = new Date('2026-09-13T12:00:00Z').getTime();

  it('reports just now for sub-minute ages', () => {
    expect(relativeTime('2026-09-13T11:59:30Z', now)).toBe('just now');
  });

  it('reports minutes', () => {
    expect(relativeTime('2026-09-13T11:55:00Z', now)).toBe('5m ago');
  });

  it('reports hours', () => {
    expect(relativeTime('2026-09-13T09:00:00Z', now)).toBe('3h ago');
  });

  it('reports days', () => {
    expect(relativeTime('2026-09-11T12:00:00Z', now)).toBe('2d ago');
  });
});

describe('groupItems', () => {
  it('keeps text items apart and folds consecutive tool calls together', () => {
    const t = (text: string) => ({ kind: 'text' as const, text });
    const u = (summary: string) => ({ kind: 'tool' as const, summary });
    expect(groupItems([u('Read(a)'), u('Bash(ls)'), t('x'), u('Edit(b)'), t('y'), t('z')])).toEqual([
      { kind: 'tools', tools: ['Read(a)', 'Bash(ls)'] },
      { kind: 'text', text: 'x' },
      { kind: 'tools', tools: ['Edit(b)'] },
      { kind: 'text', text: 'y' },
      { kind: 'text', text: 'z' },
    ]);
    expect(groupItems([])).toEqual([]);
  });
});

describe('toolName / toolGroupLabel', () => {
  it('takes the name before the argument list', () => {
    expect(toolName('Bash(command=ls -la)')).toBe('Bash');
    expect(toolName('mcp__fleet__list_sessions()')).toBe('mcp__fleet__list_sessions');
    expect(toolName('NoParens')).toBe('NoParens');
  });

  it('counts calls and lists up to three distinct names in order', () => {
    expect(toolGroupLabel(['Read(a)', 'Read(b)', 'Bash(x)'])).toBe('3 tool calls · Read, Bash');
    expect(toolGroupLabel(['A()', 'B()', 'C()', 'D()', 'A()'])).toBe('5 tool calls · A, B, C +1');
  });
});

describe('isLongPrompt', () => {
  it('flags prompts over the clamp in lines or characters', () => {
    expect(isLongPrompt('short')).toBe(false);
    expect(isLongPrompt(Array.from({ length: PROMPT_CLAMP_LINES + 1 }, () => 'l').join('\n'))).toBe(true);
    expect(isLongPrompt('x'.repeat(601))).toBe(true);
  });
});

describe('transcriptCarries', () => {
  it('is false until the transcript has more turns with the text than at send time', () => {
    const pending = { prompt: 'continue', at: '2026-09-13T10:00:00.000Z', seen: 1 };
    const c = conv({ turns: [{ prompt: 'continue', at: null, items: [] }] });
    expect(transcriptCarries(c, pending)).toBe(false);
    c.turns.push({ prompt: 'continue', at: null, items: [] });
    expect(transcriptCarries(c, pending)).toBe(true);
  });

  it('ignores turns with a different prompt', () => {
    const pending = { prompt: 'run tests', at: '2026-09-13T10:00:00.000Z', seen: 0 };
    expect(transcriptCarries(conv({ turns: [{ prompt: 'fix the bug', at: null, items: [] }] }), pending)).toBe(false);
  });
});

describe('composerStatus', () => {
  it('names a stuck session first, then a working one, else nothing', () => {
    expect(composerStatus({ claude_status: 'working', stuck_kind: 'auth_menu' })).toMatch(/stuck \(auth_menu\)/);
    expect(composerStatus({ claude_status: 'working', stuck_kind: null })).toMatch(/working/);
    expect(composerStatus({ claude_status: 'idle', stuck_kind: null })).toBeNull();
    expect(composerStatus({ claude_status: null, stuck_kind: null })).toBeNull();
  });
});

describe('matchSlashCommands', () => {
  it('is empty unless the draft is a single slash token', () => {
    expect(matchSlashCommands('')).toEqual([]);
    expect(matchSlashCommands('fix it')).toEqual([]);
    expect(matchSlashCommands('/clear now')).toEqual([]);
    expect(matchSlashCommands('/clear\n')).toEqual([]);
    expect(matchSlashCommands(' /clear')).toEqual([]);
  });

  it('a bare slash lists every command; a prefix narrows it, case-insensitively', () => {
    expect(matchSlashCommands('/')).toEqual(SLASH_COMMANDS);
    const names = matchSlashCommands('/co').map((c) => c.name);
    expect(names).toContain('compact');
    expect(names).toContain('cost');
    expect(names).not.toContain('clear');
    expect(matchSlashCommands('/CLE').map((c) => c.name)).toEqual(['clear']);
    expect(matchSlashCommands('/zzz')).toEqual([]);
  });

  it('every catalog entry has a name, a description and a unique name', () => {
    const names = SLASH_COMMANDS.map((c) => c.name);
    expect(new Set(names).size).toBe(names.length);
    for (const c of SLASH_COMMANDS) {
      expect(c.name).toMatch(/^[a-z][a-z0-9-]*$/);
      expect(c.description.length).toBeGreaterThan(0);
    }
  });
});

describe('completeSlashCommand', () => {
  it('yields the command, with a trailing space only when it takes arguments', () => {
    expect(completeSlashCommand({ name: 'clear', description: '' })).toBe('/clear');
    expect(completeSlashCommand({ name: 'model', description: '', args: true })).toBe('/model ');
  });
});
