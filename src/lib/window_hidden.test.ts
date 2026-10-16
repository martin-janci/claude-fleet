import { afterEach, describe, expect, it, vi } from 'vitest';
import { pollWhileShown, windowHidden } from './window_hidden';

describe('windowHidden', () => {
  it('is true only for a hidden document', () => {
    expect(windowHidden({ visibilityState: 'hidden' })).toBe(true);
    expect(windowHidden({ visibilityState: 'visible' })).toBe(false);
    expect(windowHidden(null)).toBe(false);
  });
});

describe('pollWhileShown', () => {
  afterEach(() => vi.useRealTimers());

  function setup() {
    vi.useFakeTimers();
    const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' as DocumentVisibilityState, activeElement: null as Element | null });
    const win = new EventTarget();
    const run = vi.fn(async () => {});
    const p = pollWhileShown(run, { every: 1000, doc: doc as never, win: win as never });
    return { doc, win, run, p };
  }

  it('reads now and on every tick while shown and focused', async () => {
    const { run, p } = setup();
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(run).toHaveBeenCalledTimes(4);
    p.stop();
    await vi.advanceTimersByTimeAsync(3000);
    expect(run).toHaveBeenCalledTimes(4);
  });

  it('skips the ticks while blurred, and reads once on focus', async () => {
    const { win, run, p } = setup();
    await vi.advanceTimersByTimeAsync(0);
    win.dispatchEvent(new Event('blur'));
    await vi.advanceTimersByTimeAsync(5000);
    expect(run).toHaveBeenCalledTimes(1);
    win.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(2);
    p.stop();
  });

  it('a blur that missed no tick reads nothing on focus', async () => {
    const { win, run, p } = setup();
    await vi.advanceTimersByTimeAsync(0);
    win.dispatchEvent(new Event('blur'));
    await vi.advanceTimersByTimeAsync(500);
    win.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(1);
    p.stop();
  });

  it('focus inside an iframe of the page is not away', async () => {
    const { doc, win, run, p } = setup();
    await vi.advanceTimersByTimeAsync(0);
    win.dispatchEvent(new Event('blur'));
    doc.activeElement = { tagName: 'IFRAME' } as Element;
    await vi.advanceTimersByTimeAsync(1000);
    expect(run).toHaveBeenCalledTimes(2);
    p.stop();
  });

  it('a poke while hidden waits for the window to show again', async () => {
    const { doc, run, p } = setup();
    await vi.advanceTimersByTimeAsync(0);
    doc.visibilityState = 'hidden';
    p.poke();
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(1);
    doc.visibilityState = 'visible';
    doc.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(2);
    p.stop();
  });

  it('stops when the read answers false', async () => {
    vi.useFakeTimers();
    const run = vi.fn(async () => false);
    pollWhileShown(run, { every: 1000, doc: null, win: null });
    await vi.advanceTimersByTimeAsync(5000);
    expect(run).toHaveBeenCalledTimes(1);
  });
});
