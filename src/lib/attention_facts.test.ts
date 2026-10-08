import { describe, it, expect, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { attentionFacts, attentionFactsFrom, blockedLine } from './attention_facts';
import { classify } from './attention';
import { hosts } from './hosts';
import { accountUsage } from './account_usage_store';
import { uiLayout } from './prefs';
import { host, session, snapshot } from './hosts_fixture';
import type { UsageStatus } from './account_usage_store';

const usage = (uuid: string, status: UsageStatus, five: number, week: number, resets: number) =>
  snapshot(uuid, {
    status,
    usage: {
      five_hour: { utilization: five, resets_at: resets },
      seven_day: { utilization: week, resets_at: resets + 100 },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
  });

afterEach(() => {
  uiLayout.set('classic');
  hosts.set([]);
  accountUsage.set({});
});

describe('attentionFactsFrom (mirrors attention::Facts::from_fleet)', () => {
  it('reads pinged hosts and the usage snapshots', () => {
    const f = attentionFactsFrom(
      [
        host('up', { reachable: true, last_pinged_at: 1 }),
        host('down', { reachable: false, last_pinged_at: 1 }),
        host('new', { reachable: false, last_pinged_at: null }),
      ],
      {
        five: usage('five', 'ok', 100, 40, 2000),
        both: usage('both', 'ok', 100, 100, 2000),
        reset: usage('reset', 'ok', 100, 10, 500),
        fine: usage('fine', 'ok', 80, 99, 2000),
        gone: usage('gone', 'login_expired', 0, 0, 2000),
        refresh: usage('refresh', 'access_token_expired', 0, 0, 2000),
      },
      1000,
    );
    expect(f.down_hosts).toEqual(['down']);
    expect(f.limited_accounts).toEqual({
      five: { window: 'five_hour', resets_at: 2000 },
      both: { window: 'weekly', resets_at: 2100 },
    });
    expect(f.uncredentialed_accounts).toEqual(['gone']);
  });

  it('is the New layout only', () => {
    hosts.set([host('down', { reachable: false, last_pinged_at: 1 })]);
    expect(get(attentionFacts)).toBeUndefined();
    uiLayout.set('new');
    expect(get(attentionFacts)?.down_hosts).toEqual(['down']);
  });
});

describe('the Blocked buckets', () => {
  it('a limit stops blocking once its window resets', () => {
    const s = session('mefistos', 'a', { claude_status: 'idle', account_uuid: 'acc' });
    const facts = { limited_accounts: { acc: { window: 'weekly' as const, resets_at: 2000 } } };
    expect(classify(s, { idleSecs: 0, now: 1999, facts })).toBe('account_limit');
    expect(classify(s, { idleSecs: 0, now: 2000, facts })).toBe('idle');
  });

  it('words each reason line as the Main board does', () => {
    const s = { host_alias: 'mefistos', account_uuid: 'acc' };
    const name = () => 'tech.silvester';
    const facts = { limited_accounts: { acc: { window: 'weekly' as const, resets_at: null } } };
    expect(blockedLine('account_limit', s, facts, name)).toBe('Paused · weekly limit on tech.silvester');
    expect(
      blockedLine('account_limit', s, { limited_accounts: { acc: { window: 'five_hour', resets_at: null } } }, name),
    ).toBe('Paused · 5-hour limit on tech.silvester');
    expect(blockedLine('host_down', s, facts, name)).toBe('Blocked · mefistos is down');
    expect(blockedLine('no_credentials', s, facts, name)).toBe('Blocked · tech.silvester is signed out');
    expect(blockedLine('waiting', s, facts, name)).toBeNull();
  });
});
