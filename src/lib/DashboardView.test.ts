import { render, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import DashboardView from './DashboardView.svelte';
import { workspace } from './workspace';
import { expectAccessible } from './a11y_check';

const mock = invoke as ReturnType<typeof vi.fn>;
const stages = { backlog: 2, in_progress: 1, in_review: 1, blocked: 1, done: 3 };

beforeEach(() => {
  mock.mockReset();
  mock.mockImplementation(async (cmd: string, a?: { args?: { filters?: { group_by?: string } } }) => {
    expect(cmd).toBe('work_stats');
    const by = a?.args?.filters?.group_by;
    return {
      group_by: by ?? 'group',
      total: 8,
      stages,
      pct_done: 37,
      needs_you: 2,
      cost_micros: 5_000_000,
      generated_at: 1_760_000_000,
      groups:
        by === 'sprint'
          ? [
              {
                group: { id: 'sprint:4', label: 'Sprint 42', source: 'sprint' },
                total: 8, stages, pct_done: 37, needs_you: 2, cost_micros: 0,
                bucket: { bucket_id: 4, kind: 'sprint', state: 'active', goal: 'Checkout', pct_time_elapsed: 70,
                          starts_at: 1_759_000_000, ends_at: 1_761_000_000, added_after_start: 3, removed_after_start: 1 },
              },
            ]
          : by
            ? [{ group: { id: 'person:ana', label: 'Ana', source: 'person' }, total: 8, stages, pct_done: 37, needs_you: 2, cost_micros: 0 }]
            : [],
    };
  });
});
afterEach(() => workspace.set('developer'));

describe('DashboardView', () => {
  it('draws the scrum master’s sprint health from work stats', async () => {
    workspace.set('scrum');
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByText('Sprint 42')).toBeTruthy());
    expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Sprint health');
    expect(r.getByText('Behind')).toBeTruthy();
    expect(r.getByText(/scope \+3\/−1 since start/)).toBeTruthy();
    expect(r.getByText('Ana')).toBeTruthy();
    // One read per distinct query, not per widget.
    const asked = new Set(mock.mock.calls.map((c) => JSON.stringify(c[1])));
    expect(mock.mock.calls.length).toBe(asked.size);
    await expectAccessible(r.container);
  });

  it('says what failed in the widget, and keeps the others', async () => {
    workspace.set('pm');
    mock.mockImplementation(async (_cmd: string, a?: { args?: { filters?: { group_by?: string } } }) => {
      if (a?.args?.filters?.group_by === 'mission') throw { code: 'E_INVALID', message: 'needs a newer hub' };
      return { group_by: 'group', total: 3, stages, pct_done: 0, needs_you: 0, cost_micros: 0, generated_at: 1, groups: [] };
    });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByText(/Could not read/)).toBeTruthy());
    expect(r.getAllByTestId('widget-stat').length).toBe(4);
  });
});
