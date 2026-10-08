import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { clearToasts, dismiss, push } from './toasts';
import { clearNotices, markAllNoticesRead, MAX_NOTIFICATIONS, notices, recordNotice, unreadNotices } from './notifications';

beforeEach(() => {
  clearToasts();
  clearNotices();
});

describe('notification centre', () => {
  it('keeps every toast, newest first, after the toast is gone', () => {
    const a = push({ kind: 'success', message: 'Saved' });
    push({ kind: 'error', code: 'E_SSH', message: 'mercury did not answer' });
    dismiss(a);
    expect(get(notices).map((n) => [n.kind, n.code, n.message])).toEqual([
      ['error', 'E_SSH', 'mercury did not answer'],
      ['success', null, 'Saved'],
    ]);
    expect(get(unreadNotices)).toBe(2);
  });

  it('counts a deduped toast once, moved to the top and unread again', () => {
    push({ kind: 'error', message: 'boom' });
    push({ kind: 'info', message: 'other' });
    markAllNoticesRead();
    push({ kind: 'error', message: 'boom' });
    const [top, next] = get(notices);
    expect([top.message, top.count, top.read]).toEqual(['boom', 2, false]);
    expect(next.message).toBe('other');
    expect(get(unreadNotices)).toBe(1);
  });

  it('keeps at most the newest MAX_NOTIFICATIONS', () => {
    for (let i = 0; i < MAX_NOTIFICATIONS + 5; i++) recordNotice(1000 + i, true, { kind: 'info', code: null, message: `n${i}` });
    const list = get(notices);
    expect(list).toHaveLength(MAX_NOTIFICATIONS);
    expect(list[0].message).toBe(`n${MAX_NOTIFICATIONS + 4}`);
  });
});
