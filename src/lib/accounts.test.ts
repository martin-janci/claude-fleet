import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  accounts,
  loadAccounts,
  probeSshAlias,
  probeSshAliasAbortable,
  accountLabel,
  setAccountNickname,
  type AccountRow,
} from './accounts';

const sample: AccountRow = {
  uuid: 'u1',
  email: 'a@b.com',
  display_name: 'A B',
  organization_name: '32bit',
  organization_uuid: 'org-1',
  seat_tier: 'max',
  last_seen_at: 1000,
  nickname: null,
  has_extra_usage: false,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  accounts.set([]);
});

describe('accounts store', () => {
  it('loadAccounts populates the store on success', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sample]);
    const r = await loadAccounts();
    expect(r.ok).toBe(true);
    expect(get(accounts)).toHaveLength(1);
    expect(get(accounts)[0].uuid).toBe('u1');
  });

  it('loadAccounts handles empty list', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
    const r = await loadAccounts();
    expect(r.ok).toBe(true);
    expect(get(accounts)).toHaveLength(0);
  });

  it('probeSshAlias passes ssh_alias and returns preview', async () => {
    const preview = {
      reachable: true,
      claude_version: '2.1.144',
      tmux_version: '3.6a',
      account: { uuid: 'u1', email: 'a@b.com', display_name: 'A B', organization_name: null, organization_uuid: null, seat_tier: 'max' },
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(preview);
    const r = await probeSshAlias('mefistos');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'probe_ssh_alias',
      { args: { ssh_alias: 'mefistos' } },
    ]);
    if (r.ok) {
      expect(r.value.account?.uuid).toBe('u1');
      expect(r.value.tmux_version).toBe('3.6a');
    }
  });

  it('probeSshAlias handles probe failure', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: 'E_PROBE', message: 'unreachable' });
    const r = await probeSshAlias('bad-host');
    expect(r.ok).toBe(false);
  });

  it('store is empty after reset (beforeEach hygiene)', () => {
    expect(get(accounts)).toHaveLength(0);
  });

  it('probeSshAliasAbortable fires cancel_command on abort', async () => {
    let resolveInvoke: (v: unknown) => void = () => {};
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation((cmd: string) => {
      if (cmd === 'cancel_command') return Promise.resolve(null);
      return new Promise((res) => {
        resolveInvoke = res;
      });
    });
    const ac = new AbortController();
    const p = probeSshAliasAbortable('test-alias', ac.signal);
    ac.abort();
    await new Promise((r) => setTimeout(r, 0));
    const sawCancel = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some(
      (c) => c[0] === 'cancel_command',
    );
    expect(sawCancel).toBe(true);
    resolveInvoke(null);
    await p;
  });

  it('probeSshAliasAbortable returns E_CANCELLED if signal already aborted', async () => {
    const ac = new AbortController();
    ac.abort();
    const r = await probeSshAliasAbortable('test-alias', ac.signal);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_CANCELLED');
  });

  it('setAccountNickname sends uuid and nickname, and merges the returned row', async () => {
    accounts.set([sample]);
    const updated: AccountRow = { ...sample, nickname: 'Home' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(updated);
    const r = await setAccountNickname('u1', 'Home');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'set_account_nickname',
      { args: { uuid: 'u1', nickname: 'Home' } },
    ]);
    expect(get(accounts)[0].nickname).toBe('Home');
  });

  it('setAccountNickname passes null to clear', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ...sample,
      nickname: null,
    });
    await setAccountNickname('u1', null);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'set_account_nickname',
      { args: { uuid: 'u1', nickname: null } },
    ]);
  });
});

describe('accountLabel', () => {
  it('prefers the nickname when set', () => {
    expect(accountLabel({ ...sample, nickname: 'Home' })).toBe('Home');
  });

  it('falls back to the email when there is no nickname', () => {
    expect(accountLabel({ ...sample, nickname: null })).toBe('a@b.com');
  });

  it('falls back to the trimmed nickname over a blank one', () => {
    expect(accountLabel({ ...sample, nickname: '   ' })).toBe('a@b.com');
  });

  it('falls back to the first 8 characters of the uuid when there is no email', () => {
    expect(
      accountLabel({ ...sample, nickname: null, email: null, uuid: 'abcdefghijkl' }),
    ).toBe('abcdefgh');
  });

  it('falls back to "unknown account" when there is nothing at all', () => {
    expect(accountLabel({ ...sample, nickname: null, email: null, uuid: '' })).toBe(
      'unknown account',
    );
  });

  it('returns "unknown account" for null or undefined', () => {
    expect(accountLabel(null)).toBe('unknown account');
    expect(accountLabel(undefined)).toBe('unknown account');
  });
});
