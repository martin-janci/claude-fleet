import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, afterEach } from 'vitest';
import { tick } from 'svelte';
import ShortcutSheet from './ShortcutSheet.svelte';
import { shortcutSheetOpen } from './app_views';

afterEach(() => shortcutSheetOpen.set(false));

// Redesign step 3.8: the `?` sheet lists the registry.
describe('ShortcutSheet', () => {
  it('? opens the sheet with every live chord, in the Mac spelling on a Mac', async () => {
    render(ShortcutSheet, { isMac: true });
    expect(screen.queryByTestId('shortcut-sheet')).toBeNull();
    await fireEvent.keyDown(document.body, { key: '?', shiftKey: true });
    await tick();
    expect(screen.getByTestId('shortcut-sheet')).toBeTruthy();
    const row = (id: string) => document.querySelector(`[data-testid="shortcut-row"][data-id="${id}"]`);
    expect(row('switcher')?.textContent).toContain('⌘K');
    expect(row('next-needs-you')?.textContent).toContain('⌥⌘N');
    expect(row('jump-n')?.textContent).toContain('⌘1–⌘9');
    expect(row('session-list.down')?.textContent).toContain('J or ↓');
    expect(row('inspector')?.textContent).toContain('⌥⌘B');
    // Planned chords are not listed until their step wires them.
    expect(row('new-terminal')).toBeNull();
    const scopes = screen.getAllByTestId('shortcut-section').map((s) => s.dataset.scope);
    expect(scopes[0]).toBe('global');
    expect(scopes).toContain('hosts');
  });

  it('spells chords for Linux and Windows off the Mac, and leaves out Mac-only ones', async () => {
    render(ShortcutSheet, { isMac: false });
    await fireEvent.keyDown(document.body, { key: '?', shiftKey: true });
    await tick();
    const row = (id: string) => document.querySelector(`[data-testid="shortcut-row"][data-id="${id}"]`);
    expect(row('switcher')?.textContent).toContain('Ctrl+Shift+K');
    expect(row('next-needs-you')?.textContent).toContain('Ctrl+Alt+N');
    expect(row('jump-n')).toBeNull();
  });

  it('? typed into a field stays in the field', async () => {
    render(ShortcutSheet, { isMac: true });
    const input = document.createElement('input');
    document.body.appendChild(input);
    await fireEvent.keyDown(input, { key: '?', shiftKey: true });
    await tick();
    expect(screen.queryByTestId('shortcut-sheet')).toBeNull();
    input.remove();
  });
});
