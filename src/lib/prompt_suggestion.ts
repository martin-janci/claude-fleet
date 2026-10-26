import type { SessionRow } from './sessions';

/** The next prompt Claude Code proposes for `row`, when it is worth offering:
 *  the session is idle (a proposal belongs to the turn that just ended, and a
 *  new turn makes it stale before the hub clears it) and the person has not
 *  started typing over it, nor has a prompt of their own in flight (`sending`:
 *  until its hook lands the row still reads idle with the old proposal, and
 *  offering it then would send it a second time). `null` otherwise. */
export function offeredSuggestion(
  row: Pick<SessionRow, 'prompt_suggestion'>,
  status: string | null | undefined,
  draft: string,
  sending = false,
): string | null {
  const text = row.prompt_suggestion?.trim();
  if (!text || sending || status !== 'idle' || draft.trim() !== '') return null;
  return text;
}
