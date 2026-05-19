import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { theme, applyTheme, cycleTheme } from './theme';

describe('theme store', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
  });

  it('defaults to "auto"', () => {
    expect(get(theme)).toBe('auto');
  });

  it('applyTheme("light") sets data-theme on <html>', () => {
    applyTheme('light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    expect(get(theme)).toBe('light');
  });

  it('applyTheme("auto") removes data-theme', () => {
    applyTheme('dark');
    applyTheme('auto');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('cycleTheme moves auto → light → dark → auto', () => {
    expect(get(theme)).toBe('auto');
    cycleTheme();
    expect(get(theme)).toBe('light');
    cycleTheme();
    expect(get(theme)).toBe('dark');
    cycleTheme();
    expect(get(theme)).toBe('auto');
  });

  it('persists the choice to localStorage', () => {
    applyTheme('dark');
    expect(localStorage.getItem('cf:theme')).toBe('dark');
  });
});
