import { describe, it, expect, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import type { SessionRow } from './sessions';
import {
  attentionIdleMinutes,
  inQuietHours,
  newlyNotifiable,
  notifySnapshot,
  notifyStateOf,
  notificationAllowed,
  parseQuietHours,
  notificationPermission,
  notifyStuckOs,
  notifyStuckToast,
  requestNotificationPermission,
  showOsNotification,
} from './notify';

type Perm = 'granted' | 'denied' | 'default';

function installFakeNotification(permission: Perm, requestResult: Perm = permission) {
  const shown: Array<{ title: string; body?: string; tag?: string }> = [];
  class FakeNotification {
    static permission: Perm = permission;
    static requestPermission = vi.fn(async () => {
      FakeNotification.permission = requestResult;
      return requestResult;
    });
    constructor(title: string, opts?: { body?: string; tag?: string }) {
      shown.push({ title, ...opts });
    }
  }
  Object.defineProperty(globalThis, 'Notification', {
    value: FakeNotification,
    configurable: true,
    writable: true,
  });
  return { shown, FakeNotification };
}

afterEach(() => {
  // @ts-expect-error jsdom has no Notification; make sure we leave none behind
  delete globalThis.Notification;
});

describe('notify prefs', () => {
  it('toast defaults on, OS notifications default off, idle threshold 30 min', () => {
    expect(get(notifyStuckToast)).toBe(true);
    expect(get(notifyStuckOs)).toBe(false);
    expect(get(attentionIdleMinutes)).toBe(30);
  });
});

describe('Notification API guards', () => {
  it('reports unsupported when the API is absent and never throws', async () => {
    expect(notificationPermission()).toBe('unsupported');
    expect(await requestNotificationPermission()).toBe('unsupported');
    expect(showOsNotification('t', 'b')).toBe(false);
  });

  it('shows a notification only when permission is granted', () => {
    const { shown } = installFakeNotification('default');
    expect(notificationPermission()).toBe('default');
    expect(showOsNotification('t', 'b', 'x')).toBe(false);
    expect(shown).toHaveLength(0);

    const granted = installFakeNotification('granted');
    expect(showOsNotification('claude-fleet', 'dev-x is stuck', 'stuck-1')).toBe(true);
    expect(granted.shown).toEqual([{ title: 'claude-fleet', body: 'dev-x is stuck', tag: 'stuck-1' }]);
  });

  it('requestNotificationPermission resolves to the OS answer', async () => {
    const { FakeNotification } = installFakeNotification('default', 'granted');
    expect(await requestNotificationPermission()).toBe('granted');
    expect(FakeNotification.requestPermission).toHaveBeenCalledTimes(1);
    expect(notificationPermission()).toBe('granted');
  });
});

describe('notifications matrix and quiet hours (11.9)', () => {
  const base = {
    'notify.desktop': 'needs_you,failed,blocked',
    'notify.quiet_hours': '22:00-07:30',
    'notify.quiet_except': 'failed',
  };
  const at = (h: number, m = 0) => new Date(2026, 9, 8, h, m);

  it('reads a range that wraps past midnight', () => {
    const r = parseQuietHours('22:00-07:30');
    expect(r).toEqual([1320, 450]);
    expect(inQuietHours(r, 23 * 60)).toBe(true);
    expect(inQuietHours(r, 3 * 60)).toBe(true);
    expect(inQuietHours(r, 7 * 60 + 30)).toBe(false);
    expect(inQuietHours(r, 12 * 60)).toBe(false);
    expect(inQuietHours(parseQuietHours('09:00-17:00'), 12 * 60)).toBe(true);
    expect(parseQuietHours('')).toBeNull();
    expect(parseQuietHours('08:00-08:00')).toBeNull();
    expect(parseQuietHours('25:00-07:00')).toBeNull();
  });

  it('sends only the states a channel ticks, and only the exceptions during quiet hours', () => {
    expect(notificationAllowed('desktop', 'blocked', base, at(12))).toBe(true);
    expect(notificationAllowed('desktop', 'done', base, at(12))).toBe(false);
    expect(notificationAllowed('phone', 'failed', base, at(12))).toBe(false);
    expect(notificationAllowed('desktop', 'blocked', base, at(23))).toBe(false);
    expect(notificationAllowed('desktop', 'failed', base, at(23))).toBe(true);
    expect(notificationAllowed('desktop', 'blocked', { ...base, 'notify.quiet_hours': '' }, at(23))).toBe(true);
  });
});

describe('the matrix watcher (11.9)', () => {
  const opts = { idleSecs: 0, now: 1_000 };
  const base = { id: 1, kind: 'work', status: 'running', claude_status: 'working', last_activity_at: 900 } as unknown as SessionRow;

  it('files attention states under the matrix rows', () => {
    expect(notifyStateOf('action_required')).toBe('needs_you');
    expect(notifyStateOf('failed')).toBe('failed');
    expect(notifyStateOf('done')).toBe('done');
    expect(notifyStateOf('working')).toBeNull();
    expect(notifyStateOf('paused')).toBeNull();
  });

  it('reports a row once, when it moves into a listed state', () => {
    const prev = notifySnapshot([base], opts);
    const asking = { ...base, claude_status: 'blocked' } as SessionRow;
    expect(newlyNotifiable(prev, [asking], opts).map((n) => n.state)).toEqual(['needs_you']);
    expect(newlyNotifiable(notifySnapshot([asking], opts), [asking], opts)).toEqual([]);
    const failedRow = { ...base, claude_status: 'failed' } as SessionRow;
    expect(newlyNotifiable(prev, [failedRow], opts).map((n) => n.state)).toEqual(['failed']);
  });

  it('leaves stuck and external rows to others', () => {
    const prev = notifySnapshot([base], opts);
    expect(newlyNotifiable(prev, [{ ...base, stuck_kind: 'oom' } as SessionRow], opts)).toEqual([]);
    expect(newlyNotifiable(new Map(), [{ ...base, kind: 'external', claude_status: 'failed' } as SessionRow], opts)).toEqual([]);
  });
});
