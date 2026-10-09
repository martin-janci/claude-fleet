import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';
import JobChip from './JobChip.svelte';

describe('JobChip', () => {
  it('is visual only (the footer’s live region announces), without a bar when it does not count', () => {
    render(JobChip, { label: 'Scanning hosts' });
    const chip = screen.getByTestId('assets-job');
    expect(chip.getAttribute('role')).toBeNull();
    expect(chip.getAttribute('aria-live')).toBeNull();
    expect(chip.textContent).toContain('Scanning hosts');
    expect(chip.querySelector('[role="progressbar"]')).toBeNull();
  });
  it('counts with a progress bar when it can', () => {
    render(JobChip, { label: 'Syncing', done: 2, total: 5 });
    const bar = screen.getByRole('progressbar');
    expect(screen.getByTestId('assets-job').textContent).toContain('Syncing 2/5');
    expect([bar.getAttribute('aria-valuenow'), bar.getAttribute('aria-valuemax')]).toEqual(['2', '5']);
  });
  it('a zero total does not count', () => {
    render(JobChip, { label: 'Syncing', done: 0, total: 0 });
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  // Review r12: the kit's 16 px Orbit, not a CSS ⟳ spinner, so it waits
  // 400 ms and follows the app's Motion setting.
  it('marks the job with the 16 px Orbit after 400 ms', async () => {
    vi.useFakeTimers();
    render(JobChip, { label: 'Scanning hosts' });
    const chip = screen.getByTestId('assets-job');
    expect(chip.textContent).not.toContain('⟳');
    expect(screen.getByTestId('assets-job-mark-pending')).toBeTruthy();
    await vi.advanceTimersByTimeAsync(400);
    const mark = screen.getByTestId('assets-job-mark');
    expect(mark.getAttribute('data-loader')).toBe('orbit');
    expect(mark.style.width).toBe('16px');
  });
});

afterEach(() => vi.useRealTimers());
