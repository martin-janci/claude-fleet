// Missions (orchestration O1, design 2026-10-07 §9): a goal with a root
// task, member tasks, the repos it may run in and its decision log. Thin
// wrappers over the `*_mission` commands; the rules (who may read or change
// one, the lifecycle, the item cap) live in fleet-core's
// `service::work::missions`, served by the hub on a paired desktop.

import { invokeCmd, type Result } from './result';
import type { WorkItemRow } from './trackers';
import { bumpWorkChanged } from './work';

/** One mission (`store::MissionRow`). */
export interface Mission {
  id: number;
  org_id?: number | null;
  owner_person_id?: number | null;
  root_item_id?: number | null;
  name: string;
  goal: string;
  non_goals?: string | null;
  done_when?: string[];
  /** `finite` | `continuous` — tolerant of more. */
  mode: string;
  /** `draft` | `active` | `paused` | `completed` | `failed` | `cancelled`. */
  state: string;
  /** The autonomy asked for, 0–3. */
  level: number;
  plan_version: number;
  created_at: number;
  updated_at: number;
  started_at?: number | null;
  finished_at?: number | null;
  version: number;
  total?: number;
  done?: number;
  repos?: MissionRepo[];
}

export interface MissionRepo {
  project_id: number;
  name: string;
  role?: string | null;
  created_at: number;
}

export interface MissionEvent {
  id: number;
  at: number;
  kind: string;
  actor: string;
  work_item_id?: number | null;
  task_id?: number | null;
  decision_id?: string | null;
  payload?: unknown;
}

/** `work_mission`'s answer. */
export interface MissionDetail {
  mission: Mission;
  items?: WorkItemRow[];
  events?: MissionEvent[];
  /** `running` | `waiting`, for an active mission only. */
  phase?: string | null;
  may_change?: boolean;
}

/** What `save_mission` writes; on a change every field is optional. */
export interface MissionInput {
  name?: string;
  goal?: string;
  non_goals?: string;
  done_when?: string[];
  mode?: string;
  level?: number;
  org_id?: number;
}

/** The lifecycle moves a person may make from each state, in button order. */
export const MISSION_MOVES: Record<string, readonly string[]> = {
  draft: ['active', 'cancelled'],
  active: ['paused', 'completed', 'failed', 'cancelled'],
  paused: ['active', 'completed', 'failed', 'cancelled'],
};

const MOVE_LABEL: Record<string, string> = {
  active: 'Start',
  paused: 'Pause',
  completed: 'Complete',
  failed: 'Mark failed',
  cancelled: 'Cancel',
};

/** The button label for a move to `state`. */
export function moveLabel(from: string, to: string): string {
  if (from === 'paused' && to === 'active') return 'Resume';
  return MOVE_LABEL[to] ?? to;
}

/** A mission that no longer changes. */
export function isFinal(state: string): boolean {
  return state === 'completed' || state === 'failed' || state === 'cancelled';
}

/** The state as a person reads it, with the loop's phase when it runs. */
export function stateLabel(state: string, phase?: string | null): string {
  const base = state.charAt(0).toUpperCase() + state.slice(1);
  return phase ? `${base} · ${phase}` : base;
}

/** `3/7 done`, or nothing for an empty mission. */
export function progressLabel(m: Pick<Mission, 'total' | 'done'>): string {
  const total = m.total ?? 0;
  return total > 0 ? `${m.done ?? 0}/${total} done` : '';
}

/** The non-empty lines of a textarea, as `done_when` rows. */
export function doneWhenRows(text: string): string[] {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

/** One log row as a sentence. */
export function eventSentence(e: MissionEvent): string {
  const p = (e.payload ?? {}) as Record<string, unknown>;
  switch (e.kind) {
    case 'created':
      return 'Created';
    case 'state':
      return `${String(p.from ?? '?')} → ${String(p.to ?? '?')}`;
    case 'updated': {
      const fields = Array.isArray(p.fields) ? (p.fields as unknown[]).map(String) : [];
      return fields.length ? `Changed ${fields.join(', ')}` : 'Saved, nothing changed';
    }
    case 'repo_added':
      return `Repo ${String(p.project_id ?? '')} allowed${p.role ? ` (${String(p.role)})` : ''}`;
    case 'repo_removed':
      return `Repo ${String(p.project_id ?? '')} removed`;
    case 'item_added':
      return `Task ${e.work_item_id ?? ''} added`;
    case 'item_removed':
      return `Task ${e.work_item_id ?? ''} removed`;
    case 'digest':
      return 'Older events, summarised';
    default:
      return e.kind.replace(/_/g, ' ');
  }
}

export function listMissions(): Promise<Result<Mission[]>> {
  return invokeCmd<Mission[]>('work_missions', { args: {} });
}

export function getMission(missionId: number, beforeEvent?: number): Promise<Result<MissionDetail>> {
  return invokeCmd<MissionDetail>('work_mission', {
    args: { mission_id: missionId, ...(beforeEvent != null ? { before_event: beforeEvent } : {}) },
  });
}

async function changed<T>(p: Promise<Result<T>>): Promise<Result<T>> {
  const r = await p;
  if (r.ok) bumpWorkChanged();
  return r;
}

/** A new draft (rooted at `itemId`, else at a new task of its name). */
export function createMission(input: MissionInput, itemId?: number): Promise<Result<Mission>> {
  return changed(
    invokeCmd<Mission>('save_mission', {
      args: { mission: input, ...(itemId != null ? { item_id: itemId } : {}) },
    }),
  );
}

export function updateMission(
  missionId: number,
  input: MissionInput,
  expectedVersion?: number,
): Promise<Result<Mission>> {
  return invokeCmd<Mission>('save_mission', {
    args: {
      mission_id: missionId,
      mission: input,
      ...(expectedVersion != null ? { expected_version: expectedVersion } : {}),
    },
  });
}

export function setMissionState(
  missionId: number,
  state: string,
  expectedVersion?: number,
): Promise<Result<Mission>> {
  return invokeCmd<Mission>('set_mission_state', {
    args: {
      mission_id: missionId,
      state,
      ...(expectedVersion != null ? { expected_version: expectedVersion } : {}),
    },
  });
}

export function setMissionRepo(
  missionId: number,
  projectId: number,
  on: boolean,
  role?: string,
): Promise<Result<Mission>> {
  const r = role?.trim();
  return invokeCmd<Mission>('set_mission_repo', {
    args: { mission_id: missionId, project_id: projectId, on, ...(r ? { role: r } : {}) },
  });
}

export function setMissionItem(missionId: number, itemId: number, on: boolean): Promise<Result<Mission>> {
  return changed(
    invokeCmd<Mission>('set_mission_item', { args: { mission_id: missionId, item_id: itemId, on } }),
  );
}

export function deleteMission(missionId: number): Promise<Result<{ removed: number }>> {
  return changed(invokeCmd<{ removed: number }>('delete_mission', { args: { mission_id: missionId } }));
}
