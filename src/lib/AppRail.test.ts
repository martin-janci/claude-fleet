import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, afterEach, vi } from 'vitest';
import AppRail from './AppRail.svelte';
import { destination } from './destination';
import { settingsOpen } from './app_views';
import { sidebarView } from './work_view';

afterEach(() => {
  destination.set('session');
  settingsOpen.set(false);
  sidebarView.set('sessions');
});

describe('AppRail', () => {
  it('names each shortcut in its title, per platform', () => {
    const mac = render(AppRail, { isMac: true, onselect: () => {} });
    expect(mac.getByTestId('rail-sessions').title).toBe('Sessions  ⌘⇧W');
    expect(mac.getByTestId('rail-accounts').title).toBe('Accounts & hosts  ⌘I');
    expect(mac.getByTestId('rail-settings').title).toBe('Settings  ⌘,');
    mac.unmount();
    const other = render(AppRail, { isMac: false, onselect: () => {} });
    expect(other.getByTestId('rail-work').title).toBe('Work  Ctrl+Shift+W');
    expect(other.getByTestId('rail-accounts').title).toBe('Accounts & hosts  Ctrl+Shift+H');
  });

  it('marks the current item and hands a click to its owner', async () => {
    const onselect = vi.fn();
    const { getByTestId, queryByTestId } = render(AppRail, { isMac: true, onselect });
    expect(queryByTestId('rail-control')).toBeNull();
    expect(getByTestId('rail-sessions').getAttribute('aria-current')).toBe('page');
    destination.set('hosts');
    await Promise.resolve();
    expect(getByTestId('rail-accounts').getAttribute('aria-current')).toBe('page');
    settingsOpen.set(true);
    await Promise.resolve();
    expect(getByTestId('rail-settings').getAttribute('aria-current')).toBe('page');
    expect(getByTestId('rail-accounts').getAttribute('aria-current')).toBeNull();
    await fireEvent.click(getByTestId('rail-work'));
    expect(onselect).toHaveBeenCalledWith('work');
  });
});
