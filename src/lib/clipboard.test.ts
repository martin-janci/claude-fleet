import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { trimSelectionText, sanitizePaste, framePaste, copyText } from './clipboard';

describe('trimSelectionText', () => {
  it('trims trailing whitespace per line', () => {
    expect(trimSelectionText('foo   \nbar\t\n')).toBe('foo\nbar\n');
  });
  it('leaves interior and leading whitespace alone', () => {
    expect(trimSelectionText('  foo bar  ')).toBe('  foo bar');
  });
  it('handles an all-blank selection', () => {
    expect(trimSelectionText('   \n   ')).toBe('\n');
  });
});

describe('sanitizePaste', () => {
  it('strips an embedded paste-end marker', () => {
    expect(sanitizePaste('a\x1b[201~b')).toBe('ab');
  });
  it('leaves ordinary text untouched', () => {
    expect(sanitizePaste('hello\nworld')).toBe('hello\nworld');
  });
});

describe('framePaste', () => {
  it('wraps in bracketed-paste markers when enabled', () => {
    expect(framePaste('hi', true)).toBe('\x1b[200~hi\x1b[201~');
  });
  it('returns raw text when disabled', () => {
    expect(framePaste('hi', false)).toBe('hi');
  });
});

describe('copyText', () => {
  const writeText = vi.fn();
  let original: PropertyDescriptor | undefined;
  const setClipboard = (value: unknown) =>
    Object.defineProperty(navigator, 'clipboard', { value, configurable: true });

  beforeEach(() => {
    original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    writeText.mockReset();
    setClipboard({ writeText });
  });
  afterEach(() => {
    if (original) Object.defineProperty(navigator, 'clipboard', original);
    else Reflect.deleteProperty(navigator, 'clipboard');
  });

  it('writes the text and resolves true', async () => {
    writeText.mockResolvedValue(undefined);
    await expect(copyText('hello')).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith('hello');
  });
  it('swallows a rejected write, resolves false and hands the error to onError', async () => {
    const err = new Error('denied');
    writeText.mockRejectedValue(err);
    const onError = vi.fn();
    await expect(copyText('x', onError)).resolves.toBe(false);
    expect(onError).toHaveBeenCalledWith(err);
  });
  it('resolves false without throwing when the Clipboard API is missing', async () => {
    setClipboard(undefined);
    await expect(copyText('x')).resolves.toBe(false);
  });
});
