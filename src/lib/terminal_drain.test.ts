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

  it('bumpDrain does not start a loop while the host is detached', async () => {
    const { loop, ticks, detach } = setup();
    detach();
    loop.bumpDrain();
    await vi.advanceTimersByTimeAsync(1000);
    expect(ticks).toEqual([]);
    expect(loop.pending()).toBe(false);
  });

  it('bumpDrain revives a loop that stopped rescheduling while attached', async () => {
    let attached = true;
    const ticks: number[] = [];
    const loop = createDrainLoop({
      drainOnce: async () => {
        ticks.push(Date.now());
        return false;
      },
      attached: () => attached,
    });
    loop.start();
    // A tick that finishes while detached does not reschedule — the loop is
    // dead even though the host is attached again a moment later.
    attached = false;
    await vi.advanceTimersByTimeAsync(30);
    attached = true;
    await vi.advanceTimersByTimeAsync(1000);
    expect(ticks).toHaveLength(1);
    expect(loop.pending()).toBe(false);

    const t1 = Date.now();
    loop.bumpDrain();
    await vi.advanceTimersByTimeAsync(30);
    expect(ticks).toHaveLength(2);
    expect(ticks[1] - t1).toBe(30);
  });

  it('bumpDrain during an in-flight tick does not start a second one', async () => {
    const ticks: number[] = [];
    const waiting: Array<() => void> = [];
    let inflight = 0;
    let peak = 0;
    const loop = createDrainLoop({
      drainOnce: async () => {
        ticks.push(Date.now());
        inflight += 1;
        peak = Math.max(peak, inflight);
        await new Promise<void>((resolve) => waiting.push(resolve));
        inflight -= 1;
        return false;
      },
      attached: () => true,
    });
    loop.start();
    await vi.advanceTimersByTimeAsync(DRAIN_MIN_MS);
    expect(inflight).toBe(1);

    // A keystroke (or a paste) lands while the pty_drain round-trip is still
    // outstanding. The loop is not dead, so nothing new may be scheduled —
    // two ticks sharing the screen would apply the PTY bytes out of order.
    loop.bumpDrain();
    await vi.advanceTimersByTimeAsync(DRAIN_MIN_MS + 5);
    expect(peak).toBe(1);
    expect(ticks).toHaveLength(1);

    // The in-flight tick still owns the loop, and the bump is not lost: it
    // comes back at the floor instead of doubling the idle delay.
    waiting.shift()!();
    await vi.advanceTimersByTimeAsync(0);
    const t1 = Date.now();
    await vi.advanceTimersByTimeAsync(DRAIN_MIN_MS);
    expect(ticks).toHaveLength(2);
    expect(ticks[1] - t1).toBe(DRAIN_MIN_MS);
    waiting.shift()?.();
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

  it('keeps polling after a tick rejects', async () => {
    const ticks: number[] = [];
    let fail = true;
    const loop = createDrainLoop({
      drainOnce: async () => {
        ticks.push(Date.now());
        if (fail) throw new Error('parser blew up');
        return true;
      },
      attached: () => true,
    });
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const t0 = Date.now();
      loop.start();
      await vi.advanceTimersByTimeAsync(30);
      // The failed tick counts as idle (no output) and the loop lives on.
      expect(ticks).toHaveLength(1);
      expect(loop.pending()).toBe(true);
      await vi.advanceTimersByTimeAsync(60);
      expect(gaps(t0, ticks)).toEqual([30, 60]);
      // …and a later healthy tick snaps it back to the floor.
      fail = false;
      await vi.advanceTimersByTimeAsync(120 + 30);
      expect(gaps(t0, ticks)).toEqual([30, 60, 120, 30]);
      expect(errors).toHaveBeenCalled();
    } finally {
      errors.mockRestore();
    }
  });

  it('a tick that rejects while detached stops the loop, and bumpDrain cannot revive it', async () => {
    const { loop, ticks, detach } = setup(() => {
      throw new Error('boom');
    });
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      loop.start();
      detach();
      await vi.advanceTimersByTimeAsync(1000);
      expect(ticks).toHaveLength(1);
      loop.bumpDrain();
      await vi.advanceTimersByTimeAsync(1000);
      expect(ticks).toHaveLength(1);
    } finally {
      errors.mockRestore();
    }
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
