// A tracker's extras on the generated Trackers page (declarative pages P4b,
// a registered custom item): its last sync pass (work graph M11.4, M13.1),
// Asana's section map (M6) and Jev's proposals (J3, assist) — the cases the
// hand-written WorkSettings held, for one tracker.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TrackerExtras from './TrackerExtras.svelte';
import { type TrackerRow, type TrackerProposals } from './trackers';
import { toasts } from './toasts';

const row = (over: Partial<TrackerRow> = {}): TrackerRow => ({
  id: 4,
  provider: 'jira',
  name: 'acme',
  site_url: 'https://acme.atlassian.net',
  state: 'ok',
  created_at: 1,
  last_sync_at: 1000,
  has_credential: true,
  username: 'me@acme.com',
  credential_hint: '…oken',
  config: { key_prefixes: ['ABC'] },
  ...over,
});

function route(listed: TrackerRow[], extra: Record<string, unknown> = {}) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd in extra) return extra[cmd];
    if (cmd === 'list_trackers') return listed;
    return null;
  });
  return inv;
}

const settle = async () => {
  for (let i = 0; i < 5; i++) await tick();
};

beforeEach(() => toasts.set([]));

describe('TrackerExtras: the last sync pass', () => {
  it('shows the tracker’s last pass', async () => {
    const inv = route([row()], {
      tracker_sync_metrics: [
        { tracker_id: 4, last_pass_at: 1000, duration_ms: 1234, items_listed: 40, items_changed: 3, frames_emitted: 12, last_error: null },
        { tracker_id: 9, last_pass_at: 1000, duration_ms: 5 },
      ],
    });
    render(TrackerExtras, { props: { tracker: row() } });
    await waitFor(() =>
      expect(screen.getByTestId('tracker-metrics').textContent).toContain('last pass 1.2 s · 40 listed · 3 changed · 12 frames'),
    );
    expect(inv.mock.calls.some((c) => c[0] === 'tracker_sync_metrics')).toBe(true);
  });

  it('shows the items a pass skipped and why (M13.1), and a failed pass’s error', async () => {
    route([row()], {
      tracker_sync_metrics: [
        { tracker_id: 4, last_pass_at: 1000, duration_ms: 80, items_failed: 1, consecutive_partial: 2, last_item_error: 'UNIQUE constraint failed', last_error: 'the tracker could not be reached' },
      ],
    });
    render(TrackerExtras, { props: { tracker: row() } });
    await waitFor(() => expect(screen.getByTestId('tracker-metrics-skipped')).toBeInTheDocument());
    expect(screen.getByTestId('tracker-metrics').textContent).toContain('1 skipped (2 passes in a row)');
    expect(screen.getByTestId('tracker-metrics-skipped').textContent).toContain('UNIQUE constraint failed');
    expect(screen.getByTestId('tracker-metrics-error').textContent).toContain('could not be reached');
  });
});

describe('TrackerExtras: Asana sections', () => {
  it('asks which Asana sections mean in progress, and saves the answer as confirmed', async () => {
    const asana = row({
      id: 8,
      provider: 'asana',
      name: 'Company B',
      site_url: 'https://app.asana.com',
      config: { section_map: { 'in progress': 'in_progress', shipped: 'done' } },
    });
    const inv = route([asana], { update_tracker: asana });
    render(TrackerExtras, { props: { tracker: asana } });
    await waitFor(() => expect(screen.getByTestId('asana-sections')).toBeInTheDocument());
    await fireEvent.change(screen.getByTestId('asana-section-shipped'), { target: { value: 'in_progress' } });
    await fireEvent.click(screen.getByTestId('asana-sections-confirm'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'update_tracker')).toBe(true));
    const up = inv.mock.calls.find((c) => c[0] === 'update_tracker')!;
    expect((up[1] as { args: Record<string, unknown> }).args).toMatchObject({
      tracker_id: 8,
      settings: { section_map: { 'in progress': 'in_progress', shipped: 'in_progress' }, section_map_confirmed: true },
    });
  });

  it('M15 G7.14: once confirmed, says how many columns are mapped and opens the map again', async () => {
    const asana = row({
      id: 8,
      provider: 'asana',
      name: 'Company B',
      site_url: 'https://app.asana.com',
      config: { section_map: { 'in progress': 'in_progress', shipped: 'done', ideas: 'todo' } },
      settings: { section_map: { 'in progress': 'in_progress', shipped: 'done' }, section_map_confirmed: true },
    });
    route([asana], { update_tracker: asana });
    render(TrackerExtras, { props: { tracker: asana } });
    expect((await screen.findByTestId('tracker-column-map')).textContent?.replace(/\s+/g, ' ').trim()).toBe(
      '2 columns mapped · Column map',
    );
    expect(screen.queryByTestId('asana-sections')).toBeNull();
    await fireEvent.click(screen.getByTestId('tracker-column-map-open'));
    expect(screen.getByTestId('asana-sections')).toBeInTheDocument();
    expect(screen.queryByTestId('tracker-column-map')).toBeNull();
  });
});

describe('TrackerExtras: Jev section proposals (status_map assist)', () => {
  const asana = (over: Partial<TrackerRow> = {}): TrackerRow =>
    row({
      id: 8,
      provider: 'asana',
      name: 'Company B',
      site_url: 'https://app.asana.com',
      username: null,
      config: { section_map: { 'in progress': 'in_progress', done: 'done' } },
      settings: { section_map: { 'in progress': 'in_progress', done: 'done' }, section_map_confirmed: true },
      ...over,
    });

  const proposals = (over: Partial<TrackerProposals> = {}): TrackerProposals[] => [
    {
      tracker_id: 8,
      name: 'Company B',
      org_id: 1,
      mode: 'assist',
      proposals: [
        {
          section: 'ideas',
          answer: 'todo',
          applies_as: 'todo',
          confidence: 0.82,
          run_id: 812,
          at: 1,
          top: [
            ['todo', 0.82],
            ['unsure', 0.11],
          ],
        },
        {
          section: 'parked <b>now</b>',
          answer: 'not_planned',
          applies_as: 'done',
          confidence: 0.7,
          run_id: 813,
          at: 1,
          top: [['not_planned', 0.7]],
        },
        { section: 'someday', answer: 'unsure', applies_as: null, confidence: 0.6, run_id: 814, at: 1 },
        // Already the person's: not shown.
        {
          section: 'backlog',
          answer: 'todo',
          applies_as: 'todo',
          run_id: 815,
          at: 1,
          person: 'todo',
          followup: 'confirmed',
        },
      ],
      shadow: [
        { section: 'in progress', rule: 'in_progress', model: 'in_progress', run_id: 700 },
        { section: 'done', rule: 'done', model: 'in_progress', run_id: 701 },
        { section: 'ideas', rule: 'none', model: 'todo', run_id: 702 },
      ],
      unknown_sections: 0,
      rejected: 0,
      ...over,
    },
  ];

  it('shows the pending proposals with their category, confidence and why, as plain text', async () => {
    route([asana()], { status_map_proposals: proposals() });
    render(TrackerExtras, { props: { tracker: asana() } });
    await waitFor(() => expect(screen.getAllByTestId('jev-proposal')).toHaveLength(3));
    expect(screen.getByTestId('jev-proposals').textContent).toContain('Proposed by Jev (assist)');
    const rows = screen.getAllByTestId('jev-proposal');
    expect(rows.map((r) => r.querySelector('[data-testid="jev-proposal-section"]')!.textContent)).toEqual([
      'ideas',
      'parked <b>now</b>',
      'someday',
    ]);
    // Third-party text is never markup.
    expect(rows[1].querySelector('b')).toBeNull();
    expect(rows[0].querySelector('[data-testid="jev-proposal-confidence"]')!.textContent).toBe('likely');
    expect(rows[0].querySelector('[data-testid="jev-proposal-why"]')!.textContent).toBe(
      'why: to do 0.82 · unsure 0.11',
    );
    expect(rows[1].querySelector('[data-testid="jev-proposal-category"]')!.textContent).toMatch(
      /not planned\s+\(applies as done\)/,
    );
    // Unsure proposes nothing: no Apply, but Apply as… and Not this.
    expect(rows[2].querySelector('[data-testid="jev-apply"]')).toBeNull();
    expect(rows[2].querySelector('[data-testid="jev-apply-as"]')).not.toBeNull();
    expect(rows[2].querySelector('[data-testid="jev-reject"]')).not.toBeNull();
    expect(screen.getByTestId('jev-shadow-agreement').textContent).toMatch(
      /agreed with the keyword rule on 1 of 2/,
    );
  });

  it('Apply, Apply as… and Not this name the run, then re-read the trackers and the proposals', async () => {
    const inv = route([asana()], {
      status_map_proposals: proposals(),
      decide_status_map_proposal: {
        run_id: 812,
        tracker_id: 8,
        section: 'ideas',
        action: 'apply',
        category: 'todo',
        followup: 'confirmed',
      },
    });
    render(TrackerExtras, { props: { tracker: asana() } });
    await waitFor(() => expect(screen.getAllByTestId('jev-proposal')).toHaveLength(3));
    const count = (cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd).length;
    const before = [count('list_trackers'), count('status_map_proposals')];

    await fireEvent.click(screen.getAllByTestId('jev-apply')[0]);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('decide_status_map_proposal', { args: { run_id: 812, action: 'apply' } }),
    );
    await waitFor(() => expect(count('status_map_proposals')).toBe(before[1] + 1));
    expect(count('list_trackers')).toBe(before[0] + 1);
    await waitFor(() =>
      expect(get(toasts).some((t) => t.kind === 'success' && t.message.includes('ideas'))).toBe(true),
    );

    await fireEvent.change(screen.getAllByTestId('jev-apply-as')[1], { target: { value: 'in_progress' } });
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('decide_status_map_proposal', {
        args: { run_id: 813, action: 'apply_as', category: 'in_progress' },
      }),
    );
    await waitFor(() => expect(count('status_map_proposals')).toBe(before[1] + 2));

    await fireEvent.click(screen.getAllByTestId('jev-reject')[2]);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('decide_status_map_proposal', { args: { run_id: 814, action: 'reject' } }),
    );
    await waitFor(() => expect(count('status_map_proposals')).toBe(before[1] + 3));
    // Nothing else wrote the tracker.
    expect(count('update_tracker')).toBe(0);
  });

  it('a failed decision is a toast', async () => {
    const inv = route([asana()]);
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_trackers') return [asana()];
      if (cmd === 'status_map_proposals') return proposals();
      if (cmd === 'decide_status_map_proposal')
        throw { code: 'E_INVALID_STATE', message: 'run 812 is not the latest proposal for this section (run 900 is)' };
      return null;
    });
    render(TrackerExtras, { props: { tracker: asana() } });
    await waitFor(() => expect(screen.getAllByTestId('jev-proposal')).toHaveLength(3));
    await fireEvent.click(screen.getAllByTestId('jev-reject')[0]);
    await waitFor(() =>
      expect(get(toasts).some((t) => t.kind === 'error' && t.message.includes('not the latest proposal'))).toBe(
        true,
      ),
    );
  });

  it('shows nothing outside assist, or when nothing is pending', async () => {
    route([asana()], { status_map_proposals: proposals({ mode: 'shadow' }) });
    const { unmount } = render(TrackerExtras, { props: { tracker: asana() } });
    await settle();
    expect(screen.queryByTestId('jev-proposals')).toBeNull();
    unmount();
    route([asana()], { status_map_proposals: proposals({ proposals: [] }) });
    render(TrackerExtras, { props: { tracker: asana() } });
    await settle();
    expect(screen.queryByTestId('jev-proposals')).toBeNull();
  });

  it('asks for no proposals for a tracker that is not Asana', async () => {
    const inv = route([row()], { status_map_proposals: proposals() });
    render(TrackerExtras, { props: { tracker: row() } });
    await settle();
    expect(inv.mock.calls.some((c) => c[0] === 'status_map_proposals')).toBe(false);
  });
});
