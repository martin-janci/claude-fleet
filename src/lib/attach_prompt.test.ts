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
    expect(tooLong('x'.repeat(100), true)).toBe(false);
    expect(tooLong('x'.repeat(PROMPT_MAX_BYTES + 1), true)).toBe(true);
  });
  it('measures bytes, not characters', () => {
    expect(tooLong('é'.repeat(PROMPT_MAX_BYTES - 10), true)).toBe(true);
  });

  // `crate::shell::quote` (crates/fleet-core/src/shell.rs) wraps the body in
  // `'...'` and replaces every embedded `'` with the 4-byte sequence `'\''`
  // before it reaches the argv this bound protects. A quote-heavy body can
  // be well under PROMPT_MAX_BYTES in its own bytes and still cross the real
  // ceiling once quoted — a byte-only check misses exactly this case. This
  // is the single quoting pass a `local` send goes through, and this exact
  // input/output pair must not change once the remote (double-quoting)
  // model exists alongside it — the two models must stay independent.
  it('accounts for one pass of shell-quote expansion for a local host', () => {
    const quoteHeavy = "'".repeat(40000);
    expect(new TextEncoder().encode(quoteHeavy).length).toBeLessThan(PROMPT_MAX_BYTES);
    expect(tooLong(quoteHeavy, true)).toBe(true);
  });

  // A send to any host other than `local` goes through a SECOND quoting
  // pass: `ssh.run` quotes the whole assembled script again
  // (`quote(&script)` in `crates/fleet-core/src/service/sessions/prompt.rs`),
  // which re-quotes every `'` the first pass just introduced — the growth
  // compounds (`bytes + 12×quotes + 10`), not merely repeats. 15,000 quote
  // characters sits below the single-pass trip point (~30,700) but above
  // the two-pass one (~10,000): fine locally, refused for anywhere else.
  it('a quote-dense prompt that is fine locally is refused for a remote host', () => {
    const quoteDense = "'".repeat(15000);
    expect(tooLong(quoteDense, true)).toBe(false);
    expect(tooLong(quoteDense, false)).toBe(true);
  });

  // The fix for the remote gap must not make the LOCAL bound stricter than
  // it already was — the single-pass model (and therefore every local
  // result above) is unchanged.
  it('the local bound is exactly what it was before the remote model existed', () => {
    expect(tooLong('x'.repeat(100), true)).toBe(false);
    expect(tooLong('x'.repeat(PROMPT_MAX_BYTES + 1), true)).toBe(true);
    expect(tooLong("'".repeat(40000), true)).toBe(true);
  });
});
