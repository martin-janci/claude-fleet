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
const session = { id: 7, tmux_name: 'api', host_alias: 'local', kind: 'work' } as unknown as SessionRow;
const DIFF = '@@ -1,2 +1,2 @@\n ctx\n-old line\n+new line\n';

let sent: ReviewNote[] = [];
let calls: Record<string, unknown>[] = [];
let failNext = false;

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
      const notes = (a.args.notes as { path: string; line: number; side: 'new' | 'old'; body: string }[]).map((n, i) => ({
        id: i + 1, session_id: 7, batch: a.args.client_msg_id as string, seq: i + 1, path: n.path, line: n.line,
        side: n.side, body: n.body, author: 'Ada · desktop', created_at: 1,
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
    expect(get(noteDrafts(7))).toHaveLength(1);
    expect(screen.getByTestId('review-notes-drafts').textContent).toContain('1 note not sent');

    // The first send times out: its id is kept, and the retry reuses it.
    failNext = true;
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(screen.getByTestId('review-notes-error')).toBeTruthy());
    expect(get(noteDrafts(7))).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('review-notes-send'));
    await waitFor(() => expect(get(noteDrafts(7))).toHaveLength(0));
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
    expect(get(noteDrafts(7))[0]).toMatchObject({ side: 'old', line: 2, quote: 'old line' });
    saveDraft(7, { ...get(noteDrafts(7))[0], body: '  ' });
    expect(get(noteDrafts(7))).toHaveLength(0);
    clearDrafts(7);
  });
});
