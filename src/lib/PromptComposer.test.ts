import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PromptComposer from './PromptComposer.svelte';
import { sessions, type SessionRow } from './sessions';
import { hosts } from './hosts';
import { accounts } from './accounts';

const source: SessionRow = {
  id: 1,
  tmux_name: 'dev-source',
  host_alias: 'local',
  project_id: 1,
  worktree_id: 10,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
};

const sibling: SessionRow = {
  id: 2,
  tmux_name: 'dev-sibling',
  host_alias: 'mefistos',
  project_id: 1,
  worktree_id: 10,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
};

const unrelated: SessionRow = {
  id: 3,
  tmux_name: 'dev-other',
  host_alias: 'local',
  project_id: 99,
  worktree_id: 100,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([source, sibling, unrelated]);
  hosts.set([]);
  accounts.set([]);
});

describe('PromptComposer', () => {
  it('defaults to showing related targets only', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    // sibling is related; unrelated must not appear by default
    expect(screen.queryByText('dev-sibling')).toBeInTheDocument();
    expect(screen.queryByText('dev-other')).toBeNull();
  });

  it('toggling Show all fleet expands the targets list', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const toggle = screen.getByTestId('show-all-fleet') as HTMLInputElement;
    await fireEvent.click(toggle);
    await tick();
    expect(screen.queryByText('dev-other')).toBeInTheDocument();
  });

  it('Send is disabled until prompt + at least one target are set', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const send = screen.getByTestId('composer-send') as HTMLButtonElement;
    expect(send.disabled).toBe(true); // prompt is empty
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'hello' } });
    await tick();
    // sibling is auto-checked by default → send is now enabled
    expect((screen.getByTestId('composer-send') as HTMLButtonElement).disabled).toBe(false);
  });

  it('clicking Send calls send_prompt for each checked target', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'send_prompt') return null;
      return null;
    });
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'echo hi' } });
    await tick();
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    const sendCalls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'send_prompt');
    expect(sendCalls).toHaveLength(1);
    const [, payload] = sendCalls[0] as [string, { args: { host_alias: string; tmux_name: string; prompt: string } }];
    expect(payload.args.host_alias).toBe('mefistos');
    expect(payload.args.tmux_name).toBe('dev-sibling');
    expect(payload.args.prompt).toBe('echo hi');
  });
});
