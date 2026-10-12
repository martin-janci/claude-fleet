import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it } from 'vitest';
import { clampListWidth, LIST_W_FALLBACK_PX, listWidthDefault, tokenPx } from './layout_tokens';

const appCss = readFileSync('src/app.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

afterEach(() => document.documentElement.style.removeProperty('--list-w'));

describe('layout tokens', () => {
  it('the fallback is app.css --list-w', () => {
    const decl = appCss.match(/--list-w:\s*(\d+)px;/);
    expect(decl).not.toBeNull();
    expect(Number(decl![1])).toBe(LIST_W_FALLBACK_PX);
  });

  it('the session list defaults to --list-w as the document defines it', () => {
    expect(listWidthDefault()).toBe(LIST_W_FALLBACK_PX);
    document.documentElement.style.setProperty('--list-w', '300px');
    expect(listWidthDefault()).toBe(300);
    expect(tokenPx('--list-w', 1)).toBe(300);
  });

  it('a drag stays between 180 and 640 px', () => {
    expect(clampListWidth(100)).toBe(180);
    expect(clampListWidth(900)).toBe(640);
    expect(clampListWidth(333)).toBe(333);
  });

  it('App reads the default from the token, not a literal', () => {
    const app = readFileSync('src/App.svelte', 'utf8');
    expect(app).toMatch(/readPref\('layout\.sidebar', listWidthDefault\(\), isNumber\)/);
    expect(app).not.toMatch(/readPref\('layout\.sidebar', \d+/);
  });
});
