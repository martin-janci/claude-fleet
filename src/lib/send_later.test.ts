import { describe, it, expect } from 'vitest';
import { parseLocalDateTime, sendLaterPlan, toLocalDateTime, tomorrowAtNine } from './send_later';

// A held clock: Saturday 10 October 2026, 14:20 local time.
const NOW = new Date(2026, 9, 10, 14, 20, 0, 0).getTime();
const sec = (d: Date) => Math.floor(d.getTime() / 1000);

describe('Send later times (G2.7 on G1.8)', () => {
  it('When it is idle sends no time at all, so an older hub reads the call it knows', () => {
    expect(sendLaterPlan('idle', NOW, '', false)).toEqual({ timing: {}, when: 'when it is idle', error: null });
  });

  it('In 1 hour holds it until an hour from now', () => {
    const p = sendLaterPlan('hour', NOW, '', false, 'en-US');
    expect(p.timing).toEqual({ notBefore: sec(new Date(NOW + 3600_000)) });
    expect(p.when).toBe('at Sat 15:20');
  });

  it('Tomorrow 09:00 is nine in the morning, local time, the next day', () => {
    expect(tomorrowAtNine(NOW)).toEqual(new Date(2026, 9, 11, 9, 0, 0, 0));
    const p = sendLaterPlan('tomorrow', NOW, '', true, 'en-US');
    expect(p.timing).toEqual({ notBefore: sec(new Date(2026, 9, 11, 9, 0)), skipIfArchived: true });
    expect(p.when).toBe('at Sun 09:00');
  });

  it('When the usage limit resets waits for the account, not a time', () => {
    expect(sendLaterPlan('limit', NOW, '', true).timing).toEqual({ untilLimitReset: true, skipIfArchived: true });
  });

  it('At… takes a local date and time, and refuses one that has passed', () => {
    const p = sendLaterPlan('at', NOW, '2026-10-12T08:30', false, 'en-US');
    expect(p.timing).toEqual({ notBefore: sec(new Date(2026, 9, 12, 8, 30)) });
    expect(p.when).toBe('at Mon 08:30');
    expect(sendLaterPlan('at', NOW, '2026-10-10T14:00', false).error).toBe('That time has passed. Pick a later one.');
    expect(sendLaterPlan('at', NOW, '', false).error).toBe('Pick a day and a time.');
  });

  it('reads and writes the datetime-local value in local time', () => {
    const d = new Date(2026, 0, 2, 3, 4);
    expect(toLocalDateTime(d)).toBe('2026-01-02T03:04');
    expect(parseLocalDateTime('2026-01-02T03:04')).toEqual(d);
    expect(parseLocalDateTime('2026-02-31T03:04')).toBeNull();
    expect(parseLocalDateTime('soon')).toBeNull();
  });
});
