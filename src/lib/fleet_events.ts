// Fleet's own prompts in Control's chat (Thread room, 2026-10-14; boards
// Main, A4Events and Events): a prompt that opens with `[fleet events]` (the
// operator feed's digest, `service/operator_feed.rs`) or `[fleet]` (a note
// after an automatic restart, `service/auto_restore.rs`) is fleet talking,
// not the person. The chat draws it as a folded "Fleet · N events" line that
// opens into the list, followed by Control's reaction as usual.
//
// The marks mirror `prompt_origin::FLEET_PROMPT_MARKS` in fleet-core, the
// one list every reader there uses; `fleet_events.test.ts` reads the Rust
// file and fails when the two disagree.

export const FLEET_PROMPT_MARKS = ['[fleet events]', '[fleet]'] as const;

/** Whether `prompt` opens with one of the marks (only at its head, as the
 *  Rust side reads it: a person quoting one mid-sentence keeps their words). */
export function isFleetPrompt(prompt: string | null | undefined): boolean {
  if (!prompt) return false;
  const t = prompt.trimStart();
  return FLEET_PROMPT_MARKS.some((m) => t.startsWith(m));
}

export interface FleetEvent {
  text: string;
  /** The session a digest line names (`… (#12) finished a turn`). */
  sessionId: number | null;
}

export interface FleetNote {
  /** `digest` from the operator feed, `note` from a restart. */
  kind: 'digest' | 'note';
  /** The text before the list (the digest's instructions to the agent). */
  header: string;
  events: FleetEvent[];
  /** Events the digest counted but did not list ("… and 3 more."). */
  more: number;
  /** listed + more. */
  total: number;
}

const MORE = /^…\s*and\s+(\d+)\s+more\.?$/;
const SESSION_REF = /\(#(\d+)\b/;

function event(text: string): FleetEvent {
  const m = SESSION_REF.exec(text);
  return { text, sessionId: m ? Number(m[1]) : null };
}

/** A fleet prompt read into its events, or null for anyone else's words. */
export function parseFleetPrompt(prompt: string | null | undefined): FleetNote | null {
  if (!isFleetPrompt(prompt)) return null;
  const t = prompt!.trimStart();
  if (t.startsWith('[fleet events]')) {
    const body = t.slice('[fleet events]'.length);
    const lines = body.split('\n');
    const header: string[] = [];
    const events: FleetEvent[] = [];
    let more = 0;
    for (const raw of lines) {
      const line = raw.trim();
      if (line.startsWith('- ')) {
        const item = line.slice(2).trim();
        const m = MORE.exec(item);
        if (m) more += Number(m[1]);
        else if (item) events.push(event(item));
      } else if (line && events.length === 0) {
        header.push(line);
      }
    }
    return { kind: 'digest', header: header.join(' '), events, more, total: events.length + more };
  }
  const text = t.slice('[fleet]'.length).trim();
  return { kind: 'note', header: '', events: text ? [event(text)] : [], more: 0, total: text ? 1 : 0 };
}

/** The folded line: "Fleet · 3 events", "Fleet · 1 event". */
export function fleetLine(n: FleetNote): string {
  const count = Math.max(n.total, 1);
  return `Fleet · ${count} ${count === 1 ? 'event' : 'events'}`;
}
