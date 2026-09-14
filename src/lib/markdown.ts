// markdown.ts — a small, dependency-free Markdown parser for the Conversation
// tab.
//
// It produces a plain data tree that MarkdownView.svelte renders with ordinary
// Svelte elements. Nothing is ever turned into HTML: transcript text is
// untrusted (a tool result can carry markup) and this webview can call Tauri
// commands, so raw HTML stays literal text and a link keeps its target only
// when `safeHref` accepts it.
//
// Coverage is the subset Claude replies actually use: ATX headings, paragraphs
// (single newlines kept as line breaks), fenced code, bullet / ordered / task
// lists with nesting, blockquotes, GFM tables, horizontal rules, and inline
// code, strong, emphasis, strikethrough, links and autolinks. Setext headings,
// indented code blocks, reference links, images and footnotes are not handled.

export type Inline =
  | { t: 'text'; v: string }
  | { t: 'code'; v: string }
  | { t: 'strong'; c: Inline[] }
  | { t: 'em'; c: Inline[] }
  | { t: 'del'; c: Inline[] }
  | { t: 'link'; href: string | null; c: Inline[] }
  | { t: 'br' };

export type Align = 'left' | 'center' | 'right' | null;

export interface ListItem {
  /** `true`/`false` for a `[x]`/`[ ]` task item, `null` for a plain one. */
  task: boolean | null;
  c: Block[];
}

export type Block =
  | { t: 'heading'; level: number; c: Inline[] }
  | { t: 'para'; c: Inline[] }
  | { t: 'code'; lang: string; v: string }
  | { t: 'quote'; c: Block[] }
  | { t: 'list'; ordered: boolean; start: number; items: ListItem[] }
  | { t: 'table'; align: Align[]; head: Inline[][]; rows: Inline[][][] }
  | { t: 'hr' };

/** Deepest nesting the block parser recurses into; deeper content is text. */
const MAX_DEPTH = 12;

/** A link target the app may hand to the system browser, or `null`. */
export function safeHref(raw: string): string | null {
  const url = raw.trim();
  if (!/^(https?:\/\/[^\s/?#]+|mailto:[^\s@]+@[^\s@]+)/i.test(url)) return null;
  if (/[\s<>"]/.test(url)) return null;
  return url;
}

// ─── blocks ──────────────────────────────────────────────────────────────────

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])[ \t]+(.*)$/;
const TABLE_SEP = /^ *\|? *:?-+:? *(\| *:?-+:? *)*\|? *$/;

function isBlank(line: string): boolean {
  return line.trim() === '';
}

/** A line that starts a new block and therefore ends a paragraph. */
function startsBlock(line: string): boolean {
  return FENCE.test(line) || HEADING.test(line) || HR.test(line) || QUOTE.test(line) || LIST_ITEM.test(line);
}

export function parseMarkdown(src: string): Block[] {
  return parseBlocks(src.replace(/\r\n?/g, '\n').split('\n'), 0);
}

function parseBlocks(lines: string[], depth: number): Block[] {
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (isBlank(line)) {
      i++;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !isClosingFence(lines[i], marker)) body.push(lines[i++]);
      i++; // the closing fence (or past the end)
      out.push({ t: 'code', lang: fence[2] ?? '', v: body.join('\n') });
      continue;
    }

    // A rule outranks a list item: `- - -` and `***` are rules.
    if (HR.test(line)) {
      out.push({ t: 'hr' });
      i++;
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      out.push({ t: 'heading', level: heading[1].length, c: parseInline(heading[2] ?? '') });
      i++;
      continue;
    }

    if (depth < MAX_DEPTH && QUOTE.test(line)) {
      const body: string[] = [];
      while (i < lines.length && !isBlank(lines[i]) && QUOTE.test(lines[i])) {
        body.push(QUOTE.exec(lines[i])![1]);
        i++;
      }
      out.push({ t: 'quote', c: parseBlocks(body, depth + 1) });
      continue;
    }

    if (depth < MAX_DEPTH && LIST_ITEM.test(line)) {
      i = parseList(lines, i, depth, out);
      continue;
    }

    if (line.includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1]) && lines[i + 1].includes('-')) {
      i = parseTable(lines, i, out);
      continue;
    }

    const para: string[] = [line.trim()];
    i++;
    while (i < lines.length && !isBlank(lines[i]) && !startsBlock(lines[i])) {
      para.push(lines[i].trim());
      i++;
    }
    out.push({ t: 'para', c: parseInline(para.join('\n')) });
  }
  return out;
}

function isClosingFence(line: string, marker: string): boolean {
  const trimmed = line.trim();
  return trimmed.length >= marker.length && trimmed === marker[0].repeat(trimmed.length);
}

/** Parse the list starting at `start`; push it and return the next line index. */
function parseList(lines: string[], start: number, depth: number, out: Block[]): number {
  const first = LIST_ITEM.exec(lines[start])!;
  const indent = first[1].length;
  const ordered = /\d/.test(first[2]);
  const items: ListItem[] = [];
  let i = start;

  while (i < lines.length) {
    const m = LIST_ITEM.exec(lines[i]);
    if (!m || m[1].length !== indent || /\d/.test(m[2]) !== ordered) break;
    // Continuation lines are indented past the marker; nested items need at
    // least two more spaces than this item's own indent.
    const contentIndent = indent + m[2].length + 1;
    const body: string[] = [m[3]];
    i++;
    while (i < lines.length) {
      const next = lines[i];
      if (isBlank(next)) {
        // A blank line continues the item only when indented content follows.
        const after = lines[i + 1];
        if (after !== undefined && leadingSpaces(after) >= Math.min(contentIndent, indent + 2) && !isBlank(after)) {
          body.push('');
          i++;
          continue;
        }
        break;
      }
      const lead = leadingSpaces(next);
      if (lead >= indent + 2) {
        body.push(next.slice(Math.min(lead, contentIndent)));
        i++;
        continue;
      }
      // A lazy continuation: plain text directly under the item.
      if (!startsBlock(next) && lead <= indent && body[body.length - 1] !== '') {
        body.push(next.trim());
        i++;
        continue;
      }
      break;
    }

    let task: boolean | null = null;
    const taskMatch = /^\[([ xX])\][ \t]+/.exec(body[0]);
    if (taskMatch) {
      task = taskMatch[1] !== ' ';
      body[0] = body[0].slice(taskMatch[0].length);
    }
    items.push({ task, c: parseBlocks(body, depth + 1) });

    // A blank line between items keeps the list going.
    if (i < lines.length && isBlank(lines[i])) {
      const after = lines[i + 1];
      const am = after === undefined ? null : LIST_ITEM.exec(after);
      if (am && am[1].length === indent && /\d/.test(am[2]) === ordered) i++;
    }
  }

  out.push({ t: 'list', ordered, start: ordered ? parseInt(first[2], 10) : 1, items });
  return i;
}

function leadingSpaces(line: string): number {
  const m = /^[ \t]*/.exec(line)!;
  return m[0].replace(/\t/g, '    ').length;
}

function parseTable(lines: string[], start: number, out: Block[]): number {
  const head = splitRow(lines[start]);
  const align: Align[] = splitRow(lines[start + 1]).map((cell) => {
    const left = cell.startsWith(':');
    const right = cell.endsWith(':');
    return left && right ? 'center' : right ? 'right' : left ? 'left' : null;
  });
  const width = head.length;
  const rows: Inline[][][] = [];
  let i = start + 2;
  while (i < lines.length && !isBlank(lines[i]) && lines[i].includes('|')) {
    const cells = splitRow(lines[i]);
    rows.push(Array.from({ length: width }, (_, k) => (k < cells.length ? parseInline(cells[k]) : [])));
    i++;
  }
  out.push({
    t: 'table',
    align: Array.from({ length: width }, (_, k) => align[k] ?? null),
    head: head.map((cell) => parseInline(cell)),
    rows,
  });
  return i;
}

/** Split a table row on unescaped pipes, dropping the outer ones. */
function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = '';
  for (let k = 0; k < s.length; k++) {
    if (s[k] === '\\' && s[k + 1] === '|') {
      cur += '|';
      k++;
    } else if (s[k] === '|') {
      cells.push(cur.trim());
      cur = '';
    } else {
      cur += s[k];
    }
  }
  cells.push(cur.trim());
  return cells;
}

// ─── inlines ─────────────────────────────────────────────────────────────────

const ESCAPABLE = /[\\`*_{}[\]()#+\-.!~|<>]/;

// Scan limits. A delimiter that finds no partner within these many chars is
// literal text; they keep pathological input (thousands of unmatched `*`,
// `[`) from turning the parse quadratic.
const EMPHASIS_WINDOW = 1_000;
const LINK_LABEL_WINDOW = 500;
const LINK_URL_WINDOW = 2_000;
const BARE_URL = /^https?:\/\/[^\s<>"]+/;
const ANGLE_URL = /^<((?:https?:\/\/|mailto:)[^\s<>]+)>/;

/** Does a delimiter run at `i` in `s` open emphasis (not flanked by space)? */
function canOpen(s: string, i: number, len: number, ch: string): boolean {
  const next = s[i + len];
  if (next === undefined || /\s/.test(next)) return false;
  // `snake_case`: an underscore inside a word never opens emphasis.
  if (ch === '_' && i > 0 && /[\p{L}\p{N}]/u.test(s[i - 1])) return false;
  return true;
}

/** Find the closing run for `delim` after `from`, or -1. */
function findClose(s: string, delim: string, from: number): number {
  const ch = delim[0];
  const limit = Math.min(s.length, from + EMPHASIS_WINDOW);
  let j = from;
  while (j < limit) {
    if (s[j] === '\\') {
      j += 2;
      continue;
    }
    if (s[j] === '`') {
      // Skip a code span so a delimiter inside it never closes.
      const run = runLength(s, j, '`');
      const end = s.indexOf('`'.repeat(run), j + run);
      if (end < 0 || end >= limit) return -1;
      j = end + run;
      continue;
    }
    if (s.startsWith(delim, j)) {
      const run = runLength(s, j, ch);
      const prev = s[j - 1];
      const after = s[j + run];
      const flankOk = prev !== undefined && !/\s/.test(prev);
      const wordOk = ch !== '_' || after === undefined || !/[\p{L}\p{N}]/u.test(after);
      if (flankOk && wordOk) {
        if (run === delim.length) return j;
        // A longer run: close a single delimiter at its end (`*a **b***`).
        if (delim.length === 1 && run === 3) return j + 2;
      }
      j += run;
      continue;
    }
    j++;
  }
  return -1;
}

function runLength(s: string, i: number, ch: string): number {
  let n = 0;
  while (s[i + n] === ch) n++;
  return n;
}

export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let buf = '';
  const flush = () => {
    if (buf) out.push({ t: 'text', v: buf });
    buf = '';
  };

  // Backtick run lengths with no closing run anywhere after the current point.
  const unmatchedTicks = new Set<number>();
  let i = 0;
  while (i < src.length) {
    const c = src[i];

    if (c === '\\' && i + 1 < src.length && ESCAPABLE.test(src[i + 1])) {
      buf += src[i + 1];
      i += 2;
      continue;
    }

    if (c === '\n') {
      flush();
      out.push({ t: 'br' });
      i++;
      continue;
    }

    if (c === '`') {
      const run = runLength(src, i, '`');
      const ticks = '`'.repeat(run);
      // A run with no partner anywhere later stays unmatched for the rest of
      // the string, so the search is done at most once per run length.
      let close = unmatchedTicks.has(run) ? -1 : src.indexOf(ticks, i + run);
      while (close >= 0 && runLength(src, close, '`') !== run) {
        close = src.indexOf(ticks, close + runLength(src, close, '`'));
      }
      if (close < 0) unmatchedTicks.add(run);
      if (close >= 0) {
        flush();
        let v = src.slice(i + run, close);
        if (v.length > 2 && v.startsWith(' ') && v.endsWith(' ')) v = v.slice(1, -1);
        out.push({ t: 'code', v });
        i = close + run;
      } else {
        buf += ticks;
        i += run;
      }
      continue;
    }

    if (c === '[') {
      const link = matchLink(src, i);
      if (link) {
        flush();
        out.push({ t: 'link', href: safeHref(link.url), c: parseInline(link.label) });
        i = link.end;
        continue;
      }
    }

    if (c === '<') {
      const m = ANGLE_URL.exec(src.slice(i));
      if (m) {
        flush();
        out.push({ t: 'link', href: safeHref(m[1]), c: [{ t: 'text', v: m[1] }] });
        i += m[0].length;
        continue;
      }
    }

    if (c === 'h' && (i === 0 || /[\s(]/.test(src[i - 1]))) {
      const m = BARE_URL.exec(src.slice(i));
      if (m) {
        const url = m[0].replace(/[.,;:!?'")\]]+$/, '');
        if (safeHref(url)) {
          flush();
          out.push({ t: 'link', href: url, c: [{ t: 'text', v: url }] });
          i += url.length;
          continue;
        }
      }
    }

    if (c === '*' || c === '_' || c === '~') {
      const run = runLength(src, i, c);
      const node = matchEmphasis(src, i, run, c);
      if (node) {
        flush();
        out.push(node.inline);
        i = node.end;
        continue;
      }
      buf += c.repeat(run);
      i += run;
      continue;
    }

    buf += c;
    i++;
  }
  flush();
  return out;
}

function matchEmphasis(
  s: string,
  i: number,
  run: number,
  ch: string,
): { inline: Inline; end: number } | null {
  const tries: { delim: string; kind: 'strong' | 'em' | 'del' }[] =
    ch === '~'
      ? run === 2
        ? [{ delim: '~~', kind: 'del' }]
        : []
      : run >= 2
        ? [
            { delim: ch + ch, kind: 'strong' },
            { delim: ch, kind: 'em' },
          ]
        : [{ delim: ch, kind: 'em' }];
  for (const { delim, kind } of tries) {
    if (!canOpen(s, i, delim.length, ch)) continue;
    const close = findClose(s, delim, i + delim.length + 1);
    if (close < 0) continue;
    const inner = s.slice(i + delim.length, close);
    return { inline: { t: kind, c: parseInline(inner) }, end: close + delim.length };
  }
  return null;
}

/** `[label](url "title")` starting at `i`, with balanced brackets and parens. */
function matchLink(s: string, i: number): { label: string; url: string; end: number } | null {
  let depth = 0;
  let j = i;
  const labelLimit = Math.min(s.length, i + LINK_LABEL_WINDOW);
  for (; j < labelLimit; j++) {
    if (s[j] === '\\') {
      j++;
      continue;
    }
    if (s[j] === '[') depth++;
    else if (s[j] === ']' && --depth === 0) break;
    else if (s[j] === '\n') return null;
  }
  if (j >= labelLimit || s[j + 1] !== '(') return null;
  const label = s.slice(i + 1, j);
  let k = j + 2;
  let parens = 1;
  const urlLimit = Math.min(s.length, k + LINK_URL_WINDOW);
  for (; k < urlLimit; k++) {
    if (s[k] === '(') parens++;
    else if (s[k] === ')' && --parens === 0) break;
    else if (s[k] === '\n') return null;
  }
  if (k >= urlLimit) return null;
  const target = s.slice(j + 2, k).trim();
  const url = target.split(/\s+/)[0] ?? '';
  return { label, url: url.replace(/^<|>$/g, ''), end: k + 1 };
}

// ─── code fences ─────────────────────────────────────────────────────────────

const FENCE_ALIASES: Record<string, string> = {
  javascript: 'clike',
  js: 'clike',
  jsx: 'clike',
  typescript: 'clike',
  ts: 'clike',
  tsx: 'clike',
  rust: 'clike',
  rs: 'clike',
  go: 'clike',
  golang: 'clike',
  java: 'clike',
  kotlin: 'clike',
  kt: 'clike',
  swift: 'clike',
  c: 'clike',
  cpp: 'clike',
  'c++': 'clike',
  csharp: 'clike',
  cs: 'clike',
  php: 'clike',
  scala: 'clike',
  dart: 'clike',
  svelte: 'html',
  vue: 'html',
  html: 'html',
  xml: 'html',
  python: 'python',
  py: 'python',
  ruby: 'python',
  rb: 'python',
  bash: 'shell',
  sh: 'shell',
  zsh: 'shell',
  shell: 'shell',
  console: 'shell',
  fish: 'shell',
  dockerfile: 'shell',
  css: 'css',
  scss: 'css',
  json: 'json',
  jsonc: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'yaml',
  md: 'md',
  markdown: 'md',
};

/** The `highlight()` language for a fence info string, or `''` for plain. */
export function fenceLang(info: string): string {
  return FENCE_ALIASES[info.trim().toLowerCase()] ?? '';
}
