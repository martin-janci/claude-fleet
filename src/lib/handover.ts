// An agent-written work handover (work graph M9.3) as the Conversation view
// draws it. Fleet asks an idle session for the hand-off between two
// nonce-tagged marker lines (crates/fleet-core/src/service/work/agent_handover.rs
// `build_prompt`); the reply carries it as
//
//   WORK_HANDOVER_BEGIN_<nonce>
//   …free text…
//   WORK_HANDOVER_END_<nonce>
//
// The text is Claude's prose, so nothing here is a format it promised to
// keep. The parser only recognises the section headings the prompt asks for
// ("What the work is", "What is done", "Where things are", …) at the start of
// a line; anything it cannot place stays in the section above it, and a
// hand-off with no heading it knows is drawn as one Markdown body.
// Everything is untrusted transcript text: it is rendered as Markdown, never
// as HTML.

export const HANDOVER_BEGIN = 'WORK_HANDOVER_BEGIN_';
export const HANDOVER_END = 'WORK_HANDOVER_END_';
/** Mirrors `agent_handover::HANDOVER_MAX_CHARS`. */
export const HANDOVER_MAX_CHARS = 6_000;

const NONCE_RE = /^[A-Za-z0-9]+$/;

export type HandoverKind = 'work' | 'done' | 'left' | 'decisions' | 'where' | 'blockers' | 'next' | 'gotchas';

/** The order the card draws sections in, whatever order they were written. */
export const HANDOVER_ORDER: HandoverKind[] = ['work', 'decisions', 'done', 'left', 'where', 'blockers', 'next', 'gotchas'];

export interface HandoverSection {
  kind: HandoverKind;
  /** The heading as written ("What blocked closing it"). */
  heading: string;
  /** The section's text, Markdown. */
  body: string;
  /** Its list items when the body is a list; empty otherwise. */
  items: string[];
}

export interface Handover {
  /** The work key the first line names (`TASK-223`), when it names one. */
  key: string | null;
  /** The quoted title on that line, or the rest of it. */
  title: string | null;
  /** Recognised sections in the card's order; one per kind. */
  sections: HandoverSection[];
  /** Text before the first heading, other than the title line. */
  intro: string;
  /** Number of words in the hand-off. */
  words: number;
}

/** A marker line, read the way `rich_blocks.markerOf` reads the done marker:
 *  the REPL's chrome and any wrapping punctuation are ignored. */
export function handoverMarker(line: string): { end: boolean; nonce: string } | null {
  const bare = line
    .trim()
    .replace(/^[^A-Za-z0-9_]+/, '')
    .replace(/[^A-Za-z0-9_]+$/, '');
  for (const [prefix, end] of [
    [HANDOVER_BEGIN, false],
    [HANDOVER_END, true],
  ] as const) {
    if (bare.startsWith(prefix)) {
      const nonce = bare.slice(prefix.length);
      return NONCE_RE.test(nonce) ? { end, nonce } : null;
    }
  }
  return null;
}

// Each pattern is a whole heading, so a sentence that merely starts with
// "Completed" or "Next" never splits a section.
const KINDS: [HandoverKind, RegExp][] = [
  ['work', /^(what the work is|the work|work|the task|task|goal|context|background|summary)$/i],
  ['done', /^(what('s| is| was| has been) (done|completed)|done( so far)?|completed|progress( so far)?)$/i],
  ['left', /^(what('s| is) left( to do)?|left|remaining|still to do|to ?do|what remains|outstanding)$/i],
  ['decisions', /^(decisions?( made)?( and why)?|what the user decided|what (was|we) decided|decided)$/i],
  ['where', /^(where things are|where (everything|it) is|locations?)$/i],
  ['blockers', /^(blockers?|blocked|what blocked\b.*|what('s| is) blocking\b.*|open (problems|issues))$/i],
  ['next', /^(next( steps?)?|next up|what to do next|how to (finish|continue)\b.*)$/i],
  ['gotchas', /^(gotchas?|pitfalls?|caveats?|watch out|(anything|what|things) that (will|might|could) trip\b.*)$/i],
];

/** A heading line: optional `#`s or bold, a short phrase, then `.` or `:` and
 *  optionally the section's first sentence. Only a phrase that names a known
 *  section counts, so ordinary prose ending in a colon stays prose. */
const HEADING_RE = /^(?:#{1,6}\s+)?(?:\*\*|__)?([A-Z][^.:!?*_`\n]{1,48}?)(?:\*\*|__)?\s*[.:](?:\*\*|__)?(?:\s+(.*))?$/;

function headingOf(line: string): { kind: HandoverKind; heading: string; rest: string } | null {
  const t = line.trim();
  // A list item is content, even when it reads "Branch: x".
  if (/^([*+-]|\d+[.)])\s/.test(t)) return null;
  const m = HEADING_RE.exec(t) ?? /^#{1,6}\s+(.{1,48})$/.exec(t);
  if (!m) return null;
  const heading = m[1].trim();
  // "Where things are (branch, files, commands)" is "Where things are".
  const bare = heading.replace(/\s*\([^)]*\)$/, '');
  for (const [kind, re] of KINDS) {
    if (re.test(bare)) return { kind, heading: bare, rest: (m[2] ?? '').trim() };
  }
  return null;
}

const ITEM_RE = /^\s*(?:[*+-]|\d+[.)])\s+(.*)$/;

/** The list items of a body whose every non-blank line is an item or a
 *  continuation of one; empty when the body is prose. */
export function listItems(body: string): string[] {
  const items: string[] = [];
  let sawProse = false;
  for (const line of body.split('\n')) {
    if (line.trim() === '') continue;
    const m = ITEM_RE.exec(line);
    if (m) items.push(m[1].trim());
    else if (items.length && /^\s+/.test(line)) items[items.length - 1] += ' ' + line.trim();
    else sawProse = true;
  }
  return sawProse ? [] : items;
}

const KEY_RE = /\b([A-Z][A-Z0-9]{1,9}-\d+|[\w.-]+\/[\w.-]+#\d+)\b/;

function titleLine(line: string): { key: string | null; title: string | null } | null {
  const t = line.trim().replace(/^#{1,6}\s+/, '').replace(/^\*\*|\*\*$/g, '');
  if (!/\bhand-?over\b/i.test(t)) return null;
  const key = KEY_RE.exec(t)?.[1] ?? null;
  const quoted = /["“]([^"”]{1,120})["”]/.exec(t)?.[1] ?? null;
  const after = t.includes(':') ? t.slice(t.indexOf(':') + 1).trim() : '';
  return { key, title: quoted ?? (after || null) };
}

/** PURE: the hand-off text (between the markers) read into sections. */
export function parseHandover(text: string): Handover {
  const body = [...text.replace(/\r\n?/g, '\n').trim()].slice(0, HANDOVER_MAX_CHARS).join('');
  const lines = body.split('\n');
  let key: string | null = null;
  let title: string | null = null;
  let start = 0;
  while (start < lines.length && lines[start].trim() === '') start++;
  const first = start < lines.length ? titleLine(lines[start]) : null;
  if (first) {
    ({ key, title } = first);
    start++;
  }

  const intro: string[] = [];
  const found = new Map<HandoverKind, { heading: string; lines: string[] }>();
  let cur: string[] = intro;
  for (const line of lines.slice(start)) {
    const h = headingOf(line);
    if (h) {
      // A kind written twice joins its first section.
      const s = found.get(h.kind) ?? { heading: h.heading, lines: [] };
      found.set(h.kind, s);
      cur = s.lines;
      if (h.rest) cur.push(h.rest);
      continue;
    }
    cur.push(line);
  }

  const sections: HandoverSection[] = [];
  for (const kind of HANDOVER_ORDER) {
    const s = found.get(kind);
    if (!s) continue;
    const text = s.lines.join('\n').trim();
    sections.push({ kind, heading: s.heading, body: text, items: listItems(text) });
  }
  return {
    key,
    title,
    sections,
    intro: intro.join('\n').trim(),
    words: body.split(/\s+/).filter(Boolean).length,
  };
}
