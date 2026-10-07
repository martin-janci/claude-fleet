// The Work view's Missions tab (orchestration O1): the list, a new mission,
// its detail with the lifecycle moves the state allows, a new task under its
// root, and a refusal shown as text.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkMissions from './WorkMissions.svelte';
import { doneWhenRows, eventSentence, moveLabel, progressLabel, stateLabel, type Mission } from './missions';

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

function mission(over: Partial<Mission> = {}): Mission {
  return {
    id: 4,
    name: 'Payments v2',
    goal: 'Cards and refunds',
    mode: 'finite',
    state: 'draft',
    level: 0,
    plan_version: 1,
    created_at: 1,
    updated_at: 1,
    version: 1,
    root_item_id: 10,
    total: 1,
    done: 0,
    repos: [],
    ...over,
  };
}

const item = (id: number, title: string) => ({
  id,
  source: 'local',
  title,
  status_category: 'todo',
  created_at: 1,
  updated_at: 1,
});

describe('WorkMissions', () => {
  let current: Mission;
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    current = mission();
    handlers = {
      work_missions: () => [current],
      work_mission: () => ({
        mission: current,
        items: [item(10, 'Payments v2')],
        events: [{ id: 1, at: 1, kind: 'created', actor: 'person:1' }],
        may_change: true,
      }),
      save_mission: () => current,
      set_mission_state: (a) => (current = mission({ state: String(a.state), version: 2 })),
      create_work_task: () => item(11, 'Refunds'),
      set_mission_item: () => (current = mission({ total: 2 })),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      if (!h) return null;
      const v = h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {});
      if (v instanceof Error) throw { code: 'E_INVALID', message: v.message };
      return v;
    });
  });

  it('lists missions and says so when there are none', async () => {
    handlers.work_missions = () => [];
    render(WorkMissions);
    await flush();
    expect(screen.getByTestId('missions-empty')).toBeTruthy();
  });

  it('creates a mission from a name and a goal, then opens it', async () => {
    handlers.work_missions = () => [];
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-new'));
    await fireEvent.input(screen.getByTestId('mission-new-name'), { target: { value: ' Payments v2 ' } });
    await fireEvent.input(screen.getByTestId('mission-new-goal'), { target: { value: 'Cards and refunds' } });
    handlers.work_missions = () => [current];
    await fireEvent.click(screen.getByTestId('mission-create'));
    await flush();
    expect(calls('save_mission')[0]).toEqual({ mission: { name: 'Payments v2', goal: 'Cards and refunds' } });
    expect(screen.getByTestId('mission-detail').textContent).toContain('Cards and refunds');
  });

  it('offers the moves the state allows and sends the version it saw', async () => {
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-move-active').textContent).toBe('Start');
    expect(screen.queryByTestId('mission-move-paused')).toBeNull();
    await fireEvent.click(screen.getByTestId('mission-move-active'));
    await flush();
    expect(calls('set_mission_state')[0]).toEqual({ mission_id: 4, state: 'active', expected_version: 1 });
    expect(screen.getByTestId('mission-move-paused').textContent).toBe('Pause');
  });

  it('adds a new task under the root and into the mission', async () => {
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    await fireEvent.input(screen.getByTestId('mission-new-task'), { target: { value: 'Refunds' } });
    await fireEvent.submit(screen.getByTestId('mission-new-task').closest('form')!);
    await flush();
    expect(calls('create_work_task')[0]).toEqual({ title: 'Refunds', parent: 'item:10' });
    expect(calls('set_mission_item')[0]).toEqual({ mission_id: 4, item_id: 11, on: true });
  });

  it('shows a refusal as text and keeps the mission open', async () => {
    handlers.set_mission_state = () => new Error('a mission does not go from draft to paused');
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    await fireEvent.click(screen.getByTestId('mission-move-active'));
    await flush();
    expect(screen.getByTestId('mission-notice').textContent).toContain('does not go from');
    expect(screen.getByTestId('mission-detail')).toBeTruthy();
  });

  it('hides the controls from someone who may only read', async () => {
    handlers.work_mission = () => ({ mission: current, items: [], events: [], may_change: false });
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.queryByTestId('mission-edit')).toBeNull();
    expect(screen.queryByTestId('mission-move-active')).toBeNull();
    expect(screen.queryByTestId('mission-delete')).toBeNull();
  });
});

describe('missions helpers', () => {
  it('labels states, moves and progress', () => {
    expect(stateLabel('active', 'running')).toBe('Active · running');
    expect(moveLabel('paused', 'active')).toBe('Resume');
    expect(moveLabel('draft', 'active')).toBe('Start');
    expect(progressLabel({ total: 7, done: 3 })).toBe('3/7 done');
    expect(progressLabel({ total: 0, done: 0 })).toBe('');
  });

  it('reads done_when one condition per line', () => {
    expect(doneWhenRows(' ci:test green \n\n review ')).toEqual(['ci:test green', 'review']);
  });

  it('turns log rows into sentences', () => {
    expect(eventSentence({ id: 1, at: 1, kind: 'state', actor: 'fleet', payload: { from: 'draft', to: 'active' } })).toBe(
      'draft → active',
    );
    expect(eventSentence({ id: 2, at: 1, kind: 'updated', actor: 'fleet', payload: { fields: ['goal'] } })).toBe(
      'Changed goal',
    );
    expect(eventSentence({ id: 3, at: 1, kind: 'task_done', actor: 'fleet' })).toBe('task done');
  });
});
