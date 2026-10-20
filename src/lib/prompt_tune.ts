// The prompt tuner: a model rewrites the person's draft into a clearer
// prompt for the agent it goes to. Mirrors `fleet_core::service::prompt_tune`;
// the composer asks `session_prompt_tune` (routed, like context help), a
// routine's prompt field asks `routines { action: tune_prompt }`.
//
// The answer is a proposal. Nothing here sends it: `PromptTune.svelte`
// shows it beside the draft and hands an accepted rewrite to the host
// component, which puts it in the box for the person's own Send.
import { invokeCmd, type Result } from './result';
import { recentHistory, HELP_COMMANDS_MAX } from './context_help';
import type { RoutineInput } from './routines';

/** Mirrors `prompt_tune::TuneAnswer`. */
export interface TuneAnswer {
  /** The rewritten prompt. */
  prompt: string;
  /** A few lines on what changed. */
  notes: string;
  /** The rewrite is the draft as it was. */
  unchanged: boolean;
  /** The model ran past the limit and the rewrite was cut there (ends in
   *  `…`). Absent from a hub older than review w07. */
  cut?: boolean;
  /** Who rewrote it (`haiku`). */
  model: string;
  host_alias: string;
  at: number;
}

/** Quick asks the panel offers, beside a free one. */
export const TUNE_ASKS = ['Shorter', 'More specific', 'Add what done looks like', 'Split into steps'] as const;

/** Tune a draft at a session's composer. */
export function tuneComposerPrompt(
  sessionId: number,
  ctx: { draft: string; history?: readonly string[]; commands?: readonly string[] },
  ask: string,
): Promise<Result<TuneAnswer>> {
  return invokeCmd<TuneAnswer>('session_prompt_tune', {
    args: {
      session_id: sessionId,
      draft: ctx.draft,
      ask: ask.trim(),
      history: recentHistory(ctx.history ?? []),
      commands: (ctx.commands ?? []).slice(0, HELP_COMMANDS_MAX),
    },
  });
}

/** What a routine Tune says when the hub predates `tune_prompt`. */
export const TUNE_NEEDS_NEWER_HUB = 'The prompt tuner needs the hub updated first';

/** Tune a routine's prompt while it is being written: on its host and
 *  profile, saved or not. `from` tunes that text in place of the routine's
 *  prompt (the rewrite on screen, for Tune again). A hub from before the
 *  tuner refuses the action as unknown; that reads as needing an update. */
export async function tuneRoutinePrompt(routine: RoutineInput, ask: string, from?: string): Promise<Result<TuneAnswer>> {
  const r = await invokeCmd<TuneAnswer>('routines', {
    args: { action: 'tune_prompt', routine: from === undefined ? routine : { ...routine, prompt: from }, ask: ask.trim() },
  });
  if (!r.ok && r.error.message.includes('got "tune_prompt"')) {
    return { ok: false, error: { ...r.error, code: 'E_HUB_PROTOCOL', message: TUNE_NEEDS_NEWER_HUB } };
  }
  return r;
}

/** The longest draft the tuner reads whole; mirrors
 *  `prompt_tune::DRAFT_MAX_CHARS`. A longer one is refused rather than cut,
 *  since accepting the rewrite replaces the whole box. */
export const TUNE_DRAFT_MAX_CHARS = 12_000;

/** Why Tune cannot run, or `null`: it rewrites what is in the box, whole. */
export function tuneBlocked(draft: string): string | null {
  const text = draft.trim();
  if (text === '') return 'Write a draft first: Tune rewrites what is in the box';
  if ([...text].length > TUNE_DRAFT_MAX_CHARS) {
    return `The draft is too long to tune (at most ${TUNE_DRAFT_MAX_CHARS.toLocaleString('en-US')} characters): tune a shorter part of it`;
  }
  return null;
}
