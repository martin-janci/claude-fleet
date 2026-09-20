/**
 * The theme palette as data, mirroring the four `:root` blocks in app.css,
 * so the contrast floors those blocks claim in comments are actually
 * asserted. Nothing imports this at runtime — it exists to be tested.
 *
 * When you add or change a token in app.css, change it here too and add the
 * pair it has to clear to CONTRAST_PAIRS.
 */

export interface ContrastPair {
  /** Key in THEME for the foreground / border colour. */
  fg: string;
  /** Key in THEME for the surface it sits on. */
  bg: string;
  /** WCAG floor: 4.5 for text, 3 for non-text. */
  min: number;
  note: string;
}

export const THEME: Record<'light' | 'dark', Record<string, string>> = {
  light: {
    bg: '#ffffff',
    'bg-pane': '#fafafa',
    fg: '#1a1a1a',
    'fg-muted': '#6b6b6b',
    border: '#e5e5e5',
    accent: '#2563eb',
    'usage-ok': '#2e7d32',
    'usage-warn': '#b45309',
    'usage-crit': '#c62828',
  },
  dark: {
    bg: '#0f0f0f',
    'bg-pane': '#161616',
    fg: '#ededed',
    'fg-muted': '#999999',
    border: '#262626',
    accent: '#60a5fa',
    'usage-ok': '#5dd17a',
    'usage-warn': '#d29b4a',
    'usage-crit': '#ef5350',
  },
};

export const CONTRAST_PAIRS: ContrastPair[] = [
  { fg: 'fg', bg: 'bg-pane', min: 4.5, note: 'body text' },
  { fg: 'fg-muted', bg: 'bg-pane', min: 4.5, note: 'muted text' },
  { fg: 'usage-ok', bg: 'bg-pane', min: 4.5, note: 'healthy context meter' },
  { fg: 'usage-warn', bg: 'bg-pane', min: 4.5, note: 'warn text and bars' },
  { fg: 'usage-crit', bg: 'bg-pane', min: 4.5, note: 'crit text and bars' },
  { fg: 'accent', bg: 'bg', min: 3, note: 'focus ring' },
];

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
