// Which loader a search shows beside its current step (redesign 3.13 and
// 9.12): the kit's Dot wave while it is short, Comet trails once it has run
// long, so a big search reads as long work. One loader at a time: the line
// swaps its mark, it never adds a second one.
import type { LoaderName } from './loader-kit.generated';

/** After this long a search counts as long work. */
export const LONG_SEARCH_MS = 3000;

export function searchLoader(elapsedMs: number): LoaderName {
  return elapsedMs >= LONG_SEARCH_MS ? 'comet-trails' : 'dot-wave';
}
