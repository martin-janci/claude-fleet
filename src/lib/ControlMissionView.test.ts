import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import ControlMissionView from './ControlMissionView.svelte';
import { controlMission } from './control_room';
import { missionOpenRequest } from './missions';
import { destination } from './destination';

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
  it('lists the missions, and one opens with its graph; Open in Work leaves for Work', async () => {
    render(ControlMissionView);
    await fireEvent.click(await screen.findByTestId('control-mission-row'));
    expect(get(controlMission)).toBe(5);
    expect((await screen.findByTestId('control-mission-name')).textContent).toBe('Hub federation v2');
    expect(screen.getByTestId('control-missions-view').querySelector('svg')).not.toBeNull();
    destination.set('control');
    await fireEvent.click(screen.getByTestId('control-mission-work'));
    expect(get(missionOpenRequest)).toEqual({ id: 5 });
    await fireEvent.click(screen.getByTestId('control-mission-back'));
    expect(get(controlMission)).toBeNull();
  });
});
