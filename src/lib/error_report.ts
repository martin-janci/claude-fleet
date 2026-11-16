import { invokeCmd } from './result';

// Frontend errors for the hub error channel (spec
// docs/superpowers/specs/2026-09-21-hub-error-channel-design.md). Each call
// becomes one `report_client_error` invoke, which the backend queues and — in
// hub-client mode — flushes to the hub. Two guards keep a render loop from
// becoming a request loop: identical component+message within DEDUPE_MS is
// counted, not resent, and at most MAX_PER_MINUTE distinct reports go out.

export const MAX_PER_MINUTE = 20;
export const DEDUPE_MS = 60_000;

interface Seen {
  at: number;
  repeats: number;
}

let seen = new Map<string, Seen>();
let windowStart = 0;
let sentThisWindow = 0;
let pendingRepeats = 0;

/** Codes with which the backend says no by rule: the caller may not do this,
 *  the project forbids that login, the session is pinned, a person has to
 *  confirm first. The toast tells the person; nothing failed, so it is not an
 *  error report. (On 2026-11-16 the hub's report log held "Kill failed:
 *  kill_session refused …" and "Recreate failed: … is forbidden for …" beside
 *  a real fault.) Keep it to codes that are a refusal whatever raised them. */
const REFUSALS: ReadonlySet<string> = new Set([
  'E_FORBIDDEN',
  'E_ACCOUNT_FORBIDDEN',
  'E_ACCOUNT_LIMIT',
  'E_CONFIRM_REQUIRED',
  'E_PINNED',
]);

/** Whether `code` is the backend refusing by rule rather than failing. */
export function isRefusal(code: string | null | undefined): boolean {
  return code != null && REFUSALS.has(code);
}

export function resetErrorReportingForTests(): void {
  seen = new Map();
  windowStart = 0;
  sentThisWindow = 0;
  pendingRepeats = 0;
}

export function reportError(
  component: string,
  message: string,
  code: string | null = null,
  context: Record<string, unknown> = {},
): void {
  const now = Date.now();
  // Prune first: a stale entry must not keep deduping forever, and must not
  // linger once its DEDUPE_MS window has passed.
  for (const [k, v] of seen) {
    if (now - v.at >= DEDUPE_MS) seen.delete(k);
  }
  const key = `${component}\u0000${message}`;
  const prior = seen.get(key);
  if (prior) {
    prior.repeats += 1;
    pendingRepeats += 1;
    return;
  }
  if (now - windowStart >= 60_000) {
    windowStart = now;
    sentThisWindow = 0;
  }
  // Rate-limited: leave `seen` untouched. Marking it here would silently
  // swallow this message's recurrences as dedupe hits instead of retrying it
  // once the window turns over.
  if (sentThisWindow >= MAX_PER_MINUTE) return;
  sentThisWindow += 1;
  seen.set(key, { at: now, repeats: 0 });
  const ctx: Record<string, unknown> = { ...context };
  if (pendingRepeats > 0) {
    ctx.repeats = pendingRepeats;
    pendingRepeats = 0;
  }
  void invokeCmd('report_client_error', {
    args: { level: 'error', component, code, message: message.slice(0, 2048), context: ctx },
  });
}

function describe(reason: unknown): string {
  if (reason instanceof Error) return `${reason.name}: ${reason.message}`;
  if (typeof reason === 'string') return reason;
  try {
    return JSON.stringify(reason).slice(0, 500);
  } catch {
    return String(reason);
  }
}

/** The browser's notice that a ResizeObserver callback changed layout again
 *  and the rest is delivered on the next frame ("ResizeObserver loop
 *  completed with undelivered notifications.", or the older "… loop limit
 *  exceeded"). It arrives as a window `error` event with no error object;
 *  nothing threw and nothing is lost, so it is not a crash to report. */
const RESIZE_OBSERVER_NOTICE = /^ResizeObserver loop (completed with undelivered notifications|limit exceeded)/;

export function installErrorReporting(win: Window = window): void {
  win.addEventListener('error', (e) => {
    if (!e.error && RESIZE_OBSERVER_NOTICE.test(e.message ?? '')) return;
    const stack = e.error instanceof Error && e.error.stack ? e.error.stack.slice(0, 2000) : undefined;
    reportError('frontend:unhandled', e.message || describe(e.error), null, {
      file: e.filename,
      line: e.lineno,
      stack,
    });
  });
  win.addEventListener('unhandledrejection', (e) => {
    const reason = (e as PromiseRejectionEvent).reason;
    const stack = reason instanceof Error && reason.stack ? reason.stack.slice(0, 2000) : undefined;
    reportError('frontend:unhandled', describe(reason), null, { stack });
  });
}
