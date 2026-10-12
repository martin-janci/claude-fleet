import { sessionActivity, splitMarker, type ActivityProbe, type ConvTurn } from './conversation';
import { promptFirstLine } from './tasks';
import type { Result } from './result';
import { finalizeBranchSlug } from './branch-slug';
import type { SessionRow } from './sessions';

/** Which of the five reply actions this turn offers, and with what anchor. */
export interface ReplyActionsView {
  canFork: boolean;
  /** Gates Rewind here. */
  canRewind: boolean;
  /**
   * Gates Retry. A rewind plus a re-send, so on top of `canRewind` it needs a
   * prompt there is something to re-send: `transcript.rs` sets `prompt_uuid`
   * on every anchored turn but leaves `prompt` null when the text is empty
   * (an image-only prompt), and the dialog promises "the same prompt is then
   * sent again". Without this, Retry would degrade to a bare rewind after the
   * user approved a re-send.
   */
  canRetry: boolean;
  /**
   * Why Retry is NOT offered on a turn that could otherwise be rewound, or
   * `null`. The row shows Retry disabled with this as its tooltip rather than
   * dropping it silently: the prompt on screen is not what would be re-sent.
   */
  retryUnavailable: string | null;
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
  const none = {
    canFork: false,
    canRewind: false,
    canRetry: false,
    retryUnavailable: null,
    forkAnchor: null,
    rewindAnchor: null,
  };
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

  const retryUnavailable = !canRewind ? null : retryBlockedReason(turns[index]);

  return {
    canFork: true,
    canRewind,
    canRetry: canRewind && retryUnavailable === null,
    retryUnavailable,
    forkAnchor,
    rewindAnchor: canRewind ? own : null,
  };
}

/**
 * Why this turn's prompt cannot be re-sent as "the same prompt", or `null`
 * when it can. `prompt` is only the prompt's text: an image-only prompt has
 * none, a prompt with an image lost the image, and a prompt too long for the
 * read budget lost its tail (`prompt_partial`, set by `transcript.rs`).
 * Sending any of those would send something other than what the user asked
 * to retry.
 */
export function retryBlockedReason(turn: ConvTurn | undefined): string | null {
  if (!turn || turn.prompt === null) {
    return 'Retry is unavailable: this prompt had no text to send again (an image only). Rewind puts the conversation back; attach the image again yourself.';
  }
  if (turn.prompt_partial) {
    return 'Retry is unavailable: the prompt shown is not the whole prompt (it was cut to fit, or held an image). Rewind here, then send it yourself.';
  }
  return null;
}

/**
 * How long Retry waits for the respawned REPL before giving up on the
 * re-send, and how often it asks. `send_prompt` is a blind tmux
 * load-buffer/paste-buffer/Enter with no readiness check, and a rewind has
 * respawned the pane milliseconds earlier — node boot plus transcript load
 * takes seconds on a remote host, so an immediate paste lands in a pty not
 * yet in raw mode. `spawn_review` waits for the same thing before its seed
 * send (`service/sessions/review.rs:96`).
 */
export const RETRY_READY_TIMEOUT_MS = 30_000;
export const RETRY_READY_POLL_MS = 750;

/**
 * Poll a session until its REPL is back at its input prompt, BOUNDED. `true`
 * means it answered ready within the bound; `false` means it did not, and the
 * caller must NOT send — the prompt goes back into the composer with an
 * error instead, so it is never silently lost.
 *
 * "Ready" is stricter than `isQuietStatus`, because the pane was respawned
 * moments ago and the probe reads a capture that includes scrollback: a
 * reading can still be the OLD REPL's footer, or a blank pane that node has
 * not drawn into yet. So readiness is `idle` — the REPL's own input chrome,
 * the one status `pane_intel` derives from the prompt being on screen; the
 * other quiet values (`completed` / `stopped` / `failed`) mean Claude is not
 * taking input at all — with no dialog and no spinner, on TWO consecutive
 * probes. One stale frame cannot pass that; a REPL that really is up passes
 * it one poll later.
 *
 * A probe that cannot answer (a refused read, a hub too old) is not evidence
 * of readiness, so it counts as "not yet" and resets the streak; the bound
 * still applies.
 *
 * The dependencies are injected so this is testable without timers.
 */
export async function waitForReplQuiet(
  sessionId: number,
  deps: {
    probe?: (id: number) => Promise<Result<ActivityProbe>>;
    sleep?: (ms: number) => Promise<void>;
    timeoutMs?: number;
    pollMs?: number;
  } = {},
): Promise<boolean> {
  const probe = deps.probe ?? sessionActivity;
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const timeoutMs = deps.timeoutMs ?? RETRY_READY_TIMEOUT_MS;
  const pollMs = deps.pollMs ?? RETRY_READY_POLL_MS;
  let streak = 0;
  for (let waited = 0; ; waited += pollMs) {
    await sleep(pollMs);
    const r = await probe(sessionId);
    streak = r.ok && isReplReady(r.value) ? streak + 1 : 0;
    if (streak >= READY_STREAK) return true;
    if (waited + pollMs >= timeoutMs) return false;
  }
}

/** Consecutive ready probes `waitForReplQuiet` needs. */
export const READY_STREAK = 2;

/** One probe showing the REPL at its input prompt: `idle`, nothing on
 *  screen asking a question, nothing generating. */
export function isReplReady(p: ActivityProbe): boolean {
  return p.claude_status === 'idle' && !p.pending_input && !p.spinner;
}

/** A reply as a Markdown block quote, ready to precede the user's own words. */
export function quoteText(text: string): string {
  const body = text
    .split('\n')
    .map((l) => (l.length === 0 ? '>' : `> ${l}`))
    .join('\n');
  return `${body}\n\n`;
}

/** `fork-of-<branch>`, slugified for use as a new worktree's name — the
 *  branch/worktree's name when this session has one, else its tmux name,
 *  which is what a `main`-checkout session forks from. The Fork sheet's
 *  prefill, from a reply and from the session's own Fork… alike. */
export function suggestedForkName(s: Pick<SessionRow, 'friendly_name' | 'tmux_name'>): string {
  const base = s.friendly_name?.trim() || s.tmux_name;
  return finalizeBranchSlug(`fork-of-${base}`) || 'fork';
}

/** One turn the session's Rewind… sheet offers. */
export interface RewindChoice {
  /** The turn's own `prompt_uuid`: the rewind's anchor. */
  anchor: string;
  /** The prompt as the composer would take it back (no hub marker). */
  prompt: string | null;
  /** Its first line, for the list. */
  line: string;
  at: string | null;
}

/** The turns a rewind can go back to before, newest first: exactly the
 *  turns whose reply row offers Rewind (`replyActionsFor`). */
export function rewindChoices(turns: ConvTurn[], truncated: boolean): RewindChoice[] {
  const out: RewindChoice[] = [];
  turns.forEach((t, i) => {
    const v = replyActionsFor(turns, i, truncated, true);
    if (!v.canRewind || !v.rewindAnchor) return;
    const prompt = t.prompt == null ? null : splitMarker(t.prompt).text;
    out.push({ anchor: v.rewindAnchor, prompt, line: promptFirstLine(prompt, 90) || '(no text)', at: t.at });
  });
  return out.reverse();
}
