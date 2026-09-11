import { describe, it, expect } from 'vitest';
import { hookHealth, hookHealthLabel, lastHookEventAt, formatAge } from './hook_health';

const rows = [
  { host_alias: 'mefistos', last_stop_at: 100 },
  { host_alias: 'mefistos', last_stop_at: 250 },
  { host_alias: 'mefistos', last_stop_at: null },
  { host_alias: 'local', last_stop_at: null },
];

describe('hook health', () => {
  it('takes the newest Stop hook per host', () => {
    expect(lastHookEventAt('mefistos', rows)).toBe(250);
    expect(lastHookEventAt('local', rows)).toBeNull();
    expect(lastHookEventAt('other', rows)).toBeNull();
  });

  it('distinguishes not installed, never seen and seen', () => {
    expect(hookHealth('local', false, rows)).toEqual({ state: 'not_installed' });
    expect(hookHealth('local', true, rows)).toEqual({ state: 'never_seen' });
    expect(hookHealth('mefistos', true, rows)).toEqual({ state: 'seen', lastAt: 250 });
    // A delivered hook outranks a missing token row.
    expect(hookHealth('mefistos', false, rows).state).toBe('seen');
  });

  it('labels each state', () => {
    expect(hookHealthLabel({ state: 'not_installed' }, 0)).toBe('not installed');
    expect(hookHealthLabel({ state: 'never_seen' }, 0)).toBe('installed · never seen');
    expect(hookHealthLabel({ state: 'seen', lastAt: 250 }, 262)).toBe('last event 12s ago');
  });

  it('formats ages compactly and clamps skew', () => {
    expect(formatAge(-5)).toBe('0s');
    expect(formatAge(59)).toBe('59s');
    expect(formatAge(60 * 5)).toBe('5m');
    expect(formatAge(3600 * 3)).toBe('3h');
    expect(formatAge(86400 * 2)).toBe('2d');
  });
});
