/**
 * The theme palette as data, mirroring the four `:root` blocks in app.css,
 * so the contrast floors the design manual claims are actually asserted.
 * Nothing imports this at runtime; it exists to be tested.
 *
 * Three copies of one palette, held together by `tokens.test.ts`:
 * `docs/design/tokens.json` (the snapshot of the Orbit Fleet design manual,
 * the source of truth), app.css (what ships) and THEME below (what the
 * contrast suite measures). Every colour the snapshot names must appear here
 * with its value resolved (`{accent}` becomes the accent's hex), and every
 * entry here must match all four app.css blocks. A colour edited in one place
 * only fails the suite.
 *
 * When you add or change a token: the snapshot first, then app.css, then
 * here, and add the pair it has to clear to CONTRAST_PAIRS.
 */

export interface ContrastPair {
  /** Key in THEME for the foreground / border colour. */
  fg: string;
  /** Key in THEME for the surface it sits on. */
  bg: string;
  /**
   * Key in THEME for a translucent `rgba()` tint laid over `bg` (a chip or
   * banner's `*-soft`); the pair is measured on the composited colour.
   */
  tint?: string;
  /** WCAG floor: 4.5 for text, 3 for non-text. */
  min: number;
  note: string;
}

export const THEME: Record<'light' | 'dark', Record<string, string>> = {
  light: {
    'bg': '#f7f7f8',
    'bg-pane': '#ffffff',
    'bg-raise': '#f2f2f4',
    'bg-hover': '#ebebee',
    'bg-sunk': '#f2f2f4',
    'fg': '#18181b',
    'fg-2': '#3f3f46',
    'fg-muted': '#6b6b74',
    'border': '#e4e4e7',
    'control-border': '#d4d4d8',
    'accent': '#2563eb',
    'accent-fg': '#ffffff',
    'accent-soft': '#e7eefe',
    'ring': '#2563eb',
    'status-working': '#3157c9',
    'status-waiting': '#8f520b',
    'status-failed': '#c62828',
    'status-done': '#17723e',
    'status-idle': '#6b6b74',
    'on-waiting': '#ffffff',
    'waiting-soft': 'rgba(143,82,11,0.13)',
    'waiting-faint': 'rgba(143,82,11,0.07)',
    'waiting-line': 'rgba(143,82,11,0.45)',
    'failed-soft': 'rgba(198,40,40,0.12)',
    'failed-line': 'rgba(198,40,40,0.35)',
    'done-soft': 'rgba(23,114,62,0.12)',
    'danger': '#c62828',
    'danger-fill': '#c62828',
    'on-danger': '#ffffff',
    'chip-bg': '#efeff2',
    'count-bg': '#e9e9ec',
    'track': '#e4e4e7',
    'code': '#0e7490',
    'syn-kw': '#8a3fb0',
    'syn-str': '#3f7d20',
    'syn-num': '#9a5a00',
    'org-1': '#2563eb',
    'org-2': '#17723e',
    'org-3': '#8a3fb0',
    'org-4': '#6b6b74',
    'brand-ink': '#1b2430',
    'brand-light': '#f2f4f7',
    'brand-amber': '#d29b4a',
    'loader-accent': '#60a5fa',
    'comet-head': '#2563eb',
    'agent-claude': '#d97757',
    'term-bg': '#0a0a0a',
    'term-fg': '#e8e8e8',
    // App tokens the manual adopted in review r10 (aliases resolved).
    'usage-ok': '#17723e',
    'usage-warn': '#8f520b',
    'usage-crit': '#c62828',
    'syn-code': '#0e7490',
    'control-bg': '#ffffff',
    'control-bg-hover': '#f0f0f0',
    'control-bg-active': '#e4e4e4',
    'control-border-strong': '#8e8e8e',
    'control-fg': '#1a1a1a',
    'control-fg-quiet': '#5a5a5a',
    'ai-pre': '#2563eb',
    'scrim': 'rgba(24,24,27,0.4)',
    'scrim-strong': 'rgba(24,24,27,0.62)',
  },
  dark: {
    'bg': '#0f0f0f',
    'bg-pane': '#161616',
    'bg-raise': '#1c1c1c',
    'bg-hover': '#232323',
    'bg-sunk': '#202020',
    'fg': '#ededed',
    'fg-2': '#b4b4b4',
    'fg-muted': '#8f8f8f',
    'border': '#2a2a2a',
    'control-border': '#3a3a3a',
    'accent': '#60a5fa',
    'accent-fg': '#0b1220',
    'accent-soft': '#1c2735',
    'ring': '#60a5fa',
    'status-working': '#7fa3ff',
    'status-waiting': '#d29b4a',
    'status-failed': '#ef5350',
    'status-done': '#5dd17a',
    'status-idle': '#a09fa8',
    'on-waiting': '#1a1205',
    'waiting-soft': 'rgba(210,155,74,0.13)',
    'waiting-faint': 'rgba(210,155,74,0.07)',
    'waiting-line': 'rgba(210,155,74,0.45)',
    'failed-soft': 'rgba(239,83,80,0.12)',
    'failed-line': 'rgba(239,83,80,0.35)',
    'done-soft': 'rgba(93,209,122,0.12)',
    'danger': '#ef5350',
    'danger-fill': '#c62828',
    'on-danger': '#ffffff',
    'chip-bg': '#232323',
    'count-bg': '#262626',
    'track': '#2a2a2a',
    'code': '#56b6c2',
    'syn-kw': '#c678dd',
    'syn-str': '#98c379',
    'syn-num': '#d19a66',
    'org-1': '#60a5fa',
    'org-2': '#5dd17a',
    'org-3': '#c084fc',
    'org-4': '#a09fa8',
    'brand-ink': '#1b2430',
    'brand-light': '#f2f4f7',
    'brand-amber': '#d29b4a',
    'loader-accent': '#60a5fa',
    'comet-head': '#f2f4f7',
    'agent-claude': '#d97757',
    'term-bg': '#0a0a0a',
    'term-fg': '#e8e8e8',
    // App tokens the manual adopted in review r10 (aliases resolved).
    'usage-ok': '#5dd17a',
    'usage-warn': '#d29b4a',
    'usage-crit': '#ef5350',
    'syn-code': '#56b6c2',
    'control-bg': '#1c1c1c',
    'control-bg-hover': '#262626',
    'control-bg-active': '#303030',
    'control-border-strong': '#6e6e6e',
    'control-fg': '#ededed',
    'control-fg-quiet': '#a8a8a8',
    'ai-pre': '#60a5fa',
    'scrim': 'rgba(0,0,0,0.4)',
    'scrim-strong': 'rgba(0,0,0,0.62)',
  },
};

const STATUSES = ['working', 'waiting', 'failed', 'done', 'idle'] as const;

export const CONTRAST_PAIRS: ContrastPair[] = [
  // Text: the manual promises fg, fg-2 and fg-muted at 4.5:1 on bg, bg-pane
  // and bg-raise in both themes.
  ...(['fg', 'fg-2', 'fg-muted'] as const).flatMap((fg): ContrastPair[] =>
    (['bg', 'bg-pane', 'bg-raise'] as const).map((bg) => ({ fg, bg, min: 4.5, note: `${fg} text on ${bg}` })),
  ),
  { fg: 'fg', bg: 'bg-hover', min: 4.5, note: 'a hovered row title' },
  { fg: 'fg', bg: 'accent-soft', min: 4.5, note: 'a selected row title, and the draft over a drag tint' },
  { fg: 'fg-2', bg: 'chip-bg', min: 4.5, note: 'chip text' },
  { fg: 'term-fg', bg: 'term-bg', min: 4.5, note: 'terminal text on its own ground, both themes' },
  { fg: 'fg', bg: 'count-bg', min: 4.5, note: 'count badges' },
  { fg: 'fg-2', bg: 'count-bg', min: 4.5, note: '.count-badge text' },
  { fg: 'status-working', bg: 'chip-bg', min: 4.5, note: '.state-chip--working' },
  { fg: 'status-idle', bg: 'chip-bg', min: 4.5, note: '.state-chip--idle' },
  // Action and focus.
  { fg: 'accent', bg: 'bg', min: 3, note: 'focus ring' },
  { fg: 'ring', bg: 'bg', min: 3, note: 'the focus ring alias' },
  { fg: 'ring', bg: 'bg-pane', min: 3, note: 'focus ring on a pane' },
  { fg: 'accent', bg: 'accent-soft', min: 3, note: 'selected-row bar and drag border over their tint' },
  { fg: 'accent-fg', bg: 'accent', min: 4.5, note: 'text on a filled primary' },
  // Status words are text (chips, the row's status label), so each clears
  // the text floor on both grounds a row sits on.
  ...STATUSES.flatMap((s): ContrastPair[] => [
    { fg: `status-${s}`, bg: 'bg-pane', min: 4.5, note: `${s} status text` },
    { fg: `status-${s}`, bg: 'bg', min: 4.5, note: `${s} status text on the page ground` },
  ]),
  // Chips and banners keep the full status colour as text over their tint.
  { fg: 'status-waiting', bg: 'bg-pane', tint: 'waiting-soft', min: 4.5, note: 'Needs you chip' },
  { fg: 'status-waiting', bg: 'bg-pane', tint: 'waiting-faint', min: 4.5, note: 'waiting banner' },
  { fg: 'fg', bg: 'bg-pane', tint: 'waiting-faint', min: 4.5, note: 'body text in a waiting banner' },
  { fg: 'status-failed', bg: 'bg-pane', tint: 'failed-soft', min: 4.5, note: 'Failed chip' },
  { fg: 'fg', bg: 'bg-pane', tint: 'failed-soft', min: 4.5, note: 'body text in a failed banner' },
  { fg: 'status-done', bg: 'bg-pane', tint: 'done-soft', min: 4.5, note: 'Done chip' },
  { fg: 'on-waiting', bg: 'status-waiting', min: 4.5, note: 'text on a filled waiting badge' },
  { fg: 'status-working', bg: 'accent-soft', min: 4.5, note: 'a frozen session chip (step 7.3)' },
  // Destructive.
  { fg: 'danger', bg: 'bg-pane', min: 4.5, note: 'error text and invalid fields' },
  { fg: 'danger', bg: 'bg', min: 4.5, note: 'error text on the page ground' },
  { fg: 'on-danger', bg: 'danger-fill', min: 5.6, note: 'a destructive confirm button (5.6:1 in the manual)' },
  // Usage meter, aliases of the status colours.
  { fg: 'usage-ok', bg: 'bg-pane', min: 4.5, note: 'healthy context meter' },
  { fg: 'usage-warn', bg: 'bg-pane', min: 4.5, note: 'warn text and bars' },
  { fg: 'usage-crit', bg: 'bg-pane', min: 4.5, note: 'crit text and bars' },
  { fg: 'status-working', bg: 'track', min: 3, note: 'a meter fill on its track' },
  // History graph lanes (CommitGraph's palette, step 7.3): lines and dots,
  // so the non-text floor, on the pane the graph is drawn on.
  ...(['org-3', 'code', 'syn-num', 'syn-str', 'syn-kw', 'status-idle'] as const).map(
    (fg): ContrastPair => ({ fg, bg: 'bg-pane', min: 3, note: `${fg} as a history-graph lane` }),
  ),
  // Code, in the bg-sunk well it sits in.
  ...(['code', 'syn-kw', 'syn-str', 'syn-num'] as const).map((fg): ContrastPair => ({ fg, bg: 'bg-sunk', min: 4.5, note: `${fg} in a code well` })),
  // Organisation swatches are non-text.
  ...([1, 2, 3, 4] as const).map((n): ContrastPair => ({ fg: `org-${n}`, bg: 'bg-pane', min: 3, note: `org ${n} swatch` })),
  // Classic controls.
  { fg: 'control-fg-quiet', bg: 'bg-pane', min: 4.5, note: 'quiet control labels' },
  { fg: 'control-border-strong', bg: 'bg-pane', min: 3, note: 'state boundaries' },
  { fg: 'control-fg', bg: 'control-bg', min: 4.5, note: 'control labels' },
];

/** `rgba(r,g,b,a)` laid over an opaque #rrggbb, as the browser composites it. */
export function composite(rgba: string, ground: string): string {
  const m = rgba.match(/^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/);
  if (!m) throw new Error(`expected rgba(r,g,b,a), got ${rgba}`);
  const a = Number(m[4]);
  const g = ground.replace('#', '');
  const ch = (i: number) => Math.round(Number(m[i + 1]) * a + parseInt(g.slice(i * 2, i * 2 + 2), 16) * (1 - a));
  return `#${[0, 1, 2].map((i) => ch(i).toString(16).padStart(2, '0')).join('')}`;
}

function channel(v: number): number {
  const c = v / 255;
  return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

export function relativeLuminance(hex: string): number {
  const h = hex.replace('#', '');
  if (h.length !== 6) throw new Error(`expected #rrggbb, got ${hex}`);
  const r = channel(parseInt(h.slice(0, 2), 16));
  const g = channel(parseInt(h.slice(2, 4), 16));
  const b = channel(parseInt(h.slice(4, 6), 16));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}
