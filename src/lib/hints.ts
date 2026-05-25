import { writable, derived, get, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { onboardingWelcomed } from './onboarding';

export type HintId =
  | 'host-filter'
  | 'bg-session'
  | 'session-actions'
  | 'terminal-header'
  | 'recency-filter';

export type Placement = 'top' | 'bottom' | 'left' | 'right';

export interface HintDef {
  id: HintId;
  text: string;
  placement: Placement;
}

/** Ordered by priority — earlier hints win when several are eligible at once. */
export const HINTS: HintDef[] = [
  {
    id: 'host-filter',
    text: 'Filter sessions by machine — the dot shows reachability.',
    placement: 'bottom',
  },
  {
    id: 'bg-session',
    text: 'Launch a background agent: headless Claude that works without an attached terminal.',
    placement: 'top',
  },
  {
    id: 'session-actions',
    text: 'Restart, rename, recreate, or kill a session from these icons.',
    placement: 'bottom',
  },
  {
    id: 'terminal-header',
    text: "You're attached to this session's tmux. Right-click for copy / paste.",
    placement: 'bottom',
  },
  {
    id: 'recency-filter',
    text: 'Narrow the list to recent activity.',
    placement: 'bottom',
  },
];

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
const isStringArray = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((x) => typeof x === 'string');

/** Ids the user has dismissed (persisted). */
export const seenHints = writable<string[]>(readPref('hints-seen', [], isStringArray));
seenHints.subscribe((v) => writePref('hints-seen', v));

/** Master on/off toggle for all feature hints (persisted, default on). */
export const hintsEnabled = writable<boolean>(readPref('hints-enabled', true, isBool));
hintsEnabled.subscribe((v) => writePref('hints-enabled', v));

export function markSeen(id: HintId): void {
  seenHints.update((s) => (s.includes(id) ? s : [...s, id]));
}

export function resetHints(): void {
  seenHints.set([]);
}

// Set of hint ids whose anchor is currently mounted AND relevant — drives
// reactivity. The actual elements live in a plain Map for lookup by HintLayer.
const registeredIds = writable<Set<HintId>>(new Set());
const anchorEls = new Map<HintId, HTMLElement>();

function register(id: HintId, node: HTMLElement): void {
  anchorEls.set(id, node);
  registeredIds.update((s) => {
    if (s.has(id)) return s;
    const n = new Set(s);
    n.add(id);
    return n;
  });
}

function unregister(id: HintId, node: HTMLElement): void {
  // Only clear if THIS node is the current holder (multiple rows may share an id,
  // e.g. session-actions — last mount wins, and only it unregisters).
  if (anchorEls.get(id) !== node) return;
  anchorEls.delete(id);
  registeredIds.update((s) => {
    const n = new Set(s);
    n.delete(id);
    return n;
  });
}

/** Look up the live anchor element for a hint (used by HintLayer). */
export function anchorEl(id: HintId): HTMLElement | undefined {
  return anchorEls.get(id);
}

export interface HintAnchorParams {
  id: HintId;
  /** When false, the feature isn't relevant yet — anchor is not registered. */
  when?: boolean;
}

/** Svelte action: tag an element as a hint anchor. `use:hintAnchor={{ id, when }}`. */
export function hintAnchor(node: HTMLElement, params: HintAnchorParams) {
  let { id, when = true } = params;
  if (when) register(id, node);
  return {
    update(next: HintAnchorParams) {
      const nextWhen = next.when ?? true;
      if (next.id === id && nextWhen === when) return;
      unregister(id, node);
      id = next.id;
      when = nextWhen;
      if (when) register(id, node);
    },
    destroy() {
      unregister(id, node);
    },
  };
}

/**
 * Pure selection: the first hint in `order` that is registered, unseen, with
 * hints enabled and the welcome already dismissed. `null` otherwise.
 */
export function pickActiveHint(
  order: readonly HintId[],
  registered: ReadonlySet<HintId>,
  seen: readonly string[],
  enabled: boolean,
  welcomed: boolean,
): HintId | null {
  if (!enabled || !welcomed) return null;
  for (const id of order) {
    if (registered.has(id) && !seen.includes(id)) return id;
  }
  return null;
}

const order = HINTS.map((h) => h.id);

export const activeHintId: Readable<HintId | null> = derived(
  [registeredIds, seenHints, hintsEnabled, onboardingWelcomed],
  ([reg, seen, enabled, welcomed]) =>
    pickActiveHint(order, reg, seen, enabled, welcomed),
);

/** Look up a hint definition by id. */
export function hintDef(id: HintId): HintDef | undefined {
  return HINTS.find((h) => h.id === id);
}

export { get };
