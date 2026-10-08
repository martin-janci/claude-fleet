// Redesign step 3.15, "After an update": the Wordmark reveal once, with the
// version and a way to what changed.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

import { openUrl } from '@tauri-apps/plugin-opener';
import UpdateReveal from './UpdateReveal.svelte';

describe('UpdateReveal', () => {
  it("plays the Wordmark reveal with the version and What's new", async () => {
    const onclose = vi.fn();
    render(UpdateReveal, { props: { version: '0.5.4', onclose } });
    expect(screen.getByTestId('update-reveal-wordmark').dataset.loader).toBe('wordmark-reveal');
    expect(screen.getByTestId('update-reveal').textContent).toContain("Orbit Fleet 0.5.4 · What's new");
    await fireEvent.click(screen.getByTestId('update-reveal-notes'));
    expect(openUrl).toHaveBeenCalledWith('https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.4');
    await fireEvent.click(screen.getByTestId('update-reveal-close'));
    expect(onclose).toHaveBeenCalledOnce();
  });
});
