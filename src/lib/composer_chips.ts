/**
 * Which quick-prompt chips the collapsed composer row shows: as many as fit
 * on one line, in order, the rest under ⋯.
 *
 * Extracted from ConversationPanel because jsdom lays nothing out (every
 * width is 0 there), so the decision is unit-tested here and the component
 * falls back to a fixed count when it cannot measure.
 */

/** Sub-pixel rounding: a row is not "overflowing" by one pixel. */
export const FIT_SLACK = 1;

/**
 * The indices of the chips that fit in `available` pixels.
 *
 * @param widths    each chip's width, in preset order
 * @param available the row's width, less whatever else sits on it
 * @param moreWidth the ⋯ chip, reserved whenever any chip is left out
 * @param gap       the row's column gap
 * @param pinned    chips shown whatever the room (a suggested Compact)
 */
export function chipsThatFit(
  widths: readonly number[],
  available: number,
  moreWidth: number,
  gap: number,
  pinned: readonly number[] = [],
): number[] {
  const n = widths.length;
  const all = widths.reduce((a, w) => a + w, 0) + gap * Math.max(0, n - 1);
  if (all <= available + FIT_SLACK) return widths.map((_, i) => i);
  const shown = new Set<number>();
  let used = moreWidth;
  for (const i of pinned) {
    if (i < 0 || i >= n || shown.has(i)) continue;
    shown.add(i);
    used += gap + widths[i];
  }
  // A prefix: the first chip that does not fit ends the row, so the order
  // the person set in Prompts & snippets is the order they see.
  for (let i = 0; i < n; i++) {
    if (shown.has(i)) continue;
    if (used + gap + widths[i] > available + FIT_SLACK) break;
    shown.add(i);
    used += gap + widths[i];
  }
  return [...shown].sort((a, b) => a - b);
}
