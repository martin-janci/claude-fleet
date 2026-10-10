// Redesign step 1.4, "Verified by: chrome above the first row measured".
// 1.4 took the permanent rows out of the list's chrome (the theme line, the
// board's instruction sentence, empty task sections, a busy status bar).
// This renders the session list and measures what sits above
// its first session row: which lines they are, and how tall they come out
// from the tokens that size them (jsdom lays nothing out, so the height is
// computed from the header's own CSS and app.css's tokens). A line added
// above the first row fails here, with the new total.
import { readFileSync } from 'node:fs';
import { render } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import Sidebar from './Sidebar.svelte';
import { projects } from './projects';
import { sessions } from './sessions';
import { onboardingDismissed } from './onboarding';
import { session } from './hosts_fixture';

const ROOT: Record<string, string> = (() => {
  const app = readFileSync('src/app.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  const root = app.slice(app.indexOf(':root'), app.indexOf('}', app.indexOf(':root')));
  return Object.fromEntries([...root.matchAll(/--([a-z0-9-]+)\s*:\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
})();
const tok = (name: string) => parseFloat(ROOT[name]);
/** A CSS length in px: a number, or a `var(--token)` read from app.css. */
const px = (v: string) => {
  const m = v.match(/^var\(--([a-z0-9-]+)\)$/);
  return m ? tok(m[1]) : parseFloat(v);
};

/** The header's box from its own rule in SidebarFilters.svelte: padding, gap and border. */
const HEADER = (() => {
  const src = readFileSync('src/lib/SidebarFilters.svelte', 'utf8');
  const rule = src.match(/\.sidebar-header\s*\{([^}]*)\}/)![1];
  const [top, , bottom] = rule.match(/padding:\s*([^;]+);/)![1].trim().split(/\s+/).map(px);
  return { top, bottom: bottom ?? top, gap: px(rule.match(/gap:\s*([^;]+);/)![1].trim()), border: 1 };
})();

/** A chrome line is one control tall: the tallest control it holds. */
const LINE_PX = tok('control-h-lg');

interface Chrome {
  lines: string[];
  px: number;
}

/** Every line drawn above the first session row, and its height. */
function chromeAboveFirstRow(container: HTMLElement): Chrome {
  const first = container.querySelector('[data-testid="sess-row"]');
  expect(first, 'a session row').not.toBeNull();
  const header = container.querySelector<HTMLElement>('[data-testid="sidebar-chrome-top"]')!;
  expect(header.compareDocumentPosition(first!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  // The header's lines: children that draw something (not a screen-reader
  // note, not an empty attention line).
  const headerLines = Array.from(header.children).filter(
    (c) => !c.classList.contains('sr-only') && (c.textContent ?? '').trim() !== '',
  );
  // Then whatever the list draws before the row: its group header.
  const groupHeaders = Array.from(container.querySelectorAll('[data-testid="proj-row"], [data-testid="row-group-header"]')).filter(
    (el) => el.compareDocumentPosition(first!) & Node.DOCUMENT_POSITION_FOLLOWING,
  );
  const name = (el: Element) =>
    el.getAttribute('data-testid') ?? Array.from(el.classList).filter((c) => !c.startsWith('svelte-')).join('.');
  const lines = [...headerLines.map(name), ...groupHeaders.map(name)];
  const px =
    HEADER.top + HEADER.bottom + HEADER.border + headerLines.length * LINE_PX + Math.max(0, headerLines.length - 1) * HEADER.gap +
    groupHeaders.length * LINE_PX;
  return { lines, px };
}

async function mount() {
  onboardingDismissed.set(true);
  projects.set([{ project: { id: 1, owner: 'o', repo: 'r', base_path: '/r', last_session_at: 1, adopted: false, system: false }, worktrees: [] }] as never);
  sessions.set([session('local', 'dev-a', { project_id: 1 } as never)]);
  const r = render(Sidebar);
  await tick();
  await tick();
  return r;
}

describe('chrome above the first row (step 1.4)', () => {
  it('the view switch and one Filters line, then the project', async () => {
    const { container } = await mount();
    const c = chromeAboveFirstRow(container);
    expect(c.lines).toEqual(['row.r0', 'filters-section', 'proj-row']);
    expect(c.px).toBe(105);
    // What 1.4 removed stays removed, and it stays under four control lines.
    expect(container.textContent).not.toMatch(/theme:/);
    expect(container.querySelector('[data-testid="theme-toggle"]')).toBeNull();
    expect(c.lines.length).toBeLessThanOrEqual(4);
  });
});
