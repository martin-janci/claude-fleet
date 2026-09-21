import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  reportError,
  installErrorReporting,
  resetErrorReportingForTests,
  MAX_PER_MINUTE,
} from './error_report';

const inv = () => invoke as ReturnType<typeof vi.fn>;
const calls = () => inv().mock.calls.filter((c) => c[0] === 'report_client_error');

beforeEach(() => {
  inv().mockReset();
  inv().mockResolvedValue(undefined);
  resetErrorReportingForTests();
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-09-21T10:00:00Z'));
});

describe('reportError', () => {
  it('invokes report_client_error with the record', () => {
    reportError('frontend', 'boom', 'E_PARSE', { url: 'x' });
    expect(calls()).toHaveLength(1);
    expect(calls()[0][1]).toEqual({
      args: { level: 'error', component: 'frontend', code: 'E_PARSE', message: 'boom', context: { url: 'x' } },
    });
  });

  it('sends an identical message once a minute, counting repeats on the next distinct one', () => {
    reportError('frontend', 'same');
    reportError('frontend', 'same');
    reportError('frontend', 'same');
    expect(calls()).toHaveLength(1);
    reportError('frontend', 'other');
    expect(calls()).toHaveLength(2);
    expect((calls()[1][1] as { args: { context: { repeats: number } } }).args.context.repeats).toBe(2);
    vi.advanceTimersByTime(61_000);
    reportError('frontend', 'same');
    expect(calls()).toHaveLength(3);
  });

  it('stops after MAX_PER_MINUTE and resumes the next minute', () => {
    for (let i = 0; i < MAX_PER_MINUTE + 5; i++) reportError('frontend', `m${i}`);
    expect(calls()).toHaveLength(MAX_PER_MINUTE);
    vi.advanceTimersByTime(60_001);
    reportError('frontend', 'later');
    expect(calls()).toHaveLength(MAX_PER_MINUTE + 1);
  });
});

describe('installErrorReporting', () => {
  it('reports window errors and unhandled rejections', () => {
    installErrorReporting(window);
    window.dispatchEvent(new ErrorEvent('error', { message: 'ReferenceError: x', filename: 'app.js', lineno: 3 }));
    const rej = new Event('unhandledrejection') as Event & { reason: unknown };
    rej.reason = new Error('rejected');
    window.dispatchEvent(rej);
    expect(calls().map((c) => (c[1] as { args: { component: string } }).args.component)).toEqual([
      'frontend:unhandled',
      'frontend:unhandled',
    ]);
    expect((calls()[1][1] as { args: { message: string } }).args.message).toContain('rejected');
  });
});
