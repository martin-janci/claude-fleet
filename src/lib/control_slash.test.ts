// Gap plan G3.9: Control's slash commands, parsed and run against the
// backend actions Work uses.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  commandReceipts,
  debugOutcome,
  proposeOutcome,
  debugPoll,
  parseControlCommand,
  parseDue,
  resetCommandReceiptsForTests,
  sessionsNamed,
  takeControlCommand,
  undoCommand,
} from './control_slash';
import { sessions, type SessionRow } from './sessions';
import { hosts, type HostRow } from './hosts';
import { controlVersions } from './control_versions';

// Friday 9 October 2026, mid-morning, local time.
const NOW = new Date(2026, 9, 9, 10, 30);
const inv = invoke as ReturnType<typeof vi.fn>;
const calls = (cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]);

const row = (over: Partial<SessionRow>): SessionRow =>
  ({ id: 1, tmux_name: 'fed-v2', host_alias: 'mac', friendly_name: null, lost_at: null, ...over }) as SessionRow;

async function receipt() {
  await vi.waitFor(() => expect(get(commandReceipts).at(-1)?.state).not.toBe('running'));
  return get(commandReceipts).at(-1)!;
}

function answer(map: Record<string, unknown>) {
  inv.mockImplementation(async (cmd: string) => {
    if (!(cmd in map)) throw new Error(`unexpected ${cmd}`);
    const v = map[cmd];
    return typeof v === 'function' ? (v as () => unknown)() : v;
  });
}

beforeEach(() => {
  inv.mockReset();
  resetCommandReceiptsForTests();
  sessions.set([]);
});

describe('parseDue', () => {
  it('reads today, tomorrow, a weekday and a date in local time', () => {
    expect(parseDue('today', NOW)).toBe('2026-10-09');
    expect(parseDue('tomorrow', NOW)).toBe('2026-10-10');
    expect(parseDue('mon', NOW)).toBe('2026-10-12');
    expect(parseDue('Monday', NOW)).toBe('2026-10-12');
    expect(parseDue('fri', NOW)).toBe('2026-10-09');
    expect(parseDue('2026-10-31', NOW)).toBe('2026-10-31');
  });
  it('refuses what is not a day', () => {
    expect(parseDue('month', NOW)).toBeNull();
    expect(parseDue('2026-02-30', NOW)).toBeNull();
    expect(parseDue('soon', NOW)).toBeNull();
  });
});

describe('parseControlCommand', () => {
  it('/task takes a title, due: and an @owner anywhere in the line', () => {
    expect(parseControlCommand('/task Fix the Windows tray icon @Martin due:mon', NOW)).toEqual({
      ok: true,
      command: { cmd: 'task', title: 'Fix the Windows tray icon', owner: 'Martin', due: '2026-10-12' },
    });
    expect(parseControlCommand('/task   Rotate the NAS password  ', NOW)).toEqual({
      ok: true,
      command: { cmd: 'task', title: 'Rotate the NAS password' },
    });
  });
  it('/done, /assign and /start take a task key, with or without #', () => {
    expect(parseControlCommand('/done #task-219')).toEqual({ ok: true, command: { cmd: 'done', key: 'TASK-219' } });
    expect(parseControlCommand('/assign PD-2592 @fed-v2')).toEqual({
      ok: true,
      command: { cmd: 'assign', key: 'PD-2592', session: 'fed-v2' },
    });
    expect(parseControlCommand('/start #TASK-235 @mercury')).toEqual({
      ok: true,
      command: { cmd: 'start', key: 'TASK-235', host: 'mercury' },
    });
    expect(parseControlCommand('/start #TASK-235')).toEqual({ ok: true, command: { cmd: 'start', key: 'TASK-235' } });
  });
  it('a command of its own typed wrong answers its usage', () => {
    expect(parseControlCommand('/task')).toEqual({ ok: false, usage: '/task <title> [due:<day>] [@owner]' });
    expect(parseControlCommand('/task Fix due:someday', NOW)).toEqual({ ok: false, usage: '/task <title> [due:<day>] [@owner]' });
    expect(parseControlCommand('/done the login bug')).toEqual({ ok: false, usage: '/done #KEY' });
    expect(parseControlCommand('/assign #TASK-1')).toEqual({ ok: false, usage: '/assign #KEY @session' });
    expect(parseControlCommand('/start')).toEqual({ ok: false, usage: '/start #KEY [@host]' });
  });
  it('/plan, the built-ins and plain prompts are not Control’s to run', () => {
    expect(parseControlCommand('/plan the Windows installer')).toBeNull();
    expect(parseControlCommand('/clear')).toBeNull();
    expect(parseControlCommand('/tidy')).toBeNull();
    expect(parseControlCommand('how is the release going?')).toBeNull();
  });
  it('reads /health and /graph', () => {
    expect(parseControlCommand('/health')).toEqual({ ok: true, command: { cmd: 'health' } });
    expect(parseControlCommand('/health @mefistos')).toEqual({ ok: true, command: { cmd: 'health', host: 'mefistos' } });
    expect(parseControlCommand('/health mefistos')).toEqual({ ok: false, usage: '/health [@host]' });
    expect(parseControlCommand('/graph')).toEqual({ ok: true, command: { cmd: 'graph' } });
    expect(parseControlCommand('/graph Hub federation')).toEqual({ ok: true, command: { cmd: 'graph', target: 'Hub federation' } });
  });
});

describe('/health and /graph', () => {
  const host = (alias: string, over: Partial<HostRow> = {}): HostRow =>
    ({ alias, reachable: true, hidden: false, health_at: Math.floor(Date.now() / 1000), cpu_pct: 20, mem_avail_kb: 6_000_000, mem_total_kb: 8_000_000, disk_home_free_kb: 50_000_000, disk_home_total_kb: 100_000_000, ...over }) as HostRow;

  it('/health lists every host, the one in trouble first, without asking the backend', async () => {
    hosts.set([host('alpha'), host('mefistos', { reachable: false })]);
    expect(takeControlCommand('/health')).toBe(true);
    const r = await receipt();
    expect(r.state).toBe('done');
    expect(r.line).toContain('mefistos');
    expect(r.rows?.map((x) => x.label)).toEqual(['mefistos', 'alpha']);
    expect(r.rows?.[0].text).toBe('offline');
    expect(r.open).toEqual({ kind: 'hosts' });
    expect(inv).not.toHaveBeenCalled();
  });

  it('/health ends with the versions Control runs against, amber when the hub is behind', async () => {
    hosts.set([host('alpha')]);
    controlVersions.set({ hub: '0.6.2', desktop: '0.6.3', phone: null, remote: true });
    takeControlCommand('/health');
    const r = await receipt();
    const last = r.rows?.at(-1);
    expect(last?.label).toBe('versions');
    expect(last?.text).toBe('hub 0.6.2 · desktop 0.6.3 · hub behind');
    expect(last?.level).toBe('warn');
    controlVersions.set({ hub: null, desktop: null, phone: null, remote: false });
  });

  it('/health @host narrows it, and names a host it does not know', async () => {
    hosts.set([host('alpha'), host('beta')]);
    takeControlCommand('/health @beta');
    const r = await receipt();
    expect(r.line).toBe('beta is fine.');
    expect(r.rows).toHaveLength(1);
    takeControlCommand('/health @gamma');
    expect((await receipt()).line).toBe('No host named gamma.');
  });

  it('/graph draws the only active mission as a graph card', async () => {
    const mission = { id: 4, name: 'Hub federation', state: 'active', root_item_id: 40 };
    answer({
      work_missions: [mission, { id: 5, name: 'Old', state: 'completed' }],
      work_mission: {
        mission,
        items: [{ id: 41, title: 'Spec' }, { id: 42, title: 'Build' }],
        graph: { nodes: [{ item_id: 41, wave: 1, state: 'done' }, { item_id: 42, wave: 2, state: 'todo' }] },
      },
    });
    takeControlCommand('/graph');
    const r = await receipt();
    expect(r.state).toBe('done');
    expect(r.line).toBe('Hub federation · 1 of 2 done');
    expect(r.graph?.total).toBe(2);
    expect(r.open).toEqual({ kind: 'mission', id: 4 });
    expect(calls('work_mission')).toEqual([{ args: { mission_id: 4 } }]);
  });

  it('/graph asks which mission when several are active', async () => {
    answer({ work_missions: [{ id: 1, name: 'A', state: 'active' }, { id: 2, name: 'B', state: 'active' }] });
    takeControlCommand('/graph');
    const r = await receipt();
    expect(r.state).toBe('failed');
    expect(r.line).toBe('2 missions are active: name one (A, B).');
  });
});

describe('takeControlCommand', () => {
  it('leaves what is not its own for the agent', () => {
    expect(takeControlCommand('/plan the Windows installer')).toBe(false);
    expect(get(commandReceipts)).toEqual([]);
    expect(inv).not.toHaveBeenCalled();
  });

  it('/done marks the task done by its key, and Undo puts the status back', async () => {
    answer({
      work_task: { task: { task_id: 'item:219', item_id: 219, key: 'TASK-219', title: 'Guide tool', status_category: 'in_progress', kind: 'local', group: {} } },
      set_work_status: { id: 219, status_category: 'done' },
    });
    expect(takeControlCommand('/done #TASK-219')).toBe(true);
    const r = await receipt();
    expect(calls('work_task')).toEqual([{ args: { task_id: 'ref:TASK-219' } }]);
    expect(calls('set_work_status')).toEqual([{ args: { item_id: 219, status: 'done' } }]);
    expect(r.state).toBe('done');
    expect(r.line).toBe('TASK-219 · Guide tool is done.');
    expect(r.open).toEqual({ kind: 'task', taskId: 'item:219' });

    await undoCommand(r.id);
    expect(calls('set_work_status').at(-1)).toEqual({ args: { item_id: 219, status: 'in_progress' } });
    expect(get(commandReceipts).at(-1)?.line).toBe('Undone.');
  });

  it('/done names an unknown key instead of guessing', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'work_task') throw { code: 'E_NOTFOUND', message: 'task ref:TASK-9 not found' };
      throw new Error(cmd);
    });
    takeControlCommand('/done TASK-9');
    const r = await receipt();
    expect(r.state).toBe('failed');
    expect(r.line).toBe('No task TASK-9.');
    expect(calls('set_work_status')).toEqual([]);
  });

  it('/assign links the task to the live session of that name', async () => {
    sessions.set([row({ id: 4, tmux_name: 'fed-v2' }), row({ id: 5, tmux_name: 'old', friendly_name: 'fed-v2', lost_at: 10 })]);
    answer({ link_session_work: row({ id: 4 }) });
    takeControlCommand('/assign #PD-2592 @FED-V2');
    const r = await receipt();
    expect(calls('link_session_work')).toEqual([{ args: expect.objectContaining({ session_id: 4, key: 'PD-2592' }) }]);
    expect(r.line).toBe('PD-2592 is assigned to fed-v2.');
    expect(r.open).toEqual({ kind: 'session', id: 4, label: 'fed-v2' });
  });

  it('/assign refuses a session it cannot name, and links nothing', async () => {
    sessions.set([row({ id: 4, tmux_name: 'a', friendly_name: 'twin' }), row({ id: 6, tmux_name: 'b', friendly_name: 'twin' })]);
    takeControlCommand('/assign #PD-1 @nobody');
    expect((await receipt()).line).toBe('No live session named nobody.');
    takeControlCommand('/assign #PD-1 @twin');
    expect((await receipt()).line).toBe('2 sessions are named twin: assign it from Work.');
    expect(inv).not.toHaveBeenCalled();
  });

  it('/start starts where a clean preview says, on the host asked for', async () => {
    const preview = {
      key: 'TASK-235',
      title: 'First-run screen',
      item_id: 235,
      plan: { key: 'TASK-235', title: 'First-run screen', item_id: 235, project_id: 3, host_alias: 'mercury', branch: 'task-235', name: 'TASK-235 First-run' },
      projects: [],
      hosts: [{ alias: 'mercury', reachable: true }],
      conflicts: [],
    };
    answer({ preview_start_work: preview, start_work: row({ id: 12, tmux_name: 'task-235', host_alias: 'mercury' }) });
    takeControlCommand('/start #TASK-235 @mercury');
    const r = await receipt();
    expect(calls('preview_start_work')).toEqual([{ args: { reference: 'TASK-235', with_brief: true, host_alias: 'mercury' } }]);
    expect(calls('start_work')).toEqual([
      { args: { reference: 'TASK-235', with_brief: true, host_alias: 'mercury', project_id: 3 } },
    ]);
    expect(r.line).toBe('Started task-235 on mercury for TASK-235.');
    expect(r.open).toEqual({ kind: 'session', id: 12, label: 'task-235' });
  });

  it('/start does not guess past a choice the preview asks for', async () => {
    answer({
      preview_start_work: { key: 'TASK-1', title: 'x', item_id: 1, plan: null, missing: 'project', projects: [], hosts: [], conflicts: [] },
    });
    takeControlCommand('/start TASK-1');
    const r = await receipt();
    expect(r.state).toBe('failed');
    expect(r.line).toBe('TASK-1 did not start: pick a project. Start it from Work.');
    expect(r.open).toEqual({ kind: 'task', taskId: 'item:1' });
    expect(calls('start_work')).toEqual([]);
  });

  it('keeps the latest three receipts', async () => {
    for (const k of ['/done', '/start', '/assign', '/task']) takeControlCommand(k);
    expect(get(commandReceipts).map((r) => r.typed)).toEqual(['/start', '/assign', '/task']);
  });
});

describe('/debug', () => {
  const selfDebug = { id: 21, agent: 'fleet', definition: 'self-debug', active: true };
  const answerJob = (state: string, result: unknown = null) => ({
    dedup_key: 'r21:manual:1:abc',
    state,
    error: state === 'failed' ? 'timed out' : null,
    result: result === null ? null : JSON.stringify(result),
  });

  it('reads the host, window, part and session around the focus', () => {
    expect(parseControlCommand('/debug @htz 6h part:hub session:fed-v2 why did it restart?')).toEqual({
      ok: true,
      command: { cmd: 'debug', request: 'why did it restart?', scope: { host: 'htz', minutes: 360, component: 'hub', session: 'fed-v2' } },
    });
    expect(parseControlCommand('/debug')).toEqual({ ok: true, command: { cmd: 'debug' } });
    expect(parseControlCommand('/debug 30m')).toEqual({ ok: true, command: { cmd: 'debug', scope: { minutes: 30 } } });
    expect(parseControlCommand('/debug part:kernel x')).toMatchObject({ ok: false });
    expect(parseControlCommand('/debug 8d')).toMatchObject({ ok: false });
  });

  it('queues the self debug agent and shows its findings worst first', async () => {
    debugPoll.everyMs = 1;
    let polls = 0;
    answer({
      agents_status: () =>
        ++polls === 1
          ? { rules: [selfDebug], jobs: [], pending: [] }
          : polls === 2
            ? { rules: [selfDebug], jobs: [answerJob('running')], pending: [] }
            : {
                rules: [selfDebug],
                jobs: [
                  answerJob('done', {
                    answer: {
                      summary: 'The hub ran out of memory twice.',
                      findings: [
                        { title: 'Slow probe', severity: 'low', component: 'host', cause: 'ssh latency' },
                        { title: 'OOM kill', severity: 'high', component: 'hub', cause: 'a 2 GB limit' },
                      ],
                    },
                  }),
                ],
                pending: [],
              },
      agents_fleet: { changed: 1, id: 21, job: 'r21:manual:1:abc' },
    });
    expect(takeControlCommand('/debug @htz 6h OOM?')).toBe(true);
    const r = await receipt();
    expect(calls('agents_fleet')[0]).toEqual({
      action: 'run',
      definition: null,
      id: 21,
      answers: null,
      request: 'OOM?',
      runScope: { minutes: 360, host: 'htz' },
    });
    expect(r.state).toBe('done');
    expect(r.line).toBe('The hub ran out of memory twice.');
    expect(r.rows).toEqual([
      { label: 'hub', text: 'OOM kill · a 2 GB limit', level: 'crit' },
      { label: 'host', text: 'Slow probe · ssh latency', level: 'ok' },
    ]);
    expect(r.open).toEqual({ kind: 'runs' });
  });

  it('says where to add the agent when there is none', async () => {
    answer({ agents_status: { rules: [{ ...selfDebug, definition: 'l2-support' }], jobs: [], pending: [] } });
    takeControlCommand('/debug');
    const r = await receipt();
    expect(r.state).toBe('failed');
    expect(r.line).toContain('Automation, Agents');
    expect(calls('agents_fleet')).toHaveLength(0);
  });

  it('reads a failed or empty run', () => {
    expect(debugOutcome(answerJob('failed'))).toMatchObject({ state: 'failed', line: 'The self debug run failed: timed out' });
    expect(debugOutcome(answerJob('done', { answer: { findings: [] } }))).toMatchObject({ state: 'done', line: 'No problems found.' });
  });
});

describe('/propose', () => {
  const proposer = { id: 22, agent: 'fleet', definition: 'proposer', active: true };

  it('takes the rest of the line as its focus', () => {
    expect(parseControlCommand('/propose the hub')).toEqual({ ok: true, command: { cmd: 'propose', request: 'the hub' } });
    expect(parseControlCommand('/propose')).toEqual({ ok: true, command: { cmd: 'propose' } });
  });

  it('runs the proposer and lists what waits in the Inbox', async () => {
    debugPoll.everyMs = 1;
    let polls = 0;
    const done = {
      dedup_key: 'r22:manual:1:abc',
      state: 'done',
      error: null,
      result: JSON.stringify({
        answer: {
          summary: 'Two repeats this week.',
          proposals: [{ title: 'Raise the probe timeout', kind: 'setting', severity: 'major', why: 'seen 7 times' }],
        },
      }),
    };
    answer({
      agents_status: () => (++polls === 1 ? { rules: [proposer], jobs: [], pending: [] } : { rules: [proposer], jobs: [done], pending: [] }),
      agents_fleet: { changed: 1, id: 22, job: 'r22:manual:1:abc' },
    });
    expect(takeControlCommand('/propose the hub')).toBe(true);
    const r = await receipt();
    expect(calls('agents_fleet')[0]).toMatchObject({ action: 'run', id: 22, request: 'the hub' });
    expect(r.line).toBe('Two repeats this week. 1 proposal waits in the Inbox.');
    expect(r.rows).toEqual([{ label: 'setting', text: 'Raise the probe timeout · seen 7 times', level: 'warn' }]);
  });

  it('says where to add the proposer, and reads a quiet day', async () => {
    answer({ agents_status: { rules: [], jobs: [], pending: [] } });
    takeControlCommand('/propose');
    expect((await receipt()).line).toBe('No proposer yet: add one under Automation, Agents.');
    expect(proposeOutcome({ state: 'done', error: null, result: '{"answer":{"proposals":[]}}' })).toMatchObject({
      line: 'Nothing to propose.',
    });
  });
});

describe('sessionsNamed', () => {
  it('matches the shown name or the tmux name, live rows only', () => {
    const rows = [row({ id: 1, tmux_name: 'x', friendly_name: 'Release' }), row({ id: 2, tmux_name: 'release', lost_at: 5 })];
    expect(sessionsNamed('release', rows).map((r) => r.id)).toEqual([1]);
  });
});
