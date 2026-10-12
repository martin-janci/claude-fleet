import { describe, it, expect, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { accountPill, accountsPageRequest, openAccount, pillLevel } from './account_pill';
import { destination } from './destination';
import { ADMIN, HOUR, NOW, RESET_5H, RESET_WEEK, account, snapshot } from './hosts_fixture';

afterEach(() => {
  destination.set('session');
  accountsPageRequest.set(null);
});

const usage = (five: number, week: number) => ({
  five_hour: { utilization: five, resets_at: RESET_5H },
  seven_day: { utilization: week, resets_at: RESET_WEEK },
  seven_day_opus: null,
  seven_day_sonnet: null,
});

describe('pillLevel', () => {
  it('is amber from 80% used and red at the limit', () => {
    expect(pillLevel(0)).toBe('ok');
    expect(pillLevel(79)).toBe('ok');
    expect(pillLevel(80)).toBe('warn');
    expect(pillLevel(99)).toBe('warn');
    expect(pillLevel(100)).toBe('limit');
  });
});

describe('accountPill', () => {
  it('shows the % left of the tighter window', () => {
    const p = accountPill(ADMIN.uuid, ADMIN, snapshot(ADMIN.uuid), NOW, 'en-GB', 'UTC');
    // 9% used of 5 h, 42% of the week: the week binds.
    expect(p.window).toBe('weekly');
    expect(p.left).toBe(58);
    expect(p.level).toBe('ok');
    expect(p.text).toBe(`${ADMIN.email} 58%`);
    expect(p.title).toContain('58% of the weekly window left');
  });

  it('turns amber at 80% used and red, worded, at the limit', () => {
    expect(accountPill(ADMIN.uuid, ADMIN, snapshot(ADMIN.uuid, { usage: usage(82, 10) }), NOW).level).toBe('warn');
    const lim = accountPill(ADMIN.uuid, ADMIN, snapshot(ADMIN.uuid, { usage: usage(30, 100) }), NOW);
    expect(lim.level).toBe('limit');
    expect(lim.text).toBe(`${ADMIN.email} LIMIT`);
    const extra = account('acc-x', 'x@example.com', { has_extra_usage: true });
    expect(accountPill(extra.uuid, extra, snapshot(extra.uuid, { usage: usage(100, 5) }), NOW).text).toBe(
      'x@example.com EXTRA USAGE',
    );
  });

  it('is at the limit only on the raw figure, not a rounded 0% left', () => {
    const p = accountPill(ADMIN.uuid, ADMIN, snapshot(ADMIN.uuid, { usage: usage(10, 99.6) }), NOW);
    expect(p.level).toBe('warn');
    expect(p.text).toBe(`${ADMIN.email} 0%`);
  });

  it('lets the weekly limit bind once the 5-hour window has reset', () => {
    const u = { ...usage(100, 100), five_hour: { utilization: 100, resets_at: NOW - 60 } };
    const p = accountPill(ADMIN.uuid, ADMIN, snapshot(ADMIN.uuid, { usage: u }), NOW);
    expect(p.window).toBe('weekly');
    expect(p.level).toBe('limit');
  });

  it('shows no number without a reading, or once it expired', () => {
    expect(accountPill(ADMIN.uuid, ADMIN, undefined, NOW).text).toBe(ADMIN.email);
    const old = accountPill(ADMIN.uuid, ADMIN, snapshot(ADMIN.uuid, { fetched_at: NOW - 5 * HOUR }), NOW);
    expect(old.left).toBeNull();
    expect(old.level).toBe('ok');
  });

  it('names an unknown account by its uuid', () => {
    expect(accountPill('0123456789ab', undefined, undefined, NOW).label).toBe('01234567');
  });
});

describe('openAccount', () => {
  it('asks the Accounts page for the account and goes there', () => {
    openAccount(ADMIN.uuid);
    expect(get(destination)).toBe('accounts');
    expect(get(accountsPageRequest)).toBe(ADMIN.uuid);
  });
});
