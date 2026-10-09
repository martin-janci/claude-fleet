import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PromptsSnippets from './PromptsSnippets.svelte';
import { composerPresets, flushComposerPresets } from './composer_presets';

beforeEach(() => {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async () => null);
});

describe('PromptsSnippets (Toolkit › Prompts & snippets)', () => {
  it('re-reads the composer presets when it opens, so an edit starts from the fleet list', async () => {
    composerPresets.set([{ label: 'Stale', text: 'stale' }]);
    const fresh = [{ label: 'Phone', text: 'from the phone' }];
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const base = inv.getMockImplementation() as (cmd: string, a?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a?: unknown) => {
      if (cmd === 'quick_replies') return fresh;
      return base(cmd, a);
    });
    render(PromptsSnippets);
    await waitFor(() => expect(inv).toHaveBeenCalledWith('quick_replies', undefined));
    await waitFor(() => expect(get(composerPresets)).toEqual(fresh));
  });

  it('lists the composer presets and edits them in place, saving through the backend', async () => {
    // The list is fleet state now (`service::quick_replies`), so the editor
    // starts from what the backend served and every edit is a write to it —
    // debounced, which is why each assertion flushes.
    const seeded = [
      { label: 'Clear', text: '/clear' },
      { label: 'Compact', text: '/compact' },
    ];
    composerPresets.set(seeded);
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const base = inv.getMockImplementation() as (cmd: string, a?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a?: { entries?: unknown }) => {
      if (cmd === 'quick_replies') return seeded;
      if (cmd === 'set_quick_replies') return a?.entries;
      return base(cmd, a);
    });
    render(PromptsSnippets);
    await tick();
    const section = screen.getByTestId('composer-section');
    expect(section.textContent).toContain('Prompts & snippets');
    expect(screen.getByTestId('toolkit-prompts-summary')).toHaveTextContent('2 chips');
    const labels = screen.getAllByTestId('preset-label') as HTMLInputElement[];
    expect(labels).toHaveLength(seeded.length);
    expect(labels[0].value).toBe('Clear');

    await fireEvent.input(labels[0], { target: { value: 'Wipe' } });
    expect(get(composerPresets)[0].label).toBe('Wipe');
    const texts = screen.getAllByTestId('preset-text') as HTMLTextAreaElement[];
    await fireEvent.input(texts[0], { target: { value: '/clear now' } });
    expect(get(composerPresets)[0].text).toBe('/clear now');
    await flushComposerPresets();
    expect(inv).toHaveBeenCalledWith(
      'set_quick_replies',
      expect.objectContaining({
        entries: [{ label: 'Wipe', text: '/clear now' }, seeded[1]],
      }),
    );

    // The Send box and the arrows save too; the arrows stop at either end.
    await fireEvent.click(screen.getAllByTestId('preset-auto-send')[1]);
    expect(get(composerPresets)[1].auto_send).toBe(true);
    expect((screen.getAllByTestId('preset-up')[0] as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getAllByTestId('preset-down')[1] as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getAllByTestId('preset-down')[0]);
    await flushComposerPresets();
    expect(inv).toHaveBeenCalledWith(
      'set_quick_replies',
      expect.objectContaining({
        entries: [{ ...seeded[1], auto_send: true }, { label: 'Wipe', text: '/clear now' }],
      }),
    );
    await fireEvent.click(screen.getAllByTestId('preset-up')[1]);
    expect(get(composerPresets)[0].label).toBe('Wipe');

    await fireEvent.click(screen.getByTestId('preset-add'));
    expect(get(composerPresets)).toHaveLength(seeded.length + 1);
    expect(screen.getAllByTestId('preset-label')).toHaveLength(seeded.length + 1);

    await fireEvent.click(screen.getAllByTestId('preset-remove')[0]);
    expect(get(composerPresets)[0].label).toBe('Compact');

    // Reset stores nothing and takes the backend's built-ins back.
    inv.mockClear();
    await fireEvent.click(screen.getByTestId('preset-reset'));
    await flushComposerPresets();
    expect(inv).toHaveBeenCalledWith('set_quick_replies', expect.objectContaining({ entries: [] }));
  });
});

describe('PromptsSnippets — moving a chip', () => {
  it('keeps the rows keyed by chip, and the focus on the chip that moved', async () => {
    const seeded = [
      { label: 'A', text: 'a' },
      { label: 'B', text: 'b' },
      { label: 'C', text: 'c' },
    ];
    composerPresets.set(seeded);
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const base = inv.getMockImplementation() as (cmd: string, a?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a?: { entries?: unknown }) => {
      if (cmd === 'quick_replies') return seeded;
      if (cmd === 'set_quick_replies') return a?.entries;
      return base(cmd, a);
    });
    render(PromptsSnippets);
    await tick();
    const rowOf = (label: string) =>
      (screen.getAllByTestId('preset-label') as HTMLInputElement[])
        .find((l) => l.value === label)!
        .closest('.preset-row') as HTMLElement;
    const aRow = rowOf('A');

    const down = aRow.querySelector('[data-testid="preset-down"]') as HTMLButtonElement;
    down.focus();
    await fireEvent.click(down);
    await tick();
    await tick();
    // The same row element moved with its chip…
    expect(rowOf('A')).toBe(aRow);
    expect((screen.getAllByTestId('preset-label') as HTMLInputElement[]).map((l) => l.value)).toEqual([
      'B',
      'A',
      'C',
    ]);
    // …and the keyboard is still on it.
    expect(document.activeElement).toBe(aRow.querySelector('[data-testid="preset-down"]'));

    // Moved to the end, its ↓ is disabled: focus falls to its ↑.
    await fireEvent.click(aRow.querySelector('[data-testid="preset-down"]') as HTMLButtonElement);
    await tick();
    await tick();
    expect(rowOf('A')).toBe(aRow);
    expect(document.activeElement).toBe(aRow.querySelector('[data-testid="preset-up"]'));
    await flushComposerPresets();
  });
});
