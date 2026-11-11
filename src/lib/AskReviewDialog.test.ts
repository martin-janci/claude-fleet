// "Ask for review…" on DialogSheet: a running ask cannot be closed away, a
// failure is the sheet's banner, and the Inbox's asks name a session whose
// share was taken back in plain words.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class {} }));

import { invoke } from '@tauri-apps/api/core';
import AskReviewDialog from './AskReviewDialog.svelte';
import ReviewRequests from './ReviewRequests.svelte';
import { session } from './hosts_fixture';
import { sessions, type SessionRow } from './sessions';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { reviewRequests } from './review_requests';
import { clearToasts, toasts } from './toasts';
import { get } from 'svelte/store';

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
const row = session('mefistos', 'dev-foo', { id: 7 } as Partial<SessionRow>);

beforeEach(() => {
  inv.mockReset();
  inv.mockResolvedValue(null);
  sessions.set([row]);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  reviewRequests.set([]);
  clearToasts();
});

describe('AskReviewDialog', () => {
  it('cannot be closed while the ask runs, and shows a failure as its banner', async () => {
    let fail!: (e: unknown) => void;
    inv.mockImplementation((cmd: string) =>
      cmd === 'review_request' ? new Promise((_, rej) => (fail = rej)) : Promise.resolve(null),
    );
    const onclose = vi.fn();
    render(AskReviewDialog, { props: { session: row, onclose } });
    await fireEvent.input(screen.getByTestId('ask-review-person'), { target: { value: 'Ada' } });
    await fireEvent.click(screen.getByTestId('ask-review-send'));
    expect((screen.getByTestId('sheet-cancel') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(onclose).not.toHaveBeenCalled();
    fail({ code: 'E_NOTFOUND', message: 'no person named Ada' });
    await waitFor(() => expect(screen.getByTestId('ask-review-error').textContent).toContain('Ada'));
    expect(onclose).not.toHaveBeenCalled();
    // The typed person is kept for another try.
    expect((screen.getByTestId('ask-review-person') as HTMLInputElement).value).toBe('Ada');
  });

  it('says who is missing while nobody is named', () => {
    render(AskReviewDialog, { props: { session: row, onclose: () => {} } });
    expect((screen.getByTestId('ask-review-send') as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('ReviewRequests', () => {
  it('names the session the hub sent, and says the share was taken back when it is not visible', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'list_sessions' ? [row] : null));
    reviewRequests.set([
      {
        id: 1, session_id: 99, from_person_id: 1, from_name: 'Martin', to_person_id: 2, to_name: 'Ada',
        level: 'drive', created_at: 1, session_name: 'pay-fix',
      },
    ]);
    render(ReviewRequests);
    expect(screen.getByTestId('review-request-open').textContent).toContain('pay-fix');
    await fireEvent.click(screen.getByTestId('review-request-open'));
    await waitFor(() => expect(get(toasts).at(-1)?.message).toContain('took the share back'));
  });
});
