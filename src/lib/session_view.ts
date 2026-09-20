/**
 * Which of the Session tab's two sub-views to show.
 *
 * Conversation and Terminal are two views of one running session, so the
 * choice between them is a remembered preference rather than a side effect
 * of which mode flag happens to be false. Some rows can only offer one of
 * the two; those override the preference for that row *without* rewriting
 * it, so stepping off such a row puts you back where you were.
 */
export type SessionView = 'conversation' | 'terminal';

export function resolveSessionView(
  pref: SessionView,
  noPane: boolean,
  hasClaudeId: boolean,
): SessionView {
  // No tmux pane (a background agent, an external Claude session) means no
  // PTY. This wins over the missing-transcript case below: a row that is
  // both has nothing else to offer, and ConversationPanel has its own empty
  // state for exactly that.
  if (noPane) return 'conversation';
  // The session has not reported a Claude session id, so there is no
  // transcript to render.
  if (!hasClaudeId) return 'terminal';
  return pref;
}

export function otherSessionView(v: SessionView): SessionView {
  return v === 'conversation' ? 'terminal' : 'conversation';
}
