// A finished mission (gap G3.7, the Finish board), driven through the
// Missions tab: Reopen, the finish checks card, the PR block, and the
// sessions to archive. Archive ends each session through its own Clean up,
// so a clean, pushed worktree is removed (its disk freed) and a dirty one
// goes to the agent's Safe remove first.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkMissions from './WorkMissions.svelte';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { toasts } from './toasts';
import type { Mission, MissionDetail } from './missions';
import {
  afterArchiveLine,
  alsoMergedLabel,
  archiveLabel,
  checkWords,
  finishChecks,
  finishPrBlock,
  finishSummary,
  prFactsLine,
  spanWords,
  waveSummary,
} from './mission_finish';
import { dollars } from './missions';

const flush = async () => {
  for (let i = 0; i < 12; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) =>
  vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => c[1] as { args: Record<string, unknown>; force?: boolean });

const DAY = 86_400;

function mission(over: Partial<Mission> = {}): Mission {
  return {
    id: 4,
    name: 'Hub federation v2',
    goal: 'Two hubs pair',
    mode: 'finite',
    state: 'completed',
    level: 1,
    plan_version: 1,
    created_at: 1,
    updated_at: 1,
    version: 3,
    root_item_id: 10,
    total: 3,
    done: 3,
    started_at: 1_000,
    finished_at: 1_000 + 3 * DAY + 4 * 3600,
    cost_micros: 31_400_000,
    ...over,
  };
}

const item = (id: number, title: string) => ({ id, source: 'local', title, status_category: 'done', created_at: 1, updated_at: 1 });

const pass = (line: string, kind: string, by: string | null = null) => ({ line, kind, state: 'pass', detail: '', by, at: null });

function detail(m: Mission): MissionDetail {
  return {
    mission: m,
    items: [item(10, 'Hub federation v2'), item(11, 'pair-api'), item(12, 'redemption'), item(13, 'pair-ui')],
    events: [],
    may_change: true,
    graph: {
      waves: 2,
      nodes: [
        { item_id: 11, state: 'done', wave: 1, verification: { state: 'verified', checks: [pass('ci', 'ci', 'task:7')] } },
        { item_id: 12, state: 'done', wave: 1, verification: { state: 'verified', checks: [pass('review', 'review', 'task:8')] } },
        { item_id: 13, state: 'done', wave: 2, verification: { state: 'verified', checks: [pass('person', 'person', 'person:1')] } },
      ],
    },
    finish: {
      sessions: [
        { session_id: 21, item_id: 11, host_alias: 'mercury', tmux_name: 'pair-api', kind: 'work', worktree_kb: 1_048_576 },
        { session_id: 22, item_id: 12, host_alias: 'mercury', tmux_name: 'redemption', kind: 'work', worktree_kb: 1_048_576 },
        { session_id: 23, item_id: 13, host_alias: 'mac', tmux_name: 'pair-ui', kind: 'work' },
      ],
      prs: [
        {
          id: 1,
          url: 'https://github.com/o/claude-fleet/pull/476',
          repo: 'o/claude-fleet',
          number: 476,
          title: 'fix hub-e2e federation pair flake',
          head_ref: 'fix-hub-e2e',
          state: 'MERGED',
          ci_status: 'passing',
          review_decision: 'APPROVED',
          first_seen_at: 1,
          updated_at: 2,
        },
        {
          id: 2,
          url: 'https://github.com/o/fleet-mobile/pull/88',
          repo: 'o/fleet-mobile',
          number: 88,
          title: 'phone pair flow',
          state: 'MERGED',
          first_seen_at: 1,
          updated_at: 1,
        },
      ],
    },
  };
}

const inspection = (dirty: boolean) => ({
  has_worktree: true,
  worktree_path: '/w',
  branch: 'b',
  upstream: 'origin/b',
  dirty_files: dirty ? [{ status: ' M', path: 'src/chip.ts' }] : [],
  unpushed_commits: 0,
  safe_to_remove: !dirty,
  error: null,
});

describe('a finished mission', () => {
  let current: Mission;
  let finish: MissionDetail['finish'];
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    current = mission();
    finish = detail(current).finish;
    sessions.set([
      session('mercury', 'pair-api', { id: 21 }),
      session('mercury', 'redemption', { id: 22 }),
      session('mac', 'pair-ui', { id: 23 }),
    ]);
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const a = (raw as { args?: Record<string, unknown> } | undefined)?.args ?? {};
      switch (cmd) {
        case 'work_missions':
          return [current];
        case 'work_mission':
          return { ...detail(current), finish: current.state === 'completed' ? finish : null };
        case 'set_mission_state':
          current = mission({ state: String(a.state), finished_at: null, version: 4 });
          return current;
        case 'inspect_safe_kill':
          return inspection(a.tmux_name === 'pair-ui');
        case 'discard_kill_session':
          // The worktree is removed with the pane: the session leaves the mission.
          finish = { ...finish, sessions: (finish?.sessions ?? []).filter((s) => s.tmux_name !== a.tmux_name) };
          return 1;
        case 'safe_kill_session':
          return session('mac', String(a.tmux_name), { id: 23, safe_kill_state: 'requested' });
        default:
          return null;
      }
    });
  });

  async function openIt() {
    render(WorkMissions);
    await flush();
    await fireEvent.click(screen.getByTestId('missions-group-finished'));
    await fireEvent.click(screen.getByTestId('mission-row'));
    await flush();
  }

  it('leads with the merged PR, its checks, approval and merge time, and puts the other merged ones on one line (G7.7)', async () => {
    const at = Math.floor(new Date(2026, 9, 10, 14, 31).getTime() / 1000);
    const pr = (id: number, repo: string, n: number, state: string, title: string) => ({
      id, url: `https://github.com/${repo}/pull/${n}`, repo, number: n, title, state,
      ci_status: 'passing', review_decision: 'APPROVED', merged_at: state === 'MERGED' ? at : null, first_seen_at: 1, updated_at: 2,
    });
    const block = finishPrBlock([
      pr(1, 'o/claude-fleet', 480, 'OPEN', 'follow-up'),
      pr(2, 'o/claude-fleet', 476, 'MERGED', 'fix flake'),
      pr(3, 'o/fleet-mobile', 88, 'MERGED', 'phone pair flow'),
    ]);
    expect(block.lead?.number).toBe(476);
    expect(block.rest.map((p) => p.number)).toEqual([480]);
    expect(block.alsoMerged.map(alsoMergedLabel)).toEqual(['fleet-mobile #88 phone pair flow']);
    const today = new Date(2026, 9, 10, 18, 0);
    expect(prFactsLine(block.lead!, today)).toMatch(/^CI passing · approved · merged \d{1,2}.31/);
    expect(prFactsLine(block.lead!, new Date(2026, 9, 12))).toMatch(/^CI passing · approved · merged Oct 10$/);
    expect(prFactsLine(block.rest[0], today)).toBe('CI passing · approved');
  });

  it('shows the summary, the finish checks, the PR and the sessions with their worktree state', async () => {
    await openIt();
    expect(screen.getByTestId('mission-finish-summary').textContent).toContain('All 3 tasks verified in 2 waves over 3 d 4 h.');
    expect(screen.getByTestId('mission-finish-checks-count').textContent).toBe('3 of 3');
    const pr = screen.getByTestId('mission-finish-pr').textContent ?? '';
    expect(pr).toContain('Merged');
    expect(pr).toContain('o/claude-fleet#476');
    expect(pr).toContain('CI passing · approved');
    expect(screen.getByTestId('mission-finish-also-merged').textContent?.replace(/\s+/g, ' ')).toContain('Also merged: fleet-mobile #88 phone pair flow');
    const states = screen.getAllByTestId('mission-finish-session-state').map((e) => e.textContent);
    expect(states).toEqual(['clean · pushed', 'clean · pushed', '1 uncommitted file · pushed']);
    expect(screen.getByTestId('mission-finish-after').textContent).toBe(
      'After archiving: moves to Missions › Completed (1). Spent $31.40 · 3 d 4 h.',
    );
    expect(screen.getByTestId('mission-archive').textContent).toBe('Archive 3 sessions');
  });

  it('archives: frees the clean worktrees and hands the dirty one to its agent', async () => {
    await openIt();
    await fireEvent.click(screen.getByTestId('mission-archive'));
    expect(screen.getByTestId('mission-archive-confirm-row').textContent).toContain('frees about 2.0 GB');
    expect(calls('discard_kill_session')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('mission-archive-confirm'));
    await flush();
    const removed = calls('discard_kill_session');
    expect(removed.map((c) => c.args.tmux_name)).toEqual(['pair-api', 'redemption']);
    expect(removed.every((c) => c.force === false)).toBe(true);
    expect(calls('safe_kill_session').map((c) => c.args.tmux_name)).toEqual(['pair-ui']);
    // The reload no longer lists the two freed sessions.
    expect(screen.getAllByTestId('mission-finish-session')).toHaveLength(1);
    const last = get(toasts).at(-1);
    expect(last?.message).toBe('Archived 2 sessions · freed about 2.0 GB · 1 agent committing and pushing first');
  });

  it('Keep archives nothing', async () => {
    await openIt();
    await fireEvent.click(screen.getByTestId('mission-archive'));
    await fireEvent.click(screen.getByTestId('mission-archive-keep'));
    await flush();
    expect(calls('discard_kill_session')).toHaveLength(0);
    expect(calls('safe_kill_session')).toHaveLength(0);
  });

  it('reopens to paused with the version it saw', async () => {
    await openIt();
    await fireEvent.click(screen.getByTestId('mission-reopen'));
    await flush();
    expect(calls('set_mission_state')[0].args).toEqual({ mission_id: 4, state: 'paused', expected_version: 3 });
    expect(screen.queryByTestId('mission-finish')).toBeNull();
    expect(screen.getByTestId('mission-state').textContent).toBe('Paused');
  });

  it('offers neither Reopen nor Archive to a reader who may not change it', async () => {
    const plain = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const v = await plain(cmd, raw as never);
      return cmd === 'work_mission' ? { ...(v as MissionDetail), may_change: false } : v;
    });
    await openIt();
    expect(screen.getByTestId('mission-finish')).toBeTruthy();
    expect(screen.queryByTestId('mission-reopen')).toBeNull();
    expect(screen.queryByTestId('mission-archive')).toBeNull();
  });
});

describe('mission_finish helpers', () => {
  it('words', () => {
    expect(archiveLabel(1)).toBe('Archive 1 session');
    expect(spanWords(2 * DAY)).toBe('2 d');
    expect(spanWords(5400)).toBe('1 h 30 min');
    expect(checkWords({ state: 'dirty', files: [], unpushed: 2, branch: null })).toBe('clean · 2 not pushed');
    expect(checkWords(undefined)).toBe('checking…');
  });

  it('a mission that ended early says how much was done', () => {
    const d = detail(mission({ state: 'cancelled' }));
    d.graph!.nodes![2] = { item_id: 13, state: 'ready', wave: 2 };
    expect(finishSummary(d)).toBe('2 of 3 tasks done in 2 waves over 3 d 4 h.');
    expect(finishChecks(d).passed).toBe(2);
    expect(waveSummary(d).map((w) => [w.wave, w.count, w.done])).toEqual([
      [1, 2, true],
      [2, 1, false],
    ]);
    expect(afterArchiveLine({ ...d, mission: { ...d.mission, budget_micros: 40_000_000 } }, 5, dollars)).toBe(
      'After archiving: moves to Missions › Completed (5). Spent $31.40 of $40.00 · 3 d 4 h.',
    );
  });
});
