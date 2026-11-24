import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { withAttachments, tooLong, PROMPT_MAX_BYTES, PROMPT_MARKER_MARGIN } from './attach_prompt';

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

  // A remote name is `dedupe_names(basenames_of(...))`'d Rust-side but never
  // sanitised, so whatever the user attached rides into the prompt body.
  // This function must pass it through verbatim: it is not the quoter (that
  // is `crate::shell::quote`, measured by `tooLong`), and silently rewriting
  // a path here would name a file that is not the one on disk.
  it('passes a hostile filename through verbatim rather than rewriting it', () => {
    const nasty = "/w/p/.claude-fleet-attachments/a file's \u00e9\u5199 name.png";
    expect(withAttachments('look', [nasty])).toBe(`look\n\nAttached files:\n${nasty}`);
  });

  // A newline in a basename is the one shape that would forge a second
  // entry in the list. Rust drops such a name before it can ever reach here
  // (`record_picked` skips any name containing \n or \r), so this pins the
  // pass-through rather than a scrub — if that Rust guard is ever removed,
  // the forged line is what it would look like, and this test is where the
  // reader is told to go look.
  it('does not invent an escape for a newline — Rust drops those names first', () => {
    expect(withAttachments('look', ['/w/a\nb.png'])).toBe('look\n\nAttached files:\n/w/a\nb.png');
  });
});

describe('tooLong', () => {
  const LIMIT = PROMPT_MAX_BYTES - PROMPT_MARKER_MARGIN;

  it('is the backend limit, read from prompt.rs', () => {
    const rs = readFileSync('crates/fleet-core/src/service/sessions/prompt.rs', 'utf8');
    const m = /pub const MAX_PROMPT_BYTES: usize = ([\d_]+);/.exec(rs);
    expect(m, 'MAX_PROMPT_BYTES in prompt.rs').not.toBeNull();
    expect(PROMPT_MAX_BYTES).toBe(Number(m![1].replace(/_/g, '')));
  });

  it('measures UTF-8 bytes after CRLF folding, with room for the marker line', () => {
    expect(tooLong('x'.repeat(100))).toBe(false);
    expect(tooLong('x'.repeat(LIMIT))).toBe(false);
    expect(tooLong('x'.repeat(LIMIT + 1))).toBe(true);
    expect(tooLong('é'.repeat(LIMIT / 2 + 1))).toBe(true);
    // Each `\r\n` reaches the backend as one byte.
    expect(tooLong('\r\n'.repeat(LIMIT))).toBe(false);
  });

  // The old model counted shell quoting, which the base64 delivery never
  // applies: 70 KiB passed and 15 000 quotes were refused for a remote host.
  it('refuses 70 KiB and takes a quote-dense prompt', () => {
    expect(tooLong('x'.repeat(70 * 1024))).toBe(true);
    expect(tooLong("'".repeat(15000))).toBe(false);
  });
});
