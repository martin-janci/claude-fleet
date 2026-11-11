// What a session row says on its second line when it needs a person (HITL
// audit H-11, `docs/ux/2026-11-10-hitl-audit`): the reason, never the last
// prompt. The design system's SessionRow already promises "line two: what it
// waits on or did last, never the last prompt"; this is the shipped row's
// half of that promise.
//
// The lead is one of the status words' leads ("Waiting for you:",
// "Failed:", "Paused ·") and is the only part drawn in the state's colour;
// the text after it is plain. Pure, so every bucket's wording is tested
// without mounting a row.
import { blockedLine } from './attention_facts';
import type { AttentionFacts, TriageBucket } from './attention';
import type { SessionRow, StuckKind } from './sessions';

export interface RowReason {
  /** "Waiting for you:", "Failed:", "Paused ·", "Needs you ·". */
  lead: string;
  /** What it waits on or why it failed, in words. */
  text: string;
  /** The colour the lead and the row's dot take. A stuck row is filed
   *  action_required by the attention table but reads Failed, so the tone
   *  follows the words, not the state (dot and lead never disagree). */
  tone: 'waiting' | 'failed' | 'paused';
}

/** Claude Code's generic permission headlines: they name no action, so the
 *  tool call says it instead. */
export const GENERIC_QUESTION = /^do you want to (proceed|make this edit|create|run|allow|continue)\b/i;

const TOOL_VERB: Record<string, string> = {
  Bash: 'run',
  Edit: 'edit',
  MultiEdit: 'edit',
  Write: 'write',
  NotebookEdit: 'edit',
  Read: 'read',
  WebFetch: 'fetch',
  WebSearch: 'search the web for',
};

/** "Bash(git push origin main)" → "run git push origin main";
 *  "Edit(docs/pricing.md)" → "edit docs/pricing.md". Null when the detail
 *  is not a tool call. */
export function toolAsk(detail: string | null | undefined): string | null {
  const m = /^\s*([A-Za-z][\w:.-]*)\((.*)\)\s*$/s.exec(detail ?? '');
  if (!m) return null;
  const [, tool, arg] = m;
  const what = arg.trim().replace(/\s+/g, ' ');
  const verb = TOOL_VERB[tool];
  if (verb) return what ? `${verb} ${what}` : verb;
  const name = tool.replace(/^mcp__/, '').replace(/__/g, ' ');
  return what ? `use ${name}: ${what}` : `use ${name}`;
}

const STUCK_REASON: Record<StuckKind, string> = {
  auth_menu: 'stuck on the sign-in menu',
  reconnect: 'stuck reconnecting',
  trust_prompt: 'stuck on the trust prompt',
  oom: 'ran out of memory',
  press_enter: 'waiting for Enter in the terminal',
};

const HTTP_WORDS: Record<string, string> = {
  '400': 'the API refused the request (400)',
  '401': 'the API refused the login (401)',
  '403': 'the API refused access (403)',
  '413': 'the request was too large (413)',
  '429': 'the API rate-limited it (429)',
  '500': 'the API had an error (500)',
  '502': 'the API was unreachable (502)',
  '503': 'the API was unavailable (503)',
  '529': 'the API was overloaded (529)',
};

/** "API Error: 529 {"type":"overloaded_error"…}" → "the API was overloaded
 *  (529)". A raw payload never reaches the row. */
export function failureWords(activity: string | null | undefined): string {
  const a = activity ?? '';
  const code = /API Error:?\s*(\d{3})/i.exec(a)?.[1];
  if (code) return HTTP_WORDS[code] ?? `the API answered ${code}`;
  if (/rate.?limit/i.test(a)) return 'the API rate-limited it';
  if (/timed? ?out/i.test(a)) return 'the request timed out';
  return 'the turn stopped with an error';
}

function prNumber(url: string | null | undefined): string | null {
  const n = url?.match(/\/pull\/(\d+)/)?.[1];
  return n ? `#${n}` : null;
}

/**
 * The row's reason for `bucket`, or null when the row needs nobody and its
 * line keeps the state word with the agent, host and activity.
 *
 * `question` and `detail` are the dialog the row waits on (`pending_input`),
 * already gated by the caller on whether this client may answer.
 */
export function rowReason(
  bucket: TriageBucket,
  sess: Pick<
    SessionRow,
    | 'host_alias'
    | 'account_uuid'
    | 'claude_profile'
    | 'stuck_kind'
    | 'current_activity'
    | 'context_pct'
    | 'pr_url'
    | 'pending_form'
    | 'safe_kill_state'
  >,
  facts: AttentionFacts | undefined,
  accountName: (uuid: string) => string,
  dialog: { question: string | null; detail: string | null } | null,
): RowReason | null {
  switch (bucket) {
    case 'waiting': {
      if (sess.pending_form) return { lead: 'Waiting for you:', text: `a form, “${sess.pending_form.title}”`, tone: 'waiting' };
      const q = dialog?.question?.trim() || null;
      const ask = toolAsk(dialog?.detail);
      const text = q && !GENERIC_QUESTION.test(q) ? q : (ask ?? q ?? 'an answer in the terminal');
      return { lead: 'Waiting for you:', text, tone: 'waiting' };
    }
    case 'stuck':
      return { lead: 'Failed:', text: sess.stuck_kind ? STUCK_REASON[sess.stuck_kind] : 'stuck', tone: 'failed' };
    case 'stop_failed':
    case 'failed':
      if (sess.safe_kill_state === 'failed') {
        return { lead: 'Failed:', text: 'clean up could not commit or push its work', tone: 'failed' };
      }
      return { lead: 'Failed:', text: failureWords(sess.current_activity), tone: 'failed' };
    case 'ci_failing': {
      const pr = prNumber(sess.pr_url);
      return { lead: 'Failed:', text: pr ? `checks failing on PR ${pr}` : 'checks failing', tone: 'failed' };
    }
    case 'context_full': {
      const pct = sess.context_pct == null ? null : Math.round(sess.context_pct);
      return { lead: 'Needs you ·', text: `context ${pct == null ? 'nearly' : `${pct}%`} full, compact or clear it`, tone: 'waiting' };
    }
    case 'stale_working':
      return { lead: 'Needs you ·', text: 'no sign of work for a while, check the terminal', tone: 'waiting' };
    case 'host_down':
    case 'account_limit':
    case 'no_credentials': {
      const line = blockedLine(bucket, sess, facts, accountName);
      if (!line) return null;
      // blockedLine says "Needs you · blocked on …" / "Paused · weekly limit
      // on …": split its own lead off so only the lead is coloured.
      const m = /^(Needs you ·|Paused ·)\s*(.*)$/.exec(line);
      const tone = m?.[1] === 'Paused ·' ? 'paused' : 'waiting';
      return m ? { lead: m[1], text: m[2], tone } : { lead: '', text: line, tone };
    }
    default:
      return null;
  }
}
