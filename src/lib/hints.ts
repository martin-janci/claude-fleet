import { writable, derived, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { onboardingWelcomed } from './onboarding';
import { hosts } from './hosts';
import { sessions } from './sessions';

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
    text: 'Hover or select a session to restart, rename, recreate, or kill it.',
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

/**
 * Ids the user has dismissed (persisted). Typed `string[]` rather than
 * `HintId[]` so a stored id from an older/newer build that no longer matches
 * the current `HintId` union is tolerated rather than crashing the validator.
 */
export const seenHints = writable<string[]>(readPref('hints-seen', [], isStringArray));
seenHints.subscribe((v) => writePref('hints-seen', v));

/** Master on/off toggle for all feature hints (persisted, default on). */
export const hintsEnabled = writable<boolean>(readPref('hints-enabled', true, isBool));
hintsEnabled.subscribe((v) => writePref('hints-enabled', v));

export function markSeen(id: HintId): void {
  seenHints.update((s) => (s.includes(id) ? s : [...s, id]));
}

/** Re-show all hints: clear the dismissed set and re-enable hints globally. */
export function resetHints(): void {
  seenHints.set([]);
  hintsEnabled.set(true);
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
  // Consequence: if the holder unmounts while earlier registrants of the same
  // id are still alive, the id is dropped until one of them re-registers. For a
  // one-shot hint this is an acceptable, rare cosmetic miss.
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
 * Hints are allowed to show once the first-run welcome is no longer in play:
 * either the user dismissed the welcome, or the fleet is already non-empty
 * (an existing user who never saw the welcome). Pure — testable.
 */
export function hintsGateOpen(
  welcomed: boolean,
  visibleHostCount: number,
  workSessionCount: number,
): boolean {
  return welcomed || visibleHostCount > 0 || workSessionCount > 0;
}

/**
 * Pure selection: the first hint in `order` that is registered, unseen, with
 * hints enabled and the first-run gate open. `null` otherwise.
 */
export function pickActiveHint(
  order: readonly HintId[],
  registered: ReadonlySet<HintId>,
  seen: readonly string[],
  enabled: boolean,
  gateOpen: boolean,
): HintId | null {
  if (!enabled || !gateOpen) return null;
  for (const id of order) {
    if (registered.has(id) && !seen.includes(id)) return id;
  }
  return null;
}

const order = HINTS.map((h) => h.id);

export const activeHintId: Readable<HintId | null> = derived(
  [registeredIds, seenHints, hintsEnabled, onboardingWelcomed, hosts, sessions],
  ([reg, seen, enabled, welcomed, hostList, sessionList]) => {
    const visibleHostCount = hostList.filter((h) => !h.hidden).length;
    const workSessionCount = sessionList.filter((s) => s.kind !== 'bg').length;
    const gateOpen = hintsGateOpen(welcomed, visibleHostCount, workSessionCount);
    return pickActiveHint(order, reg, seen, enabled, gateOpen);
  },
);

/** Look up a hint definition by id. */
export function hintDef(id: HintId): HintDef | undefined {
  return HINTS.find((h) => h.id === id);
}

