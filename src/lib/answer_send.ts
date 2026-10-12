// The one way a key reaches a session's question dialog: re-read the pane,
// and press only if the dialog it shows is still the one the caller drew.
// Shared by the answer card (AnswerPrompt) and the ⌘K "Approve" command
// (redesign step 3.9), so a second caller cannot forget the check.
//
// The row is written by the 20 s reconcile tick, so without the re-read a
// press on a stale view could approve a permission dialog the person never
// saw, or type a digit into the REPL of a session that has moved on.
import { get } from 'svelte/store';
import { sessionActivity } from './conversation';
import { hubActionBlocked, hubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { sessionActionBlocked } from './share';
import { answerDialog, type SessionRow } from './sessions';
import { answerFingerprint, pendingInputFor, type AnswerView } from './pending_input';

export type AnswerOutcome =
  | { ok: true }
  | { ok: false; stale: string }
  | { ok: false; error: string }
  /** The client may not write to the session (asked again after the read). */
  | { ok: false; blocked: string };

/** The pane as it is right now, or `null` when it shows no dialog.
 *
 *  The reading has to come from the PROBE: `pendingInputFor` falls back to
 *  the row when handed no probe, and the row is precisely the stale value
 *  this check exists to distrust. */
async function reread(session: SessionRow): Promise<{ view: AnswerView | null } | { error: string }> {
  const r = await sessionActivity(session.id);
  if (!r.ok) return { error: r.error.message };
  if (!r.value) return { view: null };
  const fresh = pendingInputFor({
    rowStatus: session.claude_status,
    rowStuck: session.stuck_kind,
    rowPending: session.pending_input,
    probe: r.value,
  });
  return { view: fresh?.live ? fresh : null };
}

/**
 * Press `key` in `session` if its pane still shows `view`'s dialog. The
 * write gate is asked here, after the read: the read is a round trip to the
 * host, and a revoke can land inside it.
 */
export async function sendAnswer(session: SessionRow, view: AnswerView, key: string): Promise<AnswerOutcome> {
  const fresh = await reread(session);
  // Not knowing what is on the pane is not the same as knowing it is
  // unchanged: without proof, a keystroke is a guess.
  if ('error' in fresh) return { ok: false, error: fresh.error };
  if (fresh.view === null) return { ok: false, stale: 'That dialog is gone — nothing was sent.' };
  if (answerFingerprint(fresh.view) !== answerFingerprint(view)) {
    return { ok: false, stale: 'The dialog changed — nothing was sent.' };
  }
  const why = hubActionBlocked('send_prompt', get(hubStatus), get(hubConnection)) ?? sessionActionBlocked(session, 'answer_dialog');
  if (why !== null) return { ok: false, blocked: why };
  const r = await answerDialog(session.host_alias, session.tmux_name, key, {
    kind: fresh.view.kind,
    question: fresh.view.question,
    options: fresh.view.options.map((o) => ({ n: o.n, label: o.label })),
    detail: fresh.view.detail,
    selected: fresh.view.options.find((o) => o.selected)?.n ?? null,
  });
  if (r.ok) return { ok: true };
  // The backend's own check, made right before its press: the dialog moved
  // between this read and the key.
  if (r.error.code === 'E_CONFLICT') return { ok: false, stale: r.error.message };
  return { ok: false, error: r.error.message };
}
