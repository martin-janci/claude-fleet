import { invokeCmd, type Result } from './result';

export type ConvItem = { kind: 'text'; text: string } | { kind: 'tool'; summary: string };

export interface ConvTurn {
  prompt: string | null;
  at: string | null;
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
  const ageSec = Math.floor((nowMs - new Date(iso).getTime()) / 1000);
  if (ageSec < 60) return 'just now';
  if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
  if (ageSec < 86400) return `${Math.floor(ageSec / 3600)}h ago`;
  const days = Math.floor(ageSec / 86400);
  return `${days}d ago`;
}

/** A reply item after folding: prose, or a run of consecutive tool calls. */
export type ConvGroup = { kind: 'text'; text: string } | { kind: 'tools'; tools: string[] };

/** Fold consecutive tool one-liners into one group; text items stay apart. */
export function groupItems(items: ConvItem[]): ConvGroup[] {
  const out: ConvGroup[] = [];
  for (const item of items) {
    if (item.kind === 'text') {
      out.push({ kind: 'text', text: item.text });
      continue;
    }
    const last = out[out.length - 1];
    if (last?.kind === 'tools') last.tools.push(item.summary);
    else out.push({ kind: 'tools', tools: [item.summary] });
  }
  return out;
}

/** The tool name of a one-liner such as `Bash(command=ls)`. */
export function toolName(summary: string): string {
  const paren = summary.indexOf('(');
  return paren > 0 ? summary.slice(0, paren) : summary;
}

/** `"7 tool calls · Bash, Read, Edit +2"` for a folded group. */
export function toolGroupLabel(tools: string[]): string {
  const names = [...new Set(tools.map(toolName))];
  const shown = names.slice(0, 3).join(', ');
  const more = names.length > 3 ? ` +${names.length - 3}` : '';
  return `${tools.length} tool calls · ${shown}${more}`;
}

/** Prompts longer than this are clamped behind "Show more". */
export const PROMPT_CLAMP_LINES = 6;
const PROMPT_CLAMP_CHARS = 600;

export function isLongPrompt(prompt: string): boolean {
  return prompt.length > PROMPT_CLAMP_CHARS || prompt.split('\n').length > PROMPT_CLAMP_LINES;
}
