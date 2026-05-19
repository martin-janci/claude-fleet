import { writable, get } from 'svelte/store';

export type Theme = 'auto' | 'light' | 'dark';
const KEY = 'cf:theme';
const ORDER: Theme[] = ['auto', 'light', 'dark'];

function read(): Theme {
  const v = localStorage.getItem(KEY);
  return v === 'light' || v === 'dark' || v === 'auto' ? v : 'auto';
}

export const theme = writable<Theme>(read());

export function applyTheme(next: Theme): void {
  theme.set(next);
  localStorage.setItem(KEY, next);
  if (next === 'auto') {
    document.documentElement.removeAttribute('data-theme');
  } else {
    document.documentElement.setAttribute('data-theme', next);
  }
}

export function cycleTheme(): void {
  const current = get(theme);
  const idx = ORDER.indexOf(current);
  applyTheme(ORDER[(idx + 1) % ORDER.length]);
}

export function initTheme(): void {
  applyTheme(read());
}
