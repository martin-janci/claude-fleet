// Open a link from rendered transcript text in the system browser.
//
// The target is re-checked with `safeHref` at click time, so only http(s) and
// mailto ever reach the opener plugin; the webview itself never navigates.

import { openUrl } from '@tauri-apps/plugin-opener';
import { safeHref } from './markdown';

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
