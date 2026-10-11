// The Work view's Missions tab (orchestration O1): the list, a new mission,
// its detail with the lifecycle moves the state allows, a new task under its
// root, and a refusal shown as text.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkMissions from './WorkMissions.svelte';
import { expectAccessible } from './a11y_check';
import { expectLastButton, expectOnePrimary } from './action_hierarchy_check';
import { hosts } from './hosts';
import { projects } from './projects';
import {
  filterMissions,
  missionFilterChoices,
  missionFilterCount,
  doneWhenRows,
  finalMoveQuestion,
  splitMoves,
  eventSentence,
  moveLabel,
  autonomyWords,
  plannerError,
  plannerRefusal,
  policyWith,
  progressLabel,
  stateLabel,
  withoutConfigKeys,
  brakeReason,
  brakesLine,
  durationWords,
  loopFooter,
  missionGroups,
  missionReason,
  ownerLine,
  plannerRunsThisHour,
  runStateLabel,
  runsOf,
  type Mission,
  type MissionDetail,
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

// An import switches the view to Graph and the choice is kept (a pref), so
// every test starts on List.
beforeEach(() => localStorage.removeItem('cf:pref:work.missions.view'));

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

  describe('G2.5: mission forms', () => {
    const tree = (id: number, owner: string, repo: string) => ({
      project: { id, owner, repo, base_path: `/p/${owner}/${repo}`, last_session_at: null, adopted: false, system: false },
      worktrees: [],
    });
    afterEach(() => projects.set([]));

    it('the planner drafts the first tasks only when asked', async () => {
      handlers.work_missions = () => [];
      handlers.plan_mission = () => ({ mission_id: 4, cards: [] });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-new'));
      expect((screen.getByTestId('mission-new-planner') as HTMLInputElement).checked).toBe(false);
      await fireEvent.input(screen.getByTestId('mission-new-name'), { target: { value: 'Payments v2' } });
      await fireEvent.input(screen.getByTestId('mission-new-goal'), { target: { value: 'Cards' } });
      await fireEvent.click(screen.getByTestId('mission-new-planner'));
      handlers.work_missions = () => [current];
      await fireEvent.click(screen.getByTestId('mission-create'));
      await flush();
      expect(calls('save_mission')).toHaveLength(1);
      expect(calls('plan_mission')[0]).toEqual({ mission_id: 4 });
    });

    it('a new mission without the box asks no planner', async () => {
      handlers.work_missions = () => [];
      handlers.plan_mission = () => ({ mission_id: 4, cards: [] });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-new'));
      await fireEvent.input(screen.getByTestId('mission-new-name'), { target: { value: 'P' } });
      await fireEvent.input(screen.getByTestId('mission-new-goal'), { target: { value: 'G' } });
      await fireEvent.click(screen.getByTestId('mission-create'));
      await flush();
      expect(calls('plan_mission')).toHaveLength(0);
    });

    it('imports a plan with a repo per row from the create form; a row with no repo must be picked first', async () => {
      projects.set([tree(3, 'acme', 'api'), tree(5, 'acme', 'web')]);
      handlers.work_missions = () => [];
      handlers.import_mission_plan = () => ({ created: 2, updated: 0, unchanged: 0, deps_added: 1, deps_removed: 0 });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-new'));
      await fireEvent.input(screen.getByTestId('mission-new-name'), { target: { value: 'P' } });
      await fireEvent.input(screen.getByTestId('mission-new-goal'), { target: { value: 'G' } });
      await fireEvent.click(screen.getByTestId('mission-new-import-open'));
      await fireEvent.input(screen.getByTestId('mission-new-import-text'), {
        target: { value: '| # | Step | Needs | Repo |\n|---|---|---|---|\n| 1.1 | Schema | | acme/api |\n| 1.2 | UI | 1.1 | mobile |' },
      });
      await flush();
      expect(screen.getByTestId('mission-new-import-repo-missing').textContent).toBe('Row 2 has no repo: pick one');
      // Not ready: the mission is not made while a row has no repo.
      await fireEvent.click(screen.getByTestId('mission-create'));
      await flush();
      expect(calls('save_mission')).toHaveLength(0);
      const pick = screen.getAllByTestId('mission-new-import-repo-pick')[1] as HTMLSelectElement;
      pick.value = '5';
      await fireEvent.change(pick);
      await flush();
      expect(screen.queryByTestId('mission-new-import-repo-missing')).toBeNull();
      handlers.work_missions = () => [current];
      await fireEvent.click(screen.getByTestId('mission-create'));
      await flush();
      await flush();
      expect(calls('save_mission')).toHaveLength(1);
      expect(calls('import_mission_plan')[0]).toEqual({
        mission_id: 4,
        plan: [
          { step: '1.1', title: 'Schema', project_id: 3 },
          { step: '1.2', title: 'UI', needs: ['1.1'], project_id: 5 },
        ],
      });
      expect(screen.getByTestId('mission-import-result').textContent).toBe('Imported: 2 added, 1 link added.');
    });

    it('answers a mission question on its page: an option, your own words, or Skip', async () => {
      const ask = (id: number) => ({
        id,
        mission_id: 4,
        decision_id: `d${id}`,
        source: 'planner',
        kind: 'ask',
        state: 'open',
        created_at: 1,
        payload: { question: 'Which gateway?', options: ['Stripe', 'Adyen'] },
      });
      handlers.work_mission = () => ({
        mission: current,
        events: [],
        may_change: true,
        graph: { nodes: [], waves: 0 },
        plan: {
          steps: [],
          cards: [ask(7)],
          autonomy: { asked: 1, ceiling: 1, effective: 1, why: 'L1', enabled: true },
          cost_micros: 0,
          counts: { total: 0, open: 0 },
        },
      });
      handlers.decide_mission_card = (a) => ({ ...ask(Number(a.card_id)), state: a.ok ? 'applied' : 'dismissed' });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      const card = screen.getByTestId('mission-card-question');
      expect(card.textContent).toContain('Which gateway?');
      const opts = screen.getAllByTestId('mission-card-option').map((b) => b.textContent ?? '');
      expect(opts).toHaveLength(2);
      expect(opts[0]).toContain('Stripe');
      expect(opts[1]).toContain('Adyen');
      await fireEvent.click(screen.getAllByTestId('mission-card-option')[1]);
      await flush();
      expect(calls('decide_mission_card')[0]).toEqual({ card_id: 7, ok: true, note: 'Adyen' });
      // Own words open a field under the answers.
      expect(screen.queryByTestId('mission-card-answer')).toBeNull();
      await fireEvent.click(screen.getByTestId('question-own-words'));
      await fireEvent.input(screen.getByTestId('mission-card-answer'), { target: { value: 'Both' } });
      await fireEvent.click(screen.getByTestId('mission-card-apply'));
      await flush();
      expect(calls('decide_mission_card')[1]).toEqual({ card_id: 7, ok: true, note: 'Both' });
      await fireEvent.click(screen.getByTestId('mission-card-skip'));
      await flush();
      expect(calls('decide_mission_card')[2]).toEqual({ card_id: 7, ok: false });
    });
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

  it('a detail that answers after another mission opened is dropped (review r07)', async () => {
    const a = mission({ id: 4, name: 'Payments v2', goal: 'Cards and refunds' });
    const b = mission({ id: 5, name: 'Search', goal: 'Faster search' });
    handlers.work_missions = () => [a, b];
    let releaseA: (() => void) | null = null;
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const args = (raw as { args: Record<string, unknown> } | undefined)?.args ?? {};
      if (cmd === 'work_mission') {
        const m = args.mission_id === 4 ? a : b;
        const v = { mission: m, items: [item(10, m.name)], events: [], may_change: true };
        if (m === a) return new Promise((res) => (releaseA = () => res(v)));
        return v;
      }
      const h = handlers[cmd];
      return h ? h(args) : null;
    });
    render(WorkMissions);
    await flush();
    const rows = screen.getAllByTestId('mission-row');
    await fireEvent.click(rows[0]);
    await fireEvent.click(rows[1]);
    await flush();
    expect(screen.getByTestId('mission-detail').textContent).toContain('Faster search');
    releaseA!();
    await flush();
    expect(screen.getByTestId('mission-detail').textContent).toContain('Faster search');
    expect(screen.getByTestId('mission-detail').textContent).not.toContain('Cards and refunds');
  });

  describe('the ⋯ menu for the moves that end a mission (parity P19)', () => {
    async function openActive() {
      current = mission({ state: 'active' });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
    }

    it('New: Pause stays a button; Complete, Mark failed and Cancel ask first from ⋯', async () => {
      await openActive();
      expect(screen.getByTestId('mission-move-paused').textContent).toBe('Pause');
      for (const to of ['completed', 'failed', 'cancelled']) expect(screen.queryByTestId(`mission-move-${to}`)).toBeNull();
      await fireEvent.click(screen.getByTestId('mission-more'));
      expect(screen.getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Complete…', 'Mark failed…', 'Cancel…']);
      await fireEvent.click(screen.getByTestId('mission-menu-failed'));
      expect(screen.queryByTestId('mission-more-menu')).toBeNull();
      expect(screen.getByTestId('mission-move-confirm-row').textContent).toContain('Mark Payments v2 failed?');
      expect(calls('set_mission_state')).toEqual([]);
      await fireEvent.click(screen.getByTestId('mission-move-keep'));
      expect(screen.queryByTestId('mission-move-confirm-row')).toBeNull();
      await fireEvent.click(screen.getByTestId('mission-more'));
      await fireEvent.click(screen.getByTestId('mission-menu-completed'));
      await fireEvent.click(screen.getByTestId('mission-move-confirm'));
      await flush();
      expect(calls('set_mission_state')).toEqual([{ mission_id: 4, state: 'completed', expected_version: 1 }]);
      expect(screen.queryByTestId('mission-more')).toBeNull();
    });

    it('New: Esc closes the menu and a draft offers only Cancel in it', async () => {
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      expect(screen.getByTestId('mission-move-active').textContent).toBe('Start');
      await fireEvent.click(screen.getByTestId('mission-more'));
      expect(screen.getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Cancel…']);
      await fireEvent.keyDown(screen.getByTestId('mission-more-menu'), { key: 'Escape' });
      expect(screen.queryByTestId('mission-more-menu')).toBeNull();
    });

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

  // Redesign 9.10: a stuck mission's card. A step goes
  // through the action a person already has; nothing is completed.
  describe('stuck mission triage', () => {
    const stuckCard = {
      stuck: { reason: 'failed', why: '1 task failed', done: 0, failed: 1, blocked: 0, total: 2 },
      next: { feature: 'mission_triage', value: 'retry', source: 'jev', confidence_pct: 80 },
      may_change: true,
    };
    async function openStuck() {
      current = mission({ state: 'active' });
      handlers.mission_triage = () => stuckCard;
      handlers.retry_work_item = () => ({ ok: true, detail: 'queued' });
      handlers.work_mission = () => ({
        mission: current,
        items: [item(10, 'Payments v2'), item(11, 'Refunds')],
        graph: {
          nodes: [
            { item_id: 10, state: 'ready', wave: 1 },
            { item_id: 11, state: 'failed', wave: 1, attempt: { task_id: 5, state: 'failed', error: 'boom' } },
          ],
          waves: 1,
        },
        events: [],
        may_change: true,
      });
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
    }

    it('retries the failed task and gives up only through the confirm', async () => {
      await openStuck();
      expect(screen.getByTestId('mission-triage-why').textContent).toBe('1 task failed');
      await fireEvent.click(screen.getByTestId('mission-triage-step-retry'));
      await flush();
      expect(calls('retry_work_item')[0]).toEqual(expect.objectContaining({ item_id: 11 }));
      await fireEvent.click(screen.getByTestId('mission-triage-step-give_up'));
      await flush();
      expect(calls('set_mission_state')).toHaveLength(0);
    });
  });

  describe('task graph', () => {
    const graphDetail = () => ({
      mission: current,
      items: [item(10, 'Payments v2'), { ...item(11, 'Schema'), status_category: 'done' }, item(12, 'API'), item(13, 'UI')],
      graph: {
        nodes: [
          { item_id: 10, state: 'ready', wave: 1 },
          { item_id: 11, state: 'done', wave: 1 },
          { item_id: 12, state: 'running', wave: 2, depends_on: [11] },
          { item_id: 13, state: 'waiting', wave: 3, depends_on: [12], waiting_for: [12] },
        ],
        waves: 3,
      },
      events: [],
      may_change: true,
    });

    async function open() {
      handlers.work_mission = graphDetail;
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
    }

    it('draws lanes × waves with progress and the critical path in the New layout', async () => {
      await open();
      await fireEvent.click(screen.getByTestId('mission-view-graph'));
      await flush();
      expect(screen.queryAllByTestId('mission-wave')).toHaveLength(0);
      expect(screen.getByTestId('mission-graph-progress').textContent).toBe('1 of 4 done · 25%');
      expect(screen.getAllByTestId('mission-graph-wave')).toHaveLength(3);
      expect(screen.getAllByTestId('mission-graph-node')).toHaveLength(4);
      expect(screen.getByTestId('mission-graph-critical').textContent).toContain('2 tasks');
      const api = screen.getAllByTestId('mission-graph-node').find((n) => n.textContent?.includes('API'))!;
      expect(api.getAttribute('aria-label')).toContain('on the critical path');
      await fireEvent.click(screen.getAllByTestId('mission-graph-node').find((n) => n.textContent?.includes('UI'))!);
      expect(screen.getByTestId('mission-graph-chosen').textContent).toContain('waits for API');
      await fireEvent.click(screen.getByTestId('mission-view-list'));
      await flush();
      expect(screen.getAllByTestId('mission-wave')).toHaveLength(3);
    });
  });

  it('imports a pasted plan, says what it did and names the needs it could not place', async () => {
    current = mission({ mode: 'plan' });
    handlers.import_mission_plan = () => ({
      created: 2,
      updated: 0,
      unchanged: 0,
      deps_added: 1,
      deps_removed: 0,
      unknown_needs: ['1.2 needs 9.9'],
    });
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-plan-mode').textContent).toContain('not run by fleet');
    await fireEvent.click(screen.getByTestId('mission-import-open'));
    await fireEvent.input(screen.getByTestId('mission-import-text'), {
      target: { value: '| # | Step | Needs | Lane |\n|---|---|---|---|\n| 1.1 | Schema | | A |\n| 1.2 | API | 1.1, 9.9 | B |' },
    });
    await flush();
    expect(screen.getByTestId('mission-import-preview').textContent).toContain('2 steps · 2 lanes · 2 links');
    await fireEvent.click(screen.getByTestId('mission-import-run'));
    await flush();
    expect(calls('import_mission_plan')[0]).toEqual({
      mission_id: 4,
      plan: [
        { step: '1.1', title: 'Schema', lane: 'A' },
        { step: '1.2', title: 'API', lane: 'B', needs: ['1.1', '9.9'] },
      ],
    });
    expect(screen.getByTestId('mission-import-result').textContent).toBe('Imported: 2 added, 1 link added.');
    expect(screen.getByTestId('mission-import-unknown').textContent).toContain('1.2 needs 9.9');
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
      const out = plannerError({
        code: 'E_CLAUDE_CLI',
        message: 'Claude login expired on nas: run `claude /login` there, then retry',
      });
      expect(out.title).toBe("The planner's Claude login has expired");
      expect(out.text).toBe("Claude Code on nas is signed out, so the planner can't run. Run claude /login there, then retry.");
      const old = plannerRefusal('not a JSON array: the answer begins "Login expired · Run /login to sign in again"');
      expect(old.title).toBe("The planner's Claude login has expired");
      expect(old.details).toContain('Login expired');
      expect(plannerRefusal('not a JSON array').title).toBe("The planner's answer couldn't be used");
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

  describe('action hierarchy (redesign 1.5)', () => {
    const view = () => document.body;
    it('the list: New mission is the one primary; its form: Create', async () => {
      render(WorkMissions);
      await flush();
      expectOnePrimary(view(), 'mission-new');
      await fireEvent.click(screen.getByTestId('mission-new'));
      await flush();
      expectOnePrimary(view(), 'mission-create');
    });

    it('a mission: Start wave is the one primary, Save while editing; Delete… is last and asks first', async () => {
      current = mission({ state: 'active' });
      handlers.work_mission = () => ({
        mission: current,
        items: [item(10, 'Payments v2'), item(11, 'Refunds')],
        events: [],
        may_change: true,
        graph: { nodes: [{ item_id: 11, state: 'ready', wave: 1 }], waves: 1 },
        plan: {
          steps: [{ kind: 'run', item_id: 11, role: 'implement', reason: 'Refunds is ready', auto: true }],
          cards: [],
          autonomy: { asked: 0, ceiling: 1, effective: 0, why: 'L0', enabled: true },
          cost_micros: 0,
          counts: { total: 1, open: 0 },
        },
      });
      const r = render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      expectOnePrimary(view(), 'mission-start-wave');
      await fireEvent.click(screen.getByTestId('mission-edit'));
      await flush();
      expectOnePrimary(view(), 'mission-edit-save');
      r.unmount();
      // A draft can be deleted: the last button of the detail, behind a confirm.
      current = mission({ state: 'draft' });
      handlers.delete_mission = () => null;
      render(WorkMissions);
      await flush();
      await fireEvent.click(screen.getByTestId('mission-row'));
      await flush();
      const del = screen.getByTestId('mission-delete');
      expectLastButton(screen.getByTestId('mission-detail'), del);
      await fireEvent.click(del);
      await flush();
      expect(screen.getByTestId('mission-delete-confirm')).toBeTruthy();
      expect(calls('delete_mission')).toHaveLength(0);
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

  it('is accessible', async () => {
    current = mission({ state: 'active', total: 3, done: 1 });
    handlers.work_mission = () => ({
      mission: current,
      items: [item(10, 'Payments v2'), { ...item(11, 'Schema'), status_category: 'done' }, item(12, 'API')],
      graph: {
        nodes: [
          { item_id: 10, state: 'ready', wave: 1 },
          { item_id: 11, state: 'done', wave: 1 },
          { item_id: 12, state: 'waiting', wave: 2, depends_on: [11], waiting_for: [11] },
        ],
        waves: 2,
      },
      events: [{ id: 1, at: 1, kind: 'created', actor: 'person:1' }],
      may_change: true,
    });
    const { container } = render(WorkMissions);
    await flush();
    await expectAccessible(container);
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    await fireEvent.click(screen.getByTestId('mission-more'));
    await expectAccessible(container);
    await fireEvent.keyDown(screen.getByTestId('mission-more-menu'), { key: 'Escape' });
    await fireEvent.click(screen.getByTestId('mission-view-graph'));
    await flush();
    await expectAccessible(container);
    // The view choice is remembered: leave it on the list for later tests.
    await fireEvent.click(screen.getByTestId('mission-view-list'));
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

describe('mission filters (G7.7)', () => {
  const m = (over: Partial<Mission>) => mission(over);
  const list = [
    m({ id: 1, org_id: 1, owner_person_id: 7, mode: 'finite', waiting_on: { reason: 'question', since: 1, open_cards: 1 } }),
    m({ id: 2, org_id: null, owner_person_id: 8, mode: 'continuous', repos: [{ project_id: 3, name: 'api', created_at: 1 }] }),
  ];
  it('lets every mission through with no filter, and each filter narrows', () => {
    expect(filterMissions(list, {}).map((x) => x.id)).toEqual([1, 2]);
    expect(filterMissions(list, { needsYou: true }).map((x) => x.id)).toEqual([1]);
    expect(filterMissions(list, { mine: true }, 8).map((x) => x.id)).toEqual([2]);
    expect(filterMissions(list, { mine: true }, null)).toEqual([]);
    expect(filterMissions(list, { orgId: 0 }).map((x) => x.id)).toEqual([2]);
    expect(filterMissions(list, { orgId: 1 }).map((x) => x.id)).toEqual([1]);
    expect(filterMissions(list, { projectId: 3 }).map((x) => x.id)).toEqual([2]);
    expect(filterMissions(list, { mode: 'finite' }).map((x) => x.id)).toEqual([1]);
    expect(missionFilterCount({ needsYou: true, orgId: 0, mode: null })).toBe(2);
  });
  it('offers the organisations, repositories and modes the list names', () => {
    expect(missionFilterChoices(list)).toEqual({ orgIds: [0, 1], repos: [{ project_id: 3, name: 'api' }], modes: ['continuous', 'finite'] });
  });
});

describe('splitMoves and finalMoveQuestion (parity P19)', () => {
  it('keeps Start, Pause and Resume inline and puts the ending moves in ⋯', () => {
    expect(splitMoves('active')).toEqual({ inline: ['paused'], menu: ['completed', 'failed', 'cancelled'] });
    expect(splitMoves('paused')).toEqual({ inline: ['active'], menu: ['completed', 'failed', 'cancelled'] });
    expect(splitMoves('draft')).toEqual({ inline: ['active'], menu: ['cancelled'] });
    expect(splitMoves('completed')).toEqual({ inline: [], menu: [] });
    expect(finalMoveQuestion('X', 'cancelled')).toBe('Cancel X? It stops changing; its tasks stay.');
  });
});

// Redesign step 9.12 (Missions part): Comet trails beside the mission's
// current steps while it runs; none while it waits on a person.
describe('WorkMissions comet trails', () => {
  const detailWith = (plan: unknown, state = 'active') => ({
    mission: mission({ state }),
    items: [item(10, 'Payments v2'), item(11, 'Schema'), item(12, 'API')],
    graph: {
      nodes: [
        { item_id: 11, state: 'running', wave: 1 },
        { item_id: 12, state: 'ready', wave: 1 },
      ],
      waves: 1,
    },
    events: [],
    may_change: true,
    plan,
  });
  const loop = (over: Record<string, unknown> = {}) => ({
    steps: [],
    cards: [],
    autonomy: { level: 1, grant: null },
    cost_micros: 0,
    counts: { total: 2, open: 1 },
    ...over,
  });

  /** Past the loaders' 400 ms delay, so an absent loader is really absent. */
  const pastDelay = () => new Promise((r) => setTimeout(r, 450));

  async function open(detail: unknown) {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_missions') return [(detail as { mission: Mission }).mission];
      if (cmd === 'work_mission') return detail;
      return null;
    });
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    await pastDelay();
  }

  it('draws trails beside the running step only', async () => {
    await open(detailWith(loop()));
    const trails = await vi.waitFor(() => screen.getAllByTestId('mission-trails'));
    expect(trails).toHaveLength(1);
    expect(trails[0].closest('[data-testid="mission-node"]')?.getAttribute('data-state')).toBe('running');
  });

  it('stops while the mission waits on a person', async () => {
    await open(
      detailWith(
        loop({ cards: [{ id: 1, mission_id: 4, decision_id: 'd', source: 'loop', kind: 'confirm', state: 'open', created_at: 1 }] }),
      ),
    );
    expect(screen.queryByTestId('mission-trails')).toBeNull();
  });

  it('stops on an ask step and a paused mission', async () => {
    await open(detailWith(loop({ steps: [{ kind: 'ask', reason: 'Which repo?', auto: false }] })));
    expect(screen.queryByTestId('mission-trails')).toBeNull();
    document.body.innerHTML = '';
    await open(detailWith(loop(), 'paused'));
    expect(screen.queryByTestId('mission-trails')).toBeNull();
  });
});

// Gap G3.6: the Missions board's list and detail: groups by state, a reason
// per row, the loop footer, the owner / planner / brakes lines and the
// Plan / Runs / Log / Repos tabs.
describe('the Missions board (gap G3.6)', () => {
  const NOW = 1_800_000_000;
  const card = (id: number, kind = 'ask', payload: Record<string, unknown> = { question: 'Which?' }) => ({
    id,
    mission_id: 1,
    decision_id: `d${id}`,
    source: 'planner',
    kind,
    payload,
    state: 'open',
    created_at: 1,
  });
  const autonomy = { asked: 3, ceiling: 1, effective: 1, why: '', enabled: true };
  const planOf = (over: Record<string, unknown> = {}) => ({
    steps: [],
    cards: [],
    autonomy,
    cost_micros: 0,
    counts: { total: 0, open: 0 },
    ...over,
  });

  const fed = mission({ id: 1, name: 'Hub federation v2', state: 'active', total: 12, done: 7, updated_at: 50 });
  const demo = mission({ id: 2, name: 'Demo mission', state: 'active', mode: 'continuous', total: 5, done: 3, updated_at: 40 });
  const receipts = mission({ id: 3, name: 'Receipt documents', state: 'paused', updated_at: 30, policy: { no_progress_secs: 7200 } });
  const installer = mission({ id: 4, name: 'Windows installer polish', state: 'draft', updated_at: 20 });
  const done = mission({ id: 5, name: 'Shipped thing', state: 'completed', updated_at: 10 });
  const failed = mission({ id: 6, name: 'Dropped thing', state: 'failed', updated_at: 9 });

  const details: Record<number, MissionDetail> = {
    1: {
      mission: fed,
      items: [item(20, 'Pairing spec'), item(21, 'Peer code redemption')],
      graph: {
        nodes: [
          { item_id: 20, state: 'done', wave: 1 },
          { item_id: 21, state: 'failed', wave: 3 },
        ],
      },
      events: [],
      phase: 'blocked',
      plan: planOf({ cards: [card(1), card(2, 'run', {})] }),
    },
    2: {
      mission: demo,
      items: [item(30, 'A'), item(31, 'B')],
      graph: {
        nodes: [
          { item_id: 30, state: 'done', wave: 1 },
          { item_id: 31, state: 'running', wave: 2 },
        ],
      },
      events: [],
      phase: 'running',
      plan: planOf(),
    },
    3: {
      mission: receipts,
      items: [],
      events: [
        { id: 9, at: 5, kind: 'no_progress', actor: 'loop', payload: { why: 'no run started or finished for 120 minutes' } },
        { id: 8, at: 5, kind: 'state', actor: 'loop', payload: { from: 'active', to: 'paused' } },
      ],
      plan: planOf({ cards: [card(3)] }),
    },
    4: {
      mission: installer,
      items: [],
      events: [],
      plan: null,
      graph: { nodes: [] },
    },
  };
  // The draft's planner answer: one create card of six tasks.
  details[4].plan = planOf({ cards: [card(4, 'create', { tree: [1, 2, 3, 4, 5, 6].map((n) => ({ title: `T${n}` })) })] });

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    handlers = {
      work_missions: () => [installer, done, receipts, fed, failed, demo],
      work_mission: (a) => details[Number(a.mission_id)],
      pause_all_missions: () => [1, 2],
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
    localStorage.removeItem('cf:pref:work.missions.group');
  });

  const rowNames = () => screen.getAllByTestId('mission-row').map((r) => r.querySelector('.title')?.textContent);

  it('groups the list Active, Paused, Drafts, then a folded Completed', async () => {
    render(WorkMissions);
    await flush();
    expect(screen.getByTestId('missions-group-active').textContent).toContain('Active');
    expect(screen.getByTestId('missions-group-active').textContent).toContain('2');
    expect(screen.getByTestId('missions-group-paused').textContent).toContain('1');
    expect(screen.getByTestId('missions-group-draft').textContent).toContain('Drafts');
    expect(rowNames()).toEqual(['Hub federation v2', 'Demo mission', 'Receipt documents', 'Windows installer polish']);
    const fold = screen.getByTestId('missions-group-finished');
    expect(fold.textContent).toContain('Completed');
    expect(fold.textContent).toContain('2');
    expect(fold.getAttribute('aria-expanded')).toBe('false');
    await fireEvent.click(fold);
    expect(rowNames()).toContain('Shipped thing');
    expect(rowNames()).toContain('Dropped thing');
  });

  it('Filters narrow the list by needs you, organisation, repository and mode, and say what they hide (G7.7)', async () => {
    const repo = (project_id: number, name: string) => ({ project_id, name, created_at: 1 });
    handlers.work_missions = () => [
      { ...fed, org_id: 1, waiting_on: { reason: 'confirm', since: 1, open_cards: 2 }, repos: [repo(3, 'claude-fleet')] },
      { ...demo, org_id: 2, repos: [repo(4, 'fleet-mobile')] },
      { ...receipts, org_id: 1, repos: [repo(4, 'fleet-mobile')] },
      installer,
    ];
    render(WorkMissions);
    await flush();
    expect(screen.queryByTestId('missions-filters-panel')).toBeNull();
    await fireEvent.click(screen.getByTestId('missions-filters'));
    await fireEvent.click(screen.getByTestId('missions-filter-needs-you'));
    await flush();
    expect(rowNames()).toEqual(['Hub federation v2']);
    expect(screen.getByTestId('missions-filters-count').textContent).toBe('1');
    expect(screen.getByTestId('missions-hidden').textContent).toContain('3 hidden by filters');
    await fireEvent.click(screen.getByTestId('missions-filter-needs-you'));
    await fireEvent.change(screen.getByTestId('missions-filter-repo'), { target: { value: '4' } });
    await flush();
    expect(rowNames()).toEqual(['Demo mission', 'Receipt documents']);
    await fireEvent.change(screen.getByTestId('missions-filter-org'), { target: { value: '1' } });
    await flush();
    expect(rowNames()).toEqual(['Receipt documents']);
    expect(screen.getByTestId('missions-filters-count').textContent).toBe('2');
    await fireEvent.change(screen.getByTestId('missions-filter-org'), { target: { value: '0' } });
    await fireEvent.change(screen.getByTestId('missions-filter-repo'), { target: { value: '' } });
    await flush();
    expect(rowNames()).toEqual(['Windows installer polish']);
    await fireEvent.click(screen.getByTestId('missions-hidden-show'));
    await flush();
    expect(rowNames()).toHaveLength(4);
    expect(screen.queryByTestId('missions-filters-count')).toBeNull();
    expect(screen.queryByTestId('missions-hidden')).toBeNull();
  });

  it('reads only the missions still going for their reasons', async () => {
    render(WorkMissions);
    await flush();
    expect(calls('work_mission').map((a) => a.mission_id).sort()).toEqual([1, 2, 3, 4]);
  });

  it('says on each row why the mission is where it is', async () => {
    render(WorkMissions);
    await flush();
    const reasons = Object.fromEntries(
      screen.getAllByTestId('mission-row').map((r) => [
        r.querySelector('.title')?.textContent,
        r.querySelector('[data-testid="mission-reason"]')?.textContent ?? null,
      ]),
    );
    expect(reasons['Hub federation v2']).toBe('Needs you · 2 cards · wave 3');
    expect(reasons['Demo mission']).toBe('Working · wave 2 · continuous');
    expect(reasons['Receipt documents']).toBe('Brake: no progress in 2 h');
    expect(reasons['Windows installer polish']).toBe('Planner proposed 6 tasks · not accepted');
  });

  it('ends the list with the loop and the fleet ceiling beside Pause all', async () => {
    render(WorkMissions);
    await flush();
    const foot = screen.getByTestId('missions-footer');
    expect(screen.getByTestId('missions-loop').textContent).toBe('Mission loop on · ceiling L1');
    await fireEvent.click(foot.querySelector('[data-testid="missions-pause-all"]')!);
    await flush();
    expect(calls('pause_all_missions')).toHaveLength(1);
  });

  it('Group: None lists every mission flat with its state', async () => {
    render(WorkMissions);
    await flush();
    await fireEvent.change(screen.getByTestId('missions-group'), { target: { value: 'none' } });
    await flush();
    expect(screen.queryByTestId('missions-group-active')).toBeNull();
    expect(screen.getAllByTestId('mission-row')).toHaveLength(6);
    expect(screen.getAllByTestId('mission-row')[0].textContent).toContain('Active');
    expect(localStorage.getItem('cf:pref:work.missions.group')).toContain('none');
  });

  it('a mission shows its owner, planner runs this hour and brakes', async () => {
    const { myPersonId } = await import('./access');
    const { orgs } = await import('./orgs');
    myPersonId.set(7);
    orgs.set([{ id: 2, name: '32bit', created_at: 1, rules: [], hosts: [], trackers: [] }]);
    vi.spyOn(Date, 'now').mockReturnValue(NOW * 1000);
    const m = mission({
      ...fed,
      owner_person_id: 7,
      org_id: 2,
      policy: { max_planner_runs_per_hour: 4, no_progress_secs: 7200 },
    });
    details[1] = {
      ...details[1],
      mission: m,
      events: [
        { id: 3, at: NOW - 60, kind: 'planned', actor: 'loop' },
        { id: 2, at: NOW - 1800, kind: 'planned', actor: 'person:7' },
        { id: 1, at: NOW - 7200, kind: 'planned', actor: 'loop' },
      ],
      plan: planOf({
        autonomy: { ...autonomy, grant: { id: 1, mission_id: 1, plan_version: 1, level: 1, granted_by: 'x', budget_micros: 40e6, created_at: 1, expires_at: NOW + 3600 } },
      }),
    };
    handlers.work_missions = () => [m];
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-owner').textContent).toBe('You · 32bit');
    expect(screen.getByTestId('mission-planner-runs').textContent).toBe('2 of 4 runs this hour');
    expect(screen.getByTestId('mission-brakes').textContent).toBe('Pause on spent budget ($40.00) or 2 h without progress');
    vi.mocked(Date.now).mockRestore();
    myPersonId.set(null);
    orgs.set([]);
  });

  it('splits the detail into Plan, Runs, Log and Repos tabs', async () => {
    const m = mission({ ...fed, repos: [{ project_id: 1, name: 'o/claude-fleet', created_at: 1 }] });
    details[1] = {
      mission: m,
      items: [item(20, 'Pairing spec'), item(21, 'Peer code redemption')],
      graph: {
        nodes: [
          { item_id: 20, state: 'done', wave: 1, attempt: { task_id: 51, role: 'implement', attempt: 1, state: 'done' } },
          { item_id: 21, state: 'failed', wave: 1, attempt: { task_id: 53, role: 'implement', attempt: 2, state: 'failed' } },
        ],
      },
      events: [
        { id: 3, at: 100, kind: 'step', actor: 'loop', work_item_id: 21, payload: { step: 'retry', role: 'implement', task_id: 53 } },
        { id: 2, at: 90, kind: 'step', actor: 'person:7', work_item_id: 21, payload: { step: 'run', role: 'implement', task_id: 52 } },
        { id: 1, at: 80, kind: 'refused', actor: 'loop', payload: { step: 'run', why: 'no host' } },
      ],
      may_change: true,
      plan: planOf(),
    };
    handlers.work_missions = () => [m];
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-tab-plan').getAttribute('aria-selected')).toBe('true');
    expect(screen.getByTestId('mission-loop')).toBeTruthy();
    expect(screen.getByTestId('mission-tab-runs').textContent).toBe('Runs3');
    expect(screen.getByTestId('mission-tab-repos').textContent).toBe('Repos1');
    expect(screen.queryByTestId('mission-events')).toBeNull();

    await fireEvent.click(screen.getByTestId('mission-tab-runs'));
    expect(screen.queryByTestId('mission-loop')).toBeNull();
    const runs = screen.getAllByTestId('mission-run');
    expect(runs.map((r) => r.getAttribute('data-state'))).toEqual(['failed', 'started', 'done']);
    expect(runs[0].textContent).toContain('Peer code redemption');
    expect(runs[0].textContent).toContain('implement #2 · Failed');
    expect(runs[0].textContent).toContain('by the loop');
    expect(runs[1].textContent).toContain('Idle · earlier run');

    await fireEvent.click(screen.getByTestId('mission-tab-log'));
    expect(screen.getByTestId('mission-events').textContent).toContain('Refused');
    await fireEvent.click(screen.getByTestId('mission-tab-repos'));
    expect(screen.getByTestId('mission-repos').textContent).toContain('o/claude-fleet');
    // Another mission opens on Plan again.
    await fireEvent.click(screen.getByTestId('mission-back'));
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    expect(screen.getByTestId('mission-tab-plan').getAttribute('aria-selected')).toBe('true');
  });

  it('a mission with no run yet says so', async () => {
    handlers.work_missions = () => [installer];
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
    await fireEvent.click(screen.getByTestId('mission-tab-runs'));
    expect(screen.getByTestId('mission-runs-empty')).toBeTruthy();
  });
});

describe('the Missions board helpers (gap G3.6)', () => {
  it('words a span', () => {
    expect(durationWords(600)).toBe('10 min');
    expect(durationWords(3600)).toBe('1 h');
    expect(durationWords(5400)).toBe('1 h 30 min');
  });

  it('words the brakes with and without a budget', () => {
    expect(brakesLine(undefined)).toBe('Pause after 1 h without progress');
    expect(brakesLine({ no_progress_secs: 7200 }, 20e6)).toBe('Pause on spent budget ($20.00) or 2 h without progress');
  });

  it('names the owner as You, a member, or a person number, with the org', () => {
    const name = (id: number) => (id === 3 ? 'Ana' : null);
    const org = (id: number) => (id === 2 ? '32bit' : null);
    expect(ownerLine({ owner_person_id: 7, org_id: 2 }, 7, name, org)).toBe('You · 32bit');
    expect(ownerLine({ owner_person_id: 3, org_id: null }, 7, name, org)).toBe('Ana');
    expect(ownerLine({ owner_person_id: 9 }, null, name, org)).toBe('person 9');
    expect(ownerLine({}, 7, name, org)).toBeNull();
  });

  it('counts planner runs in the last hour only', () => {
    const ev = (at: number, kind = 'planned') => ({ id: at, at, kind, actor: 'loop' });
    expect(plannerRunsThisHour([ev(1000), ev(3000), ev(3500, 'step'), ev(-10)], 3600)).toBe(2);
  });

  it('a paused mission names the brake only when the brake paused it last', () => {
    const m = mission({ state: 'paused', policy: { no_progress_secs: 1800 } });
    const ev = (id: number, kind: string) => ({ id, at: id, kind, actor: 'loop' });
    expect(brakeReason({ mission: m, events: [ev(2, 'no_progress'), ev(1, 'state')] })).toBe('no progress in 30 min');
    expect(brakeReason({ mission: m, events: [ev(3, 'budget')] })).toBe('budget spent');
    // A person resumed and paused it again after the brake.
    expect(brakeReason({ mission: m, events: [ev(3, 'state'), ev(2, 'no_progress')] })).toBeNull();
  });

  it('a row with no detail, or an ended one, says only what it knows', () => {
    expect(missionReason(mission({ state: 'active' }), null)).toBeNull();
    expect(missionReason(mission({ state: 'completed' }), null)).toBeNull();
    expect(missionReason(mission({ state: 'cancelled' }), null)).toBe('Cancelled');
    expect(missionGroups([mission({ state: 'failed' })]).map((g) => g.label)).toEqual(['Completed']);
  });

  it('the footer reads the loop from any open plan', () => {
    const plan = { autonomy: { asked: 1, ceiling: 2, effective: 1, why: '', enabled: false }, cost_micros: 0, counts: { total: 0, open: 0 } };
    expect(loopFooter([null, { mission: mission(), plan }])).toBe('Mission loop off · ceiling L2');
    expect(loopFooter([])).toBeNull();
  });

  it('runs: every started step, the latest attempt carrying its state', () => {
    const d: MissionDetail = {
      mission: mission(),
      events: [
        { id: 2, at: 20, kind: 'step', actor: 'loop', work_item_id: 5, payload: { step: 'review', role: 'review', task_id: 9 } },
        { id: 1, at: 10, kind: 'step', actor: 'loop', work_item_id: 5, payload: { step: 'run', task_id: null } },
      ],
      graph: { nodes: [{ item_id: 5, state: 'running', wave: 1, attempt: { task_id: 9, state: 'running', attempt: 1 } }] },
    };
    expect(runsOf(d)).toEqual([{ task_id: 9, item_id: 5, role: 'review', attempt: 1, state: 'running', at: 20, actor: 'loop' }]);
    expect(runStateLabel('running')).toBe('Working');
    expect(runStateLabel('cancelled')).toBe('Idle · cancelled');
  });
});
