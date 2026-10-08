// The task graph's layout: lanes, waves, the critical path, progress and
// the chain a hover lights up.
import { describe, it, expect } from 'vitest';
import type { GraphNode, MissionDetail } from './missions';
import type { WorkItemRow } from './trackers';
import { G, chainOf, criticalPath, defaultLaneBy, layoutMission, progressLine, toneOf } from './mission_graph';

function item(id: number, over: Partial<WorkItemRow> = {}): WorkItemRow {
  return { id, source: 'native', title: `Task ${id}`, status_category: 'todo', created_at: 1, updated_at: 1, ...over };
}

const node = (item_id: number, wave: number, state: string, depends_on: number[] = []): GraphNode => ({
  item_id,
  wave,
  state,
  depends_on,
});

function detail(items: WorkItemRow[], nodes: GraphNode[]): MissionDetail {
  return {
    mission: { id: 1, name: 'M', goal: 'g', mode: 'finite', state: 'active', level: 0, plan_version: 1, created_at: 1, updated_at: 1, version: 1 },
    items,
    graph: { nodes, waves: Math.max(0, ...nodes.map((n) => n.wave)) },
  };
}

// 1 → 2 → 4, 1 → 3; 5 alone; 6 rejected.
const nodes = [
  node(1, 1, 'done'),
  node(5, 1, 'ready'),
  node(6, 1, 'rejected'),
  node(2, 2, 'running', [1]),
  node(3, 2, 'ready', [1]),
  node(4, 3, 'waiting', [2]),
];
const items = [
  item(1, { project_id: 10 }),
  item(2, { project_id: 10, assignees: ['ana'] }),
  item(3, { project_id: 11, assignees: ['bo'] }),
  item(4, { project_id: 10 }),
  item(5),
  item(6, { proposal_state: 'rejected' }),
];

describe('criticalPath', () => {
  it('is the longest unfinished chain, done tasks left out', () => {
    expect(criticalPath(nodes)).toEqual([2, 4]);
  });
  it('is empty when nothing chains', () => {
    expect(criticalPath([node(1, 1, 'ready'), node(2, 1, 'ready')])).toEqual([]);
  });
  it('breaks ties by the lower id and survives a cycle', () => {
    const tie = [node(1, 1, 'ready'), node(2, 2, 'ready', [1]), node(3, 1, 'ready'), node(4, 2, 'ready', [3])];
    expect(criticalPath(tie)).toEqual([1, 2]);
    expect(criticalPath([node(1, 1, 'ready', [2]), node(2, 1, 'ready', [1])]).length).toBeLessThanOrEqual(2);
  });
});

describe('chainOf', () => {
  it('holds what a task waits for and what waits on it', () => {
    expect([...chainOf(nodes, 2)].sort()).toEqual([1, 2, 4]);
    expect([...chainOf(nodes, 5)]).toEqual([5]);
  });
});

describe('defaultLaneBy', () => {
  it('picks repos when the tasks span several, then assignees, then one lane', () => {
    expect(defaultLaneBy(detail(items, nodes))).toBe('repo');
    expect(defaultLaneBy(detail([item(1, { assignees: ['a'] }), item(2, { assignees: ['b'] })], []))).toBe('assignee');
    expect(defaultLaneBy(detail([item(1), item(2)], []))).toBe('none');
  });
});

describe('layoutMission', () => {
  const names: Record<number, string> = { 10: 'acme/api', 11: 'acme/web' };
  const l = layoutMission(detail(items, nodes), 'repo', (id) => names[id] ?? null);

  it('counts progress without the rejected proposal', () => {
    expect(l.progress).toMatchObject({ done: 1, total: 5 });
    expect(l.progress.byState.running).toBe(1);
    expect(progressLine(l.progress)).toBe('1 of 5 done · 20%');
  });

  it('puts a lane per repo with the catch-all last, and a column per wave', () => {
    expect(l.lanes.map((x) => x.label)).toEqual(['acme/api', 'acme/web', 'No repo']);
    expect(l.lanes[0]).toMatchObject({ done: 1, total: 3 });
    expect(l.waves.map((w) => [w.wave, w.done, w.total])).toEqual([
      [1, 1, 2],
      [2, 0, 2],
      [3, 0, 1],
    ]);
  });

  it('places a task in its lane row and its wave column', () => {
    const at = new Map(l.nodes.map((n) => [n.item_id, n]));
    expect(at.get(4)!.x).toBeGreaterThan(at.get(2)!.x);
    expect(at.get(2)!.x).toBeGreaterThan(at.get(1)!.x);
    expect(at.get(3)!.y).toBeGreaterThanOrEqual(l.lanes[1].y);
    expect(at.get(1)!.x).toBe(G.laneHead + (G.col - G.nodeW) / 2);
    expect(l.width).toBe(G.laneHead + 3 * G.col);
  });

  it('draws an arrow per dependency inside the mission and marks the critical ones', () => {
    expect(l.edges.map((e) => [e.from, e.to, e.met, e.critical])).toEqual(
      expect.arrayContaining([
        [1, 2, true, false],
        [1, 3, true, false],
        [2, 4, false, true],
      ]),
    );
    expect(l.edges).toHaveLength(3);
    expect(l.nodes.filter((n) => n.critical).map((n) => n.item_id).sort()).toEqual([2, 4]);
  });

  it('stacks tasks that share a lane and a wave', () => {
    const one = layoutMission(detail(items, nodes), 'none');
    expect(one.lanes).toHaveLength(1);
    const w1 = one.nodes.filter((n) => n.wave === 1).map((n) => n.y);
    expect(new Set(w1).size).toBe(w1.length);
    expect(one.lanes[0].h).toBe(G.pad * 2 + 3 * G.node + 2 * G.gap);
  });

  it('lanes by assignee name the unassigned', () => {
    const by = layoutMission(detail(items, nodes), 'assignee');
    expect(by.lanes.map((x) => x.label)).toEqual(['ana', 'bo', 'Unassigned']);
  });

  it('lays out items an older hub left out of the graph in wave 1', () => {
    const old = layoutMission({ ...detail([item(1), item(2, { status_category: 'done' })], []), graph: undefined }, 'none');
    expect(old.nodes.map((n) => [n.item_id, n.wave, n.state])).toEqual([
      [1, 1, 'ready'],
      [2, 1, 'done'],
    ]);
    expect(old.edges).toEqual([]);
  });

  it('says so when there is nothing yet', () => {
    const none = layoutMission(detail([], []), 'none');
    expect(none.nodes).toEqual([]);
    expect(progressLine(none.progress)).toBe('No tasks yet');
  });
});

describe('toneOf', () => {
  it('maps node states onto the five design-system states', () => {
    expect(['done', 'running', 'proposed', 'blocked', 'held', 'mystery'].map(toneOf)).toEqual([
      'done',
      'working',
      'waiting',
      'failed',
      'idle',
      'idle',
    ]);
  });
});
