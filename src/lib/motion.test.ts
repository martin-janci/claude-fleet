import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { DURATIONS, durationMs, effectiveMotion, initMotion, motionPref, resolveMotion } from './motion';

afterEach(() => motionPref.set('system'));

describe('Motion pref', () => {
  it('System follows the OS; an explicit pick wins over it', () => {
    expect(resolveMotion('system', false)).toBe('full');
    expect(resolveMotion('system', true)).toBe('reduced');
    for (const os of [false, true]) {
      expect(resolveMotion('full', os)).toBe('full');
      expect(resolveMotion('reduced', os)).toBe('reduced');
      expect(resolveMotion('off', os)).toBe('off');
    }
  });

  it('writes the effective level to <html data-motion> and keeps it live', () => {
    const stop = initMotion();
    try {
      motionPref.set('off');
      expect(document.documentElement.getAttribute('data-motion')).toBe('off');
      motionPref.set('reduced');
      expect(document.documentElement.getAttribute('data-motion')).toBe('reduced');
      motionPref.set('full');
      expect(get(effectiveMotion)).toBe('full');
      expect(document.documentElement.getAttribute('data-motion')).toBe('full');
    } finally {
      stop();
    }
  });

  it('follows a live OS change while on System', async () => {
    let listener: ((e: { matches: boolean }) => void) | undefined;
    const mq = {
      matches: false,
      addEventListener: (_: string, fn: (e: { matches: boolean }) => void) => (listener = fn),
      removeEventListener: () => {},
    };
    vi.stubGlobal('matchMedia', vi.fn(() => mq));
    vi.resetModules();
    try {
      const m = await import('./motion');
      const seen: string[] = [];
      const stop = m.effectiveMotion.subscribe((v) => seen.push(v));
      listener?.({ matches: true });
      stop();
      expect(seen).toEqual(['full', 'reduced']);
    } finally {
      vi.unstubAllGlobals();
      vi.resetModules();
    }
  });

  it('durationMs reads the current level', () => {
    motionPref.set('full');
    expect(durationMs('slow')).toBe(280);
    motionPref.set('reduced');
    expect(durationMs('slow')).toBe(80);
    motionPref.set('off');
    expect(durationMs('fast')).toBe(0);
  });
});

// app.css is the source the browser reads; DURATIONS must say the same.
describe('app.css retimes the duration tokens per motion level', () => {
  const css = readFileSync('src/app.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  const block = (sel: string) => css.slice(css.indexOf(sel)).split('}')[0];
  const ms = (v: string) => (v.endsWith('ms') ? Number(v.slice(0, -2)) : Number(v.slice(0, -1)) * 1000);
  const root = block('\n:root {');
  const decl = (b: string, name: string) => b.match(new RegExp(`--${name}:\\s*([^;]+);`))?.[1].trim();

  it('Full is the manual tokens', () => {
    for (const [k, v] of Object.entries(DURATIONS.full)) expect(ms(decl(root, `dur-${k}`)!), k).toBe(v);
  });

  it('Reduced makes every UI duration the fast fade', () => {
    const b = block(":root[data-motion='reduced'] {");
    expect(decl(b, 'dur-base')).toBe('var(--dur-fast)');
    expect(decl(b, 'dur-slow')).toBe('var(--dur-fast)');
    expect(DURATIONS.reduced).toEqual({ fast: 80, base: 80, slow: 80 });
  });

  it('Off zeroes them', () => {
    const b = block(":root[data-motion='off'] {");
    for (const k of ['fast', 'base', 'slow']) expect(decl(b, `dur-${k}`), k).toBe('0ms');
  });
});

// The Verified line of step 0.6: no transition uses a raw duration, so the
// Motion pref reaches every one of them.
describe('no transition uses a raw duration', () => {
  // Serial files (one open PR at a time, per the redesign plan) move onto the
  // tokens in the step that next owns them. Each entry is the exact count of
  // raw transitions left, so the list can only shrink.
  const PENDING: Record<string, number> = {};

  const files = (dir: string): string[] =>
    readdirSync(dir, { recursive: true })
      .filter((n) => /\.(svelte|css)$/.test(n))
      .map((n) => `${dir}/${n.replaceAll('\\', '/')}`);

  const RAW = /transition(?:-duration)?\s*:[^;{}]*\b\d*\.?\d+m?s\b/g;

  it('every transition reads a --dur-* token', () => {
    const offenders: Record<string, number> = {};
    for (const f of files('src')) {
      const src = readFileSync(f, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      const n = src.match(RAW)?.length ?? 0;
      if (n) offenders[f] = n;
    }
    expect(offenders).toEqual(PENDING);
  });
});

// The same rule for loops: an `animation` (or `animation-duration`) reads a
// token (`--loop-*`, `--dur-*`, `--loader-reduced`), never a raw time, and
// Reduced / Off reach it by the Loader kit's rule: one --loader-reduced fade,
// or still. The kit's own generated stylesheet is the manual's drawing; the
// Loader governs it (`ofl--still`, loader-kit.css), so it is left out here.
describe('no animation uses a raw duration', () => {
  const files = (dir: string): string[] =>
    readdirSync(dir, { recursive: true })
      .filter((n) => /\.(svelte|css)$/.test(n))
      .map((n) => `${dir}/${n.replaceAll('\\', '/')}`)
      .filter((f) => !f.endsWith('/loader-kit.generated.css'));

  const RAW = /animation(?:-duration)?\s*:[^;{}"]*\b\d*\.?\d+m?s\b/g;

  it('reads raw times, and lets tokens by', () => {
    const hits = (css: string) => css.match(RAW)?.length ?? 0;
    expect(hits('.a { animation: spin 1.6s linear infinite; }')).toBe(1);
    expect(hits('.a { animation-duration: 300ms; }')).toBe(1);
    expect(hits('.a { animation: spin var(--loop-slow) linear infinite; }')).toBe(0);
    expect(hits('.a { animation: none; }')).toBe(0);
  });

  it('every animation reads a token', () => {
    const offenders: Record<string, string[]> = {};
    for (const f of files('src')) {
      const src = readFileSync(f, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      const m = src.match(RAW);
      if (m) offenders[f] = m;
    }
    expect(offenders).toEqual({});
  });

  it('app.css defines the loop tokens and the fade they become', () => {
    const css = readFileSync('src/app.css', 'utf8');
    expect(css).toMatch(/--loop-fast:\s*[\d.]+m?s;/);
    expect(css).toMatch(/--loop-slow:\s*[\d.]+m?s;/);
    expect(css).toMatch(/@keyframes motion-fade\s*\{/);
  });
});
