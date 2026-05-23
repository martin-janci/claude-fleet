import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import BgSessionPanel from './BgSessionPanel.svelte';
import type { SessionRow } from './sessions';

function bgSession(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1, tmux_name: 'bg:abc', host_alias: 'local', project_id: 1, worktree_id: null,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'bg', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: 'sess-abc', claude_status: 'working', effort_level: null,
    pr_url: null, current_activity: 'editing files', ...over,
  };
}

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('BgSessionPanel', () => {
  it('renders status and activity and the fetched transcript', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue('hello from claude logs');
    render(BgSessionPanel, { session: bgSession() });
    await tick(); await tick(); await Promise.resolve(); await tick();
    expect(screen.getByText('working')).toBeTruthy();
    expect(screen.getByText('editing files')).toBeTruthy();
    expect(await screen.findByText(/hello from claude logs/)).toBeTruthy();
    expect(mockedInvoke).toHaveBeenCalledWith('peek_session', { args: { host_alias: 'local', claude_session_id: 'sess-abc' } });
  });

  it('renders a placeholder and does not fetch when claude_session_id is null', async () => {
    render(BgSessionPanel, { session: bgSession({ claude_session_id: null }) });
    await tick(); await tick();
    expect(screen.getByText(/no logs available yet/i)).toBeTruthy();
    expect(mockedInvoke).not.toHaveBeenCalled();
  });

  it('keeps the last good transcript and shows an error when a later fetch fails', async () => {
    const invokeMock = mockedInvoke as ReturnType<typeof vi.fn>;
    // First fetch (on mount) succeeds, second (manual refresh) fails.
    invokeMock.mockResolvedValueOnce('good transcript');
    render(BgSessionPanel, { session: bgSession() });
    expect(await screen.findByText(/good transcript/)).toBeTruthy();

    invokeMock.mockRejectedValueOnce(new Error('boom'));
    await fireEvent.click(screen.getByTestId('bg-refresh'));

    // Last good transcript is still shown...
    expect(screen.getByText(/good transcript/)).toBeTruthy();
    // ...and an inline error is surfaced.
    expect(await screen.findByTestId('bg-log-error')).toBeTruthy();
  });
});
