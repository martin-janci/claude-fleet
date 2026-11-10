// Review notes on the diff (docs/review-notes.md): drafts stay on the device,
// Send types them as one prompt with an id a retry reuses, sent notes show
// who sent them and a person resolves them.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as realInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import ReviewDiff from './ReviewDiff.svelte';
import { sessionIdentity, type SessionRow } from './sessions';
import {
  noteDrafts,
  ordered,
  pendingSend,
  resetNoteDraftsForTest,
  saveDraft,
  sendFailureText,
  type ReviewNote,
} from './review_notes';

const invoke = realInvoke as unknown as ReturnType<typeof vi.fn>;
const session = { id: 7, tmux_name: 'api', host_alias: 'local', kind: 'work', created_at: 100 } as unknown as SessionRow;
const K = sessionIdentity(session);
const DIFF = '@@ -1,2 +1,2 @@\n ctx\n-old line\n+new line\n';

let sent: ReviewNote[] = [];
let calls: Record<string, unknown>[] = [];
let failNext = false;
/** The next send answers this error instead. */
let refuseNext: { code: string; message: string } | null = null;
/** The next send waits for this to resolve. */
let holdNext: Promise<void> | null = null;
/** The next send answers `repeat` with these notes. */
let repeatNext: ReviewNote[] | null = null;

const note = (over: Partial<ReviewNote>): ReviewNote => ({
  id: 1, session_id: 7, batch: 'b', seq: 1, path: 'src/a.ts', line: 2, side: 'new', quote: 'new line', body: 'Cap this',
  author: 'Ada · desktop', created_at: 1, ...over,
});

async function typeNote(lineIndex: number, body: string): Promise<void> {
  await fireEvent.click(screen.getAllByTestId('diff-note-line')[lineIndex]);
  await fireEvent.input(screen.getByTestId('review-note-text'), { target: { value: body } });
  await fireEvent.click(screen.getByTestId('review-note-save'));
}

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
  refuseNext = null;
  holdNext = null;
  repeatNext = null;
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, a?: { args: Record<string, unknown> }) => {
    if (cmd !== 'review_notes' || !a) return null;
    calls.push(a.args);
    const action = a.args.action ?? 'list';
    if (action === 'list') return sent;
    if (action === 'send') {
      if (holdNext) {
        const h = holdNext;
        holdNext = null;
        await h;
      }
      if (failNext) {
        failNext = false;
        throw { code: 'E_HUB_TIMEOUT', message: 'timed out' };
      }
      if (refuseNext) {
        const e = refuseNext;
        refuseNext = null;
        throw e;
      }
      if (repeatNext) {
        const notes = repeatNext;
        repeatNext = null;
        return { session_id: 7, notes, delivered: false, repeat: true };
      }
      const notes = (a.args.notes as { path: string; line: number; side: 'new' | 'old'; quote?: string; body: string }[]).map((n, i) => ({
        id: i + 1, session_id: 7, batch: a.args.client_msg_id as string, seq: i + 1, path: n.path, line: n.line,
        side: n.side, quote: n.quote, body: n.body, author: 'Ada · desktop', created_at: 1,
      }));
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
    expect(get(noteDrafts(K))).toHaveLength(1);
    expect(screen.getByTestId('review-notes-drafts').textContent).toContain('1 note not sent');

    // The first send times out: its id is kept, and the retry reuses it.
    failNext = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error')).toBeTruthy());
    expect(get(noteDrafts(K))).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(K))).toHaveLength(0));
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
    expect(get(noteDrafts(K))[0]).toMatchObject({ side: 'old', line: 2, quote: 'old line' });
    saveDraft(K, { ...get(noteDrafts(K))[0], body: '  ' });
    expect(get(noteDrafts(K))).toHaveLength(0);
  });

  it('keeps the send id across a remount for the same notes, and mints a new one for changed notes', async () => {
    const first = render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await typeNote(2, 'Cap this');
    failNext = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error')).toBeTruthy());
    first.unmount();
    // A new view (FilesPanel remounts) still holds the id.
    resetNoteDraftsForTest(); // and a reload: it comes back from storage
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    expect(get(pendingSend(K))).toMatchObject({ inFlight: false });
    failNext = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(calls.filter((c) => c.action === 'send')).toHaveLength(2));
    // Another note: the notes changed, so the send is a new one.
    await typeNote(1, 'Why?');
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(K))).toHaveLength(0));
    const ids = calls.filter((c) => c.action === 'send').map((c) => c.client_msg_id);
    expect(ids[0]).toBe(ids[1]);
    expect(ids[2]).not.toBe(ids[1]);
    expect(get(pendingSend(K))).toBeNull();
  });

  it('shows a send in flight in a second view of the session, which cannot send it again', async () => {
    let release!: () => void;
    holdNext = new Promise((r) => (release = r));
    saveDraft(K, { path: 'src/a.ts', line: 2, side: 'new', quote: 'new line', body: 'Cap this' });
    const a = render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await fireEvent.click(a.getByTestId('review-notes-send'));
    const b = render(ReviewDiff, { session, path: 'src/b.ts', diff: DIFF });
    const sendB = b.getAllByTestId('review-notes-send').at(-1)!;
    expect(sendB.textContent).toBe('Sending…');
    expect((sendB as HTMLButtonElement).disabled).toBe(true);
    release();
    await waitFor(() => expect(get(noteDrafts(K))).toHaveLength(0));
    expect(calls.filter((c) => c.action === 'send')).toHaveLength(1);
  });

  it('on a repeat drops only the drafts that were in that send', async () => {
    saveDraft(K, { path: 'src/a.ts', line: 2, side: 'new', quote: 'new line', body: 'Cap this' });
    saveDraft(K, { path: 'src/a.ts', line: 2, side: 'old', quote: 'old line', body: 'Why?' });
    repeatNext = [note({ body: 'Cap this' })];
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(K)).map((d) => d.body)).toEqual(['Why?']));
  });

  it('says E_IN_FLIGHT and E_CONFLICT in words and keeps the drafts', async () => {
    saveDraft(K, { path: 'src/a.ts', line: 2, side: 'new', quote: 'new line', body: 'Cap this' });
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    refuseNext = { code: 'E_CONFLICT', message: 'E_CONFLICT: batch differs' };
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error').textContent).toContain('drafts are kept'));
    expect(get(noteDrafts(K))).toHaveLength(1);
    // A conflicting id is not reused.
    expect(get(pendingSend(K))).toBeNull();
    refuseNext = { code: 'E_IN_FLIGHT', message: 'E_IN_FLIGHT' };
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error').textContent).toContain('still being sent'));
    expect(get(noteDrafts(K))).toHaveLength(1);
    expect(sendFailureText({ code: 'E_HUB_TIMEOUT', message: 'timed out' })).not.toContain('E_HUB');
  });

  it('places a sent note only where the row reads as its quote, and lists the rest above the diff', async () => {
    sent = [
      note({ id: 1, body: 'on the line' }),
      note({ id: 2, line: 2, quote: 'something else', body: 'line changed' }),
      note({ id: 3, path: 'src/other.ts', body: 'other file' }),
      note({ id: 4, line: null, quote: null, body: 'whole file' }),
      note({ id: 5, line: 40, body: 'line gone' }),
    ];
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await waitFor(() => expect(screen.getAllByTestId('review-note-sent')).toHaveLength(1));
    expect(screen.getByTestId('review-note-sent').textContent).toContain('on the line');
    const elsewhere = screen.getAllByTestId('review-note-elsewhere');
    expect(elsewhere.map((e) => e.textContent)).toEqual([
      expect.stringContaining('src/a.ts:2'),
      expect.stringContaining('src/other.ts:2'),
      expect.stringContaining('whole file'),
      expect.stringContaining('src/a.ts:40'),
    ]);
    expect(elsewhere[0].textContent).toContain('something else');
    // Every one of them can be resolved.
    await fireEvent.click(elsewhere[1].querySelector('[data-testid="review-note-resolve"]')!);
    await waitFor(() => expect(calls.at(-1)).toMatchObject({ action: 'resolve', note_id: 3 }));
    await waitFor(() => expect(screen.getAllByTestId('review-note-elsewhere')[1].textContent).toContain('Resolved by Martin'));
    expect(screen.getByTestId('review-notes-open').textContent).toContain('4 sent');
  });

  it('lists once per session, not on every new row object, and keys drafts by the session identity', async () => {
    saveDraft(K, { path: 'src/a.ts', line: 2, side: 'new', quote: 'new line', body: 'old session' });
    const v = render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await waitFor(() => expect(calls.filter((c) => !c.action)).toHaveLength(1));
    expect(screen.getByTestId('review-notes-drafts')).toBeTruthy();
    await v.rerender({ session: { ...session }, path: 'src/a.ts', diff: DIFF });
    await Promise.resolve();
    expect(calls.filter((c) => !c.action)).toHaveLength(1);
    // A new session that reuses the id has none of the dead one's drafts.
    await v.rerender({ session: { ...session, created_at: 999 } as SessionRow, path: 'src/a.ts', diff: DIFF });
    expect(screen.queryByTestId('review-notes-drafts')).toBeNull();
  });

  it('keeps a typed note when another line is clicked, and caps notes as the hub does', async () => {
    render(ReviewDiff, { session, path: 'src/a.ts', diff: DIFF });
    await fireEvent.click(screen.getAllByTestId('diff-note-line')[2]);
    expect(screen.getByTestId('review-note-text').getAttribute('maxlength')).toBe('4000');
    await fireEvent.input(screen.getByTestId('review-note-text'), { target: { value: 'Typed, not saved' } });
    await fireEvent.click(screen.getAllByTestId('diff-note-line')[1]);
    expect(get(noteDrafts(K)).map((d) => d.body)).toEqual(['Typed, not saved']);
    // Clicking the saved line again edits that draft rather than adding one.
    await fireEvent.click(screen.getAllByTestId('diff-note-line')[2]);
    expect((screen.getByTestId('review-note-text') as HTMLTextAreaElement).value).toBe('Typed, not saved');

    for (let i = 0; i < 50; i++) saveDraft(K, { path: 'src/z.ts', line: i + 1, side: 'new', body: `n${i}` });
    await waitFor(() => expect((screen.getByTestId('review-notes-send') as HTMLButtonElement).disabled).toBe(true));
    expect(screen.getByTestId('review-notes-error').textContent).toContain('At most 50');
  });
});
