/**
 * Putting attachment paths into a prompt, and the bound on its size.
 *
 * The body travels base64-encoded on the command line, so shell quoting no
 * longer changes its size: the backend folds `\r\n` and a lone `\r` to
 * `\n` and refuses anything over `MAX_PROMPT_BYTES` (65 536 bytes of UTF-8,
 * `crates/fleet-core/src/service/sessions/prompt.rs`). `tooLong` measures
 * the same way, so an over-long prompt gets the composer's own sentence
 * before its attachments are uploaded, never a raw `E_VALIDATE` after
 * (reviews r4 w09-6, w16-1; the old check modelled `send-keys -l` quoting
 * and allowed 120 KiB).
 */

/** `MAX_PROMPT_BYTES` in prompt.rs; attach_prompt.test.ts holds the two together. */
export const PROMPT_MAX_BYTES = 65_536;

/** Room for the untrusted-input marker line a hub puts ahead of a paired
 *  client's prompt (about 80 bytes) before it measures the body. */
export const PROMPT_MARKER_MARGIN = 256;

/** Whether the backend would refuse `text` as too long. */
export function tooLong(text: string): boolean {
  const folded = text.replace(/\r\n?/g, '\n');
  return new TextEncoder().encode(folded).length > PROMPT_MAX_BYTES - PROMPT_MARKER_MARGIN;
}

export function withAttachments(draft: string, paths: string[]): string {
  if (paths.length === 0) return draft;
  const block = `Attached files:\n${paths.join('\n')}`;
  return draft.trim().length === 0 ? block : `${draft}\n\n${block}`;
}
