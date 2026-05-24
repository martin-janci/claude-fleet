// Native clipboard access via tauri-plugin-clipboard-manager. Used instead of
// navigator.clipboard, which in WKWebView shows a permission tooltip on read
// and fails silently. Returns Result so callers surface errors instead of
// swallowing them.
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager';
import type { Result } from './result';

function toErr(raw: unknown): { code: string; message: string } {
  if (raw instanceof Error) return { code: 'E_CLIPBOARD', message: raw.message };
  return { code: 'E_CLIPBOARD', message: String(raw) };
}

export async function nativeReadText(): Promise<Result<string>> {
  try {
    return { ok: true, value: (await readText()) ?? '' };
  } catch (raw) {
    return { ok: false, error: toErr(raw) };
  }
}

export async function nativeWriteText(text: string): Promise<Result<void>> {
  try {
    await writeText(text);
    return { ok: true, value: undefined };
  } catch (raw) {
    return { ok: false, error: toErr(raw) };
  }
}
