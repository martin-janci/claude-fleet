// The accessibility check component tests run (Orbit Fleet redesign step
// 7.2): axe-core over the rendered DOM, plus the app's own rules from
// `a11y_audit.ts` (status dot with no word, disclosure with no expanded
// state, selected row only a colour marks).
//
//   import { expectAccessible } from './a11y_check';
//   const { container } = render(Thing, { props });
//   await expectAccessible(container);
//
// jsdom has no layout, so axe's colour-contrast and target-size rules are
// off here: contrast is pinned on the tokens (`tokens.test.ts`, kit.test.ts)
// and the 11 px floor and 24 px targets on the source. Page-level rules
// (a main landmark, one h1, a lang) are off because a component is not a
// page.
import axe from 'axe-core';
import { expect } from 'vitest';
import { a11yReport } from './a11y_audit';

const OFF = [
  'color-contrast',
  'color-contrast-enhanced',
  'target-size',
  'region',
  'landmark-one-main',
  'page-has-heading-one',
  'html-has-lang',
  'document-title',
  'bypass',
];

/**
 * Known and left for a follow-up: a row that is itself a `role="button"`
 * (or `option`) holding real controls. ARIA makes a button's children
 * presentational, so a screen reader reads the row as one button and its
 * actions only by Tab. The fix is the list as a tree (rows as treeitems, a
 * named action group inside), which moves Sidebar's and SessionRowItem's
 * keyboard handling; it is its own change. Every other row must pass.
 */
export const KNOWN: readonly { rule: string; testid: string }[] = [
  { rule: 'nested-interactive', testid: 'sess-row' },
  { rule: 'nested-interactive', testid: 'proj-row' },
  { rule: 'nested-interactive', testid: 'tidy-row' },
  // The tidy sheet's listbox holds its group headings and hints beside the
  // options; the same tree change gives them a group.
  { rule: 'aria-required-children', testid: 'tidy-sheet' },
];

function known(line: string): boolean {
  return KNOWN.some((k) => line.startsWith(`${k.rule}: `) && line.includes(`data-testid="${k.testid}"`));
}

/** axe's violations in `root`, one line per node. */
export async function axeReport(root: Element): Promise<string[]> {
  const rules = Object.fromEntries(OFF.map((id) => [id, { enabled: false }]));
  const res = await axe.run(root, { rules, resultTypes: ['violations'] });
  return res.violations
    .flatMap((v) => v.nodes.map((n) => `${v.id}: ${n.html.slice(0, 160)}`))
    .filter((l) => !known(l));
}

/** Fails with every axe and audit violation in `root`. */
export async function expectAccessible(root: Element): Promise<void> {
  const found = [...(await axeReport(root)), ...a11yReport(root)];
  expect(found).toEqual([]);
}
