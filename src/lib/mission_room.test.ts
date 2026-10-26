// Work → Missions as a Thread room (2026-10-15): the list's groups and
// rows, the room's timeline helpers, the panel's width and tabs, and the
// composer's route to Control.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  RIGHT_MIN,
} from './control_room';
import {
  defaultPanelTab,
  dragMissionRight,
  missionPanelOpen,
  missionPanelTab,
  missionRightPx,
  missionRowLine,
  missionSessionId,
  missionTone,
  openMissionRoom,
  openMissionSession,
  proposalSummary,
  resetMissionRoomForTests,
  roomCreating,
  roomEvents,
  roomGroupOf,
  roomGroups,
  roomMissionId,
  rowProgress,
  sendSteer,
  startNewMission,
  statsLine,
  steerBlocked,
  steerBusy,
  steerPrefix,
  steerReceipts,
} from './mission_room';
import { destination } from './destination';
import { outbox } from './outbox';
import { readPref } from './prefs';
import { session } from './hosts_fixture';
import type { Mission, MissionDetail } from './missions';

const NOW = 1_800_000_000;
const DAY = 86_400;

function mission(over: Partial<Mission> = {}): Mission {
  return {
    id: 1,
    name: 'Hub federation v2',
    goal: 'Peers',
    mode: 'finite',
    state: 'active',
    level: 1,
    plan_version: 1,
    created_at: 1,
    updated_at: NOW - 60,
    version: 1,
    total: 7,
    done: 4,
    ...over,
  };
}

const plan = (cards: unknown[] = [], steps: unknown[] = []) =>
  ({
    steps,
    cards,
    autonomy: { asked: 1, ceiling: 1, effective: 1, why: '', enabled: true },
    cost_micros: 0,
    counts: { total: 0, open: 0 },
  }) as MissionDetail['plan'];

const card = { id: 1, mission_id: 1, decision_id: 'd', source: 'planner', kind: 'confirm', state: 'open', created_at: 1 };

beforeEach(() => {
  resetMissionRoomForTests();
  outbox.resetForTests();
  destination.set('session');
});

describe('the list groups Needs you / Working / Draft and paused / Done this week / Earlier', () => {
  it('puts a mission that waits on a person in Needs you, whatever its state', () => {
    expect(roomGroupOf(mission({ waiting_on: { reason: 'question', since: 1, open_cards: 1 } }), null, NOW)).toBe('needs');
    const paused = mission({ state: 'paused' });
    expect(roomGroupOf(paused, { mission: paused, plan: plan([card]) }, NOW)).toBe('needs');
    expect(roomGroupOf(paused, { mission: paused, plan: plan([], [{ kind: 'ask', reason: 'which?', auto: false }]) }, NOW)).toBe('needs');
    expect(roomGroupOf(paused, null, NOW)).toBe('paused');
    expect(roomGroupOf(mission({ state: 'draft' }), null, NOW)).toBe('paused');
    expect(roomGroupOf(mission(), null, NOW)).toBe('running');
  });

  it('an ended mission is Done this week for seven days after it finished, then Earlier', () => {
    expect(roomGroupOf(mission({ state: 'completed', finished_at: NOW - 6 * DAY }), null, NOW)).toBe('done');
    expect(roomGroupOf(mission({ state: 'failed', finished_at: NOW - 8 * DAY }), null, NOW)).toBe('earlier');
    // No finish time: its last change stands in.
    expect(roomGroupOf(mission({ state: 'cancelled', updated_at: NOW - DAY }), null, NOW)).toBe('done');
    // An ended mission never needs you, even with a stale wait on it.
    expect(roomGroupOf(mission({ state: 'completed', finished_at: NOW, waiting_on: { reason: 'confirm', since: 1, open_cards: 1 } }), null, NOW)).toBe('done');
  });

  it('orders the groups and their rows newest first, and leaves empty groups out', () => {
    const a = mission({ id: 1, name: 'A', updated_at: 10 });
    const b = mission({ id: 2, name: 'B', updated_at: 30 });
    const c = mission({ id: 3, name: 'C', state: 'draft', updated_at: 20 });
    const d = mission({ id: 4, name: 'D', state: 'completed', finished_at: NOW - DAY });
    const e = mission({ id: 5, name: 'E', state: 'paused', waiting_on: { reason: 'sign_grant', since: 1, open_cards: 0 } });
    const groups = roomGroups([a, b, c, d, e], new Map(), NOW);
    expect(groups.map((g) => [g.label, g.missions.map((m) => m.name)])).toEqual([
      ['Needs you', ['E']],
      ['Working', ['B', 'A']],
      ['Draft and paused', ['C']],
      ['Done this week', ['D']],
    ]);
  });

  it('words a row: its reason, else why it waits, else what it is', () => {
    expect(missionRowLine(mission({ waiting_on: { reason: 'sign_grant', since: 1, open_cards: 0 } }))).toBe('Grant needs signing');
    expect(missionRowLine(mission({ state: 'draft', total: 1 }))).toBe('Draft · not planned yet');
    expect(missionRowLine(mission({ state: 'draft', total: 4 }))).toBe('Draft · 4 tasks');
    expect(missionRowLine(mission({ state: 'completed', total: 6, cost_micros: 3_200_000 }))).toBe('Finished · 6 tasks · $3.20');
    expect(missionRowLine(mission({ state: 'failed' }))).toBe('Failed');
    const m = mission();
    const detail: MissionDetail = { mission: m, phase: 'running', graph: { nodes: [{ item_id: 1, state: 'running', wave: 2 }] }, plan: plan() };
    expect(missionRowLine(m, detail)).toBe('Working · wave 2');
    expect(missionRowLine(mission({ state: 'paused' }))).toBe('Paused');
  });

  it("a row's dot and meter", () => {
    expect(missionTone(mission())).toBe('working');
    expect(missionTone(mission({ state: 'paused' }))).toBe('idle');
    expect(missionTone(mission({ waiting_on: { reason: 'question', since: 1, open_cards: 1 } }))).toBe('waiting');
    expect(missionTone(mission({ state: 'failed' }))).toBe('failed');
    expect(rowProgress({ done: 4, total: 7 })).toEqual({ value: 4 / 7, text: '4/7', label: '4 of 7 tasks done' });
    expect(rowProgress({ done: 0, total: 0 })).toBeNull();
  });
});

describe('the room', () => {
  it('reads the log oldest first and sums up the proposals by wave', () => {
    const m = mission();
    const d: MissionDetail = {
      mission: m,
      events: [
        { id: 3, at: 30, kind: 'note', actor: 'loop' },
        { id: 1, at: 10, kind: 'created', actor: 'person:1' },
        { id: 2, at: 10, kind: 'planned', actor: 'loop' },
      ],
      items: [
        { id: 11, source: 'local', title: 'CSV writer', status_category: 'todo', created_at: 1, updated_at: 1, proposal_state: 'proposed' },
        { id: 10, source: 'local', title: 'Ledger query', status_category: 'todo', created_at: 1, updated_at: 1, proposal_state: 'proposed', key: 'PD-1' },
        { id: 12, source: 'local', title: 'Root', status_category: 'todo', created_at: 1, updated_at: 1 },
      ] as MissionDetail['items'],
      graph: {
        nodes: [
          { item_id: 11, state: 'proposed', wave: 2 },
          { item_id: 10, state: 'proposed', wave: 1 },
        ],
      },
    };
    expect(roomEvents(d).map((e) => e.id)).toEqual([1, 2, 3]);
    expect(proposalSummary(d)).toEqual({ ids: [10, 11], waves: 2, titles: ['PD-1 · Ledger query', 'CSV writer'] });
  });

  it('opens on Session while a running mission has live sessions, else on Plan', () => {
    const live = [{ session_id: 7, item_id: 1, host_alias: 'mac', tmux_name: 'w', kind: 'claude' }];
    expect(defaultPanelTab({ mission: mission(), sessions: live })).toBe('session');
    expect(defaultPanelTab({ mission: mission(), sessions: [] })).toBe('plan');
    expect(defaultPanelTab({ mission: mission({ state: 'completed' }), sessions: live })).toBe('plan');
  });

  it('a row opens the room, + New its form, a session the Session tab', () => {
    openMissionRoom(4);
    expect(get(destination)).toBe('missions');
    expect(get(roomMissionId)).toBe(4);
    startNewMission();
    expect(get(roomCreating)).toBe(true);
    expect(get(roomMissionId)).toBe(4);
    openMissionRoom(5);
    expect(get(roomCreating)).toBe(false);
    missionPanelOpen.set(false);
    openMissionSession(7);
    expect(get(missionSessionId)).toBe(7);
    expect(get(missionPanelTab)).toBe('session');
    expect(get(missionPanelOpen)).toBe(true);
  });

  it("resizes like Control's panel, under its own remembered key, and hides on a drag to zero", () => {
    vi.useFakeTimers();
    missionRightPx.set(380);
    let raw = dragMissionRight(380, -120);
    expect(get(missionRightPx)).toBe(500);
    vi.advanceTimersByTime(300);
    vi.useRealTimers();
    expect(readPref('layout.missions-right', 0, (v): v is number => typeof v === 'number')).toBe(500);
    raw = dragMissionRight(raw, 1000);
    expect(raw).toBe(RIGHT_MIN);
    expect(get(missionPanelOpen)).toBe(false);
    expect(get(missionRightPx)).toBe(500);
  });
});

describe('Steer: the composer routes to Control', () => {
  it('prefixes the text with the mission and sends it to the operator through the outbox', async () => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(null);
    const op = session('local', 'fleet-operator', { id: 99 });
    expect(steerPrefix({ id: 4, name: 'Receipts export' })).toBe('About mission #4 "Receipts export":');
    const r = sendSteer({ id: 4, name: 'Receipts export' }, 'Retry step 2 on htz', op);
    expect(r.missionId).toBe(4);
    expect(get(steerReceipts)).toEqual([r]);
    const msgs = get(outbox.store).msgs[99] ?? [];
    expect(msgs).toHaveLength(1);
    expect(msgs[0].text).toBe('Retry step 2 on htz');
    expect(msgs[0].prefix).toBe('About mission #4 "Receipts export":');
    for (let i = 0; i < 10; i++) await tick();
    const sent = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'send_prompt');
    expect(sent?.[1]).toEqual({
      args: {
        host_alias: 'local',
        tmux_name: 'fleet-operator',
        prompt: 'About mission #4 "Receipts export":\n\nRetry step 2 on htz',
        keys: null,
      },
    });
    // Control follows the mission from now on (w04-11).
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('control_journal', {
      args: { action: 'follow', thread: 'mission', id: 4, text: 'Retry step 2 on htz' },
    });
  });

  it('says why it cannot send with no operator, or a lost one', () => {
    expect(steerBlocked(null)).toContain("Control's agent isn't running");
    expect(steerBlocked(session('local', 'op', { id: 1, lost_at: 5 }))).toContain('was lost');
    expect(steerBlocked(session('local', 'op', { id: 1 }))).toBeNull();
  });

  it("waits while Control is mid-turn, and behind lines queued in Control's chat (w04-12)", () => {
    const idle = session('local', 'op', { id: 1, claude_status: 'idle' });
    expect(steerBusy(idle, false, 0)).toBeNull();
    expect(steerBusy({ ...idle, claude_status: 'working' }, false, 0)).toContain('mid-turn');
    expect(steerBusy(idle, true, 0)).toContain('mid-turn');
    expect(steerBusy(idle, false, 2)).toContain('2 lines queued');
  });
});

describe('the metrics line (lanes design L0)', () => {
  const base = {
    items: 3,
    items_done: 2,
    runs_by_role: { implement: 4, review: 2, test: 1 },
    session_runs: 6,
    sessions: 6,
    checkouts: 5,
    failed_runs: 1,
    retries: 2,
    runs_per_done_item_x100: 350,
    item_secs_p50: 2700,
    span_secs: 3 * 3600,
    idle_secs: 3600,
  };

  it('counts runs per done task, retries, cold starts, a task\'s time and the idle share', () => {
    expect(statsLine(base)).toBe(
      '7 runs (3.5 per done task) · 2 retries · 6 cold starts · a task takes 45 min · idle 33% of 3h 0m',
    );
  });

  it('says nothing before the first run, or from an older hub', () => {
    expect(statsLine(undefined)).toBeNull();
    expect(statsLine({ ...base, span_secs: 0, idle_secs: 0 })).toBeNull();
  });

  it('leaves out what has not happened yet', () => {
    expect(
      statsLine({
        ...base,
        items_done: 0,
        runs_per_done_item_x100: null,
        retries: 0,
        item_secs_p50: null,
        runs_by_role: { implement: 1 },
        sessions: 1,
      }),
    ).toBe('1 run · 1 cold start · idle 33% of 3h 0m');
  });
});
