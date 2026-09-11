import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createDrainLoop, DRAIN_MIN_MS, DRAIN_MAX_MS } from './terminal_drain';

describe('createDrainLoop', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** A loop whose drainOnce records the fake-clock time of every tick and
   *  reports output as `output()` says. */
  function setup(output: () => boolean = () => false) {
    const ticks: number[] = [];
    let attached = true;
    const loop = createDrainLoop({
      drainOnce: async () => {
        ticks.push(Date.now());
        return output();
      },
      attached: () => attached,
    });
    return { loop, ticks, detach: () => (attached = false) };
  }

  /** Delay before each tick: the first measured from `start`, the rest from
   *  the previous tick. */
  function gaps(start: number, ticks: number[]): number[] {
    return ticks.map((t, i) => t - (i === 0 ? start : ticks[i - 1]));
  }

  it('backs off by doubling from the 30 ms floor to the 250 ms cap while idle', async () => {
    expect(DRAIN_MIN_MS).toBe(30);
    expect(DRAIN_MAX_MS).toBe(250);
    const { loop, ticks } = setup();
    const t0 = Date.now();
    loop.start();
    await vi.advanceTimersByTimeAsync(30 + 60 + 120 + 240 + 250 + 250);
    // 120 doubles to 240 (still under the cap), then clamps to 250 and stays.
    expect(gaps(t0, ticks)).toEqual([30, 60, 120, 240, 250, 250]);
  });

  it('snaps back to the floor as soon as a tick sees output', async () => {
    let out = false;
    const { loop, ticks } = setup(() => out);
    const t0 = Date.now();
    loop.start();
    await vi.advanceTimersByTimeAsync(30 + 60 + 120); // next delay: 240
    out = true;
    await vi.advanceTimersByTimeAsync(240 + 30);
    expect(gaps(t0, ticks)).toEqual([30, 60, 120, 240, 30]);
  });

  it('bumpDrain resets a backed-off loop to the floor immediately', async () => {
    const { loop, ticks } = setup();
    const t0 = Date.now();
    loop.start();
    await vi.advanceTimersByTimeAsync(30 + 60 + 120 + 240); // next delay: 250
    loop.bumpDrain();
    await vi.advanceTimersByTimeAsync(30);
    expect(gaps(t0, ticks)).toEqual([30, 60, 120, 240, 30]);
    // …and the idle back-off restarts from the floor.
    await vi.advanceTimersByTimeAsync(60);
    expect(gaps(t0, ticks)).toEqual([30, 60, 120, 240, 30, 60]);
  });

  it('bumpDrain does not start a loop that is not running', async () => {
    const { loop, ticks } = setup();
    loop.bumpDrain();
    await vi.advanceTimersByTimeAsync(1000);
    expect(ticks).toEqual([]);
    expect(loop.pending()).toBe(false);
  });

  it('stop cancels the pending tick; start resumes at the floor', async () => {
    const { loop, ticks } = setup();
    loop.start();
    await vi.advanceTimersByTimeAsync(30 + 60); // next delay: 120
    loop.stop();
    expect(loop.pending()).toBe(false);
    await vi.advanceTimersByTimeAsync(1000);
    expect(ticks).toHaveLength(2);
    const t1 = Date.now();
    loop.start();
    await vi.advanceTimersByTimeAsync(30);
    expect(ticks).toHaveLength(3);
    expect(ticks[2] - t1).toBe(30);
  });

  it('does not double-schedule when a restart claims the loop mid-tick', async () => {
    const ticks: number[] = [];
    let restart: (() => void) | null = null;
    const loop = createDrainLoop({
      drainOnce: async () => {
        ticks.push(Date.now());
        // A concurrent openTerm() would start its own loop while this tick
        // is still awaiting; the finished tick must not add a second timer.
        restart?.();
        restart = null;
        return false;
      },
      attached: () => true,
    });
    restart = () => loop.start();
    const t0 = Date.now();
    loop.start();
    await vi.advanceTimersByTimeAsync(30);
    expect(loop.pending()).toBe(true);
    // Past the restarted loop's tick at +60 and past +90, where the timer the
    // finished tick would have queued (at its backed-off 60 ms) would fire.
    await vi.advanceTimersByTimeAsync(90);
    expect(gaps(t0, ticks)).toEqual([30, 30]);
  });

  it('does not reschedule once the host is detached', async () => {
    const { loop, ticks, detach } = setup();
    loop.start();
    detach();
    await vi.advanceTimersByTimeAsync(1000);
    expect(ticks).toHaveLength(1);
    expect(loop.pending()).toBe(false);
  });
});
