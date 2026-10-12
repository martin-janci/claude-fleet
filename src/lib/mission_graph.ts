// A mission's task graph laid out as lanes × waves (the second step the
// orchestration design names after the wave list, 2026-10-07 §4.4): every
// member a box in its lane's row and its wave's column, every dependency an
// arrow, the longest unfinished chain marked as the critical path, and the
// progress per lane, per wave and overall.
//
// Pure: it reads the `work_mission` answer and returns geometry, so
// MissionGraph.svelte only draws. Nothing here is stored; the hub derives
// each node's state and wave (`service::work::graph`), and a lane is a
// grouping this view picks (the task's repo or assignee), not a field.

import type { MissionDetail, GraphNode } from './missions';
import { wavesOf } from './missions';
import type { WorkItemRow } from './trackers';

/** What a lane is. */
export type LaneBy = 'repo' | 'assignee' | 'none';

export const LANE_BY: readonly LaneBy[] = ['repo', 'assignee', 'none'];

export function isLaneBy(v: unknown): v is LaneBy {
  return v === 'repo' || v === 'assignee' || v === 'none';
}

/** The design system's five states (StatusChip): a node's state mapped
 *  onto the one the chip and the dot draw. */
export type Tone = 'done' | 'working' | 'waiting' | 'failed' | 'idle';

const TONE: Record<string, Tone> = {
  done: 'done',
  running: 'working',
  doing: 'working',
  verifying: 'working',
  failed: 'failed',
  blocked: 'failed',
  // A proposal waits for a person to accept it: Needs you.
  proposed: 'waiting',
  held: 'idle',
  waiting: 'idle',
  ready: 'idle',
  rejected: 'idle',
};

export function toneOf(state: string): Tone {
  return TONE[state] ?? 'idle';
}

/** Geometry, in px. Exported so the view and its tests agree. */
export const G = {
  laneHead: 132,
  waveHead: 28,
  col: 212,
  node: 40,
  nodeW: 184,
  gap: 8,
  pad: 10,
} as const;

export interface LaneBox {
  id: string;
  label: string;
  done: number;
  total: number;
  y: number;
  h: number;
}

export interface WaveBox {
  wave: number;
  done: number;
  total: number;
  x: number;
}

export interface NodeBox {
  item_id: number;
  key: string | null;
  title: string;
  url: string | null;
  state: string;
  tone: Tone;
  wave: number;
  lane: string;
  critical: boolean;
  x: number;
  y: number;
  node: GraphNode;
}

export interface EdgeBox {
  from: number;
  to: number;
  /** The task it waits for is done. */
  met: boolean;
  critical: boolean;
  /** An SVG path from the right edge of `from` to the left edge of `to`. */
  d: string;
}

export interface Progress {
  done: number;
  /** Every member but a rejected proposal. */
  total: number;
  /** Members per node state. */
  byState: Record<string, number>;
}

export interface MissionLayout {
  lanes: LaneBox[];
  waves: WaveBox[];
  nodes: NodeBox[];
  edges: EdgeBox[];
  /** The longest chain of unfinished tasks, first to last; empty when
   *  nothing is left. */
  critical: number[];
  progress: Progress;
  width: number;
  height: number;
}

const UNASSIGNED = 'Unassigned';
const NO_REPO = 'No repo';

/** The lane a task sits in. */
function laneOf(it: WorkItemRow | undefined, by: LaneBy, repoName: (id: number) => string | null): string {
  if (by === 'none') return 'All tasks';
  if (by === 'assignee') return it?.assignees?.[0] ?? UNASSIGNED;
  const pid = it?.project_id;
  return pid != null ? (repoName(pid) ?? `Repo ${pid}`) : NO_REPO;
}

/** The lanes that tell most apart: repos when the tasks span several,
 *  else assignees when they name several, else one lane. */
export function defaultLaneBy(detail: MissionDetail): LaneBy {
  const items = (detail.items ?? []).filter((i) => i.proposal_state !== 'rejected');
  const repos = new Set(items.map((i) => i.project_id ?? null));
  if (repos.size > 1) return 'repo';
  const people = new Set(items.map((i) => i.assignees?.[0] ?? null));
  if (people.size > 1) return 'assignee';
  return 'none';
}

const counts = (s: string) => s !== 'rejected';

/** The longest chain of unfinished members, by count; ties go to the
 *  lower item id so the answer is stable. Only edges inside the mission
 *  count: an outside dependency makes a node blocked, not longer. */
export function criticalPath(nodes: GraphNode[]): number[] {
  const open = new Map(nodes.filter((n) => n.state !== 'done' && counts(n.state)).map((n) => [n.item_id, n]));
  const memo = new Map<number, number[]>();
  const visiting = new Set<number>();
  const longestTo = (id: number): number[] => {
    const hit = memo.get(id);
    if (hit) return hit;
    // The store refuses cycles; guard anyway so a bad answer cannot hang.
    if (visiting.has(id)) return [];
    visiting.add(id);
    let best: number[] = [];
    for (const d of [...(open.get(id)?.depends_on ?? [])].sort((a, b) => a - b)) {
      if (!open.has(d)) continue;
      const p = longestTo(d);
      if (p.length > best.length) best = p;
    }
    visiting.delete(id);
    const out = [...best, id];
    memo.set(id, out);
    return out;
  };
  let best: number[] = [];
  for (const id of [...open.keys()].sort((a, b) => a - b)) {
    const p = longestTo(id);
    if (p.length > best.length) best = p;
  }
  return best.length > 1 ? best : [];
}

/** Everything `id` waits for, directly or not, and everything waiting on
 *  it: the chain a hover lights up. */
export function chainOf(nodes: GraphNode[], id: number): Set<number> {
  const up = new Map(nodes.map((n) => [n.item_id, n.depends_on ?? []]));
  const down = new Map<number, number[]>();
  for (const n of nodes) for (const d of n.depends_on ?? []) down.set(d, [...(down.get(d) ?? []), n.item_id]);
  const out = new Set<number>([id]);
  const walk = (start: number, next: Map<number, number[]>) => {
    const todo = [start];
    while (todo.length) {
      for (const m of next.get(todo.pop()!) ?? []) {
        if (!out.has(m) && up.has(m)) {
          out.add(m);
          todo.push(m);
        }
      }
    }
  };
  walk(id, up);
  walk(id, down);
  return out;
}

function edgePath(x1: number, y1: number, x2: number, y2: number): string {
  const mid = (x2 - x1) / 2;
  return `M${x1},${y1} C${x1 + mid},${y1} ${x2 - mid},${y2} ${x2},${y2}`;
}

/** Lay the mission out. `repoName` names a project id (the projects store). */
export function layoutMission(
  detail: MissionDetail,
  by: LaneBy,
  repoName: (id: number) => string | null = () => null,
): MissionLayout {
  const items = new Map((detail.items ?? []).map((i) => [i.id, i]));
  const all = wavesOf(detail).flatMap((w) => w.nodes);
  const nodes = all.filter((n) => items.has(n.item_id));
  const critical = criticalPath(nodes);
  const onPath = new Set(critical);
  const pathEdge = new Set(critical.slice(1).map((id, i) => `${critical[i]}>${id}`));

  const byState: Record<string, number> = {};
  for (const n of nodes) byState[n.state] = (byState[n.state] ?? 0) + 1;
  const progress: Progress = {
    done: nodes.filter((n) => n.state === 'done').length,
    total: nodes.filter((n) => counts(n.state)).length,
    byState,
  };

  // Lanes in first-seen order by wave then id, so the lane holding W1's
  // first task is on top; the catch-all lanes sink to the bottom.
  const laneIds: string[] = [];
  const cell = new Map<string, GraphNode[]>();
  for (const n of nodes) {
    const lane = laneOf(items.get(n.item_id), by, repoName);
    if (!laneIds.includes(lane)) laneIds.push(lane);
    const k = `${lane}|${n.wave}`;
    cell.set(k, [...(cell.get(k) ?? []), n]);
  }
  laneIds.sort((a, b) => Number(a === UNASSIGNED || a === NO_REPO) - Number(b === UNASSIGNED || b === NO_REPO));

  const waveNums = [...new Set(nodes.map((n) => n.wave))].sort((a, b) => a - b);
  const col = new Map(waveNums.map((w, i) => [w, i]));
  const waves: WaveBox[] = waveNums.map((w, i) => {
    const ns = nodes.filter((n) => n.wave === w && counts(n.state));
    return { wave: w, done: ns.filter((n) => n.state === 'done').length, total: ns.length, x: G.laneHead + i * G.col };
  });

  const lanes: LaneBox[] = [];
  const boxes: NodeBox[] = [];
  let y = G.waveHead;
  for (const lane of laneIds) {
    const rows = Math.max(1, ...waveNums.map((w) => cell.get(`${lane}|${w}`)?.length ?? 0));
    const h = G.pad * 2 + rows * G.node + (rows - 1) * G.gap;
    let done = 0;
    let total = 0;
    for (const w of waveNums) {
      (cell.get(`${lane}|${w}`) ?? []).forEach((n, i) => {
        const it = items.get(n.item_id)!;
        if (counts(n.state)) total++;
        if (n.state === 'done') done++;
        boxes.push({
          item_id: n.item_id,
          key: it.key ?? null,
          title: it.title,
          url: it.url ?? null,
          state: n.state,
          tone: toneOf(n.state),
          wave: n.wave,
          lane,
          critical: onPath.has(n.item_id),
          x: G.laneHead + col.get(w)! * G.col + (G.col - G.nodeW) / 2,
          y: y + G.pad + i * (G.node + G.gap),
          node: n,
        });
      });
    }
    lanes.push({ id: lane, label: lane, done, total, y, h });
    y += h;
  }

  const at = new Map(boxes.map((b) => [b.item_id, b]));
  const edges: EdgeBox[] = [];
  for (const b of boxes) {
    for (const d of b.node.depends_on ?? []) {
      const a = at.get(d);
      if (!a) continue;
      edges.push({
        from: d,
        to: b.item_id,
        met: a.state === 'done',
        critical: pathEdge.has(`${d}>${b.item_id}`),
        d: edgePath(a.x + G.nodeW, a.y + G.node / 2, b.x, b.y + G.node / 2),
      });
    }
  }

  return {
    lanes,
    waves,
    nodes: boxes,
    edges,
    critical,
    progress,
    width: G.laneHead + Math.max(1, waveNums.length) * G.col,
    height: y,
  };
}

/** "12 of 30 done · 40%". */
export function progressLine(p: Progress): string {
  if (p.total === 0) return 'No tasks yet';
  return `${p.done} of ${p.total} done · ${Math.round((p.done / p.total) * 100)}%`;
}

/** The states worth a count in the header, in the order a person acts on
 *  them: what needs them, what broke, what runs, what can start. */
export const SUMMARY_STATES = ['proposed', 'failed', 'blocked', 'running', 'doing', 'verifying', 'ready', 'waiting', 'held'] as const;
