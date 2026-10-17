import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { theme, applyTheme, cycleTheme, sunFor, resolveContrast, paint } from './theme';

describe('theme store', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
    theme.set('sun');
  });

  it('defaults to "sun"', () => {
    expect(get(theme)).toBe('sun');
  });

  it('applyTheme("light") sets data-theme on <html> and paints the engine tokens', () => {
    applyTheme('light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    expect(get(theme)).toBe('light');
    expect(document.documentElement.style.getPropertyValue('--bg')).toMatch(/^#[0-9a-f]{6}$/);
    expect(document.documentElement.style.getPropertyValue('--glow')).toBe('none');
  });

  it('applyTheme("dark") paints a dark ground and the halo', () => {
    applyTheme('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    expect(document.documentElement.style.getPropertyValue('--glow')).toMatch(/^0 0 0 1px/);
  });

  it('cycleTheme moves sun → auto → light → dark → sun', () => {
    expect(get(theme)).toBe('sun');
    cycleTheme();
    expect(get(theme)).toBe('auto');
    cycleTheme();
    expect(get(theme)).toBe('light');
    cycleTheme();
    expect(get(theme)).toBe('dark');
    cycleTheme();
    expect(get(theme)).toBe('sun');
  });

  it('persists the choice to localStorage', () => {
    applyTheme('dark');
    expect(localStorage.getItem('cf:theme')).toBe('dark');
  });

  it('Sun is light at noon and dark at midnight', () => {
    const noon = new Date(2026, 9, 17, 12, 30);
    const midnight = new Date(2026, 9, 17, 0, 30);
    expect(sunFor('sun', noon, false)).toBeGreaterThan(0);
    expect(sunFor('sun', midnight, false)).toBeLessThan(0);
    applyTheme('sun');
    paint(midnight);
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    paint(noon);
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    // The flip crossfades.
    expect(document.documentElement.hasAttribute('data-theme-fade')).toBe(true);
  });

  it('System follows the OS light/dark, Light and Dark hold', () => {
    expect(sunFor('auto', new Date(), true)).toBeLessThan(0);
    expect(sunFor('auto', new Date(), false)).toBeGreaterThan(0);
    expect(sunFor('light', new Date(2026, 0, 1, 2), false)).toBeGreaterThan(0);
    expect(sunFor('dark', new Date(2026, 0, 1, 12), false)).toBeLessThan(0);
  });

  it('System contrast is High when the OS asks for more', () => {
    expect(resolveContrast('system', true)).toBe('high');
    expect(resolveContrast('system', false)).toBe('std');
    expect(resolveContrast('soft', true)).toBe('soft');
  });
});
