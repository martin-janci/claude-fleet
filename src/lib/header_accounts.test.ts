import { describe, it, expect } from 'vitest';
import { headerAccount, headerAccounts } from './header_accounts';
import { ADMIN, GMAIL, HOUR, NOW, RESET_WEEK, WORK, snapshot } from './hosts_fixture';

describe('header account pills (3.17)', () => {
  it('says both windows as % left in one short line', () => {
    const a = headerAccount(ADMIN, snapshot(ADMIN.uuid), NOW);
    expect(a.meta).toBe('5h 91% · wk 58%');
    expect(a.health).toBe('ok');
    expect(a.limited).toBe(false);
    expect(a.aria).toBe(`Account ${ADMIN.email}, 91 percent of the 5-hour window left, 58 percent of the weekly window left`);
  });

  it('names the limit and when it lifts instead of numbers', () => {
    const snap = snapshot(WORK.uuid, {
      usage: { five_hour: { utilization: 30, resets_at: null }, seven_day: { utilization: 100, resets_at: RESET_WEEK }, seven_day_opus: null, seven_day_sonnet: null },
    });
    const a = headerAccount(WORK, snap, NOW, 'en-GB', 'UTC');
    expect(a.health).toBe('limit');
    expect(a.limited).toBe(true);
    expect(a.meta).toBe('weekly limit · Thu 09:00');
  });

  it('shows no number for a reading too old to hold', () => {
    const a = headerAccount(GMAIL, snapshot(GMAIL.uuid, { fetched_at: NOW - 5 * HOUR }), NOW);
    expect(a.meta).toBe('');
    expect(a.health).toBe('unknown');
  });

  it('puts the worst account first', () => {
    const low = snapshot(WORK.uuid, {
      usage: { five_hour: { utilization: 70, resets_at: null }, seven_day: null, seven_day_opus: null, seven_day_sonnet: null },
    });
    const list = headerAccounts([ADMIN, GMAIL, WORK], { [ADMIN.uuid]: snapshot(ADMIN.uuid), [WORK.uuid]: low }, NOW);
    expect(list.map((a) => [a.uuid, a.health])).toEqual([
      [WORK.uuid, 'caution'],
      [ADMIN.uuid, 'ok'],
      [GMAIL.uuid, 'unknown'],
    ]);
  });
});
