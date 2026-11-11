// Review notes on the diff (docs/review-notes.md): drafts stay on the device,
// Send types them as one prompt with an id a retry reuses, sent notes show
// who sent them and a person resolves them.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as realInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import ReviewDiff from './ReviewDiff.svelte';
import type { SessionRow } from './sessions';
import { clearDrafts, noteDrafts, ordered, resetNoteDraftsForTest, saveDraft, type ReviewNote } from './review_notes';

const invoke = realInvoke as unknown as ReturnType<typeof vi.fn>;
const session = { id: 7, tmux_name: 'api', host_alias: 'local', kind: 'work', created_at: 100 } as unknown as SessionRow;
const DIFF = '@@ -1,2 +1,2 @@\n ctx\n-old line\n+new line\n';

let sent: ReviewNote[] = [];
let calls: Record<string, unknown>[] = [];
let failNext = false;
/** The hub stores and types the batch, but its answer never comes. */
let storedThenFail = false;

beforeEach(() => {
  try {
    localStorage.clear();
  } catch {
    /* none */
  }
  resetNoteDraftsForTest();
  sent = [];
  calls = [];
  failNext = false;
  storedThenFail = false;
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, a?: { args: Record<string, unknown> }) => {
    if (cmd !== 'review_notes' || !a) return null;
    calls.push(a.args);
    const action = a.args.action ?? 'list';
    if (action === 'list') return sent;
    if (action === 'send') {
      if (failNext) {
        failNext = false;
        throw { code: 'E_HUB_TIMEOUT', message: 'timed out' };
      }
      // A batch the backend already stored: its notes again, nothing typed.
      if (sent.length > 0 && sent[0].batch === a.args.client_msg_id) return { session_id: 7, notes: sent, delivered: false, repeat: true };
      const notes = (a.args.notes as { path: string; line: number; side: 'new' | 'old'; body: string }[]).map((n, i) => ({
        id: i + 1, session_id: 7, batch: a.args.client_msg_id as string, seq: i + 1, path: n.path, line: n.line,
        side: n.side, body: n.body, author: 'Ada · desktop', created_at: 1,
      }));
      if (storedThenFail) {
        storedThenFail = false;
        sent = notes;
        throw { code: 'E_HUB_TIMEOUT', message: 'It may have gone through anyway', details: { outcome_unknown: true } };
      }
      sent = notes;
      return { session_id: 7, notes, delivered: true };
    }
    if (action === 'resolve') {
      const n = sent.find((x) => x.id === a.args.note_id)!;
      return { ...n, resolved_at: 2, resolved_by: 'Martin' };
    }
    return null;
  });
});

describe('review notes', () => {
  it('orders drafts by file and line, as the prompt numbers them', () => {
    const d = (path: string, line: number, side: 'new' | 'old' = 'new') => ({ id: `${path}${line}`, path, line, side, body: 'x' });
    expect(ordered([d('b.ts', 1), d('a.ts', 9), d('a.ts', 2)]).map((x) => x.id)).toEqual(['a.ts2', 'a.ts9', 'b.ts1']);
  });

  it('keeps a draft on the device, then sends every draft as one send', async () => {
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    const lines = screen.getAllByTestId('diff-note-line');
    // ctx (new 1), removed (old 2), added (new 2)
    expect(lines).toHaveLength(3);
    await fireEvent.click(lines[2]);
    await fireEvent.input(screen.getByTestId('review-note-text'), { target: { value: 'Cap this' } });
    await fireEvent.click(screen.getByTestId('review-note-save'));
    expect(screen.getByTestId('review-note-draft').textContent).toContain('Cap this');
    expect(get(noteDrafts(session))).toHaveLength(1);
    expect(screen.getByTestId('review-notes-drafts').textContent).toContain('1 note not sent');

    // The first send times out: its id is kept, and the retry reuses it.
    failNext = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error')).toBeTruthy());
    expect(get(noteDrafts(session))).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(session))).toHaveLength(0));
    const sends = calls.filter((c) => c.action === 'send');
    expect(sends).toHaveLength(2);
    expect(sends[0].client_msg_id).toBe(sends[1].client_msg_id);
    expect(sends[1].notes).toEqual([{ path: 'src/a.ts', line: 2, side: 'new', quote: 'new line', body: 'Cap this' }]);

    await waitFor(() => expect(screen.getByTestId('review-note-sent').textContent).toContain('Ada · desktop'));
    await fireEvent.click(screen.getByTestId('review-note-resolve'));
    await waitFor(() => expect(screen.getByTestId('review-note-sent').textContent).toContain('Resolved by Martin'));
    expect(calls.at(-1)).toEqual({ session_id: 7, action: 'resolve', note_id: 1, resolved: true });
  });

  it('notes a removed line on its old side, and an empty edit drops the draft', async () => {
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await fireEvent.click(screen.getAllByTestId('diff-note-line')[1]);
    await fireEvent.input(screen.getByTestId('review-note-text'), { target: { value: 'Why?' } });
    await fireEvent.click(screen.getByTestId('review-note-save'));
    expect(get(noteDrafts(session))[0]).toMatchObject({ side: 'old', line: 2, quote: 'old line' });
    saveDraft(session, { ...get(noteDrafts(session))[0], body: '  ' });
    expect(get(noteDrafts(session))).toHaveLength(0);
    clearDrafts(session);
  });

  it('a retry after a lost answer clears only the notes that send carried; ones added or edited since stay', async () => {
    saveDraft(session, { id: 'n1', path: 'src/a.ts', line: 1, side: 'new', body: 'one' });
    saveDraft(session, { id: 'n2', path: 'src/a.ts', line: 2, side: 'new', body: 'two' });
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    storedThenFail = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error')).toBeTruthy());
    // Since then: note 2 fixed, note 3 added.
    saveDraft(session, { id: 'n2', path: 'src/a.ts', line: 2, side: 'new', body: 'two, fixed' });
    saveDraft(session, { id: 'n3', path: 'src/a.ts', line: 1, side: 'old', body: 'three' });
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(session)).map((d) => d.id).sort()).toEqual(['n2', 'n3']));
    const sends = calls.filter((c) => c.action === 'send');
    expect(sends[0].client_msg_id).toBe(sends[1].client_msg_id);
    // The next send is a new batch, and it goes through.
    await waitFor(() => expect((screen.getByTestId('review-notes-send') as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(session))).toHaveLength(0));
    const third = calls.filter((c) => c.action === 'send')[2];
    expect(third.client_msg_id).not.toBe(sends[0].client_msg_id);
    expect((third.notes as { body: string }[]).map((n) => n.body).sort()).toEqual(['three', 'two, fixed']);
  });

  it('a pending send survives a remount of the viewer', async () => {
    saveDraft(session, { id: 'n1', path: 'src/a.ts', line: 1, side: 'new', body: 'one' });
    const first = render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    storedThenFail = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error')).toBeTruthy());
    first.unmount();
    resetNoteDraftsForTest();
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(session))).toHaveLength(0));
    const sends = calls.filter((c) => c.action === 'send');
    expect(sends).toHaveLength(2);
    expect(sends[1].client_msg_id).toBe(sends[0].client_msg_id);
  });

  it('drafts written for a session are not another session’s that later got its id', () => {
    saveDraft(session, { id: 'n1', path: 'src/a.ts', line: 1, side: 'new', body: 'about the old worktree' });
    resetNoteDraftsForTest();
    const reused = { ...session, tmux_name: 'web', created_at: 200 } as SessionRow;
    expect(get(noteDrafts(reused))).toHaveLength(0);
    // The session they were written for still has them.
    expect(get(noteDrafts(session)).map((d) => d.body)).toEqual(['about the old worktree']);
    // Another fleet's session with the same id and name is not this one either.
    expect(get(noteDrafts({ ...session, fleet: 'company' } as SessionRow))).toHaveLength(0);
  });
});
