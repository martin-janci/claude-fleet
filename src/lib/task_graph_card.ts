// The task graph card in Control's chat (Thread room, 2026-10-14; boards
// Main and A3Mission): when the chat shows a mission handoff or a proposed
// tree of tasks, a compact card lays its steps out by wave, each tinted by
// its state, with "Open full graph". Pure: the mission's own graph
// (`work_mission`, laid out in full by mission_graph.ts / MissionGraph) or
// the tree's items and their `depends_on` (handoffs.treeWaves).
import { treeWaves, workState, type ControlHandoff } from './handoffs';
import { nodeLabel, type MissionDetail } from './missions';
import { toneOf, type Tone } from './mission_graph';

export interface GraphStep {
  id: number;
  title: string;
  tone: Tone;
  /** The state in words, under the title. */
  sub: string;
  /** The step the work is at now (the first that needs you, else running). */
  current: boolean;
}

export interface GraphCard {
  title: string;
  waves: GraphStep[][];
  done: number;
  total: number;
}

/** The most steps a wave shows before "+N". */
export const STEPS_PER_WAVE = 3;

function markCurrent(waves: GraphStep[][]): GraphStep[][] {
  const flat = waves.flat();
  const at = flat.find((s) => s.tone === 'waiting') ?? flat.find((s) => s.tone === 'working') ?? null;
  if (at) at.current = true;
  return waves;
}

/** A proposed tree as a graph: its items by wave. Null with no items. */
export function graphFromTree(h: ControlHandoff): GraphCard | null {
  const items = h.items ?? [];
  if (items.length === 0) return null;
  const waves = treeWaves(items).map((w) =>
    w.map((i): GraphStep => {
      const proposed = i.proposal_state === 'proposed';
      const tone: Tone = proposed ? 'idle' : workState(i.status);
      return { id: i.id, title: i.title, tone, sub: proposed ? 'proposed' : i.status.replace(/_/g, ' '), current: false };
    }),
  );
  const done = items.filter((i) => workState(i.status) === 'done').length;
  return { title: h.item?.title ?? 'Proposed tasks', waves: markCurrent(waves), done, total: items.length };
}

/** A mission's graph: its nodes by wave, titled by their items. Null when
 *  the hub sends no graph (an older hub) or the mission has no steps yet. */
export function graphFromMission(d: MissionDetail): GraphCard | null {
  const nodes = d.graph?.nodes ?? [];
  if (nodes.length === 0) return null;
  const titles = new Map((d.items ?? []).map((i) => [i.id, i.title]));
  const byWave = new Map<number, GraphStep[]>();
  for (const n of nodes) {
    if (n.state === 'rejected') continue;
    const w = Math.max(1, n.wave || 1);
    const step: GraphStep = { id: n.item_id, title: titles.get(n.item_id) ?? `Task ${n.item_id}`, tone: toneOf(n.state), sub: nodeLabel(n.state), current: false };
    (byWave.get(w) ?? byWave.set(w, []).get(w)!).push(step);
  }
  const waves = [...byWave.entries()].sort((a, b) => a[0] - b[0]).map(([, s]) => s);
  const steps = waves.flat();
  return {
    title: d.mission.name,
    waves: markCurrent(waves),
    done: steps.filter((s) => s.tone === 'done').length,
    total: steps.length,
  };
}

/** "3 of 5 done". */
export function graphProgress(g: GraphCard): string {
  return `${g.done} of ${g.total} done`;
}
