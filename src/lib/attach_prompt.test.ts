import { describe, it, expect } from 'vitest';
import { withAttachments, tooLong, PROMPT_MAX_BYTES } from './attach_prompt';

describe('withAttachments', () => {
  it('names the files under the prompt, one per line', () => {
    expect(withAttachments('look at this', ['/w/p/.claude-fleet-attachments/a.png'])).toBe(
      'look at this\n\nAttached files:\n/w/p/.claude-fleet-attachments/a.png',
    );
  });
  it('leaves a prompt without attachments alone', () => {
    expect(withAttachments('hello', [])).toBe('hello');
  });
  it('works when the draft is empty', () => {
    expect(withAttachments('', ['/w/a.png'])).toBe('Attached files:\n/w/a.png');
  });
});

describe('tooLong', () => {
  it('bounds the prompt below the 128 KiB argv ceiling', () => {
    expect(PROMPT_MAX_BYTES).toBeLessThan(128 * 1024);
    expect(tooLong('x'.repeat(100))).toBe(false);
    expect(tooLong('x'.repeat(PROMPT_MAX_BYTES + 1))).toBe(true);
  });
  it('measures bytes, not characters', () => {
    expect(tooLong('é'.repeat(PROMPT_MAX_BYTES - 10))).toBe(true);
  });
});
