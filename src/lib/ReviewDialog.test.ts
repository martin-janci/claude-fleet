import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ReviewDialog from './ReviewDialog.svelte';
import { DEFAULT_REVIEW_PROMPT, type SessionRow } from './sessions';

const source: SessionRow = {
  id: 1, tmux_name: 'dev-source', host_alias: 'local',
  project_id: 1, worktree_id: 10, created_at: 1, last_activity_at: 1,
  status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null,
  worktree_key: 'main', lost_at: null,
  claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null,
  friendly_name: null,
};

beforeEach(() => { (mockedInvoke as ReturnType<typeof vi.fn>).mockReset(); });

describe('ReviewDialog', () => {
  it('prefills the default multipass prompt', async () => {
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    const ta = screen.getByTestId('review-textarea') as HTMLTextAreaElement;
    expect(ta.value).toBe(DEFAULT_REVIEW_PROMPT);
  });

  it('Start review calls spawn_review with source id + prompt', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'spawn_review') return { ...source, id: 2, tmux_name: 'dev-source--review-abc', kind: 'review', reviews_session_id: 1 };
      return null;
    });
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('review-start'));
    for (let i = 0; i < 6; i++) await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'spawn_review');
    expect(call).toBeDefined();
    const payload = call![1] as { args: { source_session_id: number; prompt: string } };
    expect(payload.args.source_session_id).toBe(1);
    expect(payload.args.prompt).toContain('Pass 1');
  });

  it('Start is disabled when prompt is emptied', async () => {
    render(ReviewDialog, { props: { source, onClose: () => {} } });
    await tick();
    const ta = screen.getByTestId('review-textarea') as HTMLTextAreaElement;
    await fireEvent.input(ta, { target: { value: '   ' } });
    await tick();
    expect((screen.getByTestId('review-start') as HTMLButtonElement).disabled).toBe(true);
  });
});
