import { describe, it, expect, vi, beforeEach } from 'vitest';

const readText = vi.fn();
const writeText = vi.fn();
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: (...a: unknown[]) => readText(...a),
  writeText: (...a: unknown[]) => writeText(...a),
}));

import { nativeReadText, nativeWriteText } from './clipboard_native';

describe('clipboard_native', () => {
  beforeEach(() => {
    readText.mockReset();
    writeText.mockReset();
  });

  it('nativeReadText returns Ok with the clipboard text', async () => {
    readText.mockResolvedValue('hello');
    const r = await nativeReadText();
    expect(r).toEqual({ ok: true, value: 'hello' });
  });

  it('nativeReadText returns Err when the plugin rejects', async () => {
    readText.mockRejectedValue(new Error('denied'));
    const r = await nativeReadText();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.message).toBe('denied');
    if (!r.ok) expect(r.error.code).toBe('E_CLIPBOARD');
  });

  it('nativeReadText coerces a null clipboard to an empty string', async () => {
    readText.mockResolvedValue(null as unknown as string);
    const r = await nativeReadText();
    expect(r).toEqual({ ok: true, value: '' });
  });

  it('nativeWriteText writes and returns Ok', async () => {
    writeText.mockResolvedValue(undefined);
    const r = await nativeWriteText('copied');
    expect(writeText).toHaveBeenCalledWith('copied');
    expect(r).toEqual({ ok: true, value: undefined });
  });

  it('nativeWriteText returns Err when the plugin rejects', async () => {
    writeText.mockRejectedValue(new Error('nope'));
    const r = await nativeWriteText('x');
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_CLIPBOARD');
  });
});
