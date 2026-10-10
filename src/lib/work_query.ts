// The Work view's search box as a small query language (search phase 2):
// `sprint:current epic:TK-10 type:bug login` narrows by sprint, epic and
// type and searches "login". A field token whose value is known becomes a
// filter (a chip in the strip) once it is finished — a space after it, or
// Enter — and leaves the box; what is left is the free-text `query`.
//
// Fields: sprint (current, none or a name, quoted when it has spaces),
// epic (a key), type, assignee (me or a name), status (backlog, todo,
// doing, review, blocked, done, open), is (mine, review, live, archived)
// and sort. Pure, so the bar and its tests share it.
import { fold } from './text_fold';
import { WORK_SORTS, type WorkStage, type WorkTreeFilters, type WorkTreeFacets } from './work_view';

export const QUERY_FIELDS = ['sprint', 'epic', 'type', 'assignee', 'status', 'is', 'sort'] as const;
export type QueryField = (typeof QUERY_FIELDS)[number];

/** What the values are checked and completed against. */
export interface WorkQueryVocab {
  facets?: WorkTreeFacets;
  /** Assignees of the tasks loaded. */
  people?: readonly string[];
}

const STATUS_WORDS: Record<string, Partial<WorkTreeFilters>> = {
  backlog: { stages: ['backlog'] },
  todo: { status: 'todo' },
  doing: { stages: ['in_progress'] },
  review: { stages: ['in_review'] },
  blocked: { stages: ['blocked'] },
  done: { stages: ['done'] },
  open: { status: 'open' },
};
const IS_WORDS: Record<string, Partial<WorkTreeFilters>> = {
  mine: { mine: true, assignee: undefined },
  review: { review: true },
  live: { has: 'active' },
  archived: { archived: true },
};
const KEY_RE = /^[A-Za-z][A-Za-z0-9_]{0,9}-\d{1,7}$/;
const ITEM_RE = /^item:\d+$/;

/** `field:value` or `field:"two words"` (an unclosed quote runs to the end). */
const TOKEN_RE = /(^|\s)([a-z]+):("([^"]*)("|$)|(\S*))/gi;

interface Token {
  field: QueryField;
  value: string;
  start: number;
  end: number;
  /** A space after it, or the closing quote: the person moved on. */
  finished: boolean;
}

function tokens(input: string): Token[] {
  const out: Token[] = [];
  for (const m of input.matchAll(TOKEN_RE)) {
    const field = m[2].toLowerCase();
    if (!(QUERY_FIELDS as readonly string[]).includes(field)) continue;
    const start = m.index! + m[1].length;
    const end = m.index! + m[0].length;
    const quoted = m[4] !== undefined;
    const value = quoted ? m[4] : (m[6] ?? '');
    const finished = quoted ? m[5] === '"' && /\s/.test(input.charAt(end)) : /\s/.test(input.charAt(end));
    out.push({ field: field as QueryField, value, start, end, finished });
  }
  return out;
}

/** The filters one field's value means, or null when the value is not one
 *  it knows. */
export function resolveToken(field: QueryField, raw: string, vocab: WorkQueryVocab): Partial<WorkTreeFilters> | null {
  const value = raw.trim();
  if (!value) return null;
  const v = fold(value);
  switch (field) {
    case 'sprint': {
      if (v === 'current' || v === 'none') return { iteration: v };
      const hit = (vocab.facets?.iterations ?? []).find((i) => fold(i.name) === v);
      return hit ? { iteration: hit.name } : null;
    }
    case 'epic': {
      const hit = (vocab.facets?.epics ?? []).find((e) => fold(e.key ?? '') === v || e.task_id === value);
      if (hit) return { epic: hit.key ?? hit.task_id };
      return KEY_RE.test(value) ? { epic: value.toUpperCase() } : ITEM_RE.test(value) ? { epic: value } : null;
    }
    case 'type': {
      const hit = (vocab.facets?.item_types ?? []).find((t) => fold(t) === v);
      return hit ? { item_type: hit } : null;
    }
    case 'assignee': {
      if (v === 'me') return { mine: true, assignee: undefined };
      const hit = (vocab.people ?? []).find((p) => fold(p) === v);
      return hit ? { mine: undefined, assignee: hit } : null;
    }
    case 'status':
      return STATUS_WORDS[v] ?? null;
    case 'is':
      return IS_WORDS[v] ?? null;
    case 'sort':
      return (WORK_SORTS as readonly string[]).includes(v) ? { sort: v as (typeof WORK_SORTS)[number] } : null;
  }
}

export interface ParsedWorkQuery {
  /** The filters the finished, known tokens set. */
  patch: Partial<WorkTreeFilters>;
  /** The box once those tokens are taken out. */
  rest: string;
  /** The free text: `rest` without any field token (an unfinished or
   *  unknown one would otherwise match nothing). */
  query: string;
}

/** Take the finished tokens out of `input`; with `all` (Enter), an
 *  unfinished last token too. */
export function parseWorkQuery(input: string, vocab: WorkQueryVocab, all = false): ParsedWorkQuery {
  const patch: Partial<WorkTreeFilters> = {};
  let rest = '';
  let at = 0;
  let stages: WorkStage[] | undefined;
  for (const t of tokens(input)) {
    if (!t.finished && !all) continue;
    const p = resolveToken(t.field, t.value, vocab);
    if (!p) continue;
    if (p.stages) stages = [...new Set([...(stages ?? []), ...p.stages])];
    Object.assign(patch, p);
    rest += input.slice(at, t.start);
    at = t.end;
  }
  if (stages) patch.stages = stages;
  rest = (rest + input.slice(at)).replace(/\s{2,}/g, ' ').replace(/^\s+/, '');
  let query = '';
  let q = 0;
  for (const t of tokens(rest)) {
    query += rest.slice(q, t.start);
    q = t.end;
  }
  query = (query + rest.slice(q)).replace(/\s+/g, ' ').trim();
  return { patch, rest, query };
}

/** Values a field offers, most useful first. */
function valuesOf(field: QueryField, vocab: WorkQueryVocab): { value: string; hint?: string }[] {
  switch (field) {
    case 'sprint':
      return [
        { value: 'current', hint: 'the active sprint' },
        ...(vocab.facets?.iterations ?? []).map((i) => ({ value: i.name, hint: `${i.active ? 'active · ' : ''}${i.count}` })),
        { value: 'none', hint: 'in no sprint' },
      ];
    case 'epic':
      return (vocab.facets?.epics ?? []).map((e) => ({ value: e.key ?? e.task_id, hint: e.title }));
    case 'type':
      return (vocab.facets?.item_types ?? []).map((t) => ({ value: t }));
    case 'assignee':
      return [{ value: 'me' }, ...(vocab.people ?? []).map((p) => ({ value: p }))];
    case 'status':
      return Object.keys(STATUS_WORDS).map((value) => ({ value }));
    case 'is':
      return Object.keys(IS_WORDS).map((value) => ({ value }));
    case 'sort':
      return WORK_SORTS.map((value) => ({ value }));
  }
}

const FIELD_HINTS: Record<QueryField, string> = {
  sprint: 'current, none or a sprint',
  epic: 'everything under an epic',
  type: 'Story, Bug …',
  assignee: 'me or a name',
  status: 'backlog, todo, doing, review, blocked, done',
  is: 'mine, review, live, archived',
  sort: 'activity, updated, key, title, due',
};

export interface WorkQuerySuggestion {
  /** The whole box once picked. */
  input: string;
  label: string;
  hint?: string;
}

/** What the last word being typed can become: a field name, or a value of
 *  the field it names. At most `max`. */
export function suggestWorkQuery(input: string, vocab: WorkQueryVocab, max = 8): WorkQuerySuggestion[] {
  // The last word: `field:"open quote…`, `field:value` or a bare word.
  const m = /(^|\s)(?:([a-z]+):(?:"([^"]*)|([^"\s]*))|([a-z]+))$/i.exec(input);
  if (!m) return [];
  const head = input.slice(0, m.index + m[1].length);
  if (m[5] !== undefined) {
    const word = m[5].toLowerCase();
    return QUERY_FIELDS.filter((f) => f.startsWith(word) && f !== word)
      .slice(0, max)
      .map((f) => ({ input: `${head}${f}:`, label: `${f}:`, hint: FIELD_HINTS[f] }));
  }
  const field = m[2].toLowerCase();
  if (!(QUERY_FIELDS as readonly string[]).includes(field)) return [];
  const typed = fold(m[3] ?? m[4] ?? '');
  return valuesOf(field as QueryField, vocab)
    .filter((v) => fold(v.value).includes(typed) && fold(v.value) !== typed)
    .sort((a, b) => Number(!fold(a.value).startsWith(typed)) - Number(!fold(b.value).startsWith(typed)))
    .slice(0, max)
    .map((v) => {
      const value = /\s/.test(v.value) ? `"${v.value}"` : v.value;
      return { input: `${head}${field}:${value} `, label: `${field}:${value}`, hint: v.hint };
    });
}
