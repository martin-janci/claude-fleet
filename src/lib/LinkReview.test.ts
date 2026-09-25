// Work graph M4.4: the batch review of link suggestions (pill, sheet, j/k and
// y/n) and the Undo toast of an automatic link.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import LinkReview from './LinkReview.svelte';
import { sessions, sessionsLoaded, type SessionRow, type SessionWork } from './sessions';
import { session } from './hosts_fixture';
import { toasts, runToastAction } from './toasts';
import { describeEvidence, newAutoLinks, autoLinkSnapshot } from './work';
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

  it('j from a focused button still moves the cursor', async () => {
    sessions.set(rows());
    render(LinkReview);
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    const close = screen.getByTestId('link-review-close');
    close.focus();
    const j = new KeyboardEvent('keydown', { key: 'j', bubbles: true, cancelable: true });
    close.dispatchEvent(j);
    await tick();
    expect(j.defaultPrevented).toBe(true);
    // Enter on the sheet now decides row 1, the cursor row.
    await fireEvent.keyDown(screen.getByTestId('link-review-sheet'), { key: 'Enter' });
    expect(invoke).toHaveBeenLastCalledWith('confirm_session_work', {
      args: { session_id: 2, link_id: 12 },
    });
  });

  it('Enter/Backspace/y/n from a focused button are not sheet chords', async () => {
    sessions.set(rows());
    render(LinkReview);
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    const chord = (key: string) =>
      new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
    // Cursor is on row 0. Enter on row 1's "Not this" must reach that button
    // (a bubbling keydown, like a real key press), not confirm row 0.
    const notThis = screen.getAllByTestId('link-review-no')[1];
    notThis.focus();
    for (const key of ['Enter', 'Backspace', 'y', 'n']) {
      const ev = chord(key);
      notThis.dispatchEvent(ev);
      expect(ev.defaultPrevented).toBe(false);
    }
    // Enter on close must close, not confirm.
    const close = screen.getByTestId('link-review-close');
    close.focus();
    const onClose = chord('Enter');
    close.dispatchEvent(onClose);
    expect(onClose.defaultPrevented).toBe(false);
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('confirm_session_work', expect.anything());
    expect(invoke).not.toHaveBeenCalledWith('reject_session_work', expect.anything());
    expect(screen.getByTestId('link-review-sheet')).toBeTruthy();
    // Escape from a focused button still closes the sheet.
    close.focus();
    const onEsc = chord('Escape');
    close.dispatchEvent(onEsc);
    await tick();
    expect(onEsc.defaultPrevented).toBe(true);
    expect(screen.queryByTestId('link-review-sheet')).toBeNull();
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    // From the sheet itself the chord still decides the cursor row.
    await fireEvent.keyDown(screen.getByTestId('link-review-sheet'), { key: 'Enter' });
    expect(invoke).toHaveBeenLastCalledWith('confirm_session_work', {
      args: { session_id: 1, link_id: 11 },
    });
  });

  it('a decided row leaves the list, the cursor stays on a row, and the sheet closes itself when nothing is left', async () => {
    sessions.set(rows());
    // Each decision answers the row without its suggestion (a next one, if
    // any, would ride in on the same row update).
    vi.mocked(invoke).mockImplementation(async (_cmd: string, a?: unknown) => {
      const id = (a as { args: { session_id: number } }).args.session_id;
      return { ...rows().find((r) => r.id === id)!, work_suggested: null };
    });
    render(LinkReview);
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    const sheet = screen.getByTestId('link-review-sheet');
    // Cursor on the second row (session 2): reject it.
    await fireEvent.keyDown(sheet, { key: 'j' });
    await fireEvent.keyDown(sheet, { key: 'n' });
    expect(invoke).toHaveBeenLastCalledWith('reject_session_work', {
      args: { session_id: 2, link_id: 12 },
    });
    await waitFor(() => expect(screen.getAllByTestId('link-review-row')).toHaveLength(1));
    const left = screen.getByTestId('link-review-row');
    expect(left.dataset.sessionId).toBe('1');
    // The cursor was on index 1, which no longer exists: it is clamped onto
    // the remaining row, so the next chord decides that one and not nothing.
    expect(left.classList.contains('cursor')).toBe(true);
    expect(screen.getByTestId('link-review-pill')).toHaveTextContent('1 link suggestion · Review');
    await fireEvent.keyDown(sheet, { key: 'y' });
    expect(invoke).toHaveBeenLastCalledWith('confirm_session_work', {
      args: { session_id: 1, link_id: 11 },
    });
    await waitFor(() => expect(screen.queryByTestId('link-review-sheet')).toBeNull());
    expect(screen.queryByTestId('link-review-pill')).toBeNull();
  });

  it('a decision the hub refuses is a toast, and the row stays to be decided again', async () => {
    sessions.set(rows());
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'confirm_session_work') throw { code: 'E_HUB', message: 'hub unreachable' };
      return null;
    });
    render(LinkReview);
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('link-review-sheet'), { key: 'y' });
    await waitFor(() => expect(get(toasts).map((t) => t.message)).toEqual([expect.stringMatching(/^Confirm failed: hub unreachable/)]));
    expect(screen.getAllByTestId('link-review-row')).toHaveLength(2);
    expect(screen.getByTestId('link-review-sheet')).toBeTruthy();
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

  it('says what a decision did, and what the session asks next', async () => {
    sessions.set(rows());
    render(LinkReview);
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await tick();
    // Confirming ABC-1 leaves the same session with its next suggestion:
    // without a word, the row only changes its key and looks untouched.
    vi.mocked(invoke).mockImplementationOnce(async () =>
      session('h', 'a', {
        id: 1,
        status: 'running',
        work: { ...sg(11, 'ABC-1'), state: 'confirmed', source: 'manual' },
        work_suggested: { ...sg(13, 'ABC-9'), suggestions: 2 },
      }),
    );
    await fireEvent.click(screen.getAllByTestId('link-review-yes')[0]);
    await vi.waitFor(() => expect(get(toasts)).toHaveLength(1));
    expect(get(toasts)[0].message).toBe('Linked a → ABC-1 · next: ABC-9? (2 left)');
    expect(screen.getAllByTestId('link-review-row')[0].textContent).toContain('ABC-9?');

    vi.mocked(invoke).mockImplementationOnce(async () => session('h', 'b', { id: 2, status: 'running' }));
    await fireEvent.click(screen.getAllByTestId('link-review-no')[1]);
    await vi.waitFor(() => expect(get(toasts)).toHaveLength(2));
    expect(get(toasts)[1].message).toBe('b: not ABC-2');
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
