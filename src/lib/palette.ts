/**
 * The Orbit palette engine (dynamic day/night theme, "Mission HUD" look).
 *
 * Three inputs make every colour the theme writes: how high the sun is
 * (`sun`, -1 at midnight to 1 at noon; the theme is dark exactly while it is
 * below the horizon), a contrast level, and one accent hue. Colours are built
 * in OKLCH, and each text colour is SOLVED for a WCAG ratio against the
 * hardest ground it sits on, so contrast holds while the grounds drift through
 * the day: lab white at noon, a lifted blue hour after sunset, graphite at
 * night with its peak text eased to cut glare.
 *
 * Status colours keep their hues (blue, amber, red, green) and only move in
 * lightness, so "Needs you" always reads as amber; code and syntax colours
 * likewise keep their hues against the code well. Brand, terminal and
 * destructive-fill colours are not touched: they stay app.css's.
 *
 * fleet-mobile's `OrbitPalette.kt` is a port of this file; the golden values
 * in `palette.test.ts` and its Kotlin twin keep the two in step.
 */

export type ContrastLevel = 'soft' | 'std' | 'high';

export interface PaletteInput {
  /** -1 (midnight) … 0 (horizon) … 1 (noon). Below 0 is the dark theme. */
  sun: number;
  contrast: ContrastLevel;
  /** The accent's OKLCH hue in degrees. */
  hue: number;
}

/** The accent hues Settings › Appearance offers; Cyan is the HUD default. */
export const ACCENTS = [
  { id: 'cyan', label: 'Cyan', hue: 205 },
  { id: 'blue', label: 'Blue', hue: 258 },
  { id: 'violet', label: 'Violet', hue: 290 },
  { id: 'teal', label: 'Teal', hue: 168 },
  { id: 'copper', label: 'Copper', hue: 45 },
] as const;
export type AccentId = (typeof ACCENTS)[number]['id'];

interface Ok {
  hex: string;
  /** Relative luminance (WCAG). */
  y: number;
  l: number;
}

function linear(l: number, c: number, h: number): [number, number, number] {
  const r = (h * Math.PI) / 180;
  const a = c * Math.cos(r);
  const b = c * Math.sin(r);
  const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
    -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
  ];
}

const inGamut = (v: number[]) => v.every((x) => x >= -0.0005 && x <= 1.0005);

/** An OKLCH colour as #rrggbb, chroma reduced until it fits sRGB. */
export function oklch(l: number, c: number, h: number): Ok {
  l = Math.max(0, Math.min(1, l));
  let v = linear(l, c, h);
  while (!inGamut(v) && c > 0.001) {
    c *= 0.92;
    v = linear(l, c, h);
  }
  const lin = v.map((x) => Math.max(0, Math.min(1, x)));
  // Quantise first, so the luminance is that of the hex the app will paint.
  const bytes = lin.map((x) => Math.round((x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055) * 255));
  const hex = `#${bytes.map((n) => n.toString(16).padStart(2, '0')).join('')}`;
  return { hex, y: luminance(bytes), l };
}

function luminance(bytes: number[]): number {
  const ch = (n: number) => {
    const c = n / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * ch(bytes[0]) + 0.7152 * ch(bytes[1]) + 0.0722 * ch(bytes[2]);
}

const ratio = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);

/**
 * The colour of hue `h` and chroma `c` closest to `ground` that still clears
 * `target` against it: lighter than the ground in the dark theme, darker in
 * the light one. A target the ground cannot reach gives the extreme.
 */
function solve(target: number, ground: Ok, c: number, h: number, dark: boolean): Ok {
  let lo = dark ? ground.l : 0;
  let hi = dark ? 1 : ground.l;
  let best = oklch(dark ? 1 : 0, c, h);
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2;
    const k = oklch(mid, c, h);
    const pass = ratio(k.y, ground.y) >= target;
    if (dark === pass) hi = mid;
    else lo = mid;
    if (pass) best = k;
  }
  return best;
}

/** `l` if it clears `target` on `ground`, else the nearest that does. */
function prefer(target: number, ground: Ok, l: number, c: number, h: number, dark: boolean): Ok {
  const k = oklch(l, c, h);
  return ratio(k.y, ground.y) >= target ? k : solve(target, ground, c, h, dark);
}

/** Ratio targets per level. Floors (muted, status) never drop under 4.5:1. */
export const TARGETS: Record<ContrastLevel, { fg: number; fg2: number; muted: number; status: number; accent: number; border: number; control: number; strong: number }> = {
  soft: { fg: 9, fg2: 6, muted: 4.6, status: 4.6, accent: 3.4, border: 1.2, control: 1.9, strong: 3.1 },
  std: { fg: 13, fg2: 8, muted: 5, status: 4.8, accent: 3.6, border: 1.3, control: 2.4, strong: 3.4 },
  high: { fg: 17, fg2: 11, muted: 7, status: 7, accent: 4.6, border: 1.6, control: 3.2, strong: 4.6 },
};

// The HUD look's neutrals: hue and chroma at noon, golden hour, blue hour and midnight.
const DAY = { h: 240, c: 0.006 };
const GOLDEN = { h: 228, c: 0.01 };
const BLUE_HOUR = { h: 250, c: 0.034 };
const NIGHT = { h: 252, c: 0.015 };
const STATUS_HUE = { working: 262, waiting: 70, failed: 27, done: 150 } as const;
// Code and syntax keep the hues of the One Dark-like set app.css ships.
const SYNTAX_HUE = { code: 210, kw: 315, str: 135, num: 62 } as const;

function rgba(hex: string, a: number): string {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${n >> 16},${(n >> 8) & 255},${n & 255},${a})`;
}

const DARK_INK = '#0b0f17';

/**
 * Every token the engine owns, as CSS custom-property names without the
 * leading `--`, for one moment of the day. `glow` is the HUD halo the primary
 * action wears in the dark theme ("none" by day).
 */
export function orbitPalette({ sun, contrast, hue }: PaletteInput): Record<string, string> {
  const dark = sun < 0;
  const t = TARGETS[contrast];
  let nh: number;
  let nc: number;
  let bgL: number;
  let ease = 1;
  if (!dark) {
    const w = 1 - Math.sqrt(Math.min(1, sun));
    nh = DAY.h + (GOLDEN.h - DAY.h) * w;
    nc = DAY.c + (GOLDEN.c - DAY.c) * w;
    bgL = 0.952 + 0.033 * Math.sqrt(Math.min(1, sun));
  } else {
    const d = Math.min(1, -sun);
    const q = d ** 0.6;
    nh = BLUE_HOUR.h + (NIGHT.h - BLUE_HOUR.h) * q;
    nc = BLUE_HOUR.c + (NIGHT.c - BLUE_HOUR.c) * q;
    bgL = 0.27 - 0.1 * q;
    // Peak text eases as the night deepens; the floors below never move.
    ease = 1 - 0.22 * d;
  }
  const s = dark ? 1 : -1;
  const g = (dl: number, c = nc) => oklch(bgL + s * dl, c, nh);
  const bg = g(0);
  const pane = oklch(Math.min(1, bgL + (dark ? 0.025 : 0.02)), nc, nh);
  const raise = g(0.04);
  const sunk = g(0.035);
  const chip = g(0.055);
  const hover = g(0.065);
  const count = g(0.06);
  const accentSoft = oklch(bgL + s * 0.075, dark ? 0.045 : 0.035, hue);
  // The hardest ground each kind of text has to read on.
  const hardest = oklch(bgL + s * 0.085, nc, nh);

  const fg = solve(Math.max(7, t.fg * ease), hardest, nc * 1.5, nh, dark);
  const fg2 = solve(Math.max(5.5, t.fg2 * ease), hardest, nc * 2, nh, dark);
  const muted = solve(t.muted, hardest, nc * 2, nh, dark);
  const border = solve(t.border, pane, nc * 2, nh, dark);
  const control = solve(t.control, pane, nc * 2, nh, dark);
  const strong = solve(t.strong, hardest, nc * 2, nh, dark);
  const accent = prefer(t.accent, hardest, dark ? 0.8 : 0.5, dark ? 0.15 : 0.18, hue, dark);
  const accentFg = ratio(accent.y, 1) >= ratio(accent.y, 0.004) ? '#ffffff' : DARK_INK;

  const status = (h: number, c: number) => prefer(t.status, hardest, dark ? 0.79 : 0.5, c, h, dark);
  const working = status(STATUS_HUE.working, 0.15);
  const waiting = status(STATUS_HUE.waiting, 0.14);
  const failed = status(STATUS_HUE.failed, 0.17);
  const done = status(STATUS_HUE.done, 0.14);
  const syntax = (h: number) => prefer(4.6, sunk, dark ? 0.78 : 0.5, 0.13, h, dark);
  const onWaiting = ratio(waiting.y, 1) >= ratio(waiting.y, 0.004) ? '#ffffff' : '#1a1205';

  return {
    bg: bg.hex,
    'bg-pane': pane.hex,
    'bg-raise': raise.hex,
    'bg-hover': hover.hex,
    'bg-sunk': sunk.hex,
    fg: fg.hex,
    'fg-2': fg2.hex,
    'fg-muted': muted.hex,
    border: border.hex,
    'control-border': control.hex,
    accent: accent.hex,
    'accent-fg': accentFg,
    'accent-soft': accentSoft.hex,
    'status-working': working.hex,
    'status-waiting': waiting.hex,
    'status-failed': failed.hex,
    'status-done': done.hex,
    'status-idle': muted.hex,
    'on-waiting': onWaiting,
    'waiting-soft': rgba(waiting.hex, 0.13),
    'waiting-faint': rgba(waiting.hex, 0.07),
    'waiting-line': rgba(waiting.hex, 0.45),
    'failed-soft': rgba(failed.hex, 0.12),
    'failed-line': rgba(failed.hex, 0.35),
    'done-soft': rgba(done.hex, 0.12),
    'chip-bg': chip.hex,
    'count-bg': count.hex,
    track: oklch(bgL + s * 0.09, nc, nh).hex,
    'org-1': accent.hex,
    'loader-accent': accent.hex,
    'control-bg': dark ? raise.hex : pane.hex,
    'control-bg-hover': hover.hex,
    'control-bg-active': oklch(bgL + s * 0.1, nc, nh).hex,
    'control-border-strong': strong.hex,
    'control-fg': fg.hex,
    'control-fg-quiet': muted.hex,
    code: syntax(SYNTAX_HUE.code).hex,
    'syn-kw': syntax(SYNTAX_HUE.kw).hex,
    'syn-str': syntax(SYNTAX_HUE.str).hex,
    'syn-num': syntax(SYNTAX_HUE.num).hex,
    glow: dark ? `0 0 0 1px ${rgba(accent.hex, 0.5)}, 0 0 18px ${rgba(accent.hex, 0.3)}` : 'none',
  };
}

// ─── The sun ──────────────────────────────────────────────────────────────

/** Sunrise and sunset in local hours (0–24) for a day and place, or
 *  'day' / 'night' when the sun never sets or never rises. The NOAA approximation: minutes of accuracy,
 *  no network, no clock beyond the one passed in. */
export function sunTimes(date: Date, lat: number, lon: number): { rise: number; set: number } | 'day' | 'night' {
  const start = Date.UTC(date.getFullYear(), 0, 0);
  const n = Math.floor((Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) - start) / 86_400_000);
  const rad = Math.PI / 180;
  const decl = -23.44 * Math.cos(((2 * Math.PI) / 365) * (n + 10)) * rad;
  const b = ((2 * Math.PI) / 365) * (n - 81);
  const eot = 9.87 * Math.sin(2 * b) - 7.53 * Math.cos(b) - 1.5 * Math.sin(b);
  const cosW = (Math.sin(-0.833 * rad) - Math.sin(lat * rad) * Math.sin(decl)) / (Math.cos(lat * rad) * Math.cos(decl));
  if (cosW <= -1) return 'day';
  if (cosW >= 1) return 'night';
  const w = Math.acos(cosW) / rad / 15;
  const noonUtc = 12 - lon / 15 - eot / 60;
  const local = -date.getTimezoneOffset() / 60;
  const wrap = (h: number) => ((h % 24) + 24) % 24;
  return { rise: wrap(noonUtc - w + local), set: wrap(noonUtc + w + local) };
}

/** -1 … 1: a sine over the day from sunrise to sunset, and its negative over the night. */
export function sunHeight(hour: number, times: { rise: number; set: number } | 'day' | 'night'): number {
  if (times === 'day') return 1;
  if (times === 'night') return -1;
  const { rise, set } = times;
  const span = (a: number, b: number) => (((b - a) % 24) + 24) % 24;
  const dayLen = span(rise, set);
  const since = span(rise, hour);
  if (since < dayLen) return Math.sin((Math.PI * since) / dayLen);
  return -Math.sin((Math.PI * (since - dayLen)) / (24 - dayLen));
}

/** Where we take the sun from without asking for a location: the time
 *  zone's standard meridian (15° per hour of UTC offset, summer time left
 *  out) at a temperate latitude. Within about half an hour of the real
 *  sunrise for most of Europe. */
export function defaultPlace(date: Date): { lat: number; lon: number } {
  const y = date.getFullYear();
  const standard = Math.max(new Date(y, 0, 1).getTimezoneOffset(), new Date(y, 6, 1).getTimezoneOffset());
  return { lat: 48, lon: (-standard / 60) * 15 };
}
