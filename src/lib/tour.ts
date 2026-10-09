// The first-run tour (Orbit Fleet redesign step 10.5, board Tour): six
// steps, each a spotlight on one part of the window with a popover
// that says what it is for and asks the person to try its key. The tour
// starts once, after the welcome, and Skip is remembered; Settings →
// Appearance → Setup guide takes it again.
import { writable } from 'svelte/store';
import { readPref, writePref } from './prefs';

export interface TourStep {
  id: string;
  title: string;
  body: string;
  /** Where the spotlight goes: the first selector that matches a laid-out
   *  element. None matching centres the popover with no spotlight. */
  targets: string[];
  /** "Try it": the chord in the manual's Mac form (⌘K, ⌥⌘B, j, ?). */
  chord: string;
  tryLabel: string;
}

export const TOUR_STEPS: readonly TourStep[] = [
  {
    id: 'command',
    title: 'Find anything, run anything',
    body: 'The command field reaches every session, task, host and command. Type a few letters of a name, or a verb like "new" or "pause".',
    targets: ['[data-testid="shell-header"] .command'],
    chord: '⌘K',
    tryLabel: 'open it',
  },
  {
    id: 'inbox',
    title: 'Your inbox is what needs you',
    body: 'Only sessions that need you land here: waiting for you, failed, or paused at a limit. The rest is one click away in All sessions. Use Filters and Group to slice it; j and k move, Enter opens.',
    targets: ['[data-testid="pane-sidebar"]'],
    chord: 'j',
    tryLabel: 'move down the list',
  },
  {
    id: 'session',
    title: 'A session is a conversation',
    body: "Read what the agent did and answer its questions with 1, 2 or 3. Terminals, Files and Details sit in the tabs beside it.",
    targets: ['[data-testid="pane-terminal"]'],
    chord: '⌘J',
    tryLabel: 'open the agent tab',
  },
  {
    id: 'inspector',
    title: 'The inspector has the facts',
    body: 'Host, account, branch, pull request, task and cost of the open session, with its actions: move, fork, review, switch account.',
    targets: ['[data-testid="inspector"]', '[data-testid="pane-terminal"]'],
    chord: '⌥⌘B',
    tryLabel: 'show or hide it',
  },
  {
    id: 'control',
    title: 'Control runs the fleet for you',
    body: "Tell Control what you want done. It starts sessions, files tasks and builds missions, and each hand-off shows up as a card you can follow.",
    targets: ['[data-testid="rail"]'],
    chord: '⌘E',
    tryLabel: 'open Control',
  },
  {
    id: 'status',
    title: 'Health and shortcuts live at the bottom',
    body: 'The status bar says whether the fleet and the hub are well, and holds downloads. Every shortcut is one key away.',
    targets: ['footer.status'],
    chord: '?',
    tryLabel: 'see all shortcuts',
  },
];

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/** The tour was finished or skipped: it does not start on its own again. */
export const tourSeen = writable<boolean>(readPref('tour-seen', false, isBool));
tourSeen.subscribe((v) => writePref('tour-seen', v));

/** The step on screen, or `null` when the tour is not running. */
export const tourStep = writable<number | null>(null);

export function startTour(): void {
  tourStep.set(0);
}

export function nextTourStep(): void {
  tourStep.update((s) => {
    if (s === null) return null;
    if (s + 1 >= TOUR_STEPS.length) {
      tourSeen.set(true);
      return null;
    }
    return s + 1;
  });
}

export function prevTourStep(): void {
  tourStep.update((s) => (s === null ? null : Math.max(0, s - 1)));
}

/** Skip, Escape and Done all end it for good. */
export function endTour(): void {
  tourSeen.set(true);
  tourStep.set(null);
}

/** Whether a keydown is the step's "try it" chord. ⌘ is Meta on the Mac
 *  and Ctrl elsewhere (the Keyboard section's rule); a bare key wants no
 *  modifier; `?` is whatever produces it, Shift included. */
export function matchesChord(
  e: Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>,
  chord: string,
  mac: boolean,
): boolean {
  const cmd = chord.includes('⌘');
  const alt = chord.includes('⌥');
  const shift = chord.includes('⇧');
  const key = chord.replace(/[⌘⌥⇧⌃]/g, '');
  const primary = mac ? e.metaKey : e.ctrlKey;
  const other = mac ? e.ctrlKey : e.metaKey;
  if (primary !== cmd || other || e.altKey !== alt) return false;
  if (key === '?') return e.key === '?';
  if (e.shiftKey !== shift) return false;
  if (/^[a-z]$/i.test(key)) {
    // ⌥ turns e.key into a symbol on the Mac; the physical key still says.
    return e.code === `Key${key.toUpperCase()}` || e.key.toLowerCase() === key.toLowerCase();
  }
  return e.key === key;
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export type Side = 'right' | 'left' | 'below' | 'above' | 'inside' | 'centre';

/** Where the popover goes beside `target` in a `vw`×`vh` window: the right
 *  (as the board draws it), else the left, below, above, else inside the
 *  target's top-right corner when it fills the window. Always on screen. */
export function placePopover(
  target: Rect | null,
  pop: { width: number; height: number },
  vw: number,
  vh: number,
  gap = 16,
): { left: number; top: number; side: Side } {
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, Math.max(lo, hi)));
  const m = 8;
  if (!target) {
    return { left: clamp((vw - pop.width) / 2, m, vw - pop.width - m), top: clamp((vh - pop.height) / 2, m, vh - pop.height - m), side: 'centre' };
  }
  const right = target.left + target.width;
  const bottom = target.top + target.height;
  const alongY = clamp(target.top + 24, m, vh - pop.height - m);
  const alongX = clamp(target.left + 24, m, vw - pop.width - m);
  if (right + gap + pop.width + m <= vw) return { left: right + gap, top: alongY, side: 'right' };
  if (target.left - gap - pop.width >= m) return { left: target.left - gap - pop.width, top: alongY, side: 'left' };
  if (bottom + gap + pop.height + m <= vh) return { left: alongX, top: bottom + gap, side: 'below' };
  if (target.top - gap - pop.height >= m) return { left: alongX, top: target.top - gap - pop.height, side: 'above' };
  return {
    left: clamp(right - pop.width - gap, m, vw - pop.width - m),
    top: clamp(target.top + gap, m, vh - pop.height - m),
    side: 'inside',
  };
}
