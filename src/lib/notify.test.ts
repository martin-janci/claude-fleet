import { describe, it, expect, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import type { SessionRow } from './sessions';

const plugin = vi.hoisted(() => ({
  clicks: [] as Array<(a: { actionId: string; notification: { id: number } | null }) => void>,
  removed: [] as Array<Array<{ id: number }>>,
}));
vi.mock('@tauri-apps/plugin-notification', () => ({
  onAction: vi.fn(async (cb: (a: { actionId: string; notification: { id: number } | null }) => void) => {
    plugin.clicks.push(cb);
    return { unregister: vi.fn() };
  }),
  removeActive: vi.fn(async (n: Array<{ id: number }>) => {
    plugin.removed.push(n);
  }),
}));

import {
  attentionIdleMinutes,
  closeOsNotification,
  heldByQuietHours,
  notificationsThroughPlugin,
  QuietHold,
  quietSummary,
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

describe('through tauri-plugin-notification (review w11-11)', () => {
  afterEach(() => {
    delete (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it('posts with a numeric id, opens on a click by that id, and takes it down by id', () => {
    const { shown } = installFakeNotification('granted');
    expect(notificationsThroughPlugin()).toBe(false);
    (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    expect(notificationsThroughPlugin()).toBe(true);
    const opened = vi.fn();
    expect(showOsNotification('dev-x needs you', 'trn', 'needs_you-7', opened)).toBe(true);
    expect(shown).toHaveLength(1);
    const id = (shown[0] as { id?: number }).id;
    expect(typeof id).toBe('number');
    expect(shown[0]).toEqual({ title: 'dev-x needs you', body: 'trn', id });
    expect(plugin.clicks).toHaveLength(1);
    plugin.clicks[0]({ actionId: 'tap', notification: { id: id! } });
    expect(opened).toHaveBeenCalledTimes(1);
    // The click took it down; a second one finds nothing to run.
    expect(plugin.removed).toContainEqual([{ id }]);
    plugin.clicks[0]({ actionId: 'tap', notification: { id: id! } });
    expect(opened).toHaveBeenCalledTimes(1);
  });

  it('withdraws a notification its row no longer earns', () => {
    const { shown } = installFakeNotification('granted');
    (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    showOsNotification('dev-y failed', 'trn', 'failed-8');
    const id = (shown[0] as { id?: number }).id;
    closeOsNotification('failed-8');
    expect(plugin.removed).toContainEqual([{ id }]);
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

describe('what quiet hours hold back (review w11-12)', () => {
  const base = {
    'notify.desktop': 'needs_you,failed',
    'notify.quiet_hours': '22:00-07:30',
    'notify.quiet_except': 'failed',
  };
  const at = (h: number, m = 0) => new Date(2026, 9, 8, h, m);

  it('holds only what the column lists and quiet hours alone keep back', () => {
    expect(heldByQuietHours('desktop', 'needs_you', base, at(23))).toBe(true);
    expect(heldByQuietHours('desktop', 'needs_you', base, at(12))).toBe(false);
    expect(heldByQuietHours('desktop', 'failed', base, at(23))).toBe(false);
    expect(heldByQuietHours('desktop', 'done', base, at(23))).toBe(false);
  });

  it('releases once after quiet hours, counting only what still waits', () => {
    const hold = new QuietHold();
    let bWaits = 1;
    hold.hold('needs_you-1', 'dev-a', () => 1);
    hold.hold('needs_you-2', 'dev-b', () => bWaits);
    hold.hold('blocked-host-trn', 'trn is down · 3 sessions', () => 3);
    expect(hold.release(base, at(3))).toBeNull();
    expect(hold.size).toBe(3);
    bWaits = 0;
    expect(hold.release(base, at(7, 30))).toEqual({ count: 4, names: ['dev-a', 'trn is down · 3 sessions'] });
    expect(hold.size).toBe(0);
    expect(hold.release(base, at(8))).toBeNull();
  });

  it('words the summary', () => {
    expect(quietSummary(1, ['dev-a'])).toEqual({ title: '1 session needs you', body: 'Held during quiet hours: dev-a' });
    expect(quietSummary(5, ['a', 'b', 'c', 'd']).body).toBe('Held during quiet hours: a, b, c and 1 more');
    expect(quietSummary(3, ['a', 'b', 'c']).title).toBe('3 sessions need you');
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

  it("does not ring for a routine run's finished turn, only for its failures and questions", () => {
    const prev = notifySnapshot([base], opts);
    const done = { ...base, lost_at: null, claude_status: 'idle', last_stop_at: 950, last_viewed_at: 900 } as unknown as SessionRow;
    expect(newlyNotifiable(prev, [done], opts).map((n) => n.state)).toEqual(['done']);
    const routineDone = { ...done, origin: 'routine', origin_ref: '4' } as SessionRow;
    expect(newlyNotifiable(prev, [routineDone], opts)).toEqual([]);
    const routineFailed = { ...base, origin: 'routine', claude_status: 'failed' } as SessionRow;
    expect(newlyNotifiable(prev, [routineFailed], opts).map((n) => n.state)).toEqual(['failed']);
  });
});
