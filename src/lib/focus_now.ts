/**
 * `use:focusNow`: focus the element as it mounts. For the button an inline
 * confirm opens on (Keep, beside Delete): the button that opened the
 * confirm unmounts, and without this keyboard focus falls to the page.
 */
export function focusNow(el: HTMLElement): void {
  el.focus();
}
