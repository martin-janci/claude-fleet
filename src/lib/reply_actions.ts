import type { ConvTurn } from './conversation';

/** Which of the five reply actions this turn offers, and with what anchor. */
export interface ReplyActionsView {
  canFork: boolean;
  /** Gates Rewind here AND Retry — Retry is a rewind plus a re-send. */
  canRewind: boolean;
  /** Keep strictly before this; `null` keeps the whole transcript. */
  forkAnchor: string | null;
  rewindAnchor: string | null;
}

/**
 * `truncated` is the conversation's own flag: index 0 is the conversation's
 * FIRST turn only when nothing older was dropped, and rewinding the first
 * turn would leave an empty conversation (that is `/clear`, under a
 * misleading name). `supported` is the backend gate — the hub version on a
 * phone, always true on a local desktop.
 */
export function replyActionsFor(
  turns: ConvTurn[],
  index: number,
  truncated: boolean,
  supported: boolean,
): ReplyActionsView {
  const none = { canFork: false, canRewind: false, forkAnchor: null, rewindAnchor: null };
  if (!supported) return none;

  // Fork keeps everything through THIS turn, so it anchors on the next later
  // prompt. Prompt-less turns (a compact boundary, a notification-only turn)
  // carry no anchor, so scan past them: keeping more history is safe, keeping
  // less would silently discard work.
  let forkAnchor: string | null = null;
  for (let j = index + 1; j < turns.length; j++) {
    const a = turns[j].prompt_uuid;
    if (a) {
      forkAnchor = a;
      break;
    }
  }

  const own = turns[index]?.prompt_uuid ?? null;
  const isConversationStart = index === 0 && !truncated;
  const canRewind = own !== null && !isConversationStart;

  return {
    canFork: true,
    canRewind,
    forkAnchor,
    rewindAnchor: canRewind ? own : null,
  };
}

/** A reply as a Markdown block quote, ready to precede the user's own words. */
export function quoteText(text: string): string {
  const body = text
    .split('\n')
    .map((l) => (l.length === 0 ? '>' : `> ${l}`))
    .join('\n');
  return `${body}\n\n`;
}
