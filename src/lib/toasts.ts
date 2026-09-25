import { get, writable } from 'svelte/store';
import type { IpcError, Result } from './result';
import { hubNextStep } from './hub';
import { reportError } from './error_report';

// Global, non-blocking notifications. Errors used to land in per-component
// `error: string | null` state that never cleared and dropped the
// `IpcError.code`; `pty_write` rejections were swallowed outright. Everything
// user-facing now goes through here so one region (Toasts.svelte, mounted in
// App.svelte) shows it with the code visible and lets the user dismiss it.

export type ToastKind = 'info' | 'success' | 'warning' | 'error';

/** One inline button on a toast (e.g. `Undo`). Running it dismisses the toast. */
export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  /** IpcError code (`E_*`) when the toast came from a backend failure. */
  code: string | null;
  message: string;
  /** Sticky toasts stay until dismissed; others auto-dismiss. */
  sticky: boolean;
  /** How many times the same code+message was pushed while visible. */
  count: number;
  action: ToastAction | null;
}

export interface PushOptions {
  message: string;
  kind?: ToastKind;
  code?: string | null;
  /** Override the default: errors are sticky, everything else auto-dismisses. */
  sticky?: boolean;
  /** Auto-dismiss delay for non-sticky toasts. */
  timeoutMs?: number;
  /** An inline button; a toast with one stays up for `ACTION_TIMEOUT_MS` by default. */
  action?: ToastAction;
}

export const INFO_TIMEOUT_MS = 4000;
/** Long enough to reach an `Undo` without hurrying. */
export const ACTION_TIMEOUT_MS = 8000;

/**
 * Hard ceiling on the visible stack. Dedup only collapses IDENTICAL
 * code+message pairs, so N failing sessions still push N distinct sticky
 * errors — and the column is bottom-anchored, so an unbounded stack grows
 * straight off the top of the viewport.
 */
export const MAX_TOASTS = 5;

export const toasts = writable<Toast[]>([]);

/**
 * How many toasts the cap has thrown away since the stack was last empty.
 * The UI shows this: dropping something silently would make `Dismiss all (5)`
 * a lie about how much went wrong. Resets to 0 the moment nothing is left.
 */
export const droppedToasts = writable<number>(0);

let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function clearTimer(id: number): void {
  const t = timers.get(id);
  if (t) {
    clearTimeout(t);
    timers.delete(id);
  }
}

/**
 * Trim an over-full stack. Which end to drop from is the whole decision:
 *
 *  - Never the newest. A sticky error discarded on arrival is a worse bug
 *    than the one this cap fixes — the user never learns it failed at all.
 *  - Transients (`sticky: false`) go first, oldest first: they were going to
 *    disappear on their own in a few seconds anyway.
 *  - Only when no transient is left does an old sticky error go, again oldest
 *    first, and every drop is counted so the UI can say how many.
 */
function capped(arr: Toast[]): Toast[] {
  if (arr.length <= MAX_TOASTS) return arr;
  const over = arr.length - MAX_TOASTS;
  const drop = new Set<number>();
  // `arr.length - 1` everywhere: the last entry is the one that just arrived.
  for (let i = 0; i < arr.length - 1 && drop.size < over; i++) {
    if (!arr[i].sticky) drop.add(i);
  }
  for (let i = 0; i < arr.length - 1 && drop.size < over; i++) {
    drop.add(i);
  }
  for (const i of drop) clearTimer(arr[i].id);
  droppedToasts.update((n) => n + drop.size);
  return arr.filter((_, i) => !drop.has(i));
}

function keyOf(code: string | null, message: string): string {
  return `${code ?? ''}\u0000${message}`;
}

/**
 * Show a toast. Deduped by code+message: pushing the same thing again while
 * it is visible bumps its counter (and restarts its timer) instead of
 * stacking a second copy — a flood of identical `pty_write` failures shows
 * once. Returns the toast id (the existing one when deduped).
 */
export function push(opts: PushOptions): number {
  const kind = opts.kind ?? 'info';
  const code = opts.code ?? null;
  const sticky = opts.sticky ?? kind === 'error';
  const key = keyOf(code, opts.message);
  const action = opts.action ?? null;
  const timeout = opts.timeoutMs ?? (action ? ACTION_TIMEOUT_MS : INFO_TIMEOUT_MS);
  const existing = get(toasts).find((t) => keyOf(t.code, t.message) === key);
  if (existing) {
    toasts.update((arr) =>
      arr.map((t) => (t.id === existing.id ? { ...t, count: t.count + 1, action: action ?? t.action } : t)),
    );
    if (!existing.sticky) arm(existing.id, timeout);
    return existing.id;
  }
  const id = nextId++;
  toasts.update((arr) => capped([...arr, { id, kind, code, message: opts.message, sticky, count: 1, action }]));
  if (!sticky) arm(id, timeout);
  return id;
}

function arm(id: number, ms: number): void {
  const prev = timers.get(id);
  if (prev) clearTimeout(prev);
  timers.set(
    id,
    setTimeout(() => dismiss(id), ms),
  );
}

export function dismiss(id: number): void {
  clearTimer(id);
  toasts.update((arr) => (arr.some((x) => x.id === id) ? arr.filter((x) => x.id !== id) : arr));
  // Nothing left on screen means the flood is over: the "+N not shown" note
  // has nothing to qualify any more, and must not outlive the stack.
  if (get(toasts).length === 0) droppedToasts.set(0);
}

/** Run a toast's action (if it still has one) and dismiss the toast. */
export function runToastAction(id: number): void {
  const t = get(toasts).find((x) => x.id === id);
  dismiss(id);
  t?.action?.run();
}

export function clearToasts(): void {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
  toasts.set([]);
  droppedToasts.set(0);
}

/** Sticky error toast for a backend `IpcError`, keeping its `E_*` code
 *  visible. `context` prefixes the message ("Kill failed: …").
 *
 *  A hub client gets one sentence more, for the errors whose next step is
 *  different here and is written down nowhere else. `E_CONFIRM_REQUIRED` is
 *  the one that matters: with `mcp.confirm_destructive` on, the hub refuses a
 *  kill, a worktree delete, a move or a task cancel until someone approves
 *  it — and this desktop's confirmation dialog answers ITS OWN queue, which
 *  in remote mode is always empty. Without the sentence a person clicks, sees
 *  a refusal, and has nowhere to go. This is the one place every backend
 *  failure passes through, which is why it is here and not at each call
 *  site; in standalone mode `hubNextStep` returns null and nothing changes. */
export function pushError(error: IpcError, context?: string): number {
  const base = context ? `${context}: ${error.message}` : error.message;
  const next = hubNextStep(error);
  reportError('frontend', base, error.code ?? null);
  return push({ kind: 'error', code: error.code, message: next ? `${base} — ${next}` : base });
}

/** Convenience: surface a failed `Result`. No-op (returns null) on Ok. */
export function pushResultError(r: Result<unknown>, context?: string): number | null {
  if (r.ok) return null;
  return pushError(r.error, context);
}
