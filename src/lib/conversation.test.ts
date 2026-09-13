import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  sessionConversation,
  sameConversation,
  isPinned,
  emptyStateText,
  relativeTime,
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
