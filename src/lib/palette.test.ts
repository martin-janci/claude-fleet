import { describe, it, expect } from 'vitest';
import { ACCENTS, orbitPalette, sunHeight, sunTimes, type ContrastLevel } from './palette';
import { THEME, CONTRAST_PAIRS, composite, contrastRatio, relativeLuminance } from './tokens';

const LEVELS: ContrastLevel[] = ['soft', 'std', 'high'];
const SUNS = Array.from({ length: 41 }, (_, i) => -1 + i * 0.05);

/** app.css's theme for that side, with the engine's tokens laid over it and
 *  the aliases app.css declares with var() pointed at the new values. */
function painted(sun: number, contrast: ContrastLevel, hue: number): Record<string, string> {
  const p = orbitPalette({ sun, contrast, hue });
  const t: Record<string, string> = { ...THEME[sun < 0 ? 'dark' : 'light'], ...p };
  t.ring = t['ai-pre'] = p.accent;
  t.danger = p['status-failed'];
  t['usage-ok'] = p['status-done'];
  t['usage-warn'] = p['status-waiting'];
  t['usage-crit'] = p['status-failed'];
  return t;
}

describe('orbitPalette holds every documented pair through the day', () => {
  for (const contrast of LEVELS) {
    for (const accent of ACCENTS) {
      it(`${contrast}, ${accent.label}: every pair clears its floor from midnight to noon`, () => {
        const fails: string[] = [];
        for (const sun of SUNS) {
          const t = painted(sun, contrast, accent.hue);
          for (const pair of CONTRAST_PAIRS) {
            const ground = pair.tint ? composite(t[pair.tint]!, t[pair.bg]!) : t[pair.bg]!;
            const r = contrastRatio(t[pair.fg]!, ground);
            if (r < pair.min) fails.push(`sun ${sun.toFixed(2)}: ${pair.fg} on ${pair.bg} ${r.toFixed(2)} < ${pair.min} (${pair.note})`);
          }
        }
        expect(fails).toEqual([]);
      });
    }
  }

  it('the control border clears 1.9:1 at every level (the old 1.5:1 exception is gone)', () => {
    for (const contrast of LEVELS)
      for (const sun of SUNS) {
        const p = orbitPalette({ sun, contrast, hue: 205 });
        expect(contrastRatio(p['control-border'], p['bg-pane'])).toBeGreaterThanOrEqual(1.9);
      }
  });

  it('is dark exactly while the sun is down', () => {
    for (const sun of SUNS) {
      const y = relativeLuminance(orbitPalette({ sun, contrast: 'std', hue: 205 }).bg);
      expect(sun < 0 ? y < 0.1 : y > 0.75).toBe(true);
    }
  });

  it('higher levels never lower the text contrast', () => {
    for (const sun of SUNS) {
      const r = LEVELS.map((contrast) => {
        const p = orbitPalette({ sun, contrast, hue: 205 });
        return contrastRatio(p.fg, p['bg-pane']);
      });
      expect(r[1]).toBeGreaterThanOrEqual(r[0] - 0.01);
      expect(r[2]).toBeGreaterThanOrEqual(r[1] - 0.01);
    }
  });

  it('eases peak text at night: midnight text is dimmer than blue-hour text', () => {
    const blue = orbitPalette({ sun: -0.05, contrast: 'std', hue: 205 });
    const night = orbitPalette({ sun: -1, contrast: 'std', hue: 205 });
    expect(relativeLuminance(night.fg)).toBeLessThan(relativeLuminance(blue.fg));
  });

  it('glows only in the dark', () => {
    expect(orbitPalette({ sun: 0.5, contrast: 'std', hue: 205 }).glow).toBe('none');
    expect(orbitPalette({ sun: -0.5, contrast: 'std', hue: 205 }).glow).toMatch(/^0 0 0 1px rgba\(/);
  });
});

describe('the sun', () => {
  it('rises and sets about when it does in Bratislava at midsummer', () => {
    // 48.15 N, 17.11 E; 21 June: sunrise 04:50, sunset 20:52 (CEST).
    const d = new Date(2026, 5, 21, 12);
    const t = sunTimes(d, 48.15, 17.11);
    expect(t).not.toBe('day');
    if (typeof t === 'string') return;
    const offset = -d.getTimezoneOffset() / 60 - 2; // the test box's zone against CEST
    expect(Math.abs(t.rise - offset - 4.83)).toBeLessThan(0.2);
    expect(Math.abs(t.set - offset - 20.87)).toBeLessThan(0.2);
  });

  it('knows the polar day and night', () => {
    expect(sunTimes(new Date(2026, 5, 21), 78, 15)).toBe('day');
    expect(sunTimes(new Date(2026, 11, 21), 78, 15)).toBe('night');
  });

  it('peaks at noon between sunrise and sunset and bottoms out between sunset and sunrise', () => {
    const t = { rise: 6, set: 18 };
    expect(sunHeight(12, t)).toBeCloseTo(1, 5);
    expect(sunHeight(0, t)).toBeCloseTo(-1, 5);
    expect(sunHeight(6, t)).toBeCloseTo(0, 5);
    expect(sunHeight(17.9, t)).toBeGreaterThan(0);
    expect(sunHeight(18.1, t)).toBeLessThan(0);
    // A window that crosses midnight.
    expect(sunHeight(23, { rise: 22, set: 2 })).toBeCloseTo(Math.sin(Math.PI / 4), 5);
  });
});
