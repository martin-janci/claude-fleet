import { describe, it, expect } from 'vitest';
import { graphFromMission, graphFromTree, graphProgress } from './task_graph_card';
import type { MissionDetail } from './missions';

// Thread room (2026-10-14): the compact task graph card in Control's chat.

describe('task graph card', () => {
  it('lays a proposed tree out by wave, with its state per step', () => {
    const g = graphFromTree({
      id: 1,
      at: 0,
      kind: 'tree',
      tool: 'propose_tree',
      item: { id: 10, title: 'Release 0.6.2', status: 'todo' },
      items: [
        { id: 11, title: 'Fix flake', status: 'done', proposal_state: 'accepted' },
        { id: 12, title: 'Merge #912', status: 'in_progress', proposal_state: 'accepted', depends_on: [11] },
        { id: 13, title: 'Push tag', status: 'todo', proposal_state: 'proposed', depends_on: [12] },
      ],
    })!;
    expect(g.title).toBe('Release 0.6.2');
    expect(g.waves.map((w) => w.map((s) => s.title))).toEqual([['Fix flake'], ['Merge #912'], ['Push tag']]);
    expect(g.waves.flat().map((s) => s.tone)).toEqual(['done', 'working', 'idle']);
    expect(g.waves[1][0].current).toBe(true);
    expect(graphProgress(g)).toBe('1 of 3 done');
  });

  it("lays a mission's own graph out by wave, titled by its items, and marks the step that needs you", () => {
    const d = {
      mission: { id: 5, name: 'Hub federation v2' },
      items: [
        { id: 1, title: 'Peer link model' },
        { id: 2, title: 'Pairing codes' },
        { id: 3, title: 'Revoke codes' },
      ],
      graph: {
        nodes: [
          { item_id: 1, state: 'done', wave: 1 },
          { item_id: 2, state: 'running', wave: 2 },
          { item_id: 3, state: 'proposed', wave: 2 },
        ],
      },
    } as unknown as MissionDetail;
    const g = graphFromMission(d)!;
    expect(g.waves.map((w) => w.map((s) => s.title))).toEqual([['Peer link model'], ['Pairing codes', 'Revoke codes']]);
    expect(g.waves[1][1].current).toBe(true);
    expect(g.waves[1][0].current).toBe(false);
    expect(graphProgress(g)).toBe('1 of 3 done');
  });

  it('draws nothing without steps', () => {
    expect(graphFromTree({ id: 1, at: 0, kind: 'tree', tool: 'x', items: [] })).toBeNull();
    expect(graphFromMission({ mission: { id: 1, name: 'x' } } as unknown as MissionDetail)).toBeNull();
  });
});
