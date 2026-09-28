import { describe, it, expect } from 'vitest';
import { detectWindows, dropPointToLogical, pointInRect, type Rect } from './geometry';

describe('pointInRect', () => {
  // A terminal grid sitting to the right of the sidebar, in logical px.
  const grid: Rect = { left: 260, top: 40, right: 1280, bottom: 800 };

  it('accepts a point inside the rect', () => {
    expect(pointInRect(770, 420, grid)).toBe(true);
  });

  it('accepts points exactly on the edges', () => {
    expect(pointInRect(260, 40, grid)).toBe(true);
    expect(pointInRect(1280, 800, grid)).toBe(true);
  });

  it('rejects a point left of / above the rect', () => {
    expect(pointInRect(100, 20, grid)).toBe(false);
  });

  it('rejects a point right of / below the rect', () => {
    expect(pointInRect(1300, 820, grid)).toBe(false);
  });

  // Regression guard for the Retina drag-drop bug: the macOS drag position is
  // already logical, so a drop must register at its true coordinates. The old
  // code divided by devicePixelRatio (2 on Retina), turning a drop near the
  // grid's top-left — e.g. (480, 70) — into (240, 35), which falls left of and
  // above the grid and misses.
  it('does not require any devicePixelRatio scaling of the point', () => {
    const trueDrop = { x: 480, y: 70 };
    const halved = { x: trueDrop.x / 2, y: trueDrop.y / 2 };
    expect(pointInRect(trueDrop.x, trueDrop.y, grid)).toBe(true);
    // The halved point lands outside the grid (left of and above) — the bug.
    expect(pointInRect(halved.x, halved.y, grid)).toBe(false);
  });
});

describe('dropPointToLogical', () => {
  const rect = { left: 100, top: 100, right: 500, bottom: 400 };
  it('leaves macOS and Linux points alone, even on a 2x display', () => {
    expect(dropPointToLogical({ x: 450, y: 350 }, false, 2)).toEqual({ x: 450, y: 350 });
  });
  it('turns Windows physical pixels into logical ones', () => {
    // 150% scaling: a drop on the lower right of the rect arrives as 675x525.
    const p = dropPointToLogical({ x: 675, y: 525 }, true, 1.5);
    expect(p).toEqual({ x: 450, y: 350 });
    expect(pointInRect(p.x, p.y, rect)).toBe(true);
    expect(pointInRect(675, 525, rect)).toBe(false);
  });
  it('ignores a nonsense scale factor', () => {
    expect(dropPointToLogical({ x: 10, y: 20 }, true, 0)).toEqual({ x: 10, y: 20 });
    expect(dropPointToLogical({ x: 10, y: 20 }, true, Number.NaN)).toEqual({ x: 10, y: 20 });
  });
});

describe('detectWindows', () => {
  it('reads WebView2 and nothing else as Windows', () => {
    expect(detectWindows({ platform: 'Win32', userAgent: '' })).toBe(true);
    expect(detectWindows({ platform: '', userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/140' })).toBe(true);
    expect(detectWindows({ platform: 'MacIntel', userAgent: 'Macintosh' })).toBe(false);
    expect(detectWindows({ platform: 'Linux x86_64', userAgent: 'X11; Linux' })).toBe(false);
    expect(detectWindows(undefined)).toBe(false);
  });
});
