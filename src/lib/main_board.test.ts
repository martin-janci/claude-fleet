// Redesign steps 3.2 and 3.17, "Verified by: snapshot against the Main
// board". The board (canvas/Main.dc.html) is read when the test runs and the
// shipped shell is held to its structure: the rail's items in the board's
// order with Settings after the spacer, the header's parts in the board's
// order (mark and name, the ⌘K field, account pills, a separator, the
// Automation pill, Pause all), the header at 44 px and the status bar at
// 25 px through the same tokens the app's CSS sizes them with.
import { readFileSync } from 'node:fs';
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import AppRail from './AppRail.svelte';
import ShellHeader from './ShellHeader.svelte';
import { accounts } from './accounts';
import { accountUsage } from './account_usage_store';
import { destination } from './destination';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { ADMIN, GMAIL, snapshot } from './hosts_fixture';

const BOARD = readFileSync('docs/ux/2026-10-08-orbit-fleet-redesign/canvas/Main.dc.html', 'utf8');
const strip = (html: string) => html.replace(/<svg[\s\S]*?<\/svg>/g, '<svg/>');
const between = (open: string, close: string) => {
  const i = BOARD.indexOf(open);
  expect(i, `the Main board has ${open}`).toBeGreaterThan(-1);
  return strip(BOARD.slice(i, BOARD.indexOf(close, i)));
};
const css = (file: string) => readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const ROOT = (() => {
  const app = css('src/app.css');
  const root = app.slice(app.indexOf(':root'), app.indexOf('}', app.indexOf(':root')));
  return Object.fromEntries([...root.matchAll(/--([a-z0-9-]+)\s*:\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
})();
const boardPx = (tag: string) => Number(BOARD.match(new RegExp(`<${tag} style="height: (\\d+)px`))?.[1]);

beforeEach(() => {
  destination.set('session');
  fleetSettings.set({ ...SETTING_DEFAULTS });
});

describe('the Main board (3.2): the rail', () => {
  const rail = between('<nav class="rail"', '</nav>');
  const boardItems = [...rail.matchAll(/<a href="#([a-z]+)"[^>]*><svg\/>([^<]+)/g)].map((m) => ({ id: m[1], label: m[2].trim() }));

  it('reads the board: eight items, Settings after the spacer', () => {
    expect(boardItems.map((i) => i.id)).toEqual(['control', 'inbox', 'sessions', 'work', 'automation', 'accounts', 'toolkit', 'settings']);
    expect(rail.lastIndexOf('<div')).toBeLessThan(rail.indexOf('href="#settings"'));
    expect(rail.lastIndexOf('<div')).toBeGreaterThan(rail.indexOf('href="#toolkit"'));
  });

  it('ships the same items, labels and order, with Settings at the bottom', () => {
    const { container } = render(AppRail, { isMac: true, onselect: () => {} });
    const links = Array.from(container.querySelectorAll<HTMLElement>('[data-testid^="rail-"]'));
    expect(links.map((a) => a.dataset.testid!.slice('rail-'.length))).toEqual(boardItems.map((i) => i.id));
    expect(links.map((a) => a.textContent?.replace(/\d+$/, '').trim())).toEqual(boardItems.map((i) => i.label));
    // The spacer sits between Toolkit and Settings, as on the board.
    const settings = links.at(-1)!;
    expect(settings.previousElementSibling?.classList.contains('grow')).toBe(true);
    // The board's 68 px rail is the --rail-w token.
    expect(rail).toContain(`width: ${ROOT['rail-w']}`);
  });
});

describe('the Main board (3.17): the header and the status bar', () => {
  const header = between('<header', '</header>');
  /** The board header's parts, in order: its child elements, read as DOM. */
  const boardParts = (): string[] => {
    const doc = new DOMParser().parseFromString(`${header}</header>`, 'text/html');
    return Array.from(doc.querySelector('header')!.children).flatMap((el): string[] => {
      const text = el.textContent ?? '';
      const style = el.getAttribute('style') ?? '';
      if (text.includes('Orbit Fleet')) return ['brand'];
      if (text.includes('Search or run a command')) return ['command'];
      if (el.getAttribute('aria-label') === 'Pause all automation') return ['pause-all'];
      if (text.includes('Automation')) return ['automation'];
      if (el.tagName === 'BUTTON' && el.querySelector('.dot')) return ['account'];
      if (el.tagName === 'SPAN' && /width: 1px/.test(style)) return ['separator'];
      return []; // the flexible gap
    });
  };

  beforeEach(() => {
    accounts.set([ADMIN, GMAIL]);
    accountUsage.set({
      [ADMIN.uuid]: snapshot(ADMIN.uuid, { fetched_at: Math.floor(Date.now() / 1000) - 60, usage: null }),
      [GMAIL.uuid]: snapshot(GMAIL.uuid, { fetched_at: Math.floor(Date.now() / 1000) - 60, usage: null }),
    });
  });

  it('reads the board: mark and name, ⌘K, accounts, a separator, Automation, Pause all', () => {
    const parts = boardParts();
    const collapsed = parts.filter((p, i) => p !== 'account' || parts[i - 1] !== 'account');
    expect(collapsed).toEqual(['brand', 'command', 'account', 'separator', 'automation', 'pause-all']);
    expect(header).toContain('<span class="kbd">⌘K</span>');
  });

  it('ships the same parts in the same order', () => {
    render(ShellHeader, { mac: true });
    const h = screen.getByTestId('shell-header');
    const parts = Array.from(h.querySelectorAll<HTMLElement>('.brand, .command, [data-testid="header-account"], .sep, [data-testid="header-automation"], [data-testid="header-pause-all"]')).map(
      (e) =>
        e.classList.contains('brand') ? 'brand'
        : e.classList.contains('command') ? 'command'
        : e.classList.contains('sep') ? 'separator'
        : e.dataset.testid!.replace('header-', ''),
    );
    const collapsed = parts.filter((p, i) => p !== 'account' || parts[i - 1] !== 'account');
    expect(collapsed).toEqual(['brand', 'command', 'account', 'separator', 'automation', 'pause-all']);
    expect(parts.filter((p) => p === 'account')).toHaveLength(2);
    expect(h.querySelector('.brand')!.textContent).toBe('Orbit Fleet');
    expect(h.querySelector('.command')!.textContent).toContain('Search or run a command');
    expect(h.querySelector('.command .of-kbd')!.textContent).toBe('⌘K');
  });

  it('sizes the header and the status bar with the board’s 44 px and 25 px, through tokens', () => {
    expect(boardPx('header')).toBe(44);
    expect(boardPx('footer')).toBe(25);
    expect(ROOT['header-h']).toBe(`${boardPx('header')}px`);
    expect(ROOT['status-h']).toBe(`${boardPx('footer')}px`);
    // The kit's header and status bar read those tokens ...
    const kit = css('src/lib/kit/of.generated.css');
    expect(kit).toMatch(/\.of-header\{height:var\(--header-h\)/);
    expect(kit).toMatch(/\.of-statusbar\{[^}]*height:var\(--status-h\)/);
    // ... and the shell's footer is that kit StatusBar, border included.
    expect(readFileSync('src/App.svelte', 'utf8')).toMatch(/<StatusBar testid="status-bar">/);
    expect(kit).toMatch(/\.of-statusbar\{box-sizing:border-box;/);
    // The board's 11 px status text.
    expect(BOARD).toMatch(/<footer style="[^"]*font-size: 11px/);
    expect(ROOT['text-2xs']).toBe('11px');
    expect(kit).toMatch(/\.of-statusbar\{[^}]*font-size:11px/);
  });
});
