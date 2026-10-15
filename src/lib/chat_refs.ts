// `#TASK` and `@host` / `@session` in Control's chat (Thread room,
// 2026-10-14; gap plan G3.9 named them in the composer): in a sent message
// a reference the fleet knows is a chip, and a click opens it in Control's
// right panel. Pure: the panel hands in what the fleet knows and draws the
// segments; nothing here sends or opens.

export type ChatRef =
  | { kind: 'session'; id: number; label: string }
  | { kind: 'host'; alias: string }
  | { kind: 'task'; key: string };

export type RefSegment = { text: string; ref?: ChatRef };

export interface RefIndex {
  /** A session by its tmux name or a one-word friendly name. */
  sessions: ReadonlyMap<string, { id: number; label: string }>;
  hosts: ReadonlySet<string>;
}

/** A task key: letters, a dash, a number (`PD-2592`, `TASK-219`, `FLT-12`). */
const TASK_KEY = /^[A-Za-z][A-Za-z0-9]*-\d+$/;
/** A `#` or `@` at the start or after whitespace or an opening bracket. */
const TOKEN = /(^|[\s([{"'])([#@])([A-Za-z0-9][\w./-]*)/g;
const TRAILING = /[.,:;!?)\]}'"/-]+$/;

/** The index from the live rows: tmux names, and friendly names with no
 *  space in them (a reference is one word). */
export function refIndex(
  rows: readonly { id: number; tmux_name: string; friendly_name?: string | null }[],
  hostAliases: readonly string[],
): RefIndex {
  const sessions = new Map<string, { id: number; label: string }>();
  for (const s of rows) {
    const label = s.friendly_name || s.tmux_name;
    sessions.set(s.tmux_name, { id: s.id, label });
    if (s.friendly_name && !/\s/.test(s.friendly_name) && !sessions.has(s.friendly_name)) sessions.set(s.friendly_name, { id: s.id, label });
  }
  return { sessions, hosts: new Set(hostAliases) };
}

function resolve(sigil: string, name: string, index: RefIndex): ChatRef | null {
  if (sigil === '#') return TASK_KEY.test(name) ? { kind: 'task', key: name } : null;
  if (index.hosts.has(name)) return { kind: 'host', alias: name };
  const s = index.sessions.get(name);
  return s ? { kind: 'session', id: s.id, label: s.label } : null;
}

/** `text` split into plain runs and references the index knows. Anything
 *  it does not know stays text, character for character. */
export function refSegments(text: string, index: RefIndex): RefSegment[] {
  const out: RefSegment[] = [];
  let last = 0;
  for (const m of text.matchAll(TOKEN)) {
    const lead = m[1];
    const sigil = m[2];
    let name = m[3];
    const tail = TRAILING.exec(name);
    if (tail) name = name.slice(0, name.length - tail[0].length);
    if (!name) continue;
    const ref = resolve(sigil, name, index);
    if (!ref) continue;
    const start = (m.index ?? 0) + lead.length;
    if (start > last) out.push({ text: text.slice(last, start) });
    out.push({ text: `${sigil}${name}`, ref });
    last = start + 1 + name.length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out.length ? out : [{ text }];
}

/** Whether a message holds any reference worth drawing. */
export function hasRefs(segments: readonly RefSegment[]): boolean {
  return segments.some((s) => s.ref);
}
