// Context help at a prompt line: a question about what the person is typing
// in a shell terminal or the composer, answered by Haiku (`work.help_model`)
// on the session's own host, with the line's history as context. Mirrors
// `fleet_core::service::context_help`; the desktop command is `context_help`
// (the same in both modes: the terminal's own ssh, like `pty_open`).
//
// The answer's `command` is a proposal for the prompt line. Nothing here
// sends or runs it: `ContextHelp.svelte` hands it to the host component,
// which puts it on the line for the person's own Enter.
import { invokeCmd, type Result } from './result';
import { SETTING_KEYS, type FleetSettings } from './fleet_settings';
import type { SlashCommand } from './conversation';

/** Where the person asked from (`context_help::Surface`). */
export type HelpSurface = 'shell' | 'composer';

/** Mirrors `context_help::HelpAnswer`. */
export interface HelpAnswer {
  answer: string;
  /** A proposal for the prompt line; absent when there is none. */
  command?: string;
  /** Who answered (`haiku`). */
  model: string;
  host_alias: string;
  /** History entries (or scrollback lines) the model was shown. */
  history_items: number;
  at: number;
}

/** What the session the line belongs to says about where the help runs. */
export interface HelpTarget {
  host_alias: string;
  /** The session's tmux name. */
  session_name: string;
  /** Its credential profile, when it runs under one. */
  profile?: string | null;
}

/** What the caller holds as context (`context_help::HelpRequest`). */
export interface HelpContext {
  surface: HelpSurface;
  /** The shell terminal asked from: its scrollback is read on the host. */
  terminal?: number | null;
  line: string;
  /** Earlier entries, oldest first. */
  history?: readonly string[];
  /** The commands the line accepts, one per entry. */
  commands?: readonly string[];
}

/** Entries of history sent, the newest (`HISTORY_MAX_ITEMS`). */
export const HELP_HISTORY_MAX = 40;
/** Commands sent (`COMMANDS_MAX_ITEMS`). */
export const HELP_COMMANDS_MAX = 80;
/** The models `work.help_model` takes (`SUMMARY_MODELS`). */
export const HELP_MODELS = ['haiku', 'sonnet', 'opus'] as const;

/** The model the setting names, Haiku when it names none we know. */
export function helpModel(settings: FleetSettings): string {
  const m = settings[SETTING_KEYS.workHelpModel];
  return (HELP_MODELS as readonly string[]).includes(m) ? m : 'haiku';
}

/** A slash command as the model reads it: `/plan #KEY or a goal — Plan …`. */
export function commandLine(c: SlashCommand): string {
  return [`/${c.name}`, c.usage ?? '', c.description ? `— ${c.description}` : '']
    .filter(Boolean)
    .join(' ');
}

/** The newest `max` non-blank entries, oldest first. */
export function recentHistory(history: readonly string[], max = HELP_HISTORY_MAX): string[] {
  return history.filter((h) => h.trim() !== '').slice(-max);
}

/** A screen's text as history lines, trailing blank lines dropped: what a
 *  shell sends when its scrollback cannot be read on the host. */
export function screenHistory(text: string): string[] {
  const lines = text.split('\n').map((l) => l.trimEnd());
  while (lines.length && lines[lines.length - 1] === '') lines.pop();
  return lines.slice(-HELP_HISTORY_MAX);
}

/** Ask. `question` may be empty when the line says enough. */
export function askContextHelp(
  target: HelpTarget,
  ctx: HelpContext,
  question: string,
  model: string,
): Promise<Result<HelpAnswer>> {
  return invokeCmd<HelpAnswer>('context_help', {
    args: {
      surface: ctx.surface,
      host_alias: target.host_alias,
      session_name: target.session_name,
      terminal: ctx.terminal ?? null,
      profile: target.profile || null,
      model,
      question: question.trim(),
      line: ctx.line,
      history: recentHistory(ctx.history ?? []),
      commands: (ctx.commands ?? []).slice(0, HELP_COMMANDS_MAX),
    },
  });
}

/** Why help cannot be asked, or `null`: there must be a question or a line. */
export function askBlocked(question: string, line: string): string | null {
  return question.trim() === '' && line.trim() === '' ? 'Ask a question, or type something to ask about' : null;
}
