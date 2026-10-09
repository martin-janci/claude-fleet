vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import { motionPref } from './motion';
import { WASH_MS, slideIn, snapshotRows, toastIn, toastOut, wash } from './motion_catalog';
import { clearToasts, holdToast, push, releaseToast, toastCountdowns, toasts, INFO_TIMEOUT_MS } from './toasts';
import Toasts from './Toasts.svelte';

// Redesign step 7.4: the Motion board's catalog. jsdom has no Web
// Animations, so each test installs a recording `animate`.

type Call = { keyframes: Keyframe[]; opts: KeyframeAnimationOptions };
let calls: Call[];

beforeEach(() => {
  calls = [];
  Element.prototype.animate = function (keyframes: Keyframe[], opts: KeyframeAnimationOptions) {
    calls.push({ keyframes, opts });
    return { cancel() {}, onfinish: null } as unknown as Animation;
  } as typeof Element.prototype.animate;
  motionPref.set('full');
});

afterEach(() => {
  delete (Element.prototype as { animate?: unknown }).animate;
  motionPref.set('system');
});

function rowAt(id: number, top: number, left = 0): HTMLElement {
  const el = document.createElement('div');
  el.dataset.sessionId = String(id);
  el.getBoundingClientRect = () => ({ top, left, width: 200, height: 30 }) as DOMRect;
  return el;
}

describe('rows slide between groups', () => {
  it('a row remounted in another group slides from its old place', () => {
    const root = document.createElement('div');
    root.append(rowAt(7, 300));
    snapshotRows(root);
    slideIn(rowAt(7, 100), 7);
    expect(calls).toHaveLength(1);
    expect(calls[0].keyframes[0]).toEqual({ transform: 'translate(0px, 200px)' });
    expect(calls[0].opts.duration).toBe(160);
  });

  it('a row that is simply new does not move, nor does one that stayed put', () => {
    const root = document.createElement('div');
    root.append(rowAt(1, 50));
    snapshotRows(root);
    slideIn(rowAt(2, 80), 2);
    slideIn(rowAt(1, 50), 1);
    expect(calls).toEqual([]);
  });

  it('a snapshot is used once', () => {
    const root = document.createElement('div');
    root.append(rowAt(3, 300));
    snapshotRows(root);
    slideIn(rowAt(3, 100), 3);
    slideIn(rowAt(3, 120), 3);
    expect(calls).toHaveLength(1);
  });

  it('only Full reads where the rows are', () => {
    let reads = 0;
    const counted = (id: number) => {
      const el = rowAt(id, 300);
      el.getBoundingClientRect = () => {
        reads++;
        return { top: 300, left: 0, width: 200, height: 30 } as DOMRect;
      };
      return el;
    };
    const root = document.createElement('div');
    root.append(counted(8), counted(9));
    motionPref.set('off');
    snapshotRows(root);
    motionPref.set('reduced');
    snapshotRows(root);
    expect(reads).toBe(0);
    slideIn(rowAt(8, 100), 8);
    expect(calls).toEqual([{ keyframes: [{ opacity: 0 }, { opacity: 1 }], opts: { duration: 80 } }]);
    motionPref.set('full');
    slideIn(rowAt(9, 100), 9);
    expect(calls).toHaveLength(1);
    snapshotRows(root);
    expect(reads).toBe(2);
  });

  it('Reduced fades the row in over 80 ms; Off does nothing', () => {
    const root = document.createElement('div');
    root.append(rowAt(4, 300), rowAt(5, 300));
    snapshotRows(root);
    motionPref.set('reduced');
    slideIn(rowAt(4, 100), 4);
    expect(calls).toEqual([{ keyframes: [{ opacity: 0 }, { opacity: 1 }], opts: { duration: 80 } }]);
    motionPref.set('off');
    slideIn(rowAt(5, 100), 5);
    expect(calls).toHaveLength(1);
  });
});

describe('one wash on a state change', () => {
  it('never washes the state a row mounts with, nor a state it keeps', () => {
    const a = wash(document.createElement('div'), 'action_required');
    a.update('action_required');
    expect(calls).toEqual([]);
  });

  it('a row that turns Needs you takes one waiting tint that fades in 1.2 s', () => {
    const a = wash(document.createElement('div'), 'working');
    a.update('action_required');
    expect(calls).toHaveLength(1);
    expect(calls[0].keyframes).toEqual([{ backgroundColor: 'var(--waiting-soft)' }, { backgroundColor: 'transparent' }]);
    expect(calls[0].opts.duration).toBe(WASH_MS);
  });

  it('Failed and Done tint in their own colour; Working and Idle arrive quietly', () => {
    const a = wash(document.createElement('div'), 'idle');
    a.update('failed');
    a.update('working');
    a.update('done');
    a.update('idle');
    expect(calls.map((c) => c.keyframes[0])).toEqual([
      { backgroundColor: 'var(--failed-soft)' },
      { backgroundColor: 'var(--done-soft)' },
    ]);
  });

  it('Reduced turns the wash into an 80 ms fade; Off drops it', () => {
    motionPref.set('reduced');
    const a = wash(document.createElement('div'), 'working');
    a.update('failed');
    expect(calls[0].opts.duration).toBe(80);
    motionPref.set('off');
    a.update('done');
    expect(calls).toHaveLength(1);
  });
});

describe('toasts', () => {
  beforeEach(() => clearToasts());
  afterEach(() => {
    clearToasts();
    vi.useRealTimers();
  });

  it('enter from the right at Full, as 80 ms fades at Reduced', () => {
    const el = document.createElement('div');
    const full = toastIn(el);
    expect(full.duration).toBe(160);
    expect(full.css?.(0.5, 0.5)).toContain('translate');
    motionPref.set('reduced');
    const reduced = toastIn(el);
    expect(reduced.duration).toBe(80);
    expect(reduced.css?.(0.5, 0.5)).not.toContain('translate');
    expect(toastOut(el).duration).toBe(80);
    motionPref.set('off');
    expect(toastIn(el).duration).toBe(0);
    expect(toastOut(el).duration).toBe(0);
  });

  it('a held toast keeps its remaining time and leaves once released', () => {
    vi.useFakeTimers();
    const id = push({ message: 'Archived 4 sessions' });
    vi.advanceTimersByTime(INFO_TIMEOUT_MS - 1000);
    holdToast(id);
    vi.advanceTimersByTime(INFO_TIMEOUT_MS * 3);
    expect(get(toasts).map((t) => t.id)).toEqual([id]);
    releaseToast(id);
    vi.advanceTimersByTime(999);
    expect(get(toasts)).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(get(toasts)).toHaveLength(0);
    expect(get(toastCountdowns)).toEqual({});
  });

  it('the pointer leaving does not resume a toast focus still holds', () => {
    vi.useFakeTimers();
    const id = push({ message: 'Moved to next' });
    holdToast(id, 'focus');
    holdToast(id, 'pointer');
    releaseToast(id, 'pointer');
    vi.advanceTimersByTime(INFO_TIMEOUT_MS * 3);
    expect(get(toasts).map((t) => t.id)).toEqual([id]);
    releaseToast(id, 'focus');
    vi.advanceTimersByTime(INFO_TIMEOUT_MS);
    expect(get(toasts)).toHaveLength(0);
  });

  it('an auto-dismissing toast shows its countdown bar; a sticky one has none', async () => {
    push({ message: 'Archived 4 sessions' });
    push({ kind: 'error', code: 'E_X', message: 'boom' });
    render(Toasts);
    await tick();
    const bars = screen.queryAllByTestId('toast-timer');
    expect(bars).toHaveLength(1);
    expect(bars[0].style.animationDuration).toBe(`${INFO_TIMEOUT_MS}ms`);
  });

  it('the countdown bar is continuous motion, so Reduced hides it', async () => {
    motionPref.set('reduced');
    push({ message: 'Archived 4 sessions' });
    render(Toasts);
    await tick();
    expect(screen.queryAllByTestId('toast-timer')).toHaveLength(0);
  });
});
