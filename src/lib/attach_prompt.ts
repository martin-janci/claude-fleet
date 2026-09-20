/**
 * Putting attachment paths into a prompt.
 *
 * The prompt is delivered by `tmux send-keys -l` inside a single `bash -lc`
 * argv word, and Linux caps one argument at 128 KiB (MAX_ARG_STRLEN). Nothing
 * on the send path validates that today: an over-long prompt surfaces as a
 * raw "Argument list too long". Paths are small, but the bound belongs here.
 */

/** Below 128 KiB, with room for the tmux wrapper around the body. */
export const PROMPT_MAX_BYTES = 120 * 1024;

export function tooLong(text: string): boolean {
  return new TextEncoder().encode(text).length > PROMPT_MAX_BYTES;
}

export function withAttachments(draft: string, paths: string[]): string {
  if (paths.length === 0) return draft;
  const block = `Attached files:\n${paths.join('\n')}`;
  return draft.trim().length === 0 ? block : `${draft}\n\n${block}`;
}
