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
    expect(screen.getByTestId('shortcut-inspector').textContent).toContain('coming');
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
