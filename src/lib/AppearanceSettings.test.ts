import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import AppearanceSettings from './AppearanceSettings.svelte';
import { uiDensity } from './prefs';
import { applyTheme, theme } from './theme';
import { motionPref } from './motion';

afterEach(() => {
  uiDensity.set('compact');
  applyTheme('auto');
  motionPref.set('system');
});

describe('AppearanceSettings', () => {
  it('has no Layout row (13.1), and forgets the retired layout keys at load', async () => {
    localStorage.setItem('cf:pref:ui.layout', '"classic"');
    localStorage.setItem('cf:pref:ui.layout.v2', '"classic"');
    localStorage.setItem('cf:pref:layout.center-collapsed', 'true');
    localStorage.setItem('cf:session-ui', '{"local:dev":{"centerPx":360}}');
    localStorage.setItem('cf:pref:sidebar.inbox-before-classic', 'true');
    vi.resetModules();
    try {
      await import('./prefs');
      expect(localStorage.getItem('cf:pref:ui.layout')).toBeNull();
      expect(localStorage.getItem('cf:pref:ui.layout.v2')).toBeNull();
      expect(localStorage.getItem('cf:pref:layout.center-collapsed')).toBeNull();
      expect(localStorage.getItem('cf:session-ui')).toBeNull();
      expect(localStorage.getItem('cf:pref:sidebar.inbox-before-classic')).toBeNull();
    } finally {
      vi.resetModules();
    }
    render(AppearanceSettings);
    expect(screen.queryByText('Layout')).toBeNull();
    expect(screen.queryByTestId('appearance-layout-new')).toBeNull();
    expect(screen.queryByTestId('appearance-layout-classic')).toBeNull();
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

  it('starts on Compact (the design row, UX audit L1) and persists Comfortable', async () => {
    render(AppearanceSettings);
    expect(screen.getByTestId('appearance-density-compact').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(screen.getByTestId('appearance-density-comfortable'));
    expect(get(uiDensity)).toBe('comfortable');
    expect(localStorage.getItem('cf:pref:ui.rowDensity')).toBe('"comfortable"');
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
