// Today's morning brief (step 9.11): only the newest read lands.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import MorningBrief from './MorningBrief.svelte';

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

describe('MorningBrief', () => {
  it('the opening read answering after a Draft does not put the old brief back (review r07)', async () => {
    let releaseOpen: (() => void) | null = null;
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const args = (raw as { args: { refresh: boolean } }).args;
      if (cmd !== 'today_brief') return null;
      if (!args.refresh) return new Promise((res) => (releaseOpen = () => res({ draft: null })));
      return { draft: { text: 'Fresh brief', model: 'haiku', host_alias: 'h', from: '3 tasks', at: 1 } };
    });
    render(MorningBrief);
    await flush();
    await fireEvent.click(screen.getByTestId('morning-brief-draft-btn'));
    await flush();
    expect(screen.getByTestId('morning-brief-at')).toBeTruthy();
    releaseOpen!();
    await flush();
    expect(screen.getByTestId('morning-brief-at')).toBeTruthy();
    expect(screen.queryByTestId('morning-brief-draft-btn')).toBeNull();
  });
});
