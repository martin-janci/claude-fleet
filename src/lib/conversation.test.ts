import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  sessionConversation,
  listConversations,
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
  isQuietStatus,
  shouldFetchTranscript,
  turnDuration,
  countItems,
  newItemCount,
  promptHistory,
  spinnerLabel,
  indicatorFor,
  QUIET_POLL_MS,
  PROMPT_CLAMP_LINES,
  PIN_THRESHOLD_PX,
  formatTokens,
  contextMeter,
  switcherEntries,
  conversationTitle,
  statusChip,
  inlineEventFor,
  buildThread,
  lastEventLabel,
  mergeEvents,
  type Conversation,
  type ConversationSummary,
  type ConvTurn,
} from './conversation';
import type { SessionRow } from './sessions';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

function conv(over: Partial<Conversation> = {}): Conversation {
  return {
    truncated: false,
    context: null,
    events: [],
    turns: [
      {
        prompt: 'fix the bug',
        at: '2026-09-13T10:00:00Z',
        ended_at: null,
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

  it('passes a turn window when one is requested', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue({ turns: [], truncated: false });
    await sessionConversation(5, 30);
    expect(mockedInvoke).toHaveBeenCalledWith('session_conversation', { args: { session_id: 5, turns: 30 } });
  });

  it('passes an earlier conversation id as claude_session_id', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue({ turns: [], truncated: false, context: null });
    await sessionConversation(5, undefined, 'uuid-b');
    expect(mockedInvoke).toHaveBeenCalledWith('session_conversation', {
      args: { session_id: 5, claude_session_id: 'uuid-b' },
    });
  });
});

describe('listConversations', () => {
  it('invokes session_conversations with the session id and a default limit', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    const r = await listConversations(7);
    expect(mockedInvoke).toHaveBeenCalledWith('session_conversations', { args: { session_id: 7, limit: 50 } });
    expect(r.ok).toBe(true);
  });

  it('passes an explicit limit', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    await listConversations(7, 5);
    expect(mockedInvoke).toHaveBeenCalledWith('session_conversations', { args: { session_id: 7, limit: 5 } });
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
    const u = (summary: string, error?: boolean) => ({ kind: 'tool' as const, summary, error });
    const l = (summary: string, error = false) => ({ summary, error });
    expect(groupItems([u('Read(a)'), u('Bash(ls)', true), t('x'), u('Edit(b)'), t('y'), t('z')])).toEqual([
      { kind: 'tools', tools: [l('Read(a)'), l('Bash(ls)', true)] },
      { kind: 'text', text: 'x' },
      { kind: 'tools', tools: [l('Edit(b)')] },
      { kind: 'text', text: 'y' },
      { kind: 'text', text: 'z' },
    ]);
    expect(groupItems([])).toEqual([]);
  });

  it('new item kinds are their own groups and break a tool run', () => {
    const g = groupItems([
      { kind: 'tool', summary: 'Bash(ls)' },
      { kind: 'interrupt', during_tool: true },
      { kind: 'tool', summary: 'Read(x)' },
    ]);
    expect(g.map((x) => x.kind)).toEqual(['tools', 'interrupt', 'tools']);
  });
});

describe('toolName / toolGroupLabel', () => {
  it('takes the name before the argument list', () => {
    expect(toolName('Bash(command=ls -la)')).toBe('Bash');
    expect(toolName('mcp__fleet__list_sessions()')).toBe('mcp__fleet__list_sessions');
    expect(toolName('NoParens')).toBe('NoParens');
  });

  it('counts calls and lists up to three distinct names in order, plus how many failed', () => {
    const l = (summary: string, error = false) => ({ summary, error });
    expect(toolGroupLabel([l('Read(a)'), l('Read(b)'), l('Bash(x)')])).toBe('3 tool calls · Read, Bash');
    expect(toolGroupLabel([l('A()'), l('B()'), l('C()'), l('D()'), l('A()')])).toBe('5 tool calls · A, B, C +1');
    expect(toolGroupLabel([l('Bash(x)', true), l('Bash(y)'), l('Read(z)', true)])).toBe('3 tool calls · Bash, Read · 2 failed');
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
    const c = conv({ turns: [{ prompt: 'continue', at: null, ended_at: null, items: [] }] });
    expect(transcriptCarries(c, pending)).toBe(false);
    c.turns.push({ prompt: 'continue', at: null, ended_at: null, items: [] });
    expect(transcriptCarries(c, pending)).toBe(true);
  });

  it('ignores turns with a different prompt', () => {
    const pending = { prompt: 'run tests', at: '2026-09-13T10:00:00.000Z', seen: 0 };
    expect(transcriptCarries(conv({ turns: [{ prompt: 'fix the bug', at: null, ended_at: null, items: [] }] }), pending)).toBe(false);
  });
});

describe('composerStatus', () => {
  it('names a stuck session first, then a working one, else nothing', () => {
    expect(composerStatus({ claude_status: 'working', stuck_kind: 'auth_menu' })).toMatch(/stuck \(auth menu\)/);
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

describe('isQuietStatus / shouldFetchTranscript', () => {
  it('idle-like statuses are quiet; null and working are not', () => {
    expect(isQuietStatus('idle')).toBe(true);
    expect(isQuietStatus('completed')).toBe(true);
    expect(isQuietStatus('stopped')).toBe(true);
    expect(isQuietStatus('failed')).toBe(true);
    expect(isQuietStatus('working')).toBe(false);
    expect(isQuietStatus('blocked')).toBe(false);
    expect(isQuietStatus(null)).toBe(false);
  });

  it('a quiet session is re-read only on a turn change or after the quiet cadence', () => {
    expect(shouldFetchTranscript({ quiet: false, sinceLastFetchMs: 0, turnSeqChanged: false })).toBe(true);
    expect(shouldFetchTranscript({ quiet: true, sinceLastFetchMs: 5_000, turnSeqChanged: false })).toBe(false);
    expect(shouldFetchTranscript({ quiet: true, sinceLastFetchMs: 5_000, turnSeqChanged: true })).toBe(true);
    expect(shouldFetchTranscript({ quiet: true, sinceLastFetchMs: QUIET_POLL_MS, turnSeqChanged: false })).toBe(true);
  });
});

describe('spinnerLabel', () => {
  it('drops the interrupt hint and the parentheses, keeps the rest', () => {
    expect(spinnerLabel('Cooking… (3s · esc to interrupt)')).toBe('Cooking… 3s');
    expect(spinnerLabel('Channelling… (running Stop hooks… 3/4 · 15s · ↓ 306 tokens)')).toBe(
      'Channelling… running Stop hooks… 3/4 · 15s · ↓ 306 tokens',
    );
    expect(spinnerLabel('Thinking…')).toBe('Thinking…');
  });
});

describe('indicatorFor', () => {
  const base = { status: null, stuckKind: null, waitingFor: null, activity: null, spinner: null, pending: false, optimistic: false } as const;
  it('working shows the spinner label, else a generic Working', () => {
    expect(indicatorFor({ ...base, status: 'working', spinner: 'Cooking… (3s · esc to interrupt)' })).toEqual({ kind: 'working', label: 'Cooking… 3s' });
    expect(indicatorFor({ ...base, status: 'working' })).toEqual({ kind: 'working', label: 'Working…' });
  });
  it('blocked carries the pane detail and what it waits for', () => {
    expect(indicatorFor({ ...base, status: 'blocked', activity: 'Do you want to proceed?', waitingFor: 'permission' })).toEqual({
      kind: 'blocked', detail: 'Do you want to proceed?', waiting: 'permission',
    });
  });
  it('a stuck session shows nothing; pending shows sent; optimistic shows working; idle shows nothing', () => {
    expect(indicatorFor({ ...base, status: 'blocked', stuckKind: 'auth_menu' })).toBeNull();
    expect(indicatorFor({ ...base, status: 'idle', pending: true })).toEqual({ kind: 'sent' });
    expect(indicatorFor({ ...base, status: 'idle', optimistic: true })).toEqual({ kind: 'working', label: 'Working…' });
    expect(indicatorFor({ ...base, status: 'idle' })).toBeNull();
  });
});

describe('turnDuration', () => {
  it('formats seconds, minutes and hours; null when an end is missing or under a second', () => {
    expect(turnDuration('2026-09-13T10:00:00Z', '2026-09-13T10:00:35Z')).toBe('35s');
    expect(turnDuration('2026-09-13T10:00:00Z', '2026-09-13T10:02:14Z')).toBe('2m 14s');
    expect(turnDuration('2026-09-13T10:00:00Z', '2026-09-13T11:05:00Z')).toBe('1h 5m');
    expect(turnDuration('2026-09-13T10:00:00Z', '2026-09-13T10:00:00.400Z')).toBeNull();
    expect(turnDuration('2026-09-13T10:00:00Z', null)).toBeNull();
    expect(turnDuration(null, '2026-09-13T10:00:00Z')).toBeNull();
    expect(turnDuration('garbage', '2026-09-13T10:00:00Z')).toBeNull();
  });
});

describe('countItems / newItemCount', () => {
  it('counts prompts and items; growth is new, trimming is not', () => {
    const a = conv();
    expect(countItems(null)).toBe(0);
    expect(countItems(a)).toBe(a.turns.reduce((n, t) => n + (t.prompt !== null ? 1 : 0) + t.items.length, 0));
    const base = countItems(a);
    const b = conv({ turns: [...a.turns, { prompt: 'more', at: null, ended_at: null, items: [{ kind: 'text', text: 'x' }] }] });
    expect(newItemCount(a, b)).toBe(2);
    expect(newItemCount(b, a)).toBe(0);
    expect(newItemCount(null, a)).toBe(base);
  });
});

describe('promptHistory', () => {
  it('lists prompts oldest first, skips slash commands and adjacent repeats, appends the pending one', () => {
    const c = conv({
      turns: [
        { prompt: 'first', at: null, ended_at: null, items: [] },
        { prompt: '/clear', at: null, ended_at: null, items: [] },
        { prompt: 'again', at: null, ended_at: null, items: [] },
        { prompt: 'again', at: null, ended_at: null, items: [] },
        { prompt: null, at: null, ended_at: null, items: [{ kind: 'text', text: 'x' }] },
      ],
    });
    expect(promptHistory(c, null)).toEqual(['first', 'again']);
    expect(promptHistory(c, { prompt: 'newest', at: '', seen: 0 })).toEqual(['first', 'again', 'newest']);
    expect(promptHistory(null, null)).toEqual([]);
  });
});

describe('formatTokens', () => {
  it('formats', () => {
    expect(formatTokens(950)).toBe('950');
    expect(formatTokens(42_300)).toBe('42k');
    expect(formatTokens(200_000)).toBe('200k');
    expect(formatTokens(1_000_000)).toBe('1M');
    expect(formatTokens(1_250_000)).toBe('1.25M');
  });
});

describe('contextMeter', () => {
  const s = (o: Partial<SessionRow>) =>
    ({ context_pct: null, context_tokens: null, context_window: null, context_stale: false, ...o }) as SessionRow;
  it('shows tokens and window when known', () => {
    expect(contextMeter(s({ context_pct: 21, context_tokens: 42_000, context_window: 200_000 }))?.label)
      .toBe('42k / 200k · 21%');
  });
  it('falls back to the percentage', () => {
    expect(contextMeter(s({ context_pct: 55 }))?.label).toBe('ctx 55%');
  });
  it('is 0 after a clear, not null', () => {
    expect(contextMeter(s({ context_pct: 0, context_tokens: 0, context_window: 200_000 }))?.label)
      .toBe('0 / 200k · 0%');
  });
  it('marks a stale value', () => {
    const m = contextMeter(s({ context_pct: 80, context_stale: true }))!;
    expect(m.stale).toBe(true);
    expect(m.title).toMatch(/before the last compaction/);
  });
  it('is null when nothing is known', () => {
    expect(contextMeter(s({}))).toBeNull();
  });
});

describe('switcherEntries / conversationTitle', () => {
  const c = (o: Partial<ConversationSummary>): ConversationSummary => ({
    id: 1, session_id: 1, claude_session_id: 'a', transcript_path: null, started_at: 1_789_000_000,
    ended_at: null, start_source: 'clear', end_reason: null, model: null, first_prompt: null,
    turns: 0, compactions: 0, current: false, ...o,
  });
  it('hides empty earlier conversations but keeps the current one', () => {
    const list = [c({ id: 3, current: true }), c({ id: 2 }), c({ id: 1, turns: 4 })];
    expect(switcherEntries(list).map((x) => x.id)).toEqual([3, 1]);
  });
  it('titles', () => {
    expect(conversationTitle(c({ current: true, turns: 1 }))).toBe('Current · /clear · 1 turn');
  });
});

describe('inlineEventFor', () => {
  const e = (kind: string, detail: string | null, id = 1) =>
    ({ id, session_id: 1, at: 100, kind, detail, claude_session_id: 'a' });
  it('maps failures, permissions, resume and end; hides the rest', () => {
    expect(inlineEventFor(e('stop_failure', 'rate_limit: slow down'), { latestId: 1, blocked: false }))
      .toMatchObject({ label: 'Turn failed: rate limit', detail: 'slow down', tone: 'error' });
    expect(inlineEventFor(e('notification', 'permission_prompt'), { latestId: 1, blocked: true }))
      .toMatchObject({ label: 'Waiting for permission', tone: 'warn' });
    expect(inlineEventFor(e('notification', 'permission_prompt'), { latestId: 2, blocked: true }))
      .toMatchObject({ label: 'Asked for permission', tone: 'info' });
    expect(inlineEventFor(e('conversation_started', 'resume'), { latestId: 1, blocked: false })?.label)
      .toBe('Resumed conversation');
    expect(inlineEventFor(e('conversation_started', 'clear'), { latestId: 1, blocked: false })).toBeNull();
    expect(inlineEventFor(e('turn_done', 'x'), { latestId: 1, blocked: false })).toBeNull();
    expect(inlineEventFor(e('compact_done', 'auto'), { latestId: 1, blocked: false })).toBeNull();
  });
});

describe('buildThread', () => {
  const turn = (at: string, prompt: string): ConvTurn => ({ prompt, at, ended_at: null, items: [] });
  const ev = (id: number, at: number) =>
    ({ id, session_id: 1, at, kind: 'stop_failure', detail: 'overloaded', claude_session_id: 'a' });
  const t0 = Date.parse('2026-09-18T10:00:00Z') / 1000;
  it('places events after the turn they follow', () => {
    const rows = buildThread(
      [turn('2026-09-18T10:00:00Z', 'a'), turn('2026-09-18T10:05:00Z', 'b')],
      [ev(1, t0 + 60), ev(2, t0 + 400)],
      { blocked: false },
      false,
    );
    expect(rows.map((r) => (r.kind === 'turn' ? r.turn.prompt : `e${r.event.id}`))).toEqual(['a', 'e1', 'b', 'e2']);
  });
  it('drops events older than the first loaded turn when truncated', () => {
    const rows = buildThread([turn('2026-09-18T10:00:00Z', 'a')], [ev(1, t0 - 60)], { blocked: false }, true);
    expect(rows).toHaveLength(1);
  });
  it('keeps them first when not truncated', () => {
    const rows = buildThread([turn('2026-09-18T10:00:00Z', 'a')], [ev(1, t0 - 60)], { blocked: false }, false);
    expect(rows[0].kind).toBe('event');
  });
});

describe('lastEventLabel / mergeEvents / statusChip', () => {
  it('labels the newest notable event', () => {
    const now = 1_000_000 * 1000;
    const evs = [
      { id: 1, session_id: 1, at: 999_700, kind: 'compact_done', detail: 'auto', claude_session_id: 'a' },
      { id: 2, session_id: 1, at: 999_900, kind: 'turn_done', detail: null, claude_session_id: 'a' },
    ];
    expect(lastEventLabel(evs, now)).toMatch(/^\/compact /);
  });
  it('merges by id, oldest first', () => {
    const a = { id: 2, session_id: 1, at: 5, kind: 'x', detail: null, claude_session_id: 'a' };
    const b = { id: 1, session_id: 1, at: 4, kind: 'y', detail: null, claude_session_id: 'a' };
    expect(mergeEvents([a], [b, a]).map((e) => e.id)).toEqual([1, 2]);
  });
  it('status chip prefers compacting', () => {
    expect(statusChip({ claude_status: 'working', current_activity: 'compacting' })).toBe('compacting');
    expect(statusChip({ claude_status: 'idle', current_activity: null })).toBe('idle');
  });
});
