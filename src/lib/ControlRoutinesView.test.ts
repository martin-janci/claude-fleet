// Gap plan G3.10 (board MCTasks ◷): Control's Routines view.
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

const calls: { cmd: string; args: Record<string, unknown> }[] = [];
let routines: unknown[] = [];
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, raw: { args: Record<string, unknown> }) => {
    calls.push({ cmd, args: raw?.args });
    if (cmd === 'routines' && raw.args.action === 'list') return routines;
    if (cmd === 'routines' && raw.args.action === 'run_now') return { id: 1, routine_id: raw.args.routine_id, state: 'running' };
    if (cmd === 'list_runs')
      return { runs: [{ id: 1, kind: 'routine', routine_id: 2, outcome: 'failed', started_at: 10, cost_micros: 0 }], total: 1 };
    throw { code: 'E_TEST', message: `no ${cmd}` };
  }),
}));
import ControlRoutinesView from './ControlRoutinesView.svelte';
import { destination } from './destination';
import { routinesRequest } from './routines';
import { expectAccessible } from './a11y_check';

const routine = (id: number, name: string, over: Record<string, unknown> = {}) => ({
  id,
  name,
  enabled: true,
  trigger: 'cron',
  cron: '30 7 * * 1-5',
  utc_offset_min: 0,
  host_alias: 'mac',
  project_id: 1,
  prompt: 'p',
  overlap: 'skip',
  skip_next: false,
  created_at: 1,
  updated_at: 1,
  ...over,
});

beforeEach(() => {
  calls.length = 0;
  routines = [routine(1, 'Alpha sweep'), routine(2, 'Nightly deps')];
});

describe('ControlRoutinesView (G3.10)', () => {
  it('lists routines with the failed one first, and a row opens it in Automation', async () => {
    const { getAllByTestId, container } = render(ControlRoutinesView);
    await waitFor(() => expect(getAllByTestId('control-routine-row')).toHaveLength(2));
    const rows = getAllByTestId('control-routine-row');
    expect(rows[0].textContent).toContain('Nightly deps');
    expect(rows[0].getAttribute('data-state')).toBe('failed');
    expect(rows[0].textContent).toContain('last run failed');
    await expectAccessible(container);
    await fireEvent.click(rows[1].querySelector('.open')!);
    expect(get(destination)).toBe('automation');
    expect(get(routinesRequest)).toMatchObject({ select: 1 });
  });

  it('Run now starts one', async () => {
    const { getAllByTestId, getByTestId } = render(ControlRoutinesView);
    await waitFor(() => expect(getAllByTestId('control-routine-run')).toHaveLength(2));
    await fireEvent.click(getAllByTestId('control-routine-run')[1]);
    await waitFor(() => expect(getByTestId('control-routines-note').textContent).toBe('Alpha sweep started.'));
    expect(calls.find((c) => c.args?.action === 'run_now')?.args).toMatchObject({ routine_id: 1 });
  });

  it('with none, offers a template', async () => {
    routines = [];
    const { findByTestId } = render(ControlRoutinesView);
    await fireEvent.click(await findByTestId('control-routines-template'));
    expect(get(routinesRequest)).toMatchObject({ template: 'morning-pr-sweep' });
  });
});
