// The routine draft's pure helpers (review wave w05).
import { describe, it, expect } from 'vitest';
import { draftCron, draftMissing, draftOf, withDays, zoneOfSaved } from './routine_draft';
import { budgetProblem, capProblem, isCronProblem, microsOf, rateLabel } from '../routines';

const base = { name: 'N', trigger: 'cron' as const, cron: '30 7 * * 1-5', host_alias: 'mac', project_id: 1, prompt: 'P' };

describe('zoneOfSaved (review w05-3: a routine saved without a zone keeps its hour)', () => {
  it('reads offset 0 as UTC, a whole-hour offset as its fixed zone, the device offset as the device zone', () => {
    expect(zoneOfSaved({ utc_offset_min: 0 }, 'Europe/Bratislava')).toEqual({ zone: 'UTC' });
    expect(zoneOfSaved({ utc_offset_min: 120 }, 'Asia/Tokyo')).toEqual({ zone: 'Etc/GMT-2' });
    expect(zoneOfSaved({ utc_offset_min: -300 }, 'Asia/Tokyo')).toEqual({ zone: 'Etc/GMT+5' });
    // Either side of the device's clock change is still the device's zone.
    expect(zoneOfSaved({ utc_offset_min: 60 }, 'Europe/Bratislava')).toEqual({ zone: '' });
    expect(zoneOfSaved({ utc_offset_min: 120 }, 'Europe/Bratislava')).toEqual({ zone: '' });
    expect(zoneOfSaved({ utc_offset_min: 330 }, 'Europe/Bratislava')).toEqual({ zone: '', offset: 330 });
    expect(zoneOfSaved({ utc_offset_min: 0, time_zone: 'Asia/Tokyo' }, 'Europe/Bratislava')).toEqual({ zone: 'Asia/Tokyo' });
    // A new routine (no offset yet) is the device's.
    expect(zoneOfSaved({}, 'Europe/Bratislava')).toEqual({ zone: '' });
  });
});

describe('withDays (review w05-7: Repeats keeps the time it shows)', () => {
  it('Custom starts from the line the picker meant', () => {
    const d = draftOf({ ...base, cron: '0 9 * * 1-5' });
    d.time = '07:30';
    withDays(d, 'custom');
    expect(d.cron).toBe('30 7 * * 1-5');
    expect(draftCron(d)).toBe('30 7 * * 1-5');
  });
  it('moves the time between the minute of Every hour and the HH:MM of the rest', () => {
    const d = draftOf({ ...base, cron: '0 * * * *' });
    expect(d.days).toBe('hourly');
    withDays(d, 'weekdays');
    expect(d.time).toBe('09:00');
    expect(draftMissing(d)).toBeNull();
    d.time = '07:45';
    withDays(d, 'hourly');
    expect(d.time).toBe('45');
    expect(draftCron(d)).toBe('45 * * * *');
  });
  it('leaving Custom reads the line it held', () => {
    const d = draftOf({ ...base, cron: '15 6 * * *' });
    withDays(d, 'custom');
    d.cron = '20 5 * * 1-5';
    withDays(d, 'weekdays');
    expect(d.time).toBe('05:20');
  });
});

describe('the limits (review w05-5, w05-6)', () => {
  it('reads a comma as the decimal point and a "$ " before the amount', () => {
    expect(microsOf('5,50')).toBe(5_500_000);
    expect(microsOf('$ 5')).toBe(5_000_000);
  });
  it('says what is wrong with a budget instead of saving it as no limit', () => {
    expect(budgetProblem('')).toBeNull();
    expect(budgetProblem('5.5')).toBeNull();
    expect(budgetProblem('1.255')).toMatch(/amount like 5/);
    expect(budgetProblem('abc')).toMatch(/amount like 5/);
    expect(budgetProblem('0')).toMatch(/above \$0/);
  });
  it('takes a time cap in whole minutes from 1 to 360', () => {
    expect(capProblem('')).toBeNull();
    expect(capProblem('30')).toBeNull();
    expect(capProblem('360')).toBeNull();
    for (const bad of ['0', '0.5', '400', 'abc']) expect(capProblem(bad)).toMatch(/1 to 360/);
  });
  it('a draft with a bad limit does not save', () => {
    const d = draftOf(base);
    d.budget_run = '5,505';
    expect(draftMissing(d)).toBe('Fix the budget per run.');
    d.budget_run = '';
    d.cap_min = '0.5';
    expect(draftMissing(d)).toBe('Fix the time cap.');
  });
});

describe('draftMissing with the known projects (review w05-11)', () => {
  it('a routine whose project is gone asks for another', () => {
    expect(draftMissing(draftOf({ ...base, project_id: 9 }), [1, 2])).toBe('Pick a project: its project is gone.');
    expect(draftMissing(draftOf(base), [1, 2])).toBeNull();
  });
});

describe('rateLabel and isCronProblem', () => {
  it('words a rate the presets lack', () => {
    expect(rateLabel(1800)).toBe('once per 30 minutes');
    expect(rateLabel(7200)).toBe('once per 2 hours');
    expect(rateLabel(60)).toBe('once per minute');
    expect(rateLabel(45)).toBe('once per 45 seconds');
  });
  it('tells a problem with the cron line from another one', () => {
    expect(isCronProblem('hour: "25" is not a number from 0 to 23')).toBe(true);
    expect(isCronProblem('a schedule has five fields (minute hour day month weekday), got 3')).toBe(true);
    expect(isCronProblem('"0 0 31 2 *" never fires')).toBe(true);
    expect(isCronProblem('host gone not found')).toBe(false);
    expect(isCronProblem(undefined)).toBe(false);
  });
});
