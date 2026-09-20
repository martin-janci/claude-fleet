/**
 * Whether the composer's chip row has more chips than fit on one line.
 *
 * Extracted from the component because jsdom lays nothing out — scrollWidth
 * and clientWidth are both 0 there — so the decision is unit-tested here and
 * the component is tested by driving its state.
 */

/** Sub-pixel rounding: a row is not "overflowing" by one pixel. */
export const OVERFLOW_SLACK = 1;

export function needsMore(scrollWidth: number, clientWidth: number): boolean {
  if (clientWidth === 0) return false;
  return scrollWidth > clientWidth + OVERFLOW_SLACK;
}
