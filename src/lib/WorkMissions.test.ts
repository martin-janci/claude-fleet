// The Work view's Missions tab (orchestration O1): the list, a new mission,
// its detail with the lifecycle moves the state allows, a new task under its
// root, and a refusal shown as text.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkMissions from './WorkMissions.svelte';
import { hosts } from './hosts';
import {
  doneWhenRows,
  eventSentence,
  moveLabel,
  autonomyWords,
  plannerError,
  plannerRefusal,
  policyWith,
  progressLabel,
  stateLabel,
  withoutConfigKeys,
  type Mission,
} from './missions';

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
      if (v instanceof Error) throw { code: (v as Error & { code?: string }).code ?? 'E_INVALID', message: v.message };
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

  it('lists the tasks by wave with what each waits for', async () => {
    handlers.work_mission = () => ({
      mission: current,
      items: [item(10, 'Payments v2'), item(11, 'Schema'), item(12, 'API')],
      graph: {
        nodes: [
          { item_id: 10, state: 'ready', wave: 1 },
          { item_id: 11, state: 'ready', wave: 1 },
          { item_id: 12, state: 'waiting', wave: 2, depends_on: [11], waiting_for: [11] },
        ],
        waves: 2,
      },
      events: [],
      may_change: true,
    });
    handlers.set_work_dep = () => ({ item_id: 12, changed: true });
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getAllByTestId('mission-wave')).toHaveLength(2);
    expect(screen.getByTestId('mission-waits').textContent).toContain('waits for Schema');
    await fireEvent.click(screen.getByTestId('mission-dep-remove'));
    await flush();
    expect(calls('set_work_dep')[0]).toEqual({ item_id: 12, depends_on: 11, on: false });
  });

  it('accepts every proposal at once and can undo it', async () => {
    handlers.work_mission = () => ({
      mission: current,
      items: [item(10, 'Payments v2'), { ...item(11, 'Plan a'), origin: 'proposed', proposal_state: 'proposed' }],
      graph: { nodes: [{ item_id: 10, state: 'ready', wave: 1 }, { item_id: 11, state: 'proposed', wave: 1 }], waves: 1 },
      events: [],
      may_change: true,
    });
    handlers.accept_work_proposals = () => [];
    handlers.undo_work_accept = () => [];
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    await fireEvent.click(screen.getByTestId('mission-accept-all'));
    await flush();
    expect(calls('accept_work_proposals')[0]).toEqual({ item_ids: [11] });
    await fireEvent.click(screen.getByTestId('mission-undo-accept'));
    await flush();
    expect(calls('undo_work_accept')[0]).toEqual({ item_ids: [11] });
    expect(screen.queryByTestId('mission-undo-accept')).toBeNull();
  });

  it('shows what an attempt left and checks a condition', async () => {
    handlers.work_mission = () => ({
      mission: current,
      items: [item(10, 'Payments v2'), { ...item(11, 'Schema'), done_when: ['review', 'person'] }],
      graph: {
        nodes: [
          { item_id: 10, state: 'ready', wave: 1 },
          {
            item_id: 11,
            state: 'done',
            wave: 1,
            attempt: {
              task_id: 5,
              role: 'implement',
              attempt: 2,
              state: 'done',
              outcome: 'done',
              evidence: { at: 1, commits_total: 3, files_total: 1 },
            },
            verification: {
              state: 'unverified',
              checks: [
                { line: 'review', kind: 'review', state: 'pass', detail: 'the reviewer approved' },
                { line: 'person', kind: 'person', state: 'pending', detail: 'waits for a person to check it' },
              ],
            },
          },
        ],
        waves: 1,
      },
      events: [],
      may_change: true,
    });
    handlers.verify_work_item = () => ({ item_id: 11, changed: true });
    handlers.set_work_done_when = () => ({ item_id: 11, changed: true });
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-attempt').textContent).toBe('implement #2 · done · reported done · 3 commits, 1 file');
    expect(screen.getByTestId('mission-verified').textContent).toBe('Unverified');
    // Only the open line offers a check.
    expect(screen.getAllByTestId('mission-check')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('mission-check'));
    await flush();
    expect(calls('verify_work_item')[0]).toEqual({ item_id: 11, line: 'person', ok: true });
    await fireEvent.click(screen.getAllByTestId('mission-conds')[1]);
    expect((screen.getByTestId('mission-conds-text') as HTMLTextAreaElement).value).toBe('review\nperson');
    await fireEvent.input(screen.getByTestId('mission-conds-text'), { target: { value: 'review\n ci:test \n' } });
    await fireEvent.click(screen.getByTestId('mission-conds-save'));
    await flush();
    expect(calls('set_work_done_when')[0]).toEqual({ item_id: 11, done_when: ['review', 'ci:test'] });
  });

  describe('the planner in words (step 1.3)', () => {
    const loopDetail = () => ({
      mission: current,
      items: [item(10, 'Payments v2')],
      events: [],
      may_change: true,
      plan: {
        steps: [],
        cards: [],
        autonomy: { asked: 3, ceiling: 1, effective: 1, why: "L1, the fleet's ceiling (orchestrator.max_level)", enabled: true },
        cost_micros: 0,
        counts: { total: 1, open: 0 },
      },
    });
    const limit = () =>
      Object.assign(new Error('the planner ran 4 times in the last hour (policy.max_planner_runs_per_hour)'), { code: 'E_LIMIT' });

    it('a failed run says what happened, with Retry and Details, and no settings key', async () => {
      current = mission({ state: 'active', level: 3 });
      handlers.work_mission = loopDetail;
      handlers.plan_mission = limit;
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      expect(screen.getByTestId('mission-autonomy').textContent).not.toContain('orchestrator.');
      await fireEvent.click(screen.getByTestId('mission-plan'));
      await flush();
      const box = screen.getByTestId('mission-planner-error');
      expect(box.textContent?.replace(/\s+/g, ' ').trim()).toMatchInlineSnapshot(
        `"The planner couldn't run It already ran 4 times in the last hour, the most this mission allows. Try again later. Retry Details ✕"`,
      );
      expect(box.textContent).not.toMatch(/policy\.|orchestrator\./);
      expect(screen.queryByTestId('mission-notice')).toBeNull();
      // Details keeps the raw code and message.
      expect(screen.queryByTestId('mission-planner-details-text')).toBeNull();
      await fireEvent.click(screen.getByTestId('mission-planner-details'));
      expect(screen.getByTestId('mission-planner-details-text').textContent).toBe(
        'E_LIMIT · the planner ran 4 times in the last hour (policy.max_planner_runs_per_hour)',
      );
      // Retry asks again; a good answer clears the error.
      handlers.plan_mission = () => ({ mission_id: 4, cards: [] });
      await fireEvent.click(screen.getByTestId('mission-planner-retry'));
      await flush();
      expect(calls('plan_mission')).toHaveLength(2);
      expect(screen.queryByTestId('mission-planner-error')).toBeNull();
    });

    it('a refused answer says nothing changed and keeps the reason under Details', async () => {
      current = mission({ state: 'active', level: 3 });
      handlers.work_mission = loopDetail;
      handlers.plan_mission = () => ({ mission_id: 4, cards: [], refused: 'no JSON array of commands: "I think we should"' });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      await fireEvent.click(screen.getByTestId('mission-plan'));
      await flush();
      const box = screen.getByTestId('mission-planner-error');
      expect(box.textContent).toContain("The planner's answer couldn't be used");
      expect(box.textContent).toContain('Nothing was changed');
      expect(box.textContent).not.toContain('no JSON array');
      await fireEvent.click(screen.getByTestId('mission-planner-details'));
      expect(screen.getByTestId('mission-planner-details-text').textContent).toContain('no JSON array');
      await fireEvent.click(screen.getByTestId('mission-planner-dismiss'));
      expect(screen.queryByTestId('mission-planner-error')).toBeNull();
    });

    it('maps each planner failure to words', () => {
      expect(plannerError({ code: 'E_INVALID_STATE', message: "claude is not on mac's PATH" }).text).toBe(
        "Claude Code isn't installed on mac, so the planner has nowhere to run.",
      );
      expect(plannerError({ code: 'E_SHELL', message: 'the planner on mac gave no answer' }).text).toContain('on mac finished without an answer');
      expect(plannerError({ code: 'E_SSH_TIMEOUT', message: 'ssh mac: timed out after 10 s' }).text).toContain("couldn't reach");
      expect(plannerError({ code: 'E_HUB_UNREACHABLE', message: 'hub down' }).text).toContain("hub didn't answer");
      expect(plannerError({ code: 'E_INVALID_STATE', message: 'Payments is completed' }).text).toContain('has ended');
      expect(plannerError({ code: 'E_X', message: 'the mission loop is off (orchestrator.enabled)' }).text).toBe('The mission loop is off.');
      expect(plannerError({ code: 'E_X', message: 'boom' }).details).toBe('E_X · boom');
      expect(plannerRefusal('bad').details).toBe('bad');
    });

    it('takes settings keys out of user text', () => {
      expect(withoutConfigKeys("L1, the fleet's ceiling (orchestrator.max_level)")).toBe("L1, the fleet's ceiling");
      expect(withoutConfigKeys('ran 4 times (policy.max_planner_runs_per_hour) today')).toBe('ran 4 times today');
      expect(withoutConfigKeys('see (this) and (e.g. that)')).toBe('see (this) and (e.g. that)');
    });
  });

  describe('the header, the grant and the policy (step 1.8)', () => {
    const policy = {
      max_parallel: 2,
      max_retries: 3,
      require_review: true,
      task_creation: 'propose',
      max_tasks: 60,
      max_planner_runs_per_hour: 6,
      no_progress_secs: 3600,
      planner_host: 'mac',
    };
    const detailOf = (m: Mission) => ({
      mission: m,
      items: [item(10, 'Payments v2')],
      events: [],
      may_change: true,
      plan: {
        steps: [],
        cards: [],
        autonomy: { asked: 3, ceiling: 1, effective: 1, why: "L1, the fleet's ceiling (orchestrator.max_level)", enabled: true },
        cost_micros: 0,
        counts: { total: 1, open: 0 },
      },
    });

    it('mission_save round-trip: parallel runs and the wake interval save, the rest of the policy is kept', async () => {
      current = mission({ state: 'active', level: 3, mode: 'continuous', policy });
      handlers.work_mission = () => detailOf(current);
      handlers.save_mission = (a) => {
        const m = (a.mission ?? {}) as Partial<Mission>;
        current = mission({ ...current, ...m, version: 2 });
        return current;
      };
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      expect(screen.getByTestId('mission-detail').textContent).toContain('2 at once');
      await fireEvent.click(screen.getByTestId('mission-edit'));
      await flush();
      expect((screen.getByTestId('mission-edit-parallel') as HTMLInputElement).value).toBe('2');
      expect((screen.getByTestId('mission-edit-wake') as HTMLInputElement).value).toBe('');
      // Under five minutes is refused before it is sent.
      await fireEvent.input(screen.getByTestId('mission-edit-wake'), { target: { value: '2' } });
      await flush();
      expect(screen.getByTestId('mission-edit-wake-bad')).toBeTruthy();
      expect((screen.getByTestId('mission-edit-save') as HTMLButtonElement).disabled).toBe(true);
      await fireEvent.input(screen.getByTestId('mission-edit-wake'), { target: { value: '30' } });
      await fireEvent.input(screen.getByTestId('mission-edit-parallel'), { target: { value: '4' } });
      await flush();
      await fireEvent.click(screen.getByTestId('mission-edit-save'));
      await flush();
      const sent = calls('save_mission')[0];
      expect(sent.expected_version).toBe(1);
      expect((sent.mission as { policy: unknown }).policy).toEqual({ ...policy, max_parallel: 4, wake_every_secs: 1800 });
      // What came back is what the header shows, and a second edit starts from it.
      const meta = screen.getByTestId('mission-detail').textContent ?? '';
      expect(meta).toContain('wakes every 30 min');
      expect(meta).toContain('4 at once');
      await fireEvent.click(screen.getByTestId('mission-edit'));
      await flush();
      expect((screen.getByTestId('mission-edit-parallel') as HTMLInputElement).value).toBe('4');
      expect((screen.getByTestId('mission-edit-wake') as HTMLInputElement).value).toBe('30');
    });

    it('says the autonomy in words, with how to change it', async () => {
      current = mission({ state: 'active', level: 3 });
      handlers.work_mission = () => detailOf(current);
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      const line = screen.getByTestId('mission-autonomy').textContent ?? '';
      expect(line).toContain('Runs at L1 · L3 asked · L1 ceiling · no grant');
      expect(line).not.toMatch(/orchestrator\.|policy\./);
      expect(screen.getByTestId('mission-autonomy-hint').textContent).toBe("The fleet's ceiling holds it at L1. Raise it in Settings.");
    });

    it('the grant form signs for the hosts ticked, and for any host when none is', async () => {
      hosts.set([
        { alias: 'mac', ssh_alias: null, hidden: false, account_uuid: null, reachable: true, claude_version: null, tmux_version: null, probed_at: null },
        { alias: 'trn', ssh_alias: null, hidden: false, account_uuid: null, reachable: true, claude_version: null, tmux_version: null, probed_at: null },
      ] as never);
      current = mission({ state: 'active', level: 2 });
      handlers.work_mission = () => detailOf(current);
      handlers.grant_mission = () => ({ id: 1, mission_id: 4, plan_version: 1, level: 2, granted_by: 'fleet', created_at: 1, expires_at: 2 });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      await fireEvent.click(screen.getByTestId('mission-grant'));
      await flush();
      const boxes = screen.getAllByTestId('mission-grant-host') as HTMLInputElement[];
      expect(boxes.map((b) => b.value)).toEqual(['mac', 'trn']);
      await fireEvent.click(boxes[1]);
      await fireEvent.click(screen.getByTestId('mission-grant-save'));
      await flush();
      expect(calls('grant_mission')[0]).toEqual({ mission_id: 4, level: 2, hours: 8, hosts: ['trn'] });
      hosts.set([]);
    });

    it('words every limit', () => {
      const base = { asked: 2, ceiling: 3, effective: 2, why: '', enabled: true };
      expect(autonomyWords(base).hint).toBeNull();
      expect(autonomyWords({ ...base, enabled: false, effective: 0 }).runs).toBe('Loop off');
      expect(autonomyWords({ ...base, enabled: false, effective: 0 }).hint).toContain('off for the whole fleet');
      expect(autonomyWords({ ...base, effective: 1 }).hint).toContain('Without a grant');
      const grant = { id: 1, mission_id: 4, plan_version: 1, level: 1, granted_by: 'p', created_at: 0, expires_at: 2_000 };
      const w = autonomyWords({ ...base, asked: 3, effective: 1, grant }, 1_000);
      expect(w.limits).toMatch(/^L3 asked · L3 ceiling · L1 grant until /);
      expect(w.hint).toContain('The grant signs L1');
      expect(policyWith({ max_parallel: 2, wake_every_secs: 600 }, { wake_every_secs: null })).toEqual({ max_parallel: 2 });
    });
  });

  it('presses the loop: a wave, a card, a grant and Pause all', async () => {
    current = mission({ state: 'active', level: 2 });
    handlers.work_mission = () => ({
      mission: current,
      items: [item(10, 'Payments v2'), item(11, 'Refunds')],
      events: [],
      may_change: true,
      graph: { nodes: [{ item_id: 11, state: 'ready', wave: 1 }], waves: 1 },
      plan: {
        steps: [
          { kind: 'run', item_id: 11, role: 'implement', reason: 'Refunds is ready', auto: true },
          { kind: 'ask', item_id: 12, reason: 'failed twice', auto: false },
        ],
        cards: [
          { id: 7, mission_id: 4, decision_id: 'p', source: 'planner', kind: 'ask', state: 'open', created_at: 1, payload: { question: 'Which gateway?' } },
          { id: 8, mission_id: 4, decision_id: 'q', source: 'planner', kind: 'run', state: 'applied', created_at: 1 },
        ],
        autonomy: { asked: 2, ceiling: 1, effective: 1, why: "L1, the fleet's ceiling", enabled: true },
        cost_micros: 1_250_000,
        counts: { total: 1, open: 0 },
      },
    });
    handlers.start_mission_wave = () => ({ mission_id: 4, results: [] });
    handlers.decide_mission_card = () => ({ id: 7, mission_id: 4, decision_id: 'p', source: 'planner', kind: 'ask', state: 'applied', created_at: 1 });
    handlers.grant_mission = () => ({ id: 1, mission_id: 4, plan_version: 1, level: 2, granted_by: 'fleet', created_at: 1, expires_at: 2 });
    handlers.pause_all_missions = () => [4];
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('missions-pause-all'));
    await flush();
    expect(calls('pause_all_missions')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-autonomy').textContent).toContain('$1.25');
    expect(screen.getAllByTestId('mission-step')).toHaveLength(2);
    // An ask is not pressed; only the run is in the wave.
    expect(screen.getByTestId('mission-start-wave').textContent).toBe('Start wave (1)');
    await fireEvent.click(screen.getByTestId('mission-start-wave'));
    await flush();
    expect(calls('start_mission_wave')[0]).toEqual({ mission_id: 4 });
    // Only the open card waits; a question needs an answer.
    expect(screen.getAllByTestId('mission-card')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('mission-card-apply'));
    await flush();
    expect(calls('decide_mission_card')).toHaveLength(0);
    await fireEvent.input(screen.getByTestId('mission-card-answer'), { target: { value: 'Stripe' } });
    await fireEvent.click(screen.getByTestId('mission-card-apply'));
    await flush();
    expect(calls('decide_mission_card')[0]).toEqual({ card_id: 7, ok: true, note: 'Stripe' });
    await fireEvent.click(screen.getByTestId('mission-grant'));
    await fireEvent.input(screen.getByTestId('mission-grant-budget'), { target: { value: '5' } });
    await fireEvent.click(screen.getByTestId('mission-grant-save'));
    await flush();
    expect(calls('grant_mission')[0]).toEqual({ mission_id: 4, level: 2, hours: 8, budget_cents: 500 });
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
    expect(eventSentence({ id: 4, at: 1, kind: 'dep_added', actor: 'fleet', work_item_id: 3, payload: { depends_on: 2 } })).toBe(
      'Task 3 waits for 2',
    );
  });
});
