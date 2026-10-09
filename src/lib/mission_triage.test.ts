// Redesign 9.10: K3 mission triage. A stuck mission's card shows fleet's
// facts and Jev's proposals; a step button only hands the choice to the
// parent, and the card's words are drafted only on Draft. A mission that is
// not stuck shows no card.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import MissionTriage from './MissionTriage.svelte';
import MissionNudge from './MissionNudge.svelte';
import { asProposal, countsLine, outcomeLabel, resetTriageCacheForTests, stepLabel, type Triage } from './mission_triage';

const stuck: Triage = {
  stuck: {
    reason: 'failed',
    why: '1 task failed',
    done: 2,
    failed: 1,
    blocked: 0,
    total: 3,
    last_failure: 'tests fail on token expiry',
  },
  outcome: { feature: 'mission_triage', value: 'partial', source: 'jev', reason: '1 task failed', confidence_pct: 82 },
  next: { feature: 'mission_triage', value: 'retry', source: 'jev', reason: '1 task failed', confidence_pct: 77 },
  may_change: true,
};

async function flush() {
  for (let i = 0; i < 6; i++) await tick();
}

function triageCalls(): { mission_id: number; refresh: boolean }[] {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === 'mission_triage')
    .map((c) => (c[1] as { args: { mission_id: number; refresh: boolean } }).args);
}

describe('mission triage helpers', () => {
  it('words the counts, steps and outcomes', () => {
    expect(countsLine(stuck.stuck!)).toBe('2 done, 1 failed of 3');
    expect(countsLine({ ...stuck.stuck!, failed: 0, blocked: 2 })).toBe('2 done, 2 blocked of 3');
    expect(stepLabel('give_up')).toBe('Give up');
    expect(outcomeLabel('done')).toBe('Done');
    expect(outcomeLabel('blocked')).toBe('Needs you · blocked');
    expect(asProposal(null)).toBeNull();
    expect(asProposal(stuck.next)).toEqual({ value: 'retry', source: 'jev', reason: '1 task failed', confidence_pct: 77 });
  });
});

describe('MissionTriage', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('shows no card for a mission that is not stuck', async () => {
    vi.mocked(invoke).mockImplementation(async (c) => (c === 'mission_triage' ? { may_change: true } : null));
    render(MissionTriage, { missionId: 4, onstep: vi.fn() });
    await flush();
    expect(screen.queryByTestId('mission-triage')).toBeNull();
    expect(triageCalls()).toEqual([{ mission_id: 4, refresh: false }]);
  });

  it("shows fleet's facts and Jev's proposals, and a step only goes to the parent", async () => {
    vi.mocked(invoke).mockImplementation(async (c) => (c === 'mission_triage' ? stuck : null));
    const onstep = vi.fn();
    render(MissionTriage, { missionId: 4, onstep });
    await flush();
    expect(screen.getByTestId('mission-triage-why').textContent).toBe('1 task failed');
    expect(screen.getByTestId('mission-triage-counts').textContent).toBe('2 done, 1 failed of 3');
    expect(screen.getByTestId('mission-triage-outcome').textContent).toBe('Outcome so far: Needs you · partly done');
    expect(screen.getByTestId('mission-triage-next-by').textContent).toContain('Proposed by Jev');
    expect(screen.getByTestId('mission-triage-step-retry').dataset.proposed).toBe('true');
    expect(screen.getByTestId('mission-triage-step-split').dataset.proposed).toBe('false');
    await fireEvent.click(screen.getByTestId('mission-triage-step-give_up'));
    expect(onstep).toHaveBeenCalledWith('give_up');
    // Opening the card ran no draft and nothing but the triage read.
    expect(triageCalls()).toEqual([{ mission_id: 4, refresh: false }]);
    expect(vi.mocked(invoke).mock.calls.every((c) => c[0] === 'mission_triage')).toBe(true);
  });

  it('pre-selects nothing when Jev is unsure or off', async () => {
    vi.mocked(invoke).mockImplementation(async (c) =>
      c === 'mission_triage' ? { ...stuck, outcome: null, next: { ...stuck.next!, confidence_pct: 30 } } : null,
    );
    render(MissionTriage, { missionId: 4, onstep: vi.fn() });
    await flush();
    expect(screen.queryByTestId('mission-triage-outcome')).toBeNull();
    expect(screen.queryByTestId('mission-triage-next-by')).toBeNull();
    for (const s of ['retry', 'split', 'give_up', 'ask']) {
      expect(screen.getByTestId(`mission-triage-step-${s}`).dataset.proposed).toBe('false');
    }
  });

  it('drafts the card only on Draft', async () => {
    vi.mocked(invoke).mockImplementation(async (c, a) => {
      if (c !== 'mission_triage') return null;
      const refresh = (a as { args: { refresh: boolean } }).args.refresh;
      return refresh
        ? { ...stuck, card: { text: 'One task failed; retry it.', model: 'haiku', host_alias: 'mercury', from: '3 tasks and the stuck reason', at: 1 } }
        : stuck;
    });
    render(MissionTriage, { missionId: 4, onstep: vi.fn() });
    await flush();
    await fireEvent.click(screen.getByTestId('mission-triage-draft'));
    await flush();
    expect(triageCalls()).toEqual([
      { mission_id: 4, refresh: false },
      { mission_id: 4, refresh: true },
    ]);
    const input = screen.getByTestId('mission-triage-card-input') as HTMLTextAreaElement;
    expect(input.value).toBe('One task failed; retry it.');
  });

  it('a viewer who may not change the mission sees the facts without steps', async () => {
    vi.mocked(invoke).mockImplementation(async (c) => (c === 'mission_triage' ? { ...stuck, may_change: false } : null));
    render(MissionTriage, { missionId: 4, onstep: vi.fn() });
    await flush();
    expect(screen.getByTestId('mission-triage')).toBeTruthy();
    expect(screen.queryByTestId('mission-triage-step-retry')).toBeNull();
    expect(screen.queryByTestId('mission-triage-draft')).toBeNull();
  });
});

describe('MissionNudge', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    resetTriageCacheForTests();
  });

  it('lists the stuck missions among the open ones, with the proposed next step', async () => {
    vi.mocked(invoke).mockImplementation(async (c, a) => {
      if (c === 'work_missions')
        return [
          { id: 1, name: 'Ship login', state: 'active' },
          { id: 2, name: 'Refunds', state: 'paused' },
          { id: 3, name: 'Old', state: 'completed' },
        ];
      if (c === 'mission_triage') {
        const id = (a as { args: { mission_id: number } }).args.mission_id;
        return id === 1 ? stuck : { may_change: true };
      }
      return null;
    });
    render(MissionNudge);
    await flush();
    const rows = screen.getAllByTestId('mission-nudge-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('Ship login');
    expect(rows[0].textContent).toContain('1 task failed');
    expect(screen.getByTestId('mission-nudge-next').textContent).toBe('Next: Retry');
    // A finished mission is never asked about.
    expect(triageCalls().map((c) => c.mission_id)).toEqual([1, 2]);
    expect(triageCalls().every((c) => !c.refresh)).toBe(true);
  });

  it('review r15: asks Jev once per mission per state, across re-opens', async () => {
    let missions = [
      { id: 1, name: 'Ship login', state: 'active', version: 3, updated_at: 100 },
      { id: 2, name: 'Refunds', state: 'paused', version: 1, updated_at: 90 },
    ];
    vi.mocked(invoke).mockImplementation(async (c) => {
      if (c === 'work_missions') return missions;
      if (c === 'mission_triage') return stuck;
      return null;
    });
    const first = render(MissionNudge);
    await flush();
    first.unmount();
    render(MissionNudge);
    await flush();
    expect(triageCalls().map((c) => c.mission_id)).toEqual([1, 2]);
    // Mission 1 moved: only it is asked again.
    missions = [{ ...missions[0], version: 4, updated_at: 120 }, missions[1]];
    render(MissionNudge);
    await flush();
    expect(triageCalls().map((c) => c.mission_id)).toEqual([1, 2, 1]);
  });
});
