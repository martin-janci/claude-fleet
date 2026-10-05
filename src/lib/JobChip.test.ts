import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import JobChip from './JobChip.svelte';

describe('JobChip', () => {
  it('announces the work politely, without a bar when it does not count', () => {
    render(JobChip, { label: 'Scanning hosts' });
    const chip = screen.getByTestId('assets-job');
    expect(chip.getAttribute('role')).toBe('status');
    expect(chip.getAttribute('aria-live')).toBe('polite');
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
});
