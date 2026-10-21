import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import ControlMissionView from './ControlMissionView.svelte';
import { controlMission } from './control_room';
import { missionOpenRequest } from './missions';
import { destination } from './destination';
import { bumpWorkChanged } from './work';

// Thread room: a mission thread or chip opens the mission in Control's
// right panel, with its task graph.

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
const mission = { id: 5, name: 'Hub federation v2', goal: '', mode: 'finite', state: 'active', level: 2, plan_version: 1, created_at: 1, updated_at: 1, version: 1, total: 2, done: 1 };

beforeEach(() => {
  controlMission.set(null);
  missionOpenRequest.set(null);
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd === 'work_missions') return [mission];
    if (cmd === 'work_mission')
      return {
        mission,
        items: [
          { id: 1, title: 'Peer link model', status: 'done' },
          { id: 2, title: 'Pairing codes', status: 'in_progress' },
        ],
        graph: { nodes: [{ item_id: 1, state: 'done', wave: 1 }, { item_id: 2, state: 'running', wave: 2, depends_on: [1] }], waves: 2 },
      };
    return null;
  });
});

describe('ControlMissionView', () => {
  it('lists the missions, and one opens with its graph; Open mission room leaves for its room', async () => {
    render(ControlMissionView);
    await fireEvent.click(await screen.findByTestId('control-mission-row'));
    expect(get(controlMission)).toBe(5);
    expect((await screen.findByTestId('control-mission-name')).textContent).toBe('Hub federation v2');
    expect(screen.getByTestId('control-missions-view').querySelector('svg')).not.toBeNull();
    destination.set('control');
    await fireEvent.click(screen.getByTestId('control-mission-work'));
    expect(get(missionOpenRequest)).toEqual({ id: 5 });
    expect(screen.getByTestId('control-mission-work').textContent).toContain('Open mission room');
    expect(screen.queryByTestId('control-mission-wait')).toBeNull();
    await fireEvent.click(screen.getByTestId('control-mission-back'));
    expect(get(controlMission)).toBeNull();
  });

  it('a mission waiting on the person says on what and offers Answer (w04-8)', async () => {
    const waiting = { ...mission, waiting_on: { reason: 'sign_grant' } };
    inv.mockImplementation(async (cmd: string) => (cmd === 'work_mission' ? { mission: waiting, items: [], graph: { nodes: [], waves: 0 } } : null));
    controlMission.set(5);
    render(ControlMissionView);
    expect((await screen.findByTestId('control-mission-wait')).textContent).toContain('Grant needs signing');
    await fireEvent.click(screen.getByTestId('control-mission-answer'));
    expect(get(missionOpenRequest)).toEqual({ id: 5 });
  });

  // Review w17-5: loading is not "No missions yet", a failure has Retry, and
  // a failed refresh keeps the graph.
  it('says nothing is empty while the list loads, and a failed read has Retry', async () => {
    let answer: (v: unknown) => void = () => {};
    inv.mockImplementation((cmd: string) => (cmd === 'work_missions' ? new Promise((r) => (answer = r)) : Promise.resolve(null)));
    render(ControlMissionView);
    await Promise.resolve();
    expect(screen.queryByTestId('control-missions-empty')).toBeNull();
    answer(Promise.reject({ code: 'E_HUB_TIMEOUT', message: 'slow' }));
    expect((await screen.findByTestId('control-missions-load-error')).textContent).toContain("Couldn't load missions");
    inv.mockImplementation(async (cmd: string) => (cmd === 'work_missions' ? [] : null));
    await fireEvent.click(screen.getByTestId('control-missions-load-error-retry'));
    expect(await screen.findByTestId('control-missions-empty')).toBeTruthy();
  });

  it('a failed refresh keeps the open mission graph', async () => {
    controlMission.set(5);
    render(ControlMissionView);
    await screen.findByTestId('control-mission-name');
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'work_mission') throw { code: 'E_HUB_TIMEOUT', message: 'slow' };
      return null;
    });
    bumpWorkChanged();
    await waitFor(() => expect(screen.getByTestId('control-missions-refresh-error')).toBeTruthy(), { timeout: 2000 });
    expect(screen.getByTestId('control-mission-name').textContent).toBe('Hub federation v2');
    expect(screen.getByTestId('control-missions-view').querySelector('svg')).not.toBeNull();
  });

  // Review w30-1 for Control: a deleted or hidden mission answers E_NOTFOUND,
  // which is "gone", not a failed refresh with a Retry that cannot work.
  it('a mission deleted while open says it is gone, with no stale graph', async () => {
    controlMission.set(5);
    render(ControlMissionView);
    await screen.findByTestId('control-mission-name');
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'work_mission') throw { code: 'E_NOTFOUND', message: 'mission 5 not found' };
      return null;
    });
    bumpWorkChanged();
    await waitFor(() => expect(screen.getByTestId('control-missions-view').textContent).toContain('not in the fleet any more'), { timeout: 2000 });
    expect(screen.queryByTestId('control-mission-name')).toBeNull();
    expect(screen.queryByTestId('control-missions-refresh-error')).toBeNull();
  });

  it('opening a mission that is already gone says so, without Retry', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'work_mission') throw { code: 'E_NOTFOUND', message: 'mission 9 not found' };
      return null;
    });
    controlMission.set(9);
    render(ControlMissionView);
    await waitFor(() => expect(screen.getByTestId('control-missions-view').textContent).toContain('not in the fleet any more'));
    expect(screen.getByTestId('control-missions-view').textContent).not.toContain("Couldn't load the mission");
  });
});
