// Work › Pull requests (redesign 6.4): the PRs sessions opened, filtered by
// state, each opening on GitHub and naming the session that opened it.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./open_external', () => ({ openExternal: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import { openExternal } from './open_external';
import WorkPrs from './WorkPrs.svelte';
import { expectAccessible } from './a11y_check';
import { sessions } from './sessions';
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
