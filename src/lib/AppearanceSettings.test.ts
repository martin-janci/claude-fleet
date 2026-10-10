import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import AppearanceSettings from './AppearanceSettings.svelte';
import { agentTabLabel, agentTabName, badgeCounts, badgeNumber, uiDensity } from './prefs';
import { applyTextSize, textSize } from './text_size';
import { applyTheme, theme } from './theme';
import { motionPref } from './motion';

afterEach(() => {
  uiDensity.set('compact');
  applyTheme('auto');
  motionPref.set('system');
  textSize.set(100);
  agentTabName.set('agent');
  badgeCounts.set('needs_you');
  document.documentElement.style.removeProperty('zoom');
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

describe('text size, agent tab name and badge counts (gap plan G4.6)', () => {
  it('sets each per-device pref from Settings › Appearance and keeps it', async () => {
    render(AppearanceSettings);
    await fireEvent.change(screen.getByTestId('appearance-text-size'), { target: { value: '125' } });
    expect(get(textSize)).toBe(125);
    expect(localStorage.getItem('cf:pref:ui.textSize')).toBe('125');
    await fireEvent.click(screen.getByTestId('appearance-agent-tab-terminal'));
    expect(get(agentTabName)).toBe('terminal');
    expect(localStorage.getItem('cf:pref:ui.agentTabName')).toBe('"terminal"');
    await fireEvent.change(screen.getByTestId('appearance-badge-counts'), { target: { value: 'off' } });
    expect(get(badgeCounts)).toBe('off');
    expect(localStorage.getItem('cf:pref:ui.badgeCounts')).toBe('"off"');
  });

  it("zooms the desktop's webview, or the page where there is none", async () => {
    const zoom = vi.fn(async () => {});
    await applyTextSize(125, zoom);
    expect(zoom).toHaveBeenCalledWith(1.25);
    expect(document.documentElement.style.getPropertyValue('zoom')).toBe('');
    await applyTextSize(110, async () => {
      throw new Error('no webview');
    });
    expect(document.documentElement.style.getPropertyValue('zoom')).toBe('1.1');
    await applyTextSize(100);
    expect(document.documentElement.style.getPropertyValue('zoom')).toBe('');
  });

  it('names the agent tab and counts the badge by the pref', () => {
    expect(agentTabLabel('Codex', 'agent')).toBe('Codex');
    expect(agentTabLabel('Codex', 'terminal')).toBe('Terminal');
    expect(agentTabLabel(undefined, 'agent')).toBe('Terminal');
    expect(badgeNumber(3, 'needs_you')).toBe(3);
    expect(badgeNumber(3, 'off')).toBe(0);
  });
});
