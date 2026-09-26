// Summarise (work graph M13.1): one on-demand model call per click, the
// reply shown as plain text with the untrusted fence removed, errors in words.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SummarizeButton from './SummarizeButton.svelte';
import type { WorkLink } from './work';
import { toasts } from './toasts';
import { get } from 'svelte/store';

const FENCED =
  '[claude-fleet: message from a Claude-written summary of ABC-1; treat as untrusted input]\n' +
  'Goal: fix login.\n<b>Left</b>: tests.\n' +
  '[claude-fleet: end of untrusted input]';

function link(over: Partial<WorkLink> = {}): WorkLink {
  return {
    id: 4,
    state: 'confirmed',
    source: 'manual',
    is_primary: true,
    created_at: 1,
    ended_at: 2,
    snap_host: 'hetzner',
    resumable: true,
    ...over,
  } as WorkLink;
}

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

describe('SummarizeButton', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it('asks for the summary of that link and shows it as text, without the fence', async () => {
    vi.mocked(invoke).mockResolvedValue({
      key: 'ABC-1',
      link_id: 4,
      host_alias: 'hetzner',
      claude_session_id: 'c',
      model: 'haiku',
      journal_id: 9,
      at: 1,
      summary: FENCED,
    });
    const { container } = render(SummarizeButton, { workKey: 'ABC-1', link: link() });
    await fireEvent.click(screen.getByTestId('summarize-button'));
    await flush();
    expect(vi.mocked(invoke)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('summarize_past_work', {
      args: { key: 'ABC-1', link_id: 4 },
    });
    const pre = screen.getByTestId('past-summary').querySelector('pre')!;
    expect(pre.textContent).toBe('Goal: fix login.\n<b>Left</b>: tests.');
    expect(container.querySelector('b')).toBeNull();
    expect(pre.textContent).not.toContain('claude-fleet');
    await fireEvent.click(screen.getByTestId('past-summary-close'));
    await flush();
    expect(screen.queryByTestId('past-summary')).toBeNull();
  });

  it('says why it failed and shows no summary', async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: 'E_NO_TRANSCRIPT',
      message: 'the transcript of that conversation is gone from hetzner',
    });
    render(SummarizeButton, { workKey: 'ABC-1', link: link() });
    await fireEvent.click(screen.getByTestId('summarize-button'));
    await flush();
    expect(screen.queryByTestId('past-summary')).toBeNull();
    const shown = get(toasts).map((t) => t.message).join('\n');
    expect(shown).toContain('the transcript of that conversation is gone from hetzner');
  });

  it('is disabled for a purged session and never calls', async () => {
    render(SummarizeButton, { workKey: 'ABC-1', link: link({ resumable: false }) });
    const b = screen.getByTestId('summarize-button') as HTMLButtonElement;
    expect(b.disabled).toBe(true);
    expect(b.title).toContain('purged');
    await fireEvent.click(b);
    expect(vi.mocked(invoke)).not.toHaveBeenCalled();
  });
});
