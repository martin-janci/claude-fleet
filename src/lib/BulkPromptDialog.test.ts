// Send prompt in the one dialog pattern (step 5.10): each target goes through
// `queue_prompt`, which types into an idle session now and keeps the prompt
// for a busy one; the dialog says which happened to each.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import BulkPromptDialog, { PRESETS } from './BulkPromptDialog.svelte';
import { sessions } from './sessions';
import { session } from './hosts_fixture';

const idle = session('mac', 'dev-a', { id: 1, status: 'running', claude_status: 'idle' });
const busy = session('mac', 'dev-b', { id: 2, status: 'running', claude_status: 'working' });

async function settle() {
  for (let i = 0; i < 6; i++) await tick();
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  sessions.set([idle, busy]);
});

describe('BulkPromptDialog', () => {
  it('reads as the dialog pattern: title, one sentence, Cancel and the verb', async () => {
    render(BulkPromptDialog, { props: { targets: [idle, busy], onClose: vi.fn() } });
    await settle();
    expect(screen.getByText('Send prompt to 2 sessions')).toBeTruthy();
    expect(screen.getByText(/Busy sessions get it when they are idle/)).toBeTruthy();
    expect(screen.getByTestId('sheet-cancel')).toHaveTextContent('Cancel');
    expect(screen.getByTestId('bulk-prompt-send')).toHaveTextContent('Send to 2');
  });

  it('a preset fills the text, which stays editable', async () => {
    render(BulkPromptDialog, { props: { targets: [idle], onClose: vi.fn() } });
    await settle();
    await fireEvent.click(screen.getByTestId('bulk-preset-rebase-on-main'));
    const ta = screen.getByTestId('bulk-prompt-textarea') as HTMLTextAreaElement;
    expect(ta.value).toBe(PRESETS.find((p) => p.label === 'Rebase on main')!.text);
    expect(ta.disabled).toBe(false);
  });

  it('says which session got it now and which gets it when idle, and stays open', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const id = (a as { args: { session_id: number } }).args.session_id;
      if (cmd === 'queue_prompt') return id === 1 ? { session_id: 1, delivered: true } : { session_id: 2, delivered: false, queued_id: 9 };
      return null;
    });
    const onClose = vi.fn();
    render(BulkPromptDialog, { props: { targets: [idle, busy], onClose } });
    await settle();
    await fireEvent.input(screen.getByTestId('bulk-prompt-textarea'), { target: { value: 'status?' } });
    await fireEvent.click(screen.getByTestId('bulk-prompt-send'));
    await waitFor(() => expect(screen.getByTestId('bulk-queued-2')).toBeTruthy());
    expect(screen.getByTestId('bulk-sent-1')).toHaveTextContent('sent');
    const sent = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'queue_prompt').map((c) => (c[1] as { args: unknown }).args);
    expect(sent).toEqual([
      { session_id: 1, prompt: 'status?' },
      { session_id: 2, prompt: 'status?' },
    ]);
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'send_prompt')).toBe(false);
    expect(screen.getByTestId('bulk-prompt-send')).toHaveTextContent('Done');
    await fireEvent.click(screen.getByTestId('bulk-prompt-send'));
    expect(onClose).toHaveBeenCalled();
  });
});
