import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, afterEach } from 'vitest';
import { get } from 'svelte/store';
import AppearanceSettings from './AppearanceSettings.svelte';
import { uiDensity, uiLayout } from './prefs';
import { applyTheme, theme } from './theme';

afterEach(() => {
  uiLayout.set('classic');
  uiDensity.set('comfortable');
  applyTheme('auto');
});

describe('AppearanceSettings', () => {
  it('starts on Classic and persists a switch to New', async () => {
    render(AppearanceSettings);
    expect(screen.getByTestId('appearance-layout-classic').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(screen.getByTestId('appearance-layout-new'));
    expect(get(uiLayout)).toBe('new');
    expect(localStorage.getItem('cf:pref:ui.layout')).toBe('"new"');
    expect(screen.getByTestId('appearance-layout-new').getAttribute('aria-pressed')).toBe('true');
  });

  it('picks the theme (the one picker since the sidebar line went in 1.4)', async () => {
    render(AppearanceSettings);
    expect(screen.getByTestId('appearance-theme-auto').textContent).toBe('System');
    await fireEvent.click(screen.getByTestId('appearance-theme-dark'));
    expect(get(theme)).toBe('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    await fireEvent.click(screen.getByTestId('appearance-theme-auto'));
    expect(get(theme)).toBe('auto');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('starts on Comfortable (0.5.4 rows) and persists Compact', async () => {
    render(AppearanceSettings);
    expect(screen.getByTestId('appearance-density-comfortable').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(screen.getByTestId('appearance-density-compact'));
    expect(get(uiDensity)).toBe('compact');
    expect(localStorage.getItem('cf:pref:ui.density')).toBe('"compact"');
  });
});
