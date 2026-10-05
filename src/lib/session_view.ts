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
  canAttach = true,
): SessionView {
  // No tmux pane (a background agent, an external Claude session) means no
  // PTY. This wins over the missing-transcript case below: a row that is
  // both has nothing else to offer, and ConversationPanel has its own empty
  // state for exactly that.
  if (noPane) return 'conversation';
  // A session reached through a GRANT (multi-user M1) has a pane, but not one
  // this client may attach: the terminal slot shows a read-only snapshot
  // instead (WatchView), which needs no transcript. So the missing-transcript
  // fallback below — which FORCES `terminal` — must not fire for one. It is
  // checked here, before that line and not after it, because the fallback is
  // the trap: it would hand a watcher the terminal slot as the one view they
  // did not choose, on exactly the rows (no `claude_session_id` yet) where
  // the pane snapshot is least use. The stored preference stands instead, so
  // both views stay reachable and a watcher who prefers the pane keeps it.
  if (!canAttach) return pref;
  // The session has not reported a Claude session id, so there is no
  // transcript to render.
  if (!hasClaudeId) return 'terminal';
  return pref;
}

export function otherSessionView(v: SessionView): SessionView {
  return v === 'conversation' ? 'terminal' : 'conversation';
}
