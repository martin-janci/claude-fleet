import { describe, it, expect } from 'vitest';
import {
  eventCategory,
  filterEvents,
  kindLabel,
  shortDetail,
  eventTime,
  type SessionEvent,
  type EventCategory,
} from './timeline';

const ev = (id: number, kind: string, detail: string | null = null): SessionEvent => ({
  id,
  session_id: 1,
  at: 1_700_000_000 + id,
  kind,
  detail,
});

describe('eventCategory', () => {
  it('maps status changes to turns, failed/blocked to errors', () => {
    expect(eventCategory(ev(1, 'status_change', 'working'))).toBe('turns');
    expect(eventCategory(ev(2, 'status_change', 'idle'))).toBe('turns');
    expect(eventCategory(ev(3, 'status_change', 'failed'))).toBe('errors');
    expect(eventCategory(ev(4, 'status_change', 'blocked'))).toBe('errors');
  });

  it('maps prompts, stuck/failures and operations', () => {
    expect(eventCategory(ev(1, 'prompt_sent', 'fix the bug'))).toBe('prompts');
    expect(eventCategory(ev(2, 'stuck', 'auth_menu'))).toBe('errors');
    expect(eventCategory(ev(3, 'safe_kill_failed'))).toBe('errors');
    expect(eventCategory(ev(4, 'gc_failed'))).toBe('errors');
    expect(eventCategory(ev(5, 'killed'))).toBe('ops');
    expect(eventCategory(ev(6, 'recreated'))).toBe('ops');
    expect(eventCategory(ev(7, 'safe_kill_requested'))).toBe('ops');
    expect(eventCategory(ev(8, 'mcp_call'))).toBe('ops');
    expect(eventCategory(ev(9, 'task_started'))).toBe('ops');
  });

  it('puts unknown kinds in other', () => {
    expect(eventCategory(ev(1, 'something_new'))).toBe('other');
  });
});

describe('filterEvents', () => {
  const all = [
    ev(1, 'status_change', 'working'),
    ev(2, 'prompt_sent', 'hi'),
    ev(3, 'stuck', 'oom'),
    ev(4, 'killed'),
  ];

  it('returns everything when no chip is active', () => {
    expect(filterEvents(all, new Set())).toEqual(all);
  });

  it('keeps only the active categories', () => {
    const on = new Set<EventCategory>(['prompts', 'errors']);
    expect(filterEvents(all, on).map((e) => e.id)).toEqual([2, 3]);
  });
});

describe('formatting', () => {
  it('humanises kinds', () => {
    expect(kindLabel('safe_kill_requested')).toBe('safe kill requested');
  });

  it('collapses whitespace and truncates details', () => {
    expect(shortDetail(null)).toBe('');
    expect(shortDetail('a\n  b')).toBe('a b');
    const long = 'x'.repeat(200);
    const out = shortDetail(long, 10);
    expect(out).toHaveLength(10);
    expect(out.endsWith('…')).toBe(true);
  });

  it('shows a date only for events not from today', () => {
    const now = new Date(2026, 8, 11, 12, 0, 0);
    const today = Math.floor(new Date(2026, 8, 11, 9, 5, 0).getTime() / 1000);
    const earlier = Math.floor(new Date(2026, 8, 1, 9, 5, 0).getTime() / 1000);
    // Today: time only. Another day: the same time with a date prefix.
    expect(eventTime(earlier, now).endsWith(eventTime(today, now))).toBe(true);
    expect(eventTime(earlier, now).length).toBeGreaterThan(eventTime(today, now).length);
  });
});
