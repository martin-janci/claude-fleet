// Whether the app's window is hidden (minimised, another space, the tray).
// A poller that only feeds what is on screen skips its tick while it is:
// each tick costs an IPC and often an SSH call, for nobody (review r16).
export function windowHidden(doc: Pick<Document, 'visibilityState'> | null = typeof document === 'undefined' ? null : document): boolean {
  return doc?.visibilityState === 'hidden';
}

type Doc = Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'> & {
  activeElement?: Element | null;
};
type Win = Pick<Window, 'addEventListener' | 'removeEventListener'>;

/** A poller started by `pollWhileShown`. */
export interface Poller {
  /** Read now, or as soon as the window shows again when it is away. */
  poke(): void;
  stop(): void;
}

/**
 * Run `run` now and every `every` ms while the window is on screen AND
 * focused. `document.hidden` alone is not enough: in the desktop's
 * WKWebView it is true only for a minimised window, so a window left behind
 * another app kept every poller asking the hub. A tick (or a `poke`) that
 * falls while the window is away is remembered, and the window's return
 * (focus, or visible again) reads once; a short absence that missed no tick
 * reads nothing. Focus is what the window's own blur/focus events say, so a
 * window opened behind another app still reads once, and focus moving into
 * an iframe of the page does not count as away. A `run` that answers
 * `false` stops the poller. A poke while a read is in flight reads once more
 * after it.
 */
export function pollWhileShown(
  run: () => Promise<boolean | void>,
  opts: { every: number; doc?: Doc | null; win?: Win | null },
): Poller {
  const doc = opts.doc === undefined ? (typeof document === 'undefined' ? null : document) : opts.doc;
  const win = opts.win === undefined ? (typeof window === 'undefined' ? null : window) : opts.win;
  let blurred = false;
  let stopped = false;
  let missed = false;
  let running = false;
  let again = false;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const away = () => windowHidden(doc) || (blurred && doc?.activeElement?.tagName !== 'IFRAME');
  const arm = () => {
    if (timer !== null) clearTimeout(timer);
    timer = stopped ? null : setTimeout(() => void fire(), opts.every);
  };
  const fire = async (): Promise<void> => {
    if (stopped) return;
    if (running) {
      again = true;
      return;
    }
    if (away()) {
      missed = true;
      arm();
      return;
    }
    missed = false;
    running = true;
    if (timer !== null) clearTimeout(timer);
    timer = null;
    let more: boolean | void = true;
    try {
      more = await run();
    } catch {
      // A failed read is the next tick's to retry.
    } finally {
      running = false;
    }
    if (stopped) return;
    if (more === false) return stop();
    if (again) {
      again = false;
      return fire();
    }
    arm();
  };
  const shown = () => {
    if (missed && !away()) void fire();
  };
  const onBlur = () => (blurred = true);
  const onFocus = () => {
    blurred = false;
    shown();
  };
  const stop = () => {
    stopped = true;
    if (timer !== null) clearTimeout(timer);
    timer = null;
    win?.removeEventListener('blur', onBlur);
    win?.removeEventListener('focus', onFocus);
    doc?.removeEventListener('visibilitychange', shown);
  };
  win?.addEventListener('blur', onBlur);
  win?.addEventListener('focus', onFocus);
  doc?.addEventListener('visibilitychange', shown);
  void fire();
  return { poke: () => void fire(), stop };
}
