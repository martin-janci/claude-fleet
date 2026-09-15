import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AuthorSessionDialog, { authorSessionOpened } from './AuthorSessionDialog.svelte';
import { selectedSession } from './selection';
import { sessions } from './sessions';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const sessionRow = {
  id: 1, tmux_name: 'catalog-skill-worktree', host_alias: 'local', project_id: 1, worktree_id: null,
  created_at: 1, last_activity_at: 1, status: 'active', notes: null, account_uuid: null, kind: 'work',
  reviews_session_id: null, worktree_key: null, lost_at: null, claude_session_id: null, claude_status: null,
  effort_level: null, pr_url: null, current_activity: null, context_pct: null, stuck_kind: null,
  friendly_name: 'author skill/worktree', safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null,
  safe_kill_requested_at: null,
};

beforeEach(() => {
  invoke.mockReset();
  sessions.set([]);
});

describe('AuthorSessionDialog', () => {
  it('prefills the instructions for an existing target', () => {
    render(AuthorSessionDialog, { kind: 'skill', name: 'worktree', onclose: () => {} });
    expect((screen.getByTestId('author-instructions') as HTMLTextAreaElement).value).toBe('Improve this skill "worktree": ');
  });

  it('prefills a create-a-new-asset instruction when no target is given', () => {
    render(AuthorSessionDialog, { onclose: () => {} });
    expect((screen.getByTestId('author-instructions') as HTMLTextAreaElement).value).toContain('Create a new asset that');
  });

  it('Open session spawns the session, selects it, sets the module flag, and closes', async () => {
    invoke.mockResolvedValueOnce(sessionRow);
    const onclose = vi.fn();
    render(AuthorSessionDialog, { kind: 'skill', name: 'worktree', onclose });

    await fireEvent.click(screen.getByTestId('author-open'));

    await waitFor(() => expect(onclose).toHaveBeenCalled());
    const call = invoke.mock.calls.find((c) => c[0] === 'catalog_spawn_author_session');
    expect(call).toBeDefined();
    const args = (call![1] as { args: { kind: string; name: string; instructions: string } }).args;
    expect(args.kind).toBe('skill');
    expect(args.name).toBe('worktree');
    expect(args.instructions).toContain('Improve this skill "worktree"');
    expect(get(selectedSession)?.tmux_name).toBe('catalog-skill-worktree');
    expect(authorSessionOpened).toBe(true);
  });

  it('shows the backend error and does not close on failure', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_INVALID', message: 'name must match [a-z0-9][a-z0-9-]*' });
    const onclose = vi.fn();
    render(AuthorSessionDialog, { kind: 'skill', name: 'worktree', onclose });

    await fireEvent.click(screen.getByTestId('author-open'));

    await waitFor(() => expect(screen.getByTestId('author-error')).toBeTruthy());
    expect(onclose).not.toHaveBeenCalled();
  });
});
