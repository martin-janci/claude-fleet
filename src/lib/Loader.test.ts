import { render, screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { readFileSync } from 'node:fs';
import { describe, it, expect, vi, afterEach } from 'vitest';
import Loader, { LOADER_DELAY_MS } from './Loader.svelte';
import { LOADER_NAMES, LOADER_SPECS } from './loader-kit.generated';
import { motionPref } from './motion';

afterEach(() => {
  vi.useRealTimers();
  motionPref.set('system');
});

describe('Loader kit', () => {
  it('has the manual’s 24 loaders: 12 logo motions, 12 particle', () => {
    expect(LOADER_NAMES).toHaveLength(24);
    expect(new Set(LOADER_NAMES).size).toBe(24);
    expect(LOADER_SPECS.filter((s) => s.kind === 'logo')).toHaveLength(12);
    expect(LOADER_NAMES).toContain('orbit');
    expect(LOADER_NAMES).toContain('comet');
  });

  it.each(LOADER_NAMES)('%s stays under 100 nodes', (name) => {
    render(Loader, { props: { name, delay: 0 } });
    const root = screen.getByTestId('loader');
    expect(root.dataset.loader).toBe(name);
    expect(root.querySelectorAll('*').length + 1).toBeLessThan(100);
  });

  it('defines every keyframe its drawings and rules name', () => {
    const css = readFileSync('src/lib/loader-kit.generated.css', 'utf8') + readFileSync('src/lib/loader-kit.css', 'utf8');
    const markup = LOADER_SPECS.map((s) => s.markup).join('');
    const defined = new Set([...css.matchAll(/@keyframes\s+(ofl-[\w-]+)/g)].map((m) => m[1]));
    const used = new Set([...(css + markup).matchAll(/animation(?:-name)?\s*:\s*(ofl-[\w-]+)/g)].map((m) => m[1]));
    expect(used.size).toBeGreaterThan(20);
    for (const name of used) expect(defined, name).toContain(name);
  });

  it('renders nothing before 400 ms, but keeps its box', () => {
    vi.useFakeTimers();
    render(Loader, { props: { name: 'comet', size: 12 } });
    expect(LOADER_DELAY_MS).toBe(400);
    expect(screen.queryByTestId('loader')).toBeNull();
    const pending = screen.getByTestId('loader-pending');
    expect(pending.childElementCount).toBe(0);
    expect(pending.style.width).toBe('12px');
    vi.advanceTimersByTime(399);
    flushSync();
    expect(screen.queryByTestId('loader')).toBeNull();
    vi.advanceTimersByTime(1);
    flushSync();
    expect(screen.getByTestId('loader')).toBeTruthy();
    expect(screen.queryByTestId('loader-pending')).toBeNull();
  });

  it('never shows for a wait that ends first', () => {
    vi.useFakeTimers();
    const r = render(Loader);
    vi.advanceTimersByTime(300);
    r.unmount();
    vi.advanceTimersByTime(500);
    expect(screen.queryByTestId('loader')).toBeNull();
  });

  it.each(['reduced', 'off'] as const)('%s motion renders the fade', (pref) => {
    motionPref.set(pref);
    render(Loader, { props: { delay: 0 } });
    expect(screen.getByTestId('loader').classList).toContain('ofl--still');
  });

  it('full motion animates', () => {
    motionPref.set('full');
    render(Loader, { props: { delay: 0 } });
    expect(screen.getByTestId('loader').classList).not.toContain('ofl--still');
  });

  it('the fade is one slow opacity loop for every element', () => {
    const css = readFileSync('src/lib/loader-kit.css', 'utf8');
    expect(css).toMatch(/\.ofl--still \*\s*\{[^}]*animation-name:\s*ofl-fade[^}]*animation-duration:\s*var\(--loader-reduced\)/);
    expect(css).toMatch(/@keyframes ofl-fade\s*\{\s*50%\s*\{\s*opacity:\s*0\.55;?\s*\}\s*\}/);
  });

  it('pauses on request and while the window is hidden', () => {
    const r = render(Loader, { props: { delay: 0, paused: true } });
    expect(screen.getByTestId('loader').classList).toContain('ofl--paused');
    r.unmount();

    render(Loader, { props: { delay: 0 } });
    const root = screen.getByTestId('loader');
    expect(root.classList).not.toContain('ofl--paused');
    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    document.dispatchEvent(new Event('visibilitychange'));
    flushSync();
    expect(root.classList).toContain('ofl--paused');
    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event('visibilitychange'));
    flushSync();
    expect(root.classList).not.toContain('ofl--paused');
    hidden.mockRestore();
  });

  it('is decorative without a label and an image with one', () => {
    const r = render(Loader, { props: { delay: 0 } });
    expect(screen.getByTestId('loader').getAttribute('aria-hidden')).toBe('true');
    r.unmount();
    render(Loader, { props: { delay: 0, label: 'Loading sessions', testid: 'sessions-loader' } });
    const img = screen.getByRole('img', { name: 'Loading sessions' });
    expect(img.dataset.testid).toBe('sessions-loader');
  });

  it('scales to its size and puts particle loaders on a stage', () => {
    render(Loader, { props: { delay: 0, name: 'orbit', size: 16, testid: 'a' } });
    expect(screen.getByTestId('a').style.width).toBe('16px');
    expect(screen.getByTestId('a').classList).not.toContain('ofl--stage');
    render(Loader, { props: { delay: 0, name: 'dot-wave', size: 95, testid: 'b' } });
    const wave = screen.getByTestId('b');
    expect(wave.style.width).toBe('95px');
    expect(wave.style.height).toBe('59px');
    expect(wave.classList).toContain('ofl--stage');
    render(Loader, { props: { delay: 0, name: 'comet', size: 12, testid: 'c' } });
    const comet = screen.getByTestId('c');
    expect(comet.classList).not.toContain('ofl--stage');
    expect(comet.style.getPropertyValue('--s')).toBe('12px');
  });

  it('a Comet on a filled primary button takes the button text colour', () => {
    const css = readFileSync('src/lib/loader-kit.css', 'utf8');
    expect(css).toMatch(/\.btn--primary \.ofl--comet,\s*\.of-btn\.primary \.ofl--comet\s*\{\s*--accent: var\(--accent-fg\);\s*--b-light: var\(--accent-fg\);/);
  });

  it('draws a known progress', () => {
    render(Loader, { props: { delay: 0, name: 'progress-ring', value: 0.64 } });
    const ring = screen.getByTestId('loader');
    expect(ring.classList).toContain('ofl--determinate');
    expect(ring.style.getPropertyValue('--ofl-p')).toBe('0.64');
  });

  // Step 3.15: Radar shows one blip per host that answers, Assemble one
  // particle per session.
  it('Radar and Assemble draw one particle per item', () => {
    const blips = (count: number) => {
      const { unmount } = render(Loader, { props: { delay: 0, name: 'radar', count, testid: 'r' } });
      const el = screen.getByTestId('r');
      const n = el.querySelectorAll('.ofl-rd2 i').length;
      expect(el.getAttribute('data-count')).toBe(String(count));
      // The sweep and its rings stay whatever the count.
      expect(el.querySelector('.ofl-sweep')).not.toBeNull();
      unmount();
      return n;
    };
    expect(blips(0)).toBe(0);
    expect(blips(3)).toBe(3);
    expect(blips(9)).toBe(9);
    expect(blips(500)).toBe(24);
    const { unmount } = render(Loader, { props: { delay: 0, name: 'assemble', count: 22, testid: 'a' } });
    expect(screen.getByTestId('a').querySelectorAll('.ofl-cv i')).toHaveLength(22);
    expect(screen.getByTestId('a').querySelector('.ofl-cvring')).not.toBeNull();
    unmount();
    // Without a count, the manual's drawing as it is.
    render(Loader, { props: { delay: 0, name: 'assemble', testid: 'b' } });
    expect(screen.getByTestId('b').querySelectorAll('.ofl-cv i')).toHaveLength(64);
    expect(screen.getByTestId('b').hasAttribute('data-count')).toBe(false);
  });

  it('a count on a loader that does not count changes nothing', () => {
    render(Loader, { props: { delay: 0, name: 'dot-wave', count: 2, testid: 'w' } });
    const spec = LOADER_SPECS.find((s) => s.id === 'dot-wave')!;
    expect(screen.getByTestId('w').querySelectorAll('i')).toHaveLength((spec.markup.match(/<i /g) ?? []).length);
  });

  // Redesign step 3.14: a lost hub must not read as a loop of hope.
  it('Signal lost plays once and rests, in every motion setting', () => {
    const css = readFileSync('src/lib/loader-kit.css', 'utf8');
    expect(css).toMatch(
      /\.ofl\.ofl-name-signal-lost \*,\s*\.ofl\.ofl-name-signal-lost\.ofl--still \*\s*\{[^}]*animation-iteration-count:\s*1 !important;[^}]*animation-fill-mode:\s*forwards !important;/,
    );
    render(Loader, { props: { name: 'signal-lost', delay: 0 } });
    expect(screen.getByTestId('loader').classList).toContain('ofl-name-signal-lost');
  });
});
