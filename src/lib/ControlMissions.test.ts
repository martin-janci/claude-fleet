// Redesign step 9.12, Control part: running missions in the Views panel with
// Comet trails beside the current step; none while a mission waits on a
// person.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import ControlMissions from './ControlMissions.svelte';
import { missionNow, type MissionDetail } from './missions';
import { expectAccessible } from './a11y_check';

const mission = (id: number, name: string, state = 'active') => ({
  id,
  name,
  goal: 'g',
  mode: 'finite',
  state,
  level: 0,
  plan_version: 1,
  created_at: 1,
  updated_at: 1,
  version: 1,
});
const item = (id: number, title: string) => ({ id, source: 'local', title, status_category: 'todo', created_at: 1, updated_at: 1 });

function detail(id: number, name: string, over: Partial<MissionDetail> = {}): MissionDetail {
  return {
    mission: mission(id, name),
    items: [item(11, 'Schema'), item(12, 'API')],
    graph: { nodes: [{ item_id: 11, state: 'running', wave: 1 }, { item_id: 12, state: 'ready', wave: 1 }], waves: 1 },
    plan: { steps: [], cards: [], autonomy: { level: 1 } as never, cost_micros: 0, counts: { total: 2, open: 1 } },
    ...over,
  } as MissionDetail;
}

describe('missionNow', () => {
  it('names the current step and draws trails while it runs', () => {
    expect(missionNow(detail(1, 'Payments'))).toEqual({ id: 1, name: 'Payments', step: 'Schema', waiting: false, trails: true });
  });

  it('waits on a person: an ask step or an open card, and no trails', () => {
    const ask = missionNow(detail(1, 'P', { plan: { ...detail(1, 'P').plan!, steps: [{ kind: 'ask', reason: 'r', auto: false }] } }));
    expect([ask.waiting, ask.trails]).toEqual([true, false]);
  });
});

describe('ControlMissions', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    const details: Record<number, MissionDetail> = {
      1: detail(1, 'Payments'),
      2: detail(2, 'Refunds', {
        plan: {
          steps: [],
          cards: [{ id: 9, mission_id: 2, decision_id: 'd', source: 'loop', kind: 'confirm', state: 'open', created_at: 1 }],
          autonomy: { level: 1 } as never,
          cost_micros: 0,
          counts: { total: 2, open: 1 },
        },
      }),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      if (cmd === 'work_missions') return [mission(1, 'Payments'), mission(2, 'Refunds'), mission(3, 'Draft', 'draft')];
      if (cmd === 'work_mission') return details[(raw as { args: { mission_id: number } }).args.mission_id];
      return null;
    });
  });

  it('lists active missions; trails beside a running step, none while one waits on you', async () => {
    const { container } = render(ControlMissions);
    const rows = await vi.waitFor(() => {
      const r = screen.getAllByTestId('control-mission');
      expect(r).toHaveLength(2);
      return r;
    });
    expect(rows[0].textContent).toContain('Schema');
    expect(rows[1].textContent).toContain('Waits for you');
    const trails = await vi.waitFor(() => screen.getAllByTestId('control-mission-trails'));
    expect(trails).toHaveLength(1);
    expect(rows[0].contains(trails[0])).toBe(true);
    await expectAccessible(container);
  });
});
