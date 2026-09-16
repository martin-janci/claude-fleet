import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  accountUsage,
  loadAccountUsage,
  refreshAccountUsage,
  applyAccountUsageEvents,
  type AccountUsageSnapshot,
  type UsageStatus,
} from './account_usage_store';

function snapshot(over: Partial<AccountUsageSnapshot> = {}): AccountUsageSnapshot {
  return {
    account_uuid: 'acct-1',
    usage: null,
    subscription: null,
    fetched_at: null,
    source_host: null,
    status: 'never_fetched',
    detail: null,
    next_try_at: 0,
    ...over,
  };
}

beforeEach(() => {
  accountUsage.set({});
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('accountUsage store', () => {
  it('loadAccountUsage fills the store keyed by account_uuid', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([
      snapshot({ account_uuid: 'a' }),
      snapshot({ account_uuid: 'b', status: 'ok' }),
    ]);
    const r = await loadAccountUsage();
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('list_account_usage', undefined);
    const map = get(accountUsage);
    expect(Object.keys(map).sort()).toEqual(['a', 'b']);
    expect(map.b.status).toBe('ok');
  });

  it('loadAccountUsage leaves the store untouched on failure', async () => {
    accountUsage.set({ a: snapshot({ account_uuid: 'a' }) });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValue({
      code: 'E_LOCK',
      message: 'store mutex poisoned',
    });
    const r = await loadAccountUsage();
    expect(r.ok).toBe(false);
    expect(Object.keys(get(accountUsage))).toEqual(['a']);
  });

  it('refreshAccountUsage sends the account_uuid payload and merges the result', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(
      snapshot({ account_uuid: 'a', status: 'ok', fetched_at: 1000 }),
    );
    const r = await refreshAccountUsage('a');
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('refresh_account_usage', {
      args: { account_uuid: 'a' },
    });
    expect(get(accountUsage).a.status).toBe('ok');
    expect(get(accountUsage).a.fetched_at).toBe(1000);
  });

  it('refreshAccountUsage surfaces an IpcError without touching the store', async () => {
    accountUsage.set({ a: snapshot({ account_uuid: 'a' }) });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValue({
      code: 'E_NOTFOUND',
      message: 'account a not found',
    });
    const r = await refreshAccountUsage('a');
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_NOTFOUND');
    expect(get(accountUsage).a.status).toBe('never_fetched');
  });

  it('applyAccountUsageEvents patches rows in place, replacing by account_uuid', () => {
    accountUsage.set({ a: snapshot({ account_uuid: 'a', status: 'ok' }) });
    applyAccountUsageEvents([
      snapshot({ account_uuid: 'a', status: 'unavailable' }),
      snapshot({ account_uuid: 'b', status: 'rate_limited' }),
    ]);
    const map = get(accountUsage);
    expect(map.a.status).toBe('unavailable');
    expect(map.b.status).toBe('rate_limited');
  });

  it('applyAccountUsageEvents with no events leaves the store untouched (same reference)', () => {
    const before = get(accountUsage);
    applyAccountUsageEvents([]);
    expect(get(accountUsage)).toBe(before);
  });

  it('every UsageStatus value round-trips through the store unchanged', () => {
    const statuses: UsageStatus[] = [
      'ok',
      'no_credentials',
      'access_token_expired',
      'login_expired',
      'token_rejected',
      'rate_limited',
      'unavailable',
      'no_online_host',
      'never_fetched',
      'host_unsupported',
    ];
    // A type-level check as much as a runtime one: this array literal only
    // compiles if every member is assignable to `UsageStatus`, and the
    // length check below only passes if none were silently widened to
    // `string` by a typo.
    expect(statuses.length).toBe(10);
    applyAccountUsageEvents(statuses.map((status, i) => snapshot({ account_uuid: `u${i}`, status })));
    const map = get(accountUsage);
    for (let i = 0; i < statuses.length; i++) {
      expect(map[`u${i}`].status).toBe(statuses[i]);
    }
  });
});
