import { describe, it, expect, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import {
  attentionIdleMinutes,
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
