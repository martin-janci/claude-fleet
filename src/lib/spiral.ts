// The spiral loader's geometry and timing, ported from a pair of Lottie
// animations ("spiral2" fast, "spiral1" slow) so the app needs neither
// lottie-web nor a React wrapper. One looped squiggle slides left by exactly
// one period while a trimmed window of its stroke runs along it; the fast
// clip plays FAST_REPEATS times, the slow one SLOW_REPEATS times, forever.
//
// Everything here is pure except `subscribeSpiralClock`, the one shared
// animation-frame loop every mounted loader draws from — a sidebar full of
// working sessions costs one rAF callback, not one per row.

type Pt = [number, number];

// The Lottie shape, verbatim: vertices with their in / out tangents.
const V: Pt[] = [
  [-12, 6], [-4.975, -1.012], [-8, -6], [-11.025, -1.012], [-4, 6], [3.025, -1.012], [0, -6],
  [-3.025, -1.012], [4, 6], [11.025, -1.012], [8, -6], [4.98, -1.012], [12, 6],
];
const IN: Pt[] = [
  [0, 0], [-0.289, 3.296], [2.218, 0], [-0.23, -2.627], [-4.452, 0], [-0.289, 3.296], [2.218, 0],
  [-0.23, -2.627], [-4.452, 0], [-0.289, 3.296], [2.218, 0], [-0.232, -2.627], [-4.443, 0],
];
const OUT: Pt[] = [
  [4.452, 0], [0.23, -2.627], [-2.218, 0], [0.289, 3.296], [4.452, 0], [0.23, -2.627], [-2.218, 0],
  [0.289, 3.296], [4.452, 0], [0.23, -2.627], [-2.218, 0], [0.292, 3.296], [0, 0],
];

const r = (n: number) => Math.round(n * 1000) / 1000;

/** The squiggle as an SVG path, centred on the origin (x −12…12, y −6…6). */
export const SPIRAL_PATH: string = (() => {
  let d = `M${V[0][0]} ${V[0][1]}`;
  for (let k = 0; k < V.length - 1; k++) {
    const c1 = [V[k][0] + OUT[k][0], V[k][1] + OUT[k][1]];
    const c2 = [V[k + 1][0] + IN[k + 1][0], V[k + 1][1] + IN[k + 1][1]];
    d += ` C${r(c1[0])} ${r(c1[1])} ${r(c2[0])} ${r(c2[1])} ${V[k + 1][0]} ${V[k + 1][1]}`;
  }
  return d;
})();

/** CSS `cubic-bezier(x1, y1, x2, y2)` evaluated at progress `t` (0…1). */
export function cubicBezier(x1: number, y1: number, x2: number, y2: number, t: number): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const bx = (s: number) => 3 * (1 - s) * (1 - s) * s * x1 + 3 * (1 - s) * s * s * x2 + s * s * s;
  const by = (s: number) => 3 * (1 - s) * (1 - s) * s * y1 + 3 * (1 - s) * s * s * y2 + s * s * s;
  // Bisection on x(s) = t: monotone for x1, x2 in [0, 1], and 24 halvings
  // is well past a pixel at any size this is drawn.
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2;
    if (bx(mid) < t) lo = mid;
    else hi = mid;
  }
  return by((lo + hi) / 2);
}

type Ease = [number, number, number, number];
interface Clip {
  /** Clip length in ms (Lottie: frames at 60 fps). */
  ms: number;
  /** Last keyframe as a fraction of the clip (keyed at op − 1). */
  last: number;
  start: Ease;
  end: Ease;
}

const FAST: Clip = {
  ms: 500,
  last: 29 / 30,
  start: [0.32, 0.154, 0.826, 0.579],
  end: [0.341, 0.488, 0.269, 0.75],
};
const SLOW: Clip = {
  ms: 1000,
  last: 59 / 60,
  start: [0.32, 0.313, 0.826, 0.143],
  end: [0.341, 0.992, 0.269, 0.491],
};
const SLIDE: Ease = [0.167, 0.167, 0.833, 0.833];

export const FAST_REPEATS = 4;
export const SLOW_REPEATS = 2;
/** One full fast-then-slow cycle, in ms. */
export const SPIRAL_CYCLE_MS = FAST_REPEATS * FAST.ms + SLOW_REPEATS * SLOW.ms;

export interface SpiralFrame {
  phase: 'fast' | 'slow';
  /** Horizontal offset of the squiggle inside the 16×16 box (12 → 4). */
  x: number;
  /** Visible window of the stroke, as percentages of its length. */
  start: number;
  end: number;
}

const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

/** The frame shown `ms` milliseconds into the loop. */
export function spiralFrame(ms: number): SpiralFrame {
  const t = ((ms % SPIRAL_CYCLE_MS) + SPIRAL_CYCLE_MS) % SPIRAL_CYCLE_MS;
  const fastSpan = FAST_REPEATS * FAST.ms;
  const phase = t < fastSpan ? 'fast' : 'slow';
  const clip = phase === 'fast' ? FAST : SLOW;
  const local = phase === 'fast' ? t % FAST.ms : (t - fastSpan) % SLOW.ms;
  const p = Math.min(1, local / clip.ms / clip.last);
  return {
    phase,
    x: lerp(12, 4, cubicBezier(...SLIDE, p)),
    start: lerp(23, 57, cubicBezier(...clip.start, p)),
    end: lerp(44, 77, cubicBezier(...clip.end, p)),
  };
}

type Tick = (ms: number) => void;
const subscribers = new Set<Tick>();
let rafId: number | null = null;

function loop(now: number) {
  for (const fn of subscribers) fn(now);
  rafId = subscribers.size > 0 ? requestAnimationFrame(loop) : null;
}

/** Calls `fn` with the shared clock every animation frame until the returned
 *  function is called. A no-op where there is no rAF (tests, SSR). */
export function subscribeSpiralClock(fn: Tick): () => void {
  if (typeof requestAnimationFrame !== 'function') return () => {};
  subscribers.add(fn);
  if (rafId === null) rafId = requestAnimationFrame(loop);
  return () => {
    subscribers.delete(fn);
    if (subscribers.size === 0 && rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  };
}
