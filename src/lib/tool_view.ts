// PURE helpers for a tool call's expanded detail in the Conversation view
// (`ToolLine.svelte`): an interleaved line diff for an edit, a Read result
// split into line numbers and code, a TodoWrite list, a Grep / Glob file
// list. Everything here takes the strings `session_tool_detail` returns and
// never throws on a shape it does not expect — the caller falls back to the
// raw text.

export interface DiffRow {
  kind: 'ctx' | 'add' | 'del' | 'gap';
  /** The line's text; empty for a `gap`. */
  text: string;
  /** 1-based line number in the old / new text (relative to the snippet:
   *  an edit does not say where in the file it sits). */
  oldNo: number | null;
  newNo: number | null;
  /** A `gap`: how many unchanged lines it stands for. */
  hidden?: number;
}

export interface LineDiff {
  rows: DiffRow[];
  added: number;
  removed: number;
}

/** Unchanged lines kept on each side of a change. */
export const DIFF_CONTEXT = 3;

/** Above this many LCS cells (old lines × new lines) the diff falls back to
 *  trimming the common ends and showing the middle as removed-then-added:
 *  the table is O(n·m) memory and an 8 000-char edit can be a lot of lines. */
const LCS_MAX_CELLS = 250_000;

function splitLines(s: string): string[] {
  return s === '' ? [] : s.split('\n');
}

type Op = { kind: 'ctx' | 'add' | 'del'; text: string };

/** A line-level diff of `old` → `next`, interleaved the way a unified diff
 *  reads (each removed block next to what replaced it), with runs of more
 *  than `2 × DIFF_CONTEXT` unchanged lines folded into a `gap` row. */
export function lineDiff(old: string, next: string): LineDiff {
  const a = splitLines(old);
  const b = splitLines(next);
  let pre = 0;
  while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre++;
  let suf = 0;
  while (suf < a.length - pre && suf < b.length - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++;

  const ops: Op[] = [];
  for (const text of a.slice(0, pre)) ops.push({ kind: 'ctx', text });
  const midA = a.slice(pre, a.length - suf);
  const midB = b.slice(pre, b.length - suf);
  if (midA.length * midB.length > LCS_MAX_CELLS) {
    for (const text of midA) ops.push({ kind: 'del', text });
    for (const text of midB) ops.push({ kind: 'add', text });
  } else {
    ops.push(...lcsOps(midA, midB));
  }
  for (const text of a.slice(a.length - suf)) ops.push({ kind: 'ctx', text });

  let oldNo = 0;
  let newNo = 0;
  let added = 0;
  let removed = 0;
  const numbered: DiffRow[] = ops.map((op) => {
    if (op.kind === 'ctx') return { kind: 'ctx', text: op.text, oldNo: ++oldNo, newNo: ++newNo };
    if (op.kind === 'del') {
      removed++;
      return { kind: 'del', text: op.text, oldNo: ++oldNo, newNo: null };
    }
    added++;
    return { kind: 'add', text: op.text, oldNo: null, newNo: ++newNo };
  });
  return { rows: foldContext(numbered), added, removed };
}

/** The LCS edit script of two line lists: deletions before additions within
 *  each changed block, so a replaced line reads old-then-new. */
function lcsOps(a: string[], b: string[]): Op[] {
  const n = a.length;
  const m = b.length;
  // lcs[i][j] = LCS length of a[i..] and b[j..], flattened.
  const w = m + 1;
  const lcs = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * w + j] = a[i] === b[j] ? lcs[(i + 1) * w + j + 1] + 1 : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
    }
  }
  const out: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      out.push({ kind: 'ctx', text: a[i] });
      i++;
      j++;
    } else if (j >= m || (i < n && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1])) {
      out.push({ kind: 'del', text: a[i++] });
    } else {
      out.push({ kind: 'add', text: b[j++] });
    }
  }
  return out;
}

/** Folds each run of unchanged lines longer than two contexts' worth into a
 *  `gap`, keeping `DIFF_CONTEXT` lines next to every change (and none at the
 *  outer edges beyond that). A diff with no change at all is left whole. */
function foldContext(rows: DiffRow[]): DiffRow[] {
  if (!rows.some((r) => r.kind !== 'ctx')) return rows;
  const out: DiffRow[] = [];
  let k = 0;
  while (k < rows.length) {
    if (rows[k].kind !== 'ctx') {
      out.push(rows[k++]);
      continue;
    }
    let end = k;
    while (end < rows.length && rows[end].kind === 'ctx') end++;
    const run = rows.slice(k, end);
    const keepHead = k === 0 ? 0 : DIFF_CONTEXT;
    const keepTail = end === rows.length ? 0 : DIFF_CONTEXT;
    if (run.length > keepHead + keepTail + 1) {
      out.push(...run.slice(0, keepHead));
      out.push({ kind: 'gap', text: '', oldNo: null, newNo: null, hidden: run.length - keepHead - keepTail });
      out.push(...run.slice(run.length - keepTail));
    } else {
      out.push(...run);
    }
    k = end;
  }
  return out;
}

/** `/a/b/c.ts` → { dir: '/a/b/', base: 'c.ts' }. */
export function splitPath(path: string): { dir: string; base: string } {
  const cut = path.lastIndexOf('/');
  return cut < 0 ? { dir: '', base: path } : { dir: path.slice(0, cut + 1), base: path.slice(cut + 1) };
}

export interface NumberedLine {
  no: number;
  text: string;
}

/** A Read result in Claude Code's `cat -n` shape (`     1\tcode`, or
 *  `     1→code` in newer builds) as numbered lines; `null` when any
 *  non-empty line is not in that shape, so the caller shows the raw text. */
export function parseNumbered(result: string): NumberedLine[] | null {
  const lines = result.split('\n');
  while (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  if (lines.length === 0) return null;
  const out: NumberedLine[] = [];
  for (const line of lines) {
    const m = /^\s*(\d+)(?:\t|→)(.*)$/.exec(line);
    if (!m) return null;
    out.push({ no: Number(m[1]), text: m[2] });
  }
  return out;
}

export type TodoStatus = 'completed' | 'in_progress' | 'pending';

export interface Todo {
  content: string;
  status: TodoStatus;
}

/** TodoWrite's `todos` from its pretty-JSON input; `null` when the input is
 *  not that shape (or was cut at the 8 000-char cap and no longer parses). */
export function parseTodos(input: string): Todo[] | null {
  let v: unknown;
  try {
    v = JSON.parse(input);
  } catch {
    return null;
  }
  const todos = (v as { todos?: unknown } | null)?.todos;
  if (!Array.isArray(todos)) return null;
  const out: Todo[] = [];
  for (const t of todos) {
    const content = (t as { content?: unknown })?.content;
    if (typeof content !== 'string') return null;
    const s = (t as { status?: unknown }).status;
    out.push({ content, status: s === 'completed' || s === 'in_progress' ? s : 'pending' });
  }
  return out;
}

/** One string field of a tool's pretty-JSON input, or null. */
export function inputField(input: string, key: string): string | null {
  try {
    const v = (JSON.parse(input) as Record<string, unknown> | null)?.[key];
    return typeof v === 'string' ? v : null;
  } catch {
    return null;
  }
}

/** A Grep (files mode) / Glob result as its list of paths: one per line,
 *  without the `Found N files` header. `null` when a line does not look
 *  like a path (Grep's content mode), so the caller shows the raw text. */
export function parseFileList(result: string): string[] | null {
  const lines = result.split('\n').map((l) => l.trim()).filter((l) => l !== '');
  const body = lines.length > 0 && /^Found \d+ files?/.test(lines[0]) ? lines.slice(1) : lines;
  if (body.length === 0) return null;
  if (body.some((l) => /\s/.test(l) && !l.includes('/'))) return null;
  if (body.some((l) => /:\d+:/.test(l))) return null;
  return body;
}

/** Which detail layout a call gets. */
export type DetailKind = 'edit' | 'bash' | 'read' | 'files' | 'todos' | 'raw';

export function detailKind(name: string, hasEdit: boolean, hasCommand: boolean): DetailKind {
  if (hasEdit) return 'edit';
  if (hasCommand) return 'bash';
  if (name === 'Read') return 'read';
  if (name === 'Grep' || name === 'Glob') return 'files';
  if (name === 'TodoWrite') return 'todos';
  return 'raw';
}
