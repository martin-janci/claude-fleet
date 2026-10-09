import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import AppearanceSettings from './AppearanceSettings.svelte';
import { uiDensity, uiLayout } from './prefs';
import { applyTheme, theme } from './theme';
import { motionPref } from './motion';

afterEach(() => {
  uiLayout.set('classic');
  uiDensity.set('comfortable');
  applyTheme('auto');
  motionPref.set('system');
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

  it('picks Motion and persists it, System first', async () => {
    render(AppearanceSettings);
    expect(screen.getByTestId('appearance-motion-system').getAttribute('aria-pressed')).toBe('true');
    for (const id of ['full', 'reduced', 'off'] as const) {
      await fireEvent.click(screen.getByTestId(`appearance-motion-${id}`));
      expect(get(motionPref)).toBe(id);
      expect(localStorage.getItem('cf:pref:ui.motion')).toBe(`"${id}"`);
    }
  });
});

// Parity P11: Appearance is shared by both layouts, so the theme picker works
// the same with New on.
describe('AppearanceSettings in the New layout', () => {
  beforeEach(() => uiLayout.set('new'));
  afterEach(() => uiLayout.set('classic'));

  it('New layout: shows New as the current layout and picks the theme', async () => {
    render(AppearanceSettings);
    expect(screen.getByTestId('appearance-layout-new').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('appearance-theme-auto').textContent).toBe('System');
    await fireEvent.click(screen.getByTestId('appearance-theme-dark'));
    expect(get(theme)).toBe('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    await fireEvent.click(screen.getByTestId('appearance-theme-auto'));
    expect(get(theme)).toBe('auto');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
    expect(get(uiLayout)).toBe('new');
  });
});
