// Control as a Thread room (direction A, 2026-10-14): the frame's own state.
// The left column (Main chat and the threads Control handed out) can be
// resized and collapsed; the right panel is resizable between 320 and
// 640 px and remembers its width the way the sidebar does
// (`layout.sidebar` in App.svelte), and hides with its edge button or a
// drag to zero. Any session the chat names (a handoff chip, a child card, a
// `#`/`@` chip, a thread row) opens in the right panel's Session tab, in
// place: Control stays, nothing is told to navigate.
import { get, writable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { sessions } from './sessions';
import { selectSession } from './selection';
import { selectView, setViewsOpen } from './control_views';
import { push } from './toasts';

export const RIGHT_MIN = 320;
export const RIGHT_MAX = 640;
export const RIGHT_DEFAULT = 380;
/** A drag that takes the panel below this hides it ("drag to zero"). */
export const RIGHT_HIDE_BELOW = 200;

export const LEFT_MIN = 200;
export const LEFT_MAX = 360;
export const LEFT_DEFAULT = 256;

const isNumber = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

export const clampRight = (px: number): number => Math.round(Math.min(RIGHT_MAX, Math.max(RIGHT_MIN, px)));
export const clampLeft = (px: number): number => Math.round(Math.min(LEFT_MAX, Math.max(LEFT_MIN, px)));

/** The right panel's width, px. */
export const controlRightPx = writable<number>(clampRight(readPref('layout.control-right', RIGHT_DEFAULT, isNumber)));
/** The left column's width, px. */
export const controlLeftPx = writable<number>(clampLeft(readPref('layout.control-left', LEFT_DEFAULT, isNumber)));
/** The left column folded to its edge button. */
export const controlLeftCollapsed = writable<boolean>(readPref('layout.control-left-collapsed', false, isBool));

// A drag writes on every frame; the pref is written once it settles.
function persist(store: typeof controlRightPx, key: string): void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let first = true;
  store.subscribe((px) => {
    if (first) {
      first = false;
      return;
    }
    clearTimeout(timer);
    timer = setTimeout(() => writePref(key, px), 200);
  });
}
persist(controlRightPx, 'layout.control-right');
persist(controlLeftPx, 'layout.control-left');
controlLeftCollapsed.subscribe((v) => writePref('layout.control-left-collapsed', v));

/**
 * One drag step on the right panel's handle (the handle sits on its LEFT
 * edge, so a drag to the left widens it). `raw` is the drag's running
 * width, unclamped: below `RIGHT_HIDE_BELOW` the panel hides and keeps its
 * last width for when it comes back. Answers the new running width.
 */
export function dragRight(raw: number, delta: number): number {
  const next = raw - delta;
  if (next < RIGHT_HIDE_BELOW) {
    setViewsOpen(false);
    return RIGHT_MIN;
  }
  controlRightPx.set(clampRight(next));
  return next;
}

export function dragLeft(delta: number): void {
  controlLeftPx.update((px) => clampLeft(px + delta));
}

/** Which thread the left column marks: null is Main chat. */
export type ActiveThread = { kind: 'session'; id: number } | { kind: 'mission'; id: number } | null;
export const activeThread = writable<ActiveThread>(null);

/** The mission the right panel's Missions view shows; null lists them. */
export const controlMission = writable<number | null>(null);

/**
 * Open a session in the right panel's Session tab, in place. Returns false,
 * with a toast, when the fleet no longer has it.
 */
export function openInControl(id: number, label?: string): boolean {
  const row = get(sessions).find((s) => s.id === id);
  if (!row) {
    push({ kind: 'info', message: `${label ?? 'That session'} is gone: the session is no longer in the fleet.` });
    return false;
  }
  selectSession(row, { stay: true });
  activeThread.set({ kind: 'session', id });
  selectView('session');
  return true;
}

/** Open a mission's detail in the right panel's Missions view. */
export function openMissionInControl(id: number): void {
  controlMission.set(id);
  activeThread.set({ kind: 'mission', id });
  selectView('missions');
}

/** Main chat: the centre is always it; the column marks it again. */
export function showMainChat(): void {
  activeThread.set(null);
}
