import { get } from 'svelte/store';
import { fade, fly, type TransitionConfig } from 'svelte/transition';
import { cubicOut } from 'svelte/easing';
import { DURATIONS, effectiveMotion, type Motion } from './motion';
import type { AttentionState } from './attention';

/**
 * The motion catalog applied (redesign step 7.4, the Motion board): the three
 * movements the manual names, each reading the Motion level at the moment it
 * plays, so Settings → Appearance → Motion governs them like every CSS
 * transition (motion.ts).
 *
 * - **Rows slide between groups**: a row whose state moves it to another
 *   group glides there (`snapshotRows` + `slideIn`, keyed by session id).
 * - **One wash on a state change**: a row that turns Needs you, Failed,
 *   Blocked or Done takes one soft tint of that state, which fades; nothing
 *   loops or pulses (`wash`).
 * - **Toasts** enter from the right and leave with a fade (`toastIn` /
 *   `toastOut`); the timer bar lives in Toasts.svelte.
 *
 * Reduced turns all of it into `--dur-fast` (80 ms) fades; Off into nothing.
 */

/** The wash holds its tint this long at Full before it has faded (board: 1.2 s). */
export const WASH_MS = 1200;

/** The states a row signals once when it enters them; the rest arrive quietly. */
const WASH_TINT: Partial<Record<AttentionState, string>> = {
  action_required: 'var(--waiting-soft)',
  blocked: 'var(--waiting-soft)',
  failed: 'var(--failed-soft)',
  done: 'var(--done-soft)',
};

/**
 * The Motion level a movement plays at. Svelte's transitions run on the Web
 * Animations API; where it is missing (jsdom, an old webview) a transition
 * would never finish and an outgoing element would never leave, so nothing
 * moves there at all.
 */
const level = (): Motion =>
  typeof Element !== 'undefined' && typeof Element.prototype.animate === 'function' ? get(effectiveMotion) : 'off';

/** How long a catalog movement runs at the current level. */
export function catalogMs(token: 'fast' | 'base' | 'slow', motion: Motion = level()): number {
  return DURATIONS[motion][token];
}

/** Reduced and Off replace a movement with a plain fade of this length. */
function quietFade(node: Element, motion: Motion): TransitionConfig {
  return fade(node, { duration: DURATIONS[motion].fast });
}

/**
 * Where each row sat before the list last changed, keyed by session id. A
 * row that changes group is a new element in its new group's block, so the
 * list snapshots positions just before it re-renders (`snapshotRows`, from a
 * `$effect.pre`) and the new element slides from the old place (`slideIn`).
 * A row that stays in its block keeps its element and never looks here.
 */
let lastRects = new Map<string, DOMRect>();

/** Record every row's place under `root` before the list re-renders. */
export function snapshotRows(root: ParentNode | null | undefined): void {
  const next = new Map<string, DOMRect>();
  root?.querySelectorAll<HTMLElement>('[data-session-id]').forEach((el) => {
    const id = el.dataset.sessionId;
    if (id) next.set(id, el.getBoundingClientRect());
  });
  lastRects = next;
}

/**
 * Svelte action: a row mounted where its session already had a row slides
 * from the old place to the new one (FLIP); a row that is simply new does
 * not move. Reduced fades it in over 80 ms; Off does nothing.
 */
export function slideIn(node: HTMLElement, key: string | number) {
  const from = lastRects.get(String(key));
  lastRects.delete(String(key));
  const motion = level();
  if (!from || motion === 'off' || typeof node.animate !== 'function') return;
  if (motion === 'reduced') {
    node.animate([{ opacity: 0 }, { opacity: 1 }], { duration: DURATIONS.reduced.fast });
    return;
  }
  const to = node.getBoundingClientRect();
  const dx = from.left - to.left;
  const dy = from.top - to.top;
  if (dx === 0 && dy === 0) return;
  node.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }], {
    duration: catalogMs('base', motion),
    easing: 'cubic-bezier(0.2, 0, 0, 1)',
  });
}

/** A toast arrives from the right (board: "enter from the right"). */
export function toastIn(node: Element): TransitionConfig {
  const motion = level();
  if (motion !== 'full') return quietFade(node, motion);
  return fly(node, { x: 16, duration: catalogMs('base', motion), easing: cubicOut });
}

/** A toast leaves with a fade, no bounce. */
export function toastOut(node: Element): TransitionConfig {
  return fade(node, { duration: catalogMs('fast') });
}

/** Whether the toast timer bar runs: it is continuous motion, so Full only. */
export function timerBarRuns(motion: Motion = get(effectiveMotion)): boolean {
  return motion === 'full';
}

/**
 * Svelte action: one tint when the row's attention state changes into one
 * that signals. The first state a row mounts with never washes, and a state
 * that stays the same never washes again.
 */
export function wash(node: HTMLElement, state: AttentionState) {
  let current = state;
  let anim: Animation | null = null;
  return {
    update(next: AttentionState) {
      if (next === current) return;
      current = next;
      const tint = WASH_TINT[next];
      const motion = level();
      if (!tint || motion === 'off' || typeof node.animate !== 'function') return;
      anim?.cancel();
      node.dataset.wash = next;
      anim = node.animate([{ backgroundColor: tint }, { backgroundColor: 'transparent' }], {
        duration: motion === 'full' ? WASH_MS : DURATIONS.reduced.fast,
        easing: 'ease-out',
      });
      anim.onfinish = () => {
        delete node.dataset.wash;
        anim = null;
      };
    },
    destroy() {
      anim?.cancel();
    },
  };
}
