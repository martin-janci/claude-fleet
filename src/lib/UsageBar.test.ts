import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import UsageBar from './UsageBar.svelte';
import type { UsageWindow } from './account_usage_store';

const NOW = 1789396320; // Mon 2026-09-14 14:32 UTC
const MIN = 60;

function bar(props: {
  window?: '5h' | 'weekly';
  win: UsageWindow | null;
  fetchedAt?: number | null;
  hasExtraUsage?: boolean;
  compact?: boolean;
}) {
  render(UsageBar, {
    props: { window: '5h', fetchedAt: NOW - 2 * MIN, now: NOW, ...props },
  });
  return screen.getByRole('meter');
}

const far = NOW + 2 * 3600;

describe('UsageBar', () => {
  it('fills with used and labels what is left', () => {
    const m = bar({ win: { utilization: 9, resets_at: far } });
    expect(m).toHaveAttribute('aria-valuemin', '0');
    expect(m).toHaveAttribute('aria-valuemax', '100');
    expect(m).toHaveAttribute('aria-valuenow', '9');
    expect(m).toHaveAttribute('aria-label', '91% left');
    expect(m).toHaveAttribute('data-level', 'ok');
    expect(m).toHaveAttribute('data-freshness', 'fresh');
    expect(m).not.toHaveClass('unknown');
    const fill = screen.getByTestId('usage-fill');
    expect(fill.getAttribute('style')).toContain('width: 9%');
    expect(fill).toHaveClass('level-ok');
  });

  it('colours caution, and stripes low and limit', () => {
    bar({ win: { utilization: 60, resets_at: far } });
    expect(screen.getByTestId('usage-fill')).toHaveClass('level-caution');
  });

  it.each([
    [85, 'low'],
    [100, 'limit'],
  ])('utilization %d is %s', (u, level) => {
    const m = bar({ win: { utilization: u, resets_at: far } });
    expect(m).toHaveAttribute('data-level', level);
    expect(screen.getByTestId('usage-fill')).toHaveClass(`level-${level}`);
  });

  it('dims a stale value and says it is approximate', () => {
    const m = bar({ win: { utilization: 38, resets_at: far }, fetchedAt: NOW - 14 * MIN });
    expect(m).toHaveClass('stale');
    expect(m).toHaveAttribute('data-freshness', 'stale');
    expect(m).toHaveAttribute('aria-label', 'about 62% left');
    expect(m).toHaveAttribute('aria-valuenow', '38');
  });

  it.each([
    ['expired by age', { win: { utilization: 38, resets_at: far }, fetchedAt: NOW - 31 * MIN }],
    ['past its reset', { win: { utilization: 38, resets_at: NOW - 1 }, fetchedAt: NOW - 2 * MIN }],
    ['never fetched', { win: { utilization: 38, resets_at: far }, fetchedAt: null }],
    ['missing', { win: null }],
  ])('renders a dashed empty track, never a solid empty bar, when %s', (_name, props) => {
    const m = bar(props as { win: UsageWindow | null; fetchedAt?: number | null });
    expect(m).toHaveClass('unknown');
    expect(m).not.toHaveClass('stale');
    expect(m).toHaveAttribute('data-freshness', 'unknown');
    expect(m).toHaveAttribute('aria-label', 'usage unknown');
    expect(m).not.toHaveAttribute('aria-valuenow');
    expect(screen.queryByTestId('usage-fill')).toBeNull();
    expect(screen.queryByTestId('usage-pace')).toBeNull();
  });

  it('draws the pace tick on weekly bars only', () => {
    const reset = NOW + 3.5 * 86400; // half the week elapsed
    bar({ window: 'weekly', win: { utilization: 42, resets_at: reset } });
    expect(screen.getByTestId('usage-pace').getAttribute('style')).toContain('left: 50%');
  });

  it('has no pace tick on the 5-hour bar', () => {
    bar({ window: '5h', win: { utilization: 42, resets_at: far } });
    expect(screen.queryByTestId('usage-pace')).toBeNull();
  });

  it('supports a compact size', () => {
    expect(bar({ win: { utilization: 1, resets_at: far }, compact: true })).toHaveClass('compact');
  });
});
