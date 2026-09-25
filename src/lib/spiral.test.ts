import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import SpiralLoader from './SpiralLoader.svelte';
import { SPIRAL_PATH, SPIRAL_CYCLE_MS, cubicBezier, spiralFrame } from './spiral';

describe('cubicBezier', () => {
  it('pins the ends and is the identity on the diagonal', () => {
    expect(cubicBezier(0.3, 0.1, 0.7, 0.9, 0)).toBe(0);
    expect(cubicBezier(0.3, 0.1, 0.7, 0.9, 1)).toBe(1);
    expect(cubicBezier(0.25, 0.25, 0.75, 0.75, 0.4)).toBeCloseTo(0.4, 4);
  });
});

describe('spiralFrame', () => {
  it('starts on the Lottie first keyframe', () => {
    expect(spiralFrame(0)).toEqual({ phase: 'fast', x: 12, start: 23, end: 44 });
  });

  it('reaches the last keyframe at the end of a fast clip', () => {
    const f = spiralFrame(499);
    expect(f.phase).toBe('fast');
    expect(f.x).toBeCloseTo(4, 5);
    expect(f.start).toBeCloseTo(57, 5);
    expect(f.end).toBeCloseTo(77, 5);
  });

  it('plays four fast clips, then two slow ones, then loops', () => {
    expect(SPIRAL_CYCLE_MS).toBe(4000);
    expect(spiralFrame(1999).phase).toBe('fast');
    expect(spiralFrame(2000).phase).toBe('slow');
    expect(spiralFrame(3999).phase).toBe('slow');
    expect(spiralFrame(4000)).toEqual(spiralFrame(0));
    // A slow clip lasts 1 s: the first one ends 3 s in, the squiggle slid home.
    expect(spiralFrame(2999).x).toBeCloseTo(4, 5);
    expect(spiralFrame(2500).x).toBeGreaterThan(5);
  });

  it('keeps the visible window inside the stroke', () => {
    for (let t = 0; t < SPIRAL_CYCLE_MS; t += 37) {
      const f = spiralFrame(t);
      expect(f.start).toBeGreaterThanOrEqual(23 - 1e-9);
      expect(f.end).toBeLessThanOrEqual(77 + 1e-9);
      expect(f.end).toBeGreaterThan(f.start);
    }
  });
});

describe('SPIRAL_PATH', () => {
  it('is one open cubic run through all thirteen vertices', () => {
    expect(SPIRAL_PATH.startsWith('M-12 6 C')).toBe(true);
    expect(SPIRAL_PATH.match(/C/g)).toHaveLength(12);
    expect(SPIRAL_PATH.endsWith(' 12 6')).toBe(true);
  });
});

describe('SpiralLoader', () => {
  it('is decorative without a label', () => {
    render(SpiralLoader);
    const svg = screen.getByTestId('spiral-loader');
    expect(svg.getAttribute('aria-hidden')).toBe('true');
    expect(svg.getAttribute('role')).toBeNull();
    expect(svg.getAttribute('width')).toBe('16');
  });

  it('is an image named by its label', () => {
    render(SpiralLoader, { props: { label: 'Loading', size: 24 } });
    const svg = screen.getByRole('img', { name: 'Loading' });
    expect(svg.getAttribute('height')).toBe('24');
  });
});
