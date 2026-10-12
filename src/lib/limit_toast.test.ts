// Toasts board (G4.8): the limit-hit toast. Once, when a reading turns an
// account's usage into a limit; its second line counts the sessions that
// limit paused; "Show paused sessions" opens the account narrowed to them.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { accountUsage, type AccountUsageSnapshot } from './account_usage_store';
import { accounts } from './accounts';
import { sessions } from './sessions';
import { hosts } from './hosts';
import { clearToasts, runToastAction, toasts } from './toasts';
import { destination } from './destination';
import { accountsPageRequest, accountsPausedRequest } from './account_pill';
import { limitHitLines, newlyLimited, startLimitToasts } from './limit_toast';
import { account, session } from './hosts_fixture';

const SILVESTER = account('acc-silvester', 'tech.silvester@example.com', { nickname: 'tech.silvester' });
const nowSec = () => Math.floor(Date.now() / 1000);

function snap(uuid: string, weeklyUsed: number, resetsAt = nowSec() + 3 * 86400): AccountUsageSnapshot {
  return {
    account_uuid: uuid,
    usage: {
      five_hour: { utilization: 10, resets_at: nowSec() + 3600 },
      seven_day: { utilization: weeklyUsed, resets_at: resetsAt },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
    subscription: null,
    fetched_at: nowSec(),
    source_host: 'mac',
    status: 'ok',
    detail: null,
    next_try_at: 0,
  };
}

let stop: (() => void) | null = null;

beforeEach(() => {
  clearToasts();
  accountUsage.set({});
  hosts.set([]);
  accounts.set([SILVESTER]);
  sessions.set([
    session('mac', 'a', { account_uuid: SILVESTER.uuid }),
    session('mac', 'b', { account_uuid: SILVESTER.uuid }),
    // Still working: not paused by the limit (yet).
    session('mac', 'c', { account_uuid: SILVESTER.uuid, claude_status: 'working' }),
    session('mac', 'd', { account_uuid: 'someone-else' }),
  ]);
  accountsPageRequest.set(null);
  accountsPausedRequest.set(false);
  destination.set('session');
});
afterEach(() => {
  stop?.();
  stop = null;
  clearToasts();
});

describe('limit-hit toast', () => {
  it('toasts when a reading turns an account onto its limit, with the paused count and Show paused sessions', () => {
    stop = startLimitToasts();
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 90) });
    expect(get(toasts)).toEqual([]);
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 100) });
    const [t] = get(toasts);
    expect(t.kind).toBe('warning');
    expect(t.message).toBe('tech.silvester hit the weekly limit');
    expect(t.sub).toMatch(/^2 sessions paused until /);
    expect(t.action?.label).toBe('Show paused sessions');

    runToastAction(t.id);
    expect(get(destination)).toBe('accounts');
    expect(get(accountsPageRequest)).toBe(SILVESTER.uuid);
    expect(get(accountsPausedRequest)).toBe(true);
  });

  it('says nothing for a limit already in place at the first reading, nor again while it holds', () => {
    stop = startLimitToasts();
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 100) });
    expect(get(toasts)).toEqual([]);
    accountUsage.set({ [SILVESTER.uuid]: { ...snap(SILVESTER.uuid, 100), fetched_at: nowSec() + 1 } });
    expect(get(toasts)).toEqual([]);
  });

  it('a limit that lifted and came back toasts again', () => {
    stop = startLimitToasts();
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 50) });
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 100) });
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 20) });
    clearToasts();
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 100) });
    expect(get(toasts)).toHaveLength(1);
  });

  it('with nothing paused there is no Show button', () => {
    sessions.set([]);
    stop = startLimitToasts();
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 90) });
    accountUsage.set({ [SILVESTER.uuid]: snap(SILVESTER.uuid, 100) });
    const [t] = get(toasts);
    expect(t.sub).toMatch(/^Nothing paused · resets /);
    expect(t.action).toBeNull();
  });
});

describe('limitHitLines / newlyLimited', () => {
  it('names the window and says when it resets, or that it does not know', () => {
    const now = 1_000_000;
    expect(limitHitLines('admin', { window: 'five_hour', resets_at: null }, 1, now)).toEqual({
      message: 'admin hit the 5-hour limit',
      sub: '1 session paused until it resets',
    });
    expect(limitHitLines('admin', { window: 'weekly', resets_at: null }, 0, now).sub).toBe('Nothing paused');
  });

  it('counts an account only when it was read before and was not already limited', () => {
    const lim = { window: 'weekly' as const, resets_at: 200 };
    expect(newlyLimited({}, { a: lim }, new Set(), 100)).toEqual([]);
    expect(newlyLimited({}, { a: lim }, new Set(['a']), 100)).toEqual(['a']);
    expect(newlyLimited({ a: lim }, { a: lim }, new Set(['a']), 100)).toEqual([]);
    // The old limit's reset is past: this is a new one.
    expect(newlyLimited({ a: { window: 'weekly', resets_at: 50 } }, { a: lim }, new Set(['a']), 100)).toEqual(['a']);
  });
});
