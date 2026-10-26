import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import ShortcutSettings from './ShortcutSettings.svelte';
import { SHORTCUTS } from './shortcuts';

// Settings → Shortcuts (step 7.1): the registry, read-only, so the list is
// the keys themselves.
describe('Settings → Shortcuts', () => {
  it('lists every registered shortcut with its Mac and Windows · Linux chords', () => {
    render(ShortcutSettings);
    for (const s of SHORTCUTS) expect(screen.getByTestId(`shortcut-${s.id}`)).toBeInTheDocument();
    const settings = screen.getByTestId('shortcut-settings');
    expect(settings.textContent).toContain('⌘,');
    expect(screen.getByTestId('shortcut-hosts').textContent).toContain('Ctrl+Shift+H');
    expect(screen.getByTestId('shortcut-inspector').textContent).not.toContain('coming');
    // Step 5.3 wired the terminal chords; Go to file is still planned.
    expect(screen.getByTestId('shortcut-new-terminal').textContent).not.toContain('coming');
    expect(screen.getByTestId('shortcut-go-to-file').textContent).not.toContain('coming');
  });

  it('shows what the ? sheet shows: the advertised chord and a 1–9 range (review w18-9)', () => {
    render(ShortcutSettings);
    const hosts = screen.getByTestId('shortcut-hosts').textContent ?? '';
    expect(hosts).toContain('Ctrl+Shift+H');
    expect(hosts).not.toContain('Super+I');
    expect(screen.getByTestId('shortcut-jump-n').textContent).toContain('⌘1–⌘9');
    expect(screen.getByTestId('shortcuts-section').textContent).not.toContain('0.5.3');
  });

  it('filters by action or chord', async () => {
    render(ShortcutSettings);
    await fireEvent.input(screen.getByTestId('shortcuts-filter'), { target: { value: 'today' } });
    expect(screen.getByTestId('shortcut-today')).toBeInTheDocument();
    expect(screen.queryByTestId('shortcut-settings')).toBeNull();
    await fireEvent.input(screen.getByTestId('shortcuts-filter'), { target: { value: 'zzz' } });
    expect(screen.getByText('No shortcut matches.')).toBeInTheDocument();
  });
});
