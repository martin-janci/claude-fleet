/**
 * Putting attachment paths into a prompt.
 *
 * The prompt is delivered by `tmux send-keys -l` inside a single `bash -lc`
 * argv word, and Linux caps one argument at 128 KiB (MAX_ARG_STRLEN). Nothing
 * on the send path validates that today: an over-long prompt surfaces as a
 * raw "Argument list too long". Paths are small, but the bound belongs here.
 *
 * `tooLong` measures the body the way it actually reaches that argv, not its
 * raw bytes: `crate::shell::quote` (`crates/fleet-core/src/shell.rs`) wraps
 * it in `'...'` and replaces every embedded `'` with the 4-byte sequence
 * `'\''`. A quote-heavy prompt can be well under `PROMPT_MAX_BYTES` in its
 * own bytes and still cross the real ceiling once quoted, so
 * `quotedByteLength` reproduces that transform's exact output length (every
 * byte passes through unchanged except a `'`, which costs 3 extra, plus 2
 * for the wrapping quotes) rather than a margin guessed to cover it.
 *
 * That models the ONE quoting pass a send to a `local` host goes through. A
 * prompt sent to any OTHER host goes through a second pass —
 * `ssh.run` quotes the whole assembled script again
 * (`quote(&script)` in `crates/fleet-core/src/service/sessions/prompt.rs`),
 * which re-quotes every `'` the first pass just introduced. This check does
 * not model that second pass (it would need to know the target host and
 * reproduce that command's exact framing to do it honestly), so for a
 * quote-heavy body sent to a non-local host the real ceiling can sit
 * meaningfully below what this function enforces. That is a known, open
 * gap, not a claim that this bound is exact for every target.
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

export function tooLong(text: string): boolean {
  return quotedByteLength(text) > PROMPT_MAX_BYTES;
}

export function withAttachments(draft: string, paths: string[]): string {
  if (paths.length === 0) return draft;
  const block = `Attached files:\n${paths.join('\n')}`;
  return draft.trim().length === 0 ? block : `${draft}\n\n${block}`;
}
