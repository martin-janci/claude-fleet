// Adaptive PTY drain loop for TerminalView (moved out of TerminalView.svelte,
// F5b). The component keeps `drainOnce` (it writes the screen and its
// reactive counters); this module owns only the scheduling.
//
// Drain loop: a self-rescheduling setTimeout (not setInterval) so a slow
// pty_drain round-trip can't pile up concurrent calls. The delay backs off
// adaptively — an idle terminal polls slowly, any output snaps it back to
// full rate — so an attached-but-quiet session costs almost nothing.

export const DRAIN_MIN_MS = 30;
export const DRAIN_MAX_MS = 250;

export interface DrainHost {
  /** Drain the PTY buffer once. Resolves true if any bytes were consumed. */
  drainOnce(): Promise<boolean>;
  /** Still attached: a screen exists and the PTY is open. */
  attached(): boolean;
}

export function createDrainLoop(host: DrainHost) {
  let drainTimer: ReturnType<typeof setTimeout> | null = null;
  let drainDelay = DRAIN_MIN_MS;

  function scheduleDrain() {
    drainTimer = setTimeout(runDrain, drainDelay);
  }

  /** One drain tick, then reschedule itself. The delay halves to the floor on
   *  any output and doubles toward DRAIN_MAX_MS when idle. */
  async function runDrain() {
    drainTimer = null;
    const got = await host.drainOnce();
    drainDelay = got ? DRAIN_MIN_MS : Math.min(DRAIN_MAX_MS, drainDelay * 2);
    // Reschedule only if still attached and no newer loop has taken over
    // (a concurrent openTerm would have set its own drainTimer).
    if (host.attached() && drainTimer === null) scheduleDrain();
  }

  /** Force the loop back to full rate now — called on keypress so typing
   *  feels responsive even if the terminal had backed off while idle. */
  function bumpDrain() {
    drainDelay = DRAIN_MIN_MS;
    if (drainTimer !== null) {
      clearTimeout(drainTimer);
      drainTimer = null;
      scheduleDrain();
    }
  }

  /** Start at full rate (after a successful attach). */
  function start() {
    drainDelay = DRAIN_MIN_MS;
    scheduleDrain();
  }

  /** Cancel any pending tick and reset the delay (on close). */
  function stop() {
    if (drainTimer) {
      clearTimeout(drainTimer);
      drainTimer = null;
    }
    drainDelay = DRAIN_MIN_MS;
  }

  /** Whether a tick is scheduled. */
  function pending(): boolean {
    return drainTimer !== null;
  }

  return { bumpDrain, start, stop, pending };
}
