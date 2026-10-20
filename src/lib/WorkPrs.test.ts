// Work › Pull requests (redesign 6.4): the PRs sessions opened, filtered by
// state, each opening on GitHub and naming the session that opened it.
import { render, screen, fireEvent, within, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./open_external', () => ({ openExternal: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import { openExternal } from './open_external';
import WorkPrs from './WorkPrs.svelte';
import { expectAccessible } from './a11y_check';
import { sessions } from './sessions';
import { PRIMARY, _resetFleetsForTest, _setFleetRowsForTest, _setFleetsForTest, emptyRows, type FleetInfo } from './fleets';
import { session } from './hosts_fixture';
import { prChecksLabel, prDiffstat, prRef, prStateLabel, type PullRequestRow } from './prs';

const pr = (over: Partial<PullRequestRow> = {}): PullRequestRow => ({
  id: 1,
  url: 'https://github.com/o/r/pull/42',
  repo: 'o/r',
  number: 42,
  title: 'Fix login',
  head_ref: 'fix/login',
  state: 'OPEN',
  draft: false,
  ci_status: 'passing',
  review_decision: 'APPROVED',
  session_id: 7,
  session_name: 'api',
  host_alias: 'mefistos',
  first_seen_at: 1790000000,
  updated_at: 1790000100,
  ...over,
});

let rows: PullRequestRow[];

async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

function calls() {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === 'list_pull_requests')
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

describe('WorkPrs', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(openExternal).mockClear();
    sessions.set([session('mefistos', 'api', { id: 7 })]);
    rows = [
      pr(),
      pr({ id: 2, number: 43, title: 'Old work', state: 'MERGED', merged_at: 1790000000, session_id: 99, session_name: 'gone-one' }),
    ];
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      if (cmd !== 'list_pull_requests') return null;
      const st = (raw as { args: { state: string } }).args.state;
      const items = rows.filter((r) => st === 'all' || r.state === st.toUpperCase());
      return { items, total: items.length };
    });
  });

  // Review w17-12: a new tab never shows the old tab's list.
  it('switching the filter drops the old list until the hub answers', async () => {
    render(WorkPrs);
    await flush();
    expect(screen.getAllByTestId('work-pr')).toHaveLength(1);
    let answer: (v: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementation(() => new Promise((r) => (answer = r)));
    await fireEvent.click(screen.getByTestId('work-prs-filter-merged'));
    await flush();
    expect(screen.queryAllByTestId('work-pr')).toHaveLength(0);
    answer({ items: [rows[1]], total: 1 });
    await flush();
    expect(screen.getAllByTestId('work-pr')).toHaveLength(1);
    expect(screen.getByTestId('work-pr').dataset.state).toBe('MERGED');
  });

  // Review w17-7: a failed background refresh keeps the list shown.
  it('a failed refresh keeps the list, with a Retry line', async () => {
    render(WorkPrs);
    await flush();
    vi.mocked(invoke).mockImplementation(async () => {
      throw { code: 'E_HUB_TIMEOUT', message: 'slow' };
    });
    sessions.set([session('mefistos', 'api', { id: 7, pr_url: 'https://github.com/o/r/pull/42' })]);
    await waitFor(() => expect(screen.getByTestId('work-prs-refresh-error')).toBeTruthy(), { timeout: 2000 });
    expect(screen.getAllByTestId('work-pr')).toHaveLength(1);
    expect(screen.queryByTestId('work-prs-error')).toBeNull();
  });

  it('opens on the open PRs, with state, CI, review and the session that opened it', async () => {
    render(WorkPrs);
    await flush();
    expect(calls()[0]).toEqual({ action: 'list', state: 'open' });
    const items = screen.getAllByTestId('work-pr');
    expect(items).toHaveLength(1);
    expect(within(items[0]).getByTestId('work-pr-state').textContent).toBe('Open');
    expect(within(items[0]).getByTestId('work-pr-ref').textContent).toBe('o/r#42');
    expect(within(items[0]).getByTestId('work-pr-checks').textContent).toContain('CI passing · approved');
    expect(within(items[0]).getByTestId('work-pr-session').textContent).toContain('api');
    await fireEvent.click(within(items[0]).getByTestId('work-pr-open'));
    expect(openExternal).toHaveBeenCalledWith('https://github.com/o/r/pull/42');
  });

  it('names the mission whose session opened it, and opens that mission (G7.8)', async () => {
    const { missionOpenRequest } = await import('./missions');
    rows = [pr({ mission_id: 5, mission_name: 'Hub federation v2' }), pr({ id: 3, number: 44 })];
    render(WorkPrs);
    await flush();
    const [fromMission, plain] = screen.getAllByTestId('work-pr');
    const link = within(fromMission).getByTestId('work-pr-mission');
    expect(link.textContent).toBe('Hub federation v2');
    expect(within(plain).queryByTestId('work-pr-mission')).toBeNull();
    await fireEvent.click(link);
    const { get } = await import('svelte/store');
    expect(get(missionOpenRequest)).toEqual({ id: 5 });
  });

  it('shows the diffstat when the hub knows it (gap plan G3.10)', async () => {
    rows = [pr({ additions: 18, deletions: 6 }), pr({ id: 3, number: 44 })];
    render(WorkPrs);
    await flush();
    const [known, unknown] = screen.getAllByTestId('work-pr');
    expect(within(known).getByTestId('work-pr-diffstat').textContent).toContain('+18 −6');
    expect(within(unknown).queryByTestId('work-pr-diffstat')).toBeNull();
    expect(prDiffstat({ additions: 3 })).toBe('+3 −0');
    expect(prDiffstat({})).toBe('');
  });

  it('the Merged filter lists merged PRs with when, and a gone session as text', async () => {
    render(WorkPrs);
    await flush();
    await fireEvent.click(screen.getByTestId('work-prs-filter-merged'));
    await flush();
    expect(calls().at(-1)).toEqual({ action: 'list', state: 'merged' });
    const items = screen.getAllByTestId('work-pr');
    expect(items).toHaveLength(1);
    expect(within(items[0]).getByTestId('work-pr-state').textContent).toBe('Merged');
    expect(within(items[0]).getByTestId('work-pr-merged').textContent).toContain('merged');
    expect(within(items[0]).queryByTestId('work-pr-session')).toBeNull();
    expect(within(items[0]).getByTestId('work-pr-session-gone').textContent).toContain('gone-one');
  });

  it('says so when there is nothing, and shows the error from an older hub', async () => {
    rows = [];
    render(WorkPrs);
    await flush();
    expect(screen.getByTestId('work-prs-empty').textContent).toContain('No open pull requests');
    vi.mocked(invoke).mockImplementation(async () => {
      throw { code: 'E_HUB', message: 'unknown tool prs' };
    });
    await fireEvent.click(screen.getByTestId('work-prs-filter-all'));
    await flush();
    expect(screen.getByTestId('work-prs-error').textContent).toContain('unknown tool prs');
  });

  it('labels', () => {
    expect(prStateLabel({ state: 'OPEN', draft: true })).toBe('Draft');
    expect(prStateLabel({ state: 'CLOSED' })).toBe('Closed');
    expect(prChecksLabel({ ci_status: 'failing', review_decision: 'CHANGES_REQUESTED' })).toBe('CI failing · changes requested');
    expect(prChecksLabel({})).toBe('');
    expect(prRef({ url: 'https://x/y', repo: null, number: null })).toBe('https://x/y');
  });

  it('is accessible', async () => {
    const { container } = render(WorkPrs);
    await flush();
    await expectAccessible(container);
  });
});

describe('WorkPrs with extra fleets (review w01)', () => {
  const primary: FleetInfo = {
    id: PRIMARY,
    label: 'home',
    url: null,
    color: '#5b8def',
    primary: true,
    local: true,
    client_name: null,
    connection: null,
    unavailable: null,
  };
  const company: FleetInfo = { ...primary, id: 'company', label: 'company', url: 'https://company.example.com', primary: false, local: false, color: '#e8833a', connection: { state: 'connected' } };
  const lab: FleetInfo = { ...company, id: 'lab', label: 'lab', url: 'https://lab.example.com' };

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    _resetFleetsForTest();
    sessions.set([]);
    _setFleetsForTest([primary, company, lab]);
  });
  afterEach(() => _resetFleetsForTest());

  it("w01-1: shows an extra fleet's mission as text, never opening the primary's mission of that id", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      if (cmd !== 'list_pull_requests') return null;
      const fleet = (raw as { fleet?: string }).fleet;
      if (fleet === 'company') return { items: [pr({ id: 9, mission_id: 7, mission_name: 'Company work' })], total: 1 };
      if (fleet === 'lab') return { items: [], total: 0 };
      return { items: [pr({ mission_id: 7, mission_name: 'Home work' })], total: 1 };
    });
    render(WorkPrs);
    await flush();
    expect(screen.getByTestId('work-pr-mission').textContent).toBe('Home work');
    expect(screen.getByTestId('work-pr-mission-text').textContent).toBe('Company work');
  });

  it('w01-7: says which fleet failed, and keeps the others when the primary fails', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      if (cmd !== 'list_pull_requests') return null;
      const fleet = (raw as { fleet?: string }).fleet;
      if (fleet === 'company') return { items: [pr({ id: 9, title: 'Company PR' })], total: 1 };
      if (fleet === 'lab') throw { code: 'E_HUB_UNREACHABLE', message: 'lab is down' };
      throw { code: 'E_HUB', message: 'home is down' };
    });
    render(WorkPrs);
    await flush();
    expect(screen.queryByTestId('work-prs-error')).toBeNull();
    expect(screen.getAllByTestId('work-pr')).toHaveLength(1);
    const notes = screen.getAllByTestId('work-prs-fleet-error').map((n) => n.textContent);
    expect(notes).toEqual([expect.stringContaining('from home: home is down'), expect.stringContaining('from lab: ')]);
  });

  it("w01-7: re-reads when an extra fleet's session opens a PR", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'list_pull_requests' ? { items: [], total: 0 } : null));
      render(WorkPrs);
      await flush();
      const before = calls().length;
      _setFleetRowsForTest('company', { ...emptyRows(), sessions: new Map([[3, session('mac', 'pos', { id: 3, pr_url: 'https://github.com/o/r/pull/1' })]]) });
      await vi.advanceTimersByTimeAsync(600);
      expect(calls().length).toBeGreaterThan(before);
    } finally {
      vi.useRealTimers();
    }
  });
});
