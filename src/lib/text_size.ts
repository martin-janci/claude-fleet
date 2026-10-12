import { writable } from 'svelte/store';
import { readPref, writePref } from './prefs';

/**
 * Text size (Settings › Appearance, gap plan G4.6): scales the whole app
 * from its 13 px base. A per-device pref, like the theme and Motion.
 *
 * The app's sizes are px tokens, so this is a page zoom rather than a root
 * font size: in the desktop app the webview's own zoom (layout, terminals
 * and hit-testing all follow it), elsewhere CSS `zoom` on `<html>`.
 */

export const TEXT_SIZES = [90, 100, 110, 125, 150] as const;
export type TextSize = (typeof TEXT_SIZES)[number];

const isTextSize = (v: unknown): v is TextSize => TEXT_SIZES.includes(v as TextSize);

export const textSize = writable<TextSize>(readPref<TextSize>('ui.textSize', 100, isTextSize));
textSize.subscribe((v) => writePref('ui.textSize', v));

/** "100%". */
export function textSizeLabel(pct: TextSize): string {
  return `${pct}%`;
}

type Zoom = (factor: number) => Promise<void>;

/** The desktop webview's zoom, or null outside the desktop app. */
async function webviewZoom(): Promise<Zoom | null> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return null;
  try {
    const { getCurrentWebview } = await import('@tauri-apps/api/webview');
    const wv = getCurrentWebview();
    return (f) => wv.setZoom(f);
  } catch {
    return null;
  }
}

/** Apply `pct` to this window. `zoom` is for tests. */
export async function applyTextSize(pct: TextSize, zoom: Zoom | null = null): Promise<void> {
  const factor = pct / 100;
  const z = zoom ?? (await webviewZoom());
  if (z) {
    try {
      await z(factor);
      return;
    } catch {
      /* fall through to CSS zoom */
    }
  }
  if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('zoom', pct === 100 ? '' : String(factor));
  }
}

/** Keep this window at the pref; returns the stop. */
export function initTextSize(): () => void {
  return textSize.subscribe((v) => void applyTextSize(v));
}
