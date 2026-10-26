import { readFileSync } from 'node:fs';
import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { theme, applyTheme, cycleTheme, sunFor, resolveContrast, paint, BOOT_KEY } from './theme';

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

// Review w20-5: a launch opens on the last painted palette, not app.css's.
describe('the boot snapshot', () => {
  const boot = () => new Function(readFileSync('public/theme-boot.js', 'utf8'))();
  const root = document.documentElement;

  beforeEach(() => {
    localStorage.clear();
    root.removeAttribute('data-theme');
    root.removeAttribute('style');
  });

  it('paint() stores what it painted, and theme-boot.js replays it on a bare page', () => {
    applyTheme('dark');
    const snap = JSON.parse(localStorage.getItem(BOOT_KEY)!);
    expect(snap.theme).toBe('dark');
    const bg = root.style.getPropertyValue('--bg');
    expect(snap.vars.bg).toBe(bg);
    root.removeAttribute('data-theme');
    root.removeAttribute('style');
    boot();
    expect(root.getAttribute('data-theme')).toBe('dark');
    expect(root.style.getPropertyValue('--bg')).toBe(bg);
  });

  it('theme-boot.js does nothing without a snapshot, or with a broken one', () => {
    boot();
    expect(root.hasAttribute('data-theme')).toBe(false);
    localStorage.setItem(BOOT_KEY, '{nope');
    boot();
    expect(root.hasAttribute('data-theme')).toBe(false);
  });
});
