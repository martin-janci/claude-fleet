// Layout sizes the app reads in script, from the same tokens the CSS uses
// (app.css `:root`), so a size is defined once.
//
// The session list's default width is `--list-w`; a person's drag still
// resizes it between LIST_MIN_PX and LIST_MAX_PX, and the width they leave
// it at is stored (App.svelte, pref `layout.sidebar`) and wins over the
// token from then on.

/** app.css `--list-w`, for a document without app.css (a test, a browser
 *  before the stylesheet). `layout_tokens.test.ts` holds the two equal. */
export const LIST_W_FALLBACK_PX = 340;
export const LIST_MIN_PX = 180;
export const LIST_MAX_PX = 640;

/** A `px` custom property of the root element, or `fallback` when it is
 *  unset or not in px. */
export function tokenPx(name: `--${string}`, fallback: number): number {
  try {
    const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    const m = raw.match(/^(\d+(?:\.\d+)?)px$/);
    return m ? Number(m[1]) : fallback;
  } catch {
    return fallback;
  }
}

/** The session list's width before anyone has dragged it. */
export function listWidthDefault(): number {
  return clampListWidth(tokenPx('--list-w', LIST_W_FALLBACK_PX));
}

export function clampListWidth(px: number): number {
  return Math.max(LIST_MIN_PX, Math.min(LIST_MAX_PX, px));
}
