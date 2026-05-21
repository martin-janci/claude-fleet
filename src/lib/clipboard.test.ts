import { describe, it, expect } from 'vitest';
import { trimSelectionText, sanitizePaste, framePaste } from './clipboard';

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
