import type { SessionRow } from './sessions';

/** The next prompt Claude Code proposes for `row`, when it is worth offering:
 *  the session is idle (a proposal belongs to the turn that just ended, and a
 *  new turn makes it stale before the hub clears it) and the person has not
 *  started typing over it. `null` otherwise. */
export function offeredSuggestion(
  row: Pick<SessionRow, 'prompt_suggestion'>,
  status: string | null | undefined,
  draft: string,
): string | null {
  const text = row.prompt_suggestion?.trim();
  if (!text || status !== 'idle' || draft.trim() !== '') return null;
  return text;
}
