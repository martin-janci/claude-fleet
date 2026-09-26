/**
 * Whether the composer's chip row has more chips than fit on one line.
 *
 * Extracted from the component because jsdom lays nothing out — scrollWidth
 * and clientWidth are both 0 there — so the decision is unit-tested here and
 * the component is tested by driving its state.
 *
 * Two measurements, one per state of the row. Collapsed (`nowrap`), the
 * row clips, so overflow is its scroll width past its client width.
 * Expanded (`wrap`), nothing clips — scrollWidth equals clientWidth by
 * construction — so asking `needsMore` there always answered "fits", the
 * row collapsed itself on the next resize tick, and More opened onto
 * nothing. Expanded, overflow is "the chips sit on more than one line".
 */

/** Sub-pixel rounding: a row is not "overflowing" by one pixel. */
export const OVERFLOW_SLACK = 1;

export function needsMore(scrollWidth: number, clientWidth: number): boolean {
  if (clientWidth === 0) return false;
  return scrollWidth > clientWidth + OVERFLOW_SLACK;
}

/** The expanded row's test: its chips' `offsetTop`s span more than one line. */
export function wrapsPastOneLine(tops: readonly number[]): boolean {
  if (tops.length < 2) return false;
  return Math.max(...tops) - Math.min(...tops) > OVERFLOW_SLACK;
}
