// The burn-up chart (persona dashboards, step 8): both series drawn and
// named, both numbers on hover, and the same numbers as a table.
import { fireEvent, render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import BurnupChart from './BurnupChart.svelte';
import { expectAccessible } from './a11y_check';

const D = 86_400;
const days = [
  { day: 100 * D, total: 4, done: 1 },
  { day: 101 * D, total: 5, done: 2 },
  { day: 102 * D, total: 5, done: 4 },
];

describe('BurnupChart', () => {
  it('draws done and scope, names both, and ends each line in its value', async () => {
    const r = render(BurnupChart, { days, title: 'Sprint burn-up' });
    expect(r.container.querySelectorAll('path.line').length).toBe(2);
    expect(r.getByText('Done')).toBeTruthy();
    expect(r.getByText('Scope')).toBeTruthy();
    const ends = Array.from(r.container.querySelectorAll('text.end')).map((t) => t.textContent);
    expect(ends).toEqual(['5', '4']);
    expect(r.getByRole('img').getAttribute('aria-label')).toMatch(/^Sprint burn-up: 4 of 5 done on /);
    await expectAccessible(r.container);
  });

  it('says both numbers for the day under the pointer', async () => {
    const r = render(BurnupChart, { days, title: 'B' });
    const hits = r.container.querySelectorAll('rect.hit');
    await fireEvent.pointerEnter(hits[1]);
    expect(r.getByTestId('burnup-tip').textContent).toContain('2 of 5 done');
    await fireEvent.pointerLeave(hits[1]);
    expect(r.queryByTestId('burnup-tip')).toBeNull();
  });

  it('shows the same numbers as a table, and says when there is no history', async () => {
    const r = render(BurnupChart, { days, title: 'B' });
    await fireEvent.click(r.getByTestId('burnup-table-toggle'));
    const rows = Array.from(r.getByTestId('burnup-table').querySelectorAll('tbody tr')).map((tr) =>
      Array.from(tr.querySelectorAll('td')).slice(1).map((td) => td.textContent),
    );
    expect(rows).toEqual([['1', '4'], ['2', '5'], ['4', '5']]);
    const empty = render(BurnupChart, { days: [], title: 'E' });
    expect(empty.getByTestId('burnup-empty')).toBeTruthy();
  });
});
