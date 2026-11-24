// Open a link from rendered transcript text in the system browser.
//
// The target is re-checked with `safeHref` at click time, so only http(s) and
// mailto ever reach the opener plugin; the webview itself never navigates.

import { openUrl } from '@tauri-apps/plugin-opener';
import { safeHref } from './markdown';

/**
 * The `href` for a link whose target is data: a pull request a host reported,
 * a tracker item, a failing check. The URL when `safeHref` accepts it,
 * otherwise `undefined`, which leaves the anchor with no target at all.
 *
 * A `target="_blank"` anchor is handed to the system browser by the opener
 * plugin only for http(s), mailto and tel; any other scheme (`file:`,
 * `smb:`, an OS protocol handler) falls through to the webview's own
 * new-window handling, which differs per platform. Such a URL comes from a
 * host's `gh` output or a tracker's API, so it never becomes a target here.
 * `link_targets.test.ts` holds every `href={…}` in the app to this.
 */
export function linkHref(url: string | null | undefined): string | undefined {
  return (url ? safeHref(url) : null) ?? undefined;
}

/** Resolves true when the URL was handed to the system browser. */
export async function openExternal(url: string): Promise<boolean> {
  const safe = safeHref(url);
  if (!safe) return false;
  try {
    await openUrl(safe);
    return true;
  } catch {
    return false;
  }
}
