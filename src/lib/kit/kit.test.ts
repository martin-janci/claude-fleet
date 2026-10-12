import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import { createRawSnippet, type Snippet } from 'svelte';
import { readFileSync, writeFileSync } from 'node:fs';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { kitCss } from './kit-extract';
import { THEME, composite, contrastRatio } from '../tokens';
import { platformChord } from './kbd';
import { OF_ICONS } from './icons';
import { OF_STATES } from './status';
import Button from './Button.svelte';
import Kbd from './Kbd.svelte';
import StatusChip from './StatusChip.svelte';
import StatusDot from './StatusDot.svelte';
import Banner from './Banner.svelte';
import QuestionCard from './QuestionCard.svelte';
import SessionRow from './SessionRow.svelte';
import ListFilters from './ListFilters.svelte';
import Rail from './Rail.svelte';
import AppHeader from './AppHeader.svelte';
import StatusBar from './StatusBar.svelte';
import Tabs from './Tabs.svelte';
import KeyValue from './KeyValue.svelte';
import Meter from './Meter.svelte';
import OrbitMark from './OrbitMark.svelte';
import Icon from './Icon.svelte';
import { expectAccessible } from '../a11y_check';

const MANUAL = 'docs/ux/2026-10-08-orbit-fleet-redesign/design-system/components/bundle.css';
const CSS_FILE = 'src/lib/kit/of.generated.css';
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const CSS = readFileSync(CSS_FILE, 'utf8');

const text = (s: string): Snippet => createRawSnippet(() => ({ render: () => `<span>${s}</span>` }));

afterEach(() => cleanup());

describe('kit stylesheet', () => {
  it('is the manual’s bundle.css', () => {
    const want = kitCss(readFileSync(MANUAL, 'utf8'));
    if (env.REGEN_KIT) {
      writeFileSync(CSS_FILE, want);
      throw new Error(`REGEN_KIT: wrote ${CSS_FILE}; read the diff, then run again without it`);
    }
    expect(CSS).toBe(want);
  });

  it('reads only tokens app.css declares', () => {
    const app = readFileSync('src/app.css', 'utf8');
    const used = [...new Set([...CSS.matchAll(/var\(--([\w-]+)\)/g)].map((m) => m[1]))];
    expect(used.length).toBeGreaterThan(30);
    expect(used.filter((v) => !new RegExp(`--${v}\\s*:`).test(app))).toEqual([]);
  });
});

// ── A small cascade over of.generated.css, enough to say which token colours
//    each piece of text and what it sits on, per state, in both themes.

interface Rule {
  selector: string;
  decls: Record<string, string>;
  order: number;
  spec: number;
}

const RULES: Rule[] = (() => {
  const out: Rule[] = [];
  const body = CSS.replace(/\/\*[\s\S]*?\*\//g, '').replace(/@media[^{]*\{([\s\S]*?\})\}/g, '');
  let order = 0;
  for (const m of body.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const decls: Record<string, string> = {};
    for (const d of m[2].split(';')) {
      const i = d.indexOf(':');
      if (i > 0) decls[d.slice(0, i).trim()] = d.slice(i + 1).trim();
    }
    for (const sel of m[1].split(',')) {
      const s = sel.trim();
      if (!s || s.includes('::')) continue;
      const spec = (s.match(/[.[:]/g)?.length ?? 0) * 10 + (s.match(/(^|[\s>+~])[a-z]/g)?.length ?? 0);
      out.push({ selector: s, decls, order: order++, spec });
    }
  }
  return out;
})();

type State = 'base' | 'hover';

function matches(el: Element, rule: Rule, state: State): boolean {
  const hover = rule.selector.includes(':hover');
  if (hover && state !== 'hover') return false;
  const sel = rule.selector.replace(/:hover/g, '');
  if (sel.includes(':focus-visible')) return false;
  try {
    return el.matches(sel);
  } catch {
    return false;
  }
}

function decl(el: Element, prop: string, state: State): string | undefined {
  const inline = (el as HTMLElement).style?.getPropertyValue(prop);
  if (inline) return inline;
  let best: Rule | undefined;
  for (const r of RULES) {
    if (r.decls[prop] === undefined || !matches(el, r, state)) continue;
    if (!best || r.spec > best.spec || (r.spec === best.spec && r.order > best.order)) best = r;
  }
  return best?.decls[prop];
}

const token = (v: string | undefined) => v?.match(/^var\(--([\w-]+)\)/)?.[1];

/** The token colouring `el`'s text: its own rule or the nearest ancestor's. */
function fgOf(el: Element, state: State): string {
  for (let e: Element | null = el; e; e = e.parentElement) {
    const t = token(decl(e, 'color', state));
    if (t) return t;
  }
  return 'fg';
}

/** The opaque colour under `el` in `theme`, compositing translucent tints,
 *  and the tokens it is made of ("failed-soft/bg-pane"). */
function groundOf(el: Element, state: State, theme: 'light' | 'dark'): { hex: string; chain: string } {
  const layers: string[] = [];
  for (let e: Element | null = el; e; e = e.parentElement) {
    const t = token(decl(e, 'background', state));
    if (!t) continue;
    layers.push(t);
    if (!THEME[theme][t].startsWith('rgba')) break;
  }
  if (!layers.length || THEME[theme][layers[layers.length - 1]].startsWith('rgba')) layers.push('bg-pane');
  let hex = '';
  for (const t of [...layers].reverse()) {
    const c = THEME[theme][t];
    hex = c.startsWith('rgba') ? composite(c, hex) : c;
  }
  return { hex, chain: layers.join('/') };
}

/** Every element that holds text of its own, with its fg and ground per theme. */
function textColours(root: Element, state: State) {
  const out: { text: string; fg: string; ground: string; light: number; dark: number }[] = [];
  for (const el of [root, ...Array.from(root.querySelectorAll('*'))]) {
    if ((el as HTMLButtonElement).disabled) continue; // WCAG exempts disabled controls
    const own = Array.from(el.childNodes)
      .filter((n) => n.nodeType === 3)
      .map((n) => n.textContent ?? '')
      .join('')
      .trim();
    if (!own) continue;
    const fg = fgOf(el, state);
    const ratio = (theme: 'light' | 'dark') => {
      const ground = groundOf(el, state, theme).hex;
      const c = THEME[theme][fg];
      return contrastRatio(c.startsWith('rgba') ? composite(c, ground) : c, ground);
    };
    const ground = groundOf(el, state, 'light').chain;
    out.push({ text: own, fg, ground, light: +ratio('light').toFixed(2), dark: +ratio('dark').toFixed(2) });
  }
  return out;
}

/**
 * Pairs the manual's own tokens leave under 4.5:1, pinned at their measured
 * worst theme so a token change that moves them fails here. Empty: the three
 * step 0.9 found (fg-muted on bg-hover, fg-muted on failed-soft, status-failed
 * on accent-soft) are fixed in the manual's bundle.css by the component
 * reading fg-2 or fg in that state, not by moving a token value.
 */
const KNOWN_SHORTFALLS: Record<string, number> = {};

function expectReadable(root: Element, what: string) {
  for (const state of ['base', 'hover'] as const) {
    for (const t of textColours(root, state)) {
      const worst = Math.min(t.light, t.dark);
      const pair = `${t.fg} on ${t.ground}`;
      const known = KNOWN_SHORTFALLS[pair];
      const msg = `${what} (${state}): "${t.text}" ${pair}: ${t.light} light, ${t.dark} dark`;
      if (known !== undefined && worst < 4.5) expect(worst, msg).toBe(known);
      else expect(worst, msg).toBeGreaterThanOrEqual(4.5);
    }
  }
}

it('every known shortfall is still real', () => {
  for (const [pair, ratio] of Object.entries(KNOWN_SHORTFALLS)) {
    const [fg, ground] = pair.split(' on ');
    const worst = Math.min(
      ...(['light', 'dark'] as const).map((theme) => {
        let hex = '';
        for (const t of ground.split('/').reverse()) {
          const c = THEME[theme][t];
          hex = c.startsWith('rgba') ? composite(c, hex) : c;
        }
        return contrastRatio(THEME[theme][fg], hex);
      }),
    );
    expect(+worst.toFixed(2), pair).toBe(ratio);
  }
});

/** Every of- class the kit renders is one the manual's stylesheet styles. */
function expectManualClasses(root: Element) {
  const classes = new Set<string>();
  for (const el of [root, ...Array.from(root.querySelectorAll('*'))]) for (const c of Array.from(el.classList)) if (!c.startsWith('svelte-')) classes.add(c);
  const styled = [...classes].filter((c) => c.startsWith('of-'));
  expect(styled.length).toBeGreaterThan(0);
  for (const c of styled) expect(CSS, `.${c} is not in the manual's bundle.css`).toContain(`.${c}`);
}

function check(root: Element, what: string) {
  expectManualClasses(root);
  expectReadable(root, what);
}

describe('kit components follow the manual', () => {
  it.each(['default', 'quiet', 'primary', 'danger', 'danger-fill'] as const)('Button %s', (variant) => {
    // The manual puts a key hint only in default and primary buttons (question cards).
    const kbd = variant === 'default' || variant === 'primary' ? '1' : undefined;
    render(Button, { variant, kbd, mac: true, children: text('Approve'), testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.classList).toContain('of-btn');
    if (variant !== 'default') expect(b.classList).toContain(variant);
    check(b, `Button ${variant}`);
  });

  it('Button sizes, icon, disabled and busy', async () => {
    const onclick = vi.fn();
    render(Button, { size: 'sm', children: text('Switch account'), testid: 'sm', onclick });
    render(Button, { size: 'lg', variant: 'primary', children: text('Start session'), testid: 'lg' });
    render(Button, { icon: true, variant: 'primary', label: 'Send', children: text('↑'), testid: 'ic' });
    render(Button, { disabled: true, children: text('Push'), testid: 'off' });
    render(Button, { busy: true, busyLabel: 'Starting…', variant: 'primary', children: text('Start'), testid: 'busy' });
    expect(screen.getByTestId('sm').classList).toContain('sm');
    expect(screen.getByTestId('lg').classList).toContain('lg');
    expect(screen.getByRole('button', { name: 'Send' }).classList).toContain('icon');
    expect((screen.getByTestId('off') as HTMLButtonElement).disabled).toBe(true);
    const busy = screen.getByTestId('busy');
    expect(busy.textContent).toContain('Starting…');
    expect(busy.getAttribute('aria-busy')).toBe('true');
    expect((busy as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByTestId('sm'));
    expect(onclick).toHaveBeenCalledOnce();
    check(screen.getByTestId('sm'), 'Button sm');
  });

  it('Kbd shows the platform chord and keeps the Mac one in its title', () => {
    expect(platformChord('⌘K', false)).toBe('Ctrl+K');
    expect(platformChord('⌥⌘T', false)).toBe('Ctrl+Alt+T');
    expect(platformChord('⌘⇧E', false)).toBe('Ctrl+Shift+E');
    expect(platformChord('⌘,', false)).toBe('Ctrl+,');
    expect(platformChord('1', false)).toBe('1');
    expect(platformChord('⌥⌘T', true)).toBe('⌥⌘T');
    const { container } = render(Kbd, { chord: '⌥⌘T', mac: false });
    const k = container.querySelector('.of-kbd')!;
    expect(k.textContent).toBe('Ctrl+Alt+T');
    expect(k.getAttribute('title')).toBe('⌥⌘T');
    check(k, 'Kbd');
  });

  it.each([...OF_STATES, 'accent', undefined] as const)('StatusChip %s keeps its word', (state) => {
    render(StatusChip, { state, label: state ? undefined : 'mac', testid: 'c' });
    const c = screen.getByTestId('c');
    expect(c.textContent?.trim()).toBe(
      { waiting: 'Needs you', working: 'Working', failed: 'Failed', done: 'Done', idle: 'Idle', accent: '', none: 'mac' }[state ?? 'none'],
    );
    if (state) expect(c.classList).toContain(state);
    if (state !== 'accent') check(c, `StatusChip ${state ?? 'neutral'}`);
  });

  it('StatusDot always has a label unless a word beside it says the state', () => {
    render(StatusDot, { state: 'failed' });
    expect(screen.getByRole('img', { name: 'Failed' }).classList).toContain('failed');
    const { container } = render(StatusDot, { state: 'done', label: null });
    expect(container.querySelector('.of-dot.done')!.getAttribute('aria-hidden')).toBe('true');
  });

  it.each(['waiting', 'failed'] as const)('Banner %s', (tone) => {
    render(Banner, {
      tone,
      headline: 'Reconnecting to the hub',
      meta: 'Lost at 14:52 · try 3 · your sessions keep running on their hosts',
      testid: 'banner',
    });
    const b = screen.getByTestId('banner');
    expect(b.classList).toContain(tone);
    expect(b.getAttribute('role')).toBe(tone === 'failed' ? 'alert' : 'status');
    check(b, `Banner ${tone}`);
  });

  it('QuestionCard numbers the answers in the agent’s order and answers from 1–3', async () => {
    const picks: string[] = [];
    const answers = ['Approve', 'Approve for this session', 'Deny'].map((label, i) => ({
      label,
      primary: i === 0,
      onselect: () => picks.push(label),
    }));
    const own = vi.fn();
    render(QuestionCard, {
      question: 'Push to origin needs your OK',
      age: 'asked 2m ago',
      detail: 'git push -u origin fix-hub-e2e-federation-pair-flake',
      answers,
      onownwords: own,
      mac: true,
      testid: 'q',
    });
    const q = screen.getByTestId('q');
    const buttons = [...Array.from(q.querySelectorAll('button'))].map((b) => b.textContent?.trim());
    expect(buttons).toEqual(['1Approve', '2Approve for this session', '3Deny', 'Answer in your own words…']);
    expect(q.textContent).toContain('git push -u origin fix-hub-e2e-federation-pair-flake');
    await fireEvent.keyDown(q, { key: '2' });
    await fireEvent.click(screen.getByText('Answer in your own words…'));
    expect(picks).toEqual(['Approve for this session']);
    expect(own).toHaveBeenCalledOnce();
    check(q, 'QuestionCard');
  });

  it.each([
    ['unselected', false],
    ['selected', true],
  ] as const)('SessionRow %s, in every state', (_name, selected) => {
    for (const state of OF_STATES) {
      const onselect = vi.fn();
      render(SessionRow, {
        state,
        title: 'Fix hub-e2e flake',
        age: '2m',
        lead: state === 'failed' ? 'Failed:' : 'Waiting for you:',
        line: 'approve push to main',
        selected,
        onselect,
        chips: text('PR #476'),
        testid: `row-${state}`,
      });
      const row = screen.getByTestId(`row-${state}`);
      expect(row.getAttribute('aria-selected')).toBe(String(selected));
      expect(row.querySelector('.of-dot')!.getAttribute('aria-label')).toBeTruthy();
      check(row, `SessionRow ${state}${selected ? ' selected' : ''}`);
    }
  });

  it('ListFilters keeps search, filter chips and grouping in view', async () => {
    const onremove = vi.fn();
    render(ListFilters, {
      title: 'Inbox',
      countText: '4 need you',
      placeholder: 'Search sessions, tasks, hosts',
      searchLabel: 'Search sessions',
      filters: [
        { id: 'host:mac', label: 'mac' },
        { id: 'project:pos', label: 'Papaya POS' },
      ],
      onremove,
      grouping: 'state',
      testid: 'lf',
    });
    const lf = screen.getByTestId('lf');
    expect(screen.getByRole('textbox', { name: 'Search sessions' })).toBeTruthy();
    expect(screen.getByRole('button', { name: /Filters/ }).textContent).toContain('2');
    expect(screen.getByRole('button', { name: 'Group: state ▾' })).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove filter Papaya POS' }));
    expect(onremove).toHaveBeenCalledWith('project:pos');
    check(lf, 'ListFilters');
  });

  it('Rail: manual order, current page, Inbox badge, Settings last', async () => {
    const onselect = vi.fn();
    render(Rail, {
      items: [
        { id: 'control', label: 'Control', icon: 'control', title: 'Control  ⌘E' },
        { id: 'inbox', label: 'Inbox', icon: 'inbox', badge: 4 },
        { id: 'sessions', label: 'Sessions', icon: 'sessions', title: 'Sessions  ⌘⇧W' },
        { id: 'settings', label: 'Settings', icon: 'settings', title: 'Settings  ⌘,', bottom: true },
      ],
      current: 'inbox',
      onselect,
      testid: 'rail',
    });
    const rail = screen.getByTestId('rail');
    const links = [...Array.from(rail.querySelectorAll('a'))];
    expect(links.map((a) => a.dataset.rail)).toEqual(['control', 'inbox', 'sessions', 'settings']);
    expect(rail.querySelector('.grow')!.nextElementSibling).toBe(links[3]);
    expect(links[1].getAttribute('aria-current')).toBe('page');
    expect(links[0].getAttribute('title')).toBe('Control  ⌘E');
    expect(links[1].querySelector('.badge')!.textContent).toBe('4');
    await fireEvent.click(links[2]);
    expect(onselect).toHaveBeenCalledWith('sessions');
    check(rail, 'Rail');
  });

  it('AppHeader: mark, name and the ⌘K field', async () => {
    const oncommand = vi.fn();
    render(AppHeader, { oncommand, mac: false, children: text('m.janci@32bit.sk'), testid: 'h' });
    const h = screen.getByTestId('h');
    expect(h.querySelector('.of-mark')).toBeTruthy();
    expect(h.textContent).toContain('Orbit Fleet');
    // The switcher's chord off the Mac (plain Ctrl+K is the terminal's).
    expect(h.querySelector('.of-kbd')!.textContent).toBe('Ctrl+Shift+K');
    await fireEvent.click(screen.getByRole('button', { name: /Search or run a command/ }));
    expect(oncommand).toHaveBeenCalledOnce();
    check(h, 'AppHeader');
  });

  it('StatusBar', () => {
    render(StatusBar, { children: text('All systems OK'), end: text('? Shortcuts'), testid: 'sb' });
    const sb = screen.getByTestId('sb');
    expect(sb.querySelector('.end')!.textContent).toBe('? Shortcuts');
    check(sb, 'StatusBar');
  });

  it('Tabs: selected by bar and weight, arrows move', async () => {
    let selected = 'conversation';
    const onselect = vi.fn((id: string) => (selected = id));
    render(Tabs, {
      tabs: [
        { id: 'conversation', label: 'Conversation' },
        { id: 'agent', label: 'Claude Code', kbd: '⌘J' },
        { id: 'terminals', label: 'Terminals', count: 2, kbd: '⌥⌘T' },
      ],
      selected,
      onselect,
      label: 'Session views',
      mac: true,
      testid: 'tabs',
    });
    const tabs = screen.getAllByRole('tab');
    expect(tabs.map((t) => t.getAttribute('aria-selected'))).toEqual(['true', 'false', 'false']);
    expect(CSS).toMatch(/\.of-tab\[aria-selected="true"\]\{[^}]*border-bottom-color:var\(--accent\);font-weight:500/);
    await fireEvent.keyDown(tabs[0], { key: 'ArrowLeft' });
    expect(onselect).toHaveBeenLastCalledWith('terminals');
    await fireEvent.click(tabs[1]);
    expect(onselect).toHaveBeenLastCalledWith('agent');
    check(screen.getByTestId('tabs'), 'Tabs');
  });

  it('KeyValue', () => {
    render(KeyValue, {
      items: [
        { label: 'Account', value: 'm.janci@32bit.sk' },
        { label: 'Branch', value: 'fix-hub-e2e-federation-pair-flake', mono: true },
        { label: 'Cost', value: '$8.51', tnum: true },
      ],
      testid: 'kv',
    });
    const kv = screen.getByTestId('kv');
    expect([...Array.from(kv.querySelectorAll('dt'))].map((d) => d.textContent)).toEqual(['Account', 'Branch', 'Cost']);
    expect(kv.querySelectorAll('dd')[1].classList).toContain('mono');
    check(kv, 'KeyValue');
  });

  it.each(['ok', 'warn', 'crit'] as const)('Meter %s', (level) => {
    render(Meter, { value: 0.82, level, label: 'Week 82% used', testid: 'm' });
    const m = screen.getByRole('meter', { name: 'Week 82% used' });
    expect(m.getAttribute('aria-valuenow')).toBe('82');
    expect((m.firstElementChild as HTMLElement).style.width).toBe('82%');
    if (level !== 'ok') expect(m.classList).toContain(level);
    expectManualClasses(m);
  });

  it('OrbitMark draws the manual’s mark', () => {
    render(OrbitMark, { size: 28 });
    const mark = screen.getByRole('img', { name: 'Orbit Fleet' });
    expect(mark.getAttribute('width')).toBe('28');
    expect(mark.querySelectorAll('.a')).toHaveLength(1);
    expect(mark.querySelectorAll('circle')).toHaveLength(5);
  });

  it('Icon: the 16-unit, 1.5 px set', () => {
    expect(Object.keys(OF_ICONS)).toEqual(
      expect.arrayContaining(['control', 'inbox', 'sessions', 'work', 'automation', 'accounts', 'toolkit', 'settings']),
    );
    expect(CSS).toMatch(/\.of-ico\{fill:none;stroke:currentColor;stroke-width:1\.5/);
    for (const name of Object.keys(OF_ICONS) as (keyof typeof OF_ICONS)[]) {
      const { container } = render(Icon, { name });
      const svg = container.querySelector('svg')!;
      expect(svg.getAttribute('viewBox')).toBe('0 0 16 16');
      expect(svg.classList).toContain('of-ico');
      expect(svg.childElementCount).toBe(OF_ICONS[name].length);
    }
  });
});

describe('the kit’s colours, light and dark', () => {
  it('match the snapshot of what each state resolves to', () => {
    const rows: Record<string, unknown> = {};
    const add = (name: string, root: Element) => {
      rows[name] = (['base', 'hover'] as const).flatMap((state) =>
        textColours(root, state).map((t) => `${state} "${t.text}" ${t.fg} on ${t.ground} ${t.light}/${t.dark}`),
      );
    };
    for (const variant of ['default', 'quiet', 'primary', 'danger', 'danger-fill'] as const) {
      const kbd = variant === 'default' || variant === 'primary' ? '1' : undefined;
      render(Button, { variant, kbd, mac: true, children: text('Go'), testid: `b-${variant}` });
      add(`Button ${variant}`, screen.getByTestId(`b-${variant}`));
    }
    for (const state of [...OF_STATES, 'accent'] as const) {
      render(StatusChip, { state, label: state, testid: `c-${state}` });
      add(`StatusChip ${state}`, screen.getByTestId(`c-${state}`));
    }
    for (const tone of ['waiting', 'failed'] as const) {
      render(Banner, { tone, headline: 'Headline', meta: 'Meta', testid: `bn-${tone}` });
      add(`Banner ${tone}`, screen.getByTestId(`bn-${tone}`));
    }
    render(SessionRow, { state: 'waiting', title: 'Title', age: '2m', lead: 'Waiting:', line: 'line', selected: true, testid: 'r' });
    add('SessionRow selected', screen.getByTestId('r'));
    expect(rows).toMatchSnapshot();
  });
});

describe('kit accessibility (7.2)', () => {
  it('every component passes the axe and audit checks', async () => {
    render(Button, { variant: 'primary', kbd: '⌘↵', mac: true, children: text('Approve') });
    render(Button, { icon: true, label: 'Send', children: text('↑') });
    render(Button, { busy: true, busyLabel: 'Starting…', children: text('Start') });
    render(StatusChip, { state: 'waiting' });
    render(StatusDot, { state: 'failed' });
    render(QuestionCard, {
      question: 'Push to origin needs your OK',
      answers: [{ label: 'Approve', primary: true, onselect: () => {} }, { label: 'Deny', onselect: () => {} }],
      onownwords: () => {},
      mac: true,
    });
    // A SessionRow is an option: it lives in the list's listbox.
    const list = document.body.appendChild(document.createElement('div'));
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Sessions');
    render(SessionRow, {
      props: { state: 'waiting', title: 'Fix hub-e2e flake', age: '2m', line: 'approve push', selected: true, onselect: () => {} },
      target: list,
    });
    render(ListFilters, {
      title: 'Inbox',
      placeholder: 'Search sessions',
      searchLabel: 'Search sessions',
      filters: [{ id: 'host:mac', label: 'mac' }],
      onremove: () => {},
      grouping: 'state',
    });
    render(Rail, { items: [{ id: 'inbox', label: 'Inbox', icon: 'inbox', badge: 4 }], current: 'inbox', onselect: () => {} });
    render(AppHeader, { oncommand: () => {}, mac: true, children: text('me') });
    render(Tabs, { tabs: [{ id: 'a', label: 'Conversation' }, { id: 'b', label: 'Terminals', count: 2 }], selected: 'a', onselect: () => {}, label: 'Session views', mac: true });
    render(KeyValue, { items: [{ label: 'Account', value: 'm' }] });
    render(Meter, { value: 0.5, level: 'ok', label: 'Week 50% used' });
    await expectAccessible(document.body);
    list.remove();
  });
});
