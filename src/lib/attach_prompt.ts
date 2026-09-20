/**
 * Putting attachment paths into a prompt.
 *
 * The prompt is delivered by `tmux send-keys -l` inside a single `bash -lc`
 * argv word, and Linux caps one argument at 128 KiB (MAX_ARG_STRLEN). Nothing
 * on the send path validated that before this module existed: an over-long
 * prompt surfaced as a raw "Argument list too long". Paths are small, but
 * the bound belongs here.
 *
 * `tooLong(text, local)` measures the body the way it actually reaches that
 * argv, not its raw bytes — and the transform differs by target, so the
 * caller says which one applies:
 *
 * - A `local` send goes through ONE quoting pass: `crate::shell::quote`
 *   (`crates/fleet-core/src/shell.rs`) wraps the body in `'...'` and
 *   replaces every embedded `'` with the 4-byte sequence `'\''` before
 *   `bash -c` runs it directly. `quotedByteLength` reproduces that
 *   transform's exact output length (every byte passes through unchanged
 *   except a `'`, which costs 3 extra, plus 2 for the wrapping quotes)
 *   rather than a margin guessed to cover it.
 * - A send to any OTHER host goes through a SECOND pass: `ssh.run` quotes
 *   the whole already-quoted script again (`quote(&script)` in
 *   `crates/fleet-core/src/service/sessions/prompt.rs`), which re-quotes
 *   every `'` the first pass just introduced. That growth compounds rather
 *   than repeats — each escaped quote's `'\''` itself contains three `'`
 *   characters for the second pass to re-escape — so `doubleQuotedByteLength`
 *   models it exactly (`bytes + 12×quotes + 10`), not as the single-pass
 *   formula applied twice. Left unmodelled, a quote-dense prompt can pass
 *   this guard at a size that is fine for a local send and still hit the
 *   real ceiling once SSH re-quotes it: the single-pass model alone trips
 *   at roughly 30,700 literal `'` characters, while the real two-pass
 *   ceiling for a remote host is around 10,000 — a ten-to-thirty-kilobyte
 *   window where the guard this module exists to provide would have missed
 *   exactly the prompts most likely to need it.
 *
 * `local` has no default: a caller must say which model applies rather than
 * risk silently getting the wrong one.
 */

/** Below 128 KiB, with room for the tmux wrapper around the body. */
export const PROMPT_MAX_BYTES = 120 * 1024;

/**
 * The length `crate::shell::quote` would produce for `text`: the 2 bytes of
 * the wrapping quotes, plus 3 extra bytes for every embedded `'` (each one
 * becomes the 4-byte `'\''`). Every other byte passes through unchanged.
 */
function quotedByteLength(text: string): number {
  const bytes = new TextEncoder().encode(text).length;
  const quoteCount = text.split("'").length - 1;
  return bytes + 3 * quoteCount + 2;
}

/**
 * The length after `crate::shell::quote` is applied TWICE: once locally to
 * build the tmux command, once more when `ssh.run` quotes that whole
 * assembled script for a remote shell. The second pass re-quotes every `'`
 * the first pass introduced (each `'\''` contains three `'` characters), so
 * this is `quotedByteLength`'s own output re-quoted — not that formula
 * doubled, and not a margin: `bytes + 12×quotes + 10` is the exact result of
 * quoting `quotedByteLength`'s output.
 */
function doubleQuotedByteLength(text: string): number {
  const bytes = new TextEncoder().encode(text).length;
  const quoteCount = text.split("'").length - 1;
  return bytes + 12 * quoteCount + 10;
}

export function tooLong(text: string, local: boolean): boolean {
  const measured = local ? quotedByteLength(text) : doubleQuotedByteLength(text);
  return measured > PROMPT_MAX_BYTES;
}

export function withAttachments(draft: string, paths: string[]): string {
  if (paths.length === 0) return draft;
  const block = `Attached files:\n${paths.join('\n')}`;
  return draft.trim().length === 0 ? block : `${draft}\n\n${block}`;
}
