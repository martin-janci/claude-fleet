import { timeAgo } from './session_status';
import { invokeCmd, type Result } from './result';
import type { ClaudeStatus, StuckKind } from './sessions';

export type ConvItem = { kind: 'text'; text: string } | { kind: 'tool'; summary: string; error?: boolean };

export interface ConvTurn {
  prompt: string | null;
  at: string | null;
  /** When the reply last advanced; null for a prompt with no reply yet. */
  ended_at: string | null;
  items: ConvItem[];
}

export interface Conversation {
  turns: ConvTurn[];
  truncated: boolean;
}

/** Poll cadence for the Conversation tab while it is visible (spec §6). */
export const CONVERSATION_POLL_MS = 5_000;

/** Scroll is considered "pinned to bottom" within this many px (spec §6). */
export const PIN_THRESHOLD_PX = 40;

export function sessionConversation(sessionId: number): Promise<Result<Conversation>> {
  return invokeCmd<Conversation>('session_conversation', { args: { session_id: sessionId } });
}

/** Deep (JSON) equality — used to decide whether a poll result actually changed. */
export function sameConversation(a: Conversation | null, b: Conversation): boolean {
  if (a === null) return false;
  return JSON.stringify(a) === JSON.stringify(b);
}

export function isPinned(scrollTop: number, clientHeight: number, scrollHeight: number): boolean {
  return scrollHeight - scrollTop - clientHeight <= PIN_THRESHOLD_PX;
}

/**
 * The Conversation tab's empty-state message, or null when turns should be
 * rendered instead. Missing Claude session id takes priority over any error
 * code (there was nothing to fetch in the first place).
 */
export function emptyStateText(code: string | null, hasId: boolean): string | null {
  if (!hasId) return 'No Claude session id yet';
  if (code === 'E_NO_TRANSCRIPT') return 'No conversation yet';
  return null;
}

/** Pure relative-time formatter for an ISO timestamp against a reference clock. */
export function relativeTime(iso: string, nowMs: number): string {
  return timeAgo(new Date(iso).getTime() / 1000, nowMs);
}

/** One tool call line; `error` when its result came back as an error. */
export interface ToolLine {
  summary: string;
  error: boolean;
}

/** A reply item after folding: prose, or a run of consecutive tool calls. */
export type ConvGroup = { kind: 'text'; text: string } | { kind: 'tools'; tools: ToolLine[] };

/** Fold consecutive tool one-liners into one group; text items stay apart. */
export function groupItems(items: ConvItem[]): ConvGroup[] {
  const out: ConvGroup[] = [];
  for (const item of items) {
    if (item.kind === 'text') {
      out.push({ kind: 'text', text: item.text });
      continue;
    }
    const line: ToolLine = { summary: item.summary, error: item.error === true };
    const last = out[out.length - 1];
    if (last?.kind === 'tools') last.tools.push(line);
    else out.push({ kind: 'tools', tools: [line] });
  }
  return out;
}

/** The tool name of a one-liner such as `Bash(command=ls)`. */
export function toolName(summary: string): string {
  const paren = summary.indexOf('(');
  return paren > 0 ? summary.slice(0, paren) : summary;
}

/** `"7 tool calls · Bash, Read, Edit +2"` for a folded group, with
 *  `" · 1 failed"` appended when any call errored. */
export function toolGroupLabel(tools: ToolLine[]): string {
  const names = [...new Set(tools.map((t) => toolName(t.summary)))];
  const shown = names.slice(0, 3).join(', ');
  const more = names.length > 3 ? ` +${names.length - 3}` : '';
  const failed = tools.filter((t) => t.error).length;
  const suffix = failed > 0 ? ` · ${failed} failed` : '';
  return `${tools.length} tool calls · ${shown}${more}${suffix}`;
}

/** Prompts longer than this are clamped behind "Show more". */
export const PROMPT_CLAMP_LINES = 6;
const PROMPT_CLAMP_CHARS = 600;

export function isLongPrompt(prompt: string): boolean {
  return prompt.length > PROMPT_CLAMP_CHARS || prompt.split('\n').length > PROMPT_CLAMP_LINES;
}

/** A prompt sent from the composer, shown as its own turn until the
 *  transcript carries it. `seen` is how many turns already had this exact
 *  text when it was sent, so re-sending an earlier prompt ("continue") is
 *  not mistaken for the transcript having caught up. */
export interface PendingPrompt {
  prompt: string;
  at: string;
  seen: number;
}

/** True once a fetched conversation has more turns with the pending text
 *  than there were when it was sent. */
export function transcriptCarries(conv: Conversation, pending: PendingPrompt): boolean {
  return conv.turns.filter((t) => t.prompt === pending.prompt).length > pending.seen;
}

/** The note under the composer's Send button, or null when the session is
 *  ready to take a prompt. A stuck session may never read what is typed; a
 *  working one queues it until the turn ends. */
export function composerStatus(s: { claude_status: string | null; stuck_kind: string | null }): string | null {
  if (s.stuck_kind) return `Session is stuck (${s.stuck_kind}). The prompt may not be read until that is cleared.`;
  if (s.claude_status === 'working') return 'Claude is working. The prompt is queued until the current turn ends.';
  return null;
}

/** A Claude Code built-in slash command offered by the composer's menu. */
export interface SlashCommand {
  name: string;
  description: string;
  /** Completes with a trailing space so the user can type the argument. */
  args?: boolean;
}

/**
 * Claude Code's built-in commands (the REPL runs them when the line is sent
 * exactly as typed, verified against v2.1 through tmux send-keys). Kept
 * short and stable: this is a hint list, not a spec of the CLI.
 */
export const SLASH_COMMANDS: readonly SlashCommand[] = [
  { name: 'clear', description: 'Clear the conversation and start fresh' },
  { name: 'compact', description: 'Summarise the context to free space (optional focus text)', args: true },
  { name: 'context', description: 'Show what is using the context window' },
  { name: 'cost', description: 'Show token usage and cost for this session' },
  { name: 'usage', description: 'Show plan usage and rate limits' },
  { name: 'status', description: 'Show version, model, account and working directory' },
  { name: 'model', description: 'Switch the model', args: true },
  { name: 'effort', description: 'Set the reasoning effort level', args: true },
  { name: 'rc', description: 'Remote Control: drive this session from claude.ai' },
  { name: 'resume', description: 'Resume an earlier conversation' },
  { name: 'rewind', description: 'Rewind the conversation and files to a checkpoint' },
  { name: 'review', description: 'Review the current changes' },
  { name: 'memory', description: 'Edit the memory files loaded into context' },
  { name: 'config', description: 'Open settings' },
  { name: 'permissions', description: 'Manage tool permissions' },
  { name: 'mcp', description: 'Manage MCP servers' },
  { name: 'agents', description: 'Manage subagent definitions' },
  { name: 'hooks', description: 'Manage hooks' },
  { name: 'doctor', description: 'Check the installation' },
  { name: 'init', description: 'Write a CLAUDE.md for this project' },
  { name: 'export', description: 'Export the conversation to a file' },
  { name: 'help', description: 'List commands and shortcuts' },
  { name: 'exit', description: 'Quit Claude Code (the tmux session stays)' },
];

/**
 * Commands whose name starts with the draft's slash token. Empty unless the
 * whole draft is one token that begins with `/` (no spaces or newlines): once
 * an argument or a second line is being typed, the menu gets out of the way.
 */
export function matchSlashCommands(draft: string): SlashCommand[] {
  if (!draft.startsWith('/') || /\s/.test(draft)) return [];
  const prefix = draft.slice(1).toLowerCase();
  return SLASH_COMMANDS.filter((c) => c.name.startsWith(prefix));
}

/** The draft text that accepting a menu item yields. */
export function completeSlashCommand(c: SlashCommand): string {
  return c.args ? `/${c.name} ` : `/${c.name}`;
}

// ─── Live indicator ──────────────────────────────────────────────────────────

/** `session_activity`: what the pane shows right now, same vocabularies as
 *  the session row so it can be laid over it. */
export interface ActivityProbe {
  claude_status: ClaudeStatus | null;
  current_activity: string | null;
  stuck_kind: StuckKind | null;
  waiting_for: 'permission' | 'input' | null;
  spinner: string | null;
}

export function sessionActivity(sessionId: number): Promise<Result<ActivityProbe>> {
  return invokeCmd<ActivityProbe>('session_activity', { args: { session_id: sessionId } });
}

/** Probe cadence while the indicator is live (working / sent / blocked). */
export const ACTIVITY_POLL_MS = 2_000;
/** Transcript re-read cadence for a quiet session (nothing is changing). */
export const QUIET_POLL_MS = 15_000;

/** Statuses under which the transcript cannot be growing. `null` (not yet
 *  classified) is NOT quiet: an unknown session keeps the 5 s cadence. */
export function isQuietStatus(s: ClaudeStatus | null): boolean {
  return s === 'idle' || s === 'completed' || s === 'stopped' || s === 'failed';
}

/** Whether a poll tick should re-read the transcript. A quiet session is
 *  read only when its turn counter moved or the quiet cadence elapsed. */
export function shouldFetchTranscript(a: { quiet: boolean; sinceLastFetchMs: number; turnSeqChanged: boolean }): boolean {
  return !a.quiet || a.turnSeqChanged || a.sinceLastFetchMs >= QUIET_POLL_MS;
}

/** `Cooking… (3s · ↓ 306 tokens · esc to interrupt)` → `Cooking… 3s · ↓ 306 tokens`. */
export function spinnerLabel(spinner: string): string {
  const parts = spinner
    .replace(/[()]/g, ' ')
    .split('·')
    .map((p) => p.trim())
    .filter((p) => p.length > 0 && !/esc to interrupt/i.test(p));
  return parts.join(' · ').replace(/\s+/g, ' ').trim();
}

export type Indicator =
  | { kind: 'working'; label: string }
  | { kind: 'sent' }
  | { kind: 'blocked'; detail: string | null; waiting: 'permission' | 'input' | null }
  | null;

/**
 * What to show under the last turn. A stuck session yields nothing here
 * (the composer's status note and the Press Enter chip cover it). `pending`
 * is a prompt sent from the composer the transcript has not carried yet;
 * `optimistic` is the window after our own send in which no probe has yet
 * reported the session idle and its turn counter has not moved.
 */
export function indicatorFor(a: {
  status: ClaudeStatus | null;
  stuckKind: StuckKind | null;
  waitingFor: 'permission' | 'input' | null;
  activity: string | null;
  spinner: string | null;
  pending: boolean;
  optimistic: boolean;
}): Indicator {
  if (a.stuckKind) return null;
  if (a.status === 'blocked') return { kind: 'blocked', detail: a.activity, waiting: a.waitingFor };
  if (a.status === 'working') return { kind: 'working', label: a.spinner ? spinnerLabel(a.spinner) : 'Working…' };
  if (a.pending) return { kind: 'sent' };
  if (a.optimistic) return { kind: 'working', label: 'Working…' };
  return null;
}

/** `2m 14s` / `35s` between a turn's prompt and its latest reply entry;
 *  null when either end is missing, unparsable, or under a second. */
export function turnDuration(at: string | null, endedAt: string | null): string | null {
  if (!at || !endedAt) return null;
  const ms = new Date(endedAt).getTime() - new Date(at).getTime();
  if (!Number.isFinite(ms) || ms < 1000) return null;
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${secs % 60}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}

/**
 * Unsent composer text per session id, kept for the life of the app. The
 * panel unmounts on every tab switch, so without this a half-typed prompt
 * would vanish when the user glances at Files or the terminal.
 */
export const composerDrafts = new Map<number, string>();

export function rememberDraft(sessionId: number, text: string): void {
  if (text.length === 0) composerDrafts.delete(sessionId);
  else composerDrafts.set(sessionId, text);
}

/** Prompts plus reply items, for "N new" while the user is scrolled up. */
export function countItems(conv: Conversation | null): number {
  if (!conv) return 0;
  return conv.turns.reduce((n, t) => n + (t.prompt !== null ? 1 : 0) + t.items.length, 0);
}

/** How many items a poll added. Never negative: a re-read that trimmed
 *  older turns is not "new" content. */
export function newItemCount(prev: Conversation | null, next: Conversation): number {
  return Math.max(0, countItems(next) - countItems(prev));
}
