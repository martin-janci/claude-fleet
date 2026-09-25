// Work graph M4.4: the batch review of link suggestions (pill, sheet, j/k and
// y/n) and the Undo toast of an automatic link.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import LinkReview from './LinkReview.svelte';
import { sessions, sessionsLoaded, type SessionRow, type SessionWork } from './sessions';
import { session } from './hosts_fixture';
import { toasts, runToastAction } from './toasts';
import { describeEvidence, newAutoLinks, autoLinkSnapshot, workWhy } from './work';
import { sessionFocus } from './session_focus';
import { selectedSession, selectSession } from './selection';

const sg = (link_id: number, key: string): SessionWork => ({
  link_id,
  item_id: null,
  key,
  title: '',
  source: 'branch',
  state: 'suggested',
  rule: 'R3b',
});

const rows = (): SessionRow[] => [
  session('h', 'a', { id: 1, status: 'running', work_suggested: sg(11, 'ABC-1') }),
  session('h', 'b', { id: 2, status: 'running', work_suggested: sg(12, 'ABC-2') }),
  session('h', 'c', { id: 3, status: 'running' }),
];

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async () => rows()[0]);
  toasts.set([]);
  sessionsLoaded.set(false);
  sessions.set([]);
  sessionFocus.set(null);
  selectSession(null);
});

describe('LinkReview', () => {
  it('has no pill when nothing is suggested', () => {
    sessions.set([session('h', 'c', { id: 3, status: 'running' })]);
    render(LinkReview);
    expect(screen.queryByTestId('link-review-pill')).toBeNull();
  });

  it('reviews from the keyboard: j moves, y confirms, n rejects', async () => {
    sessions.set(rows());
    render(LinkReview);
    const pill = screen.getByTestId('link-review-pill');
    expect(pill.textContent).toContain('2 link suggestions · Review');
    await fireEvent.click(pill);
    await tick();
    const sheet = screen.getByTestId('link-review-sheet');
    expect(screen.getAllByTestId('link-review-row')).toHaveLength(2);
    await fireEvent.keyDown(sheet, { key: 'y' });
    expect(invoke).toHaveBeenLastCalledWith('confirm_session_work', {
      args: { session_id: 1, link_id: 11 },
    });
    await tick();
    await fireEvent.keyDown(sheet, { key: 'j' });
    await fireEvent.keyDown(sheet, { key: 'n' });
    expect(invoke).toHaveBeenLastCalledWith('reject_session_work', {
      args: { session_id: 2, link_id: 12 },
    });
    await fireEvent.keyDown(sheet, { key: 'Escape' });
    await tick();
    expect(screen.queryByTestId('link-review-sheet')).toBeNull();
  });

  it('clicking a suggestion shows only that session; closing the sheet lifts it', async () => {
    sessions.set(rows());
    render(LinkReview);
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    await fireEvent.click(screen.getAllByTestId('link-review-row')[1]);
    expect(get(sessionFocus)).toEqual({ id: 2, label: 'b' });
    expect(get(selectedSession)?.id).toBe(2);
    // y now decides the clicked row.
    await fireEvent.keyDown(screen.getByTestId('link-review-sheet'), { key: 'y' });
    expect(invoke).toHaveBeenLastCalledWith('confirm_session_work', {
      args: { session_id: 2, link_id: 12 },
    });
    await fireEvent.click(screen.getByTestId('link-review-close'));
    await tick();
    expect(get(sessionFocus)).toBeNull();
    // The session stays open in the center pane.
    expect(get(selectedSession)?.id).toBe(2);
  });

  it('toasts a new automatic link with an Undo that rejects it', async () => {
    const base = session('h', 'a', { id: 1, status: 'running', friendly_name: 'blue-sirius' });
    sessions.set([base]);
    sessionsLoaded.set(true);
    render(LinkReview);
    sessions.set([
      {
        ...base,
        work: {
          link_id: 21, item_id: null, key: 'ABC-123', title: '', source: 'branch',
          state: 'confirmed', strength: 'strong', rule: 'R3',
        },
      },
    ]);
    await tick();
    const t = get(toasts);
    expect(t).toHaveLength(1);
    expect(t[0].message).toBe('Linked blue-sirius → ABC-123 (branch)');
    expect(t[0].action?.label).toBe('Undo');
    runToastAction(t[0].id);
    expect(invoke).toHaveBeenCalledWith('reject_session_work', {
      args: { session_id: 1, link_id: 21 },
    });
  });
});

describe('work helpers', () => {
  it('explains evidence lines', () => {
    const at = new Date(2026, 8, 24, 9, 5).getTime() / 1000;
    expect(describeEvidence({ signal: 'branch', rule: 'R3', text: 'abc-123-login', at })).toBe(
      'branch `abc-123-login` since 09:05 · R3',
    );
    expect(
      describeEvidence({ signal: 'prompt_key', rule: 'R6', text: 'ABC-99', at, note: 'reference' }),
    ).toBe('mentioned ABC-99 in a prompt at 09:05 (reference) · R6');
    expect(describeEvidence({ signal: 'agent_inferred', rule: 'R11', text: 'ABC-1', at })).toBe(
      'Claude guessed ABC-1 when asked at 09:05 · R11',
    );
  });

  it("an agent's guess is never an automatic link", () => {
    const r = session('h', 'a', {
      id: 1,
      status: 'running',
      work: { link_id: 1, item_id: null, key: 'A-1', title: '', source: 'agent_inferred', state: 'confirmed' },
    });
    expect(newAutoLinks(new Map(), [r])).toEqual([]);
    expect(workWhy({ source: 'agent_inferred', state: 'suggested', rule: 'R11' })).toBe(
      "Claude's guess · rule R11",
    );
  });

  it('a manual link is never an automatic one', () => {
    const r = session('h', 'a', {
      id: 1,
      status: 'running',
      work: { link_id: 1, item_id: null, key: 'A-1', title: '', source: 'manual', state: 'confirmed' },
    });
    expect(newAutoLinks(new Map(), [r])).toEqual([]);
    expect(autoLinkSnapshot([r]).size).toBe(0);
  });
});
