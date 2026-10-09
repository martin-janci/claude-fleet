// Redesign step 1.5's rule, as a check the view tests share: one primary per
// view, and the destructive action last, behind a confirm. Session details,
// Hosts, Missions and Task detail each hold themselves to it.
import { expect } from 'vitest';

/**
 * The primary controls rendered under `root`: every `.btn--primary` (and the
 * kit's `.of-btn.primary`). A split button (`.split`, the Work button) is one
 * control whose two halves are both drawn primary, so it counts once.
 */
export function primaryControls(root: ParentNode): HTMLElement[] {
  const out: HTMLElement[] = [];
  const groups = new Set<Element>();
  for (const el of Array.from(root.querySelectorAll<HTMLElement>('.btn--primary, .of-btn.primary'))) {
    const group = el.closest('.split') ?? el;
    if (groups.has(group)) continue;
    groups.add(group);
    out.push(el);
  }
  return out;
}

/** Exactly one primary under `root`, and (when named) it is `testid`. */
export function expectOnePrimary(root: ParentNode, testid?: string): HTMLElement {
  const found = primaryControls(root);
  expect(
    found.map((el) => el.getAttribute('data-testid') ?? el.textContent?.trim()),
    'one primary per view (redesign 1.5)',
  ).toHaveLength(1);
  if (testid) expect(found[0].getAttribute('data-testid')).toBe(testid);
  return found[0];
}

/** `el` is the last button of `group`: the destructive action comes last. */
export function expectLastButton(group: ParentNode, el: Element): void {
  const buttons = Array.from(group.querySelectorAll('button'));
  expect(buttons.at(-1), 'the destructive action is last').toBe(el);
}
