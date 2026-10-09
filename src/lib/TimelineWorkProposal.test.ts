// Redesign 6.8: the J1 suggested link at the head of the Details timeline.
// Shown only for the decision model's own suggestion (rule R12), which the
// backend writes only in assist mode; a rule's suggestion shows
// nothing here. Link / Not this are a person's clicks.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TimelineWorkProposal from './TimelineWorkProposal.svelte';
import { session } from './hosts_fixture';
import { linkProposal, type WorkLink } from './work';
import type { SessionRow, SessionWork } from './sessions';

const jev: SessionWork = {
  link_id: 31,
  item_id: 5,
  key: 'TASK-219',
  title: 'Fix the pairing flake',
  source: 'jev',
  state: 'suggested',
  strength: 'inferred',
  rule: 'R12',
  preselected: true,
};
const link: WorkLink = {
  id: 31,
  ref_key: 'TASK-219',
  state: 'suggested',
  source: 'jev',
  rule: 'R12',
  created_at: 1_790_000_000,
  evidence: [{ signal: 'jev', rule: 'R12', text: 'TASK-219', note: '82%', at: 1_790_000_000 }],
};
const row = (over: Partial<SessionRow> = {}): SessionRow =>
  session('mac', 'fix-flake', { id: 7, status: 'running', work_suggested: jev, ...over });

describe('J1 in the Details timeline (6.8)', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'session_work_links' ? [link] : null));
  });

  it('reads the proposer from an R12 link, with the confidence its evidence holds', () => {
    expect(linkProposal(link)).toEqual({
      value: 'TASK-219',
      source: 'jev',
      reason: 'from the first prompt',
      confidence_pct: 82,
    });
    expect(linkProposal({ ...link, rule: 'R5' })).toBeNull();
  });

  it('shows the suggestion with ✦, "Proposed by Jev" and its reason; Link confirms by link id', async () => {
    render(TimelineWorkProposal, { session: row() });
    const box = screen.getByTestId('timeline-work-proposal');
    expect(box.dataset.state).toBe('suggested');
    expect(screen.getByTestId('timeline-work-chip').dataset.proposed).toBe('jev');
    const by = await screen.findByTestId('timeline-proposed-by');
    expect(by.textContent).toContain('Proposed by Jev');
    expect(by.textContent).toContain('from the first prompt');
    expect(by.textContent).toContain('82%');
    expect(invoke).not.toHaveBeenCalledWith('confirm_session_work', expect.anything());
    await fireEvent.click(screen.getByTestId('timeline-work-link'));
    expect(invoke).toHaveBeenCalledWith('confirm_session_work', { args: { session_id: 7, link_id: 31 } });
  });

  it('Not this rejects that link', async () => {
    render(TimelineWorkProposal, { session: row() });
    await fireEvent.click(screen.getByTestId('timeline-work-reject'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('reject_session_work', { args: { session_id: 7, link_id: 31 } }));
  });

  it("keeps who proposed a link the person confirmed", async () => {
    render(TimelineWorkProposal, {
      session: row({ work_suggested: null, work: { ...jev, state: 'confirmed', preselected: false } }),
    });
    expect(screen.getByTestId('timeline-work-proposal').dataset.state).toBe('confirmed');
    expect(screen.getByTestId('timeline-work-proposal').textContent).toContain('you confirmed');
    expect(screen.queryByTestId('timeline-work-link')).toBeNull();
    expect((await screen.findByTestId('timeline-proposed-by')).textContent).toContain('Proposed by Jev');
  });

  it('a rule suggestion and a low-confidence answer show nothing of Jev', async () => {
    const { unmount } = render(TimelineWorkProposal, { session: row({ work_suggested: { ...jev, source: 'prompt', rule: 'R5' } }) });
    expect(screen.queryByTestId('timeline-work-proposal')).toBeNull();
    unmount();
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'session_work_links' ? [{ ...link, evidence: [{ ...link.evidence![0], note: '30%' }] }] : null,
    );
    const low = render(TimelineWorkProposal, { session: row() });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('session_work_links', { args: { session_id: 7 } }));
    expect(screen.queryByTestId('timeline-proposed-by')).toBeNull();
    low.unmount();
  });
});
