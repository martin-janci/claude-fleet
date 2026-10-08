// The copy lint (Orbit Fleet redesign step 7.8): the manual's content rules,
// checked on the Svelte source by `copy_lint.test.ts`.
//
// - A status reads as one of six words (`STATUS_WORDS`), optionally followed
//   by " · " and the reason: "Failed · auth menu". No glyph prefixes.
// - A label that opens a dialog ends with "…", and no label ends in "...".
//
// What "opens a dialog" means here: a dialog is a component whose markup
// renders a `<Modal` (or `<DialogSheet`, the one dialog pattern that wraps
// it). It is shown by an `{#if flag}` around it, either where it
// is used (`{#if reviewOpen}<ReviewDialog …>`) or inside itself
// (`{#if $open}<Modal …>`). A button opens it when its `onclick` makes that
// flag truthy: inline, through a function in the same file, or through an
// exported function of a `.ts` module that sets a store flag
// (`openSettingsAt`). The scan reads source text, not an AST: it knows the
// shapes this codebase uses and nothing cleverer, and `copy_lint.test.ts`
// pins each shape on a fixture so a blind spot shows up as a failing case.
import { STATUS_WORDS } from './kit/status';

export interface CopyFinding {
  rule: 'status-word' | 'dialog-ellipsis' | 'three-dots';
  file: string;
  line: number;
  text: string;
}

/** The classes that mark a status surface. */
export const STATUS_CLASSES = [
  'claude-chip',
  'stuck-chip',
  'inactive-chip',
  'status-word',
  'state-chip',
  'sub-status',
  'bg-status',
  'bg-item-status',
];

/** What a status surface's first computed text may come from. */
export const STATUS_SOURCES = /\b(?:claudeStatusLabel|stuckStatus|sessionStatusWord|backgroundStatusWord|STATE_WORD)\b/;

/** A status label's head (before " · ") is one of the six words. */
export function isStatusCopy(text: string): boolean {
  const head = text.split(' · ')[0].trim();
  return (STATUS_WORDS as readonly string[]).includes(head);
}

function lineOf(src: string, at: number): number {
  let n = 1;
  for (let i = 0; i < at; i++) if (src.charCodeAt(i) === 10) n++;
  return n;
}

/** Blank `<script>`, `<style>` and comments, keeping offsets and lines. */
export function markupOf(src: string): string {
  const blank = (s: string) => s.replace(/[^\n]/g, ' ');
  return src
    .replace(/<script[^>]*>[\s\S]*?<\/script>/g, blank)
    .replace(/<style[^>]*>[\s\S]*?<\/style>/g, blank)
    .replace(/<!--[\s\S]*?-->/g, blank);
}

function scriptOf(src: string): string {
  return [...src.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n');
}

interface Flag {
  name: string;
  store: boolean;
}

/** The `{#if}` expressions open at `at`, outermost first. Inside an
 *  `{:else if X}` the expression is X; inside an `{:else}` it is '' (no
 *  flag: what shows there shows when the flag is false). */
function openIfs(markup: string, at: number): string[] {
  const stack: string[] = [];
  const re = /\{#if\s+([^}]*)\}|\{:else\s+if\s+([^}]*)\}|\{:else\}|\{\/if\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(markup)) && m.index < at) {
    if (m[1] !== undefined) stack.push(m[1]);
    else if (m[2] !== undefined) stack[stack.length - 1] = m[2];
    else if (m[0] === '{:else}') stack[stack.length - 1] = '';
    else stack.pop();
  }
  return stack;
}

function flagOf(expr: string): Flag | null {
  const id = expr.match(/^[\s(]*(\$?)([A-Za-z_]\w*)/);
  return id ? { name: id[2], store: id[1] === '$' } : null;
}

/** The flag of the innermost `{#if}` open at `at`, or null. */
function gateAt(markup: string, at: number): Flag | null {
  const expr = openIfs(markup, at).at(-1);
  return expr === undefined ? null : flagOf(expr);
}

export interface Tag {
  /** Offset of `<`. */
  at: number;
  attrs: string;
  /** The markup between the open and close tags. */
  body: string;
}

/** Every `<name …>…</name>`, reading `{…}` attribute values (where `=>` lives)
 *  with brace depth so a `>` inside one does not end the tag. */
export function tagsOf(markup: string, name: string): Tag[] {
  const out: Tag[] = [];
  const open = new RegExp(`<${name}\\b`, 'g');
  let m: RegExpExecArray | null;
  while ((m = open.exec(markup))) {
    let i = m.index + name.length + 1;
    let depth = 0;
    let quote = '';
    for (; i < markup.length; i++) {
      const c = markup[i];
      if (quote) {
        if (c === quote) quote = '';
      } else if (c === '{') depth++;
      else if (c === '}') depth--;
      else if (depth === 0 && (c === '"' || c === "'")) quote = c;
      else if (depth > 0 && (c === '"' || c === "'" || c === '`')) quote = c;
      else if (c === '>' && depth === 0) break;
    }
    const attrs = markup.slice(m.index + name.length + 1, i);
    let body = '';
    if (!attrs.trimEnd().endsWith('/')) {
      const close = new RegExp(`</${name}\\s*>`, 'g');
      close.lastIndex = i;
      const c = close.exec(markup);
      body = c ? markup.slice(i + 1, c.index) : '';
    }
    out.push({ at: m.index, attrs, body });
  }
  return out;
}

/** `{…}` expressions removed, brace-matched past strings and template
 *  literals (`{n ? `(${n})` : ''}` is one expression). */
function stripExprs(t: string): string {
  let out = '';
  let depth = 0;
  let quote = '';
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (depth === 0) {
      if (c === '{') depth = 1;
      else out += c;
      continue;
    }
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = '';
    } else if (c === '"' || c === "'" || c === '`') quote = c;
    else if (c === '{') depth++;
    else if (c === '}' && --depth === 0) out += ' ';
  }
  return out;
}

/** The words a button shows at rest: of an `{#if}…{:else}…{/if}` the last
 *  branch (`{#if busy}Starting…{:else}Start review{/if}` is "Start review"),
 *  an `{#if}` without an `{:else}` dropped (an optional suffix), `{…}`
 *  expressions and tags removed. A nested element goes whole when the rest
 *  still has words (a muted count after the label is not the label). */
export function literalText(body: string): string {
  let s = body;
  const innerIf = /\{#if[^}]*\}((?:(?!\{#if)[\s\S])*?)\{\/if\}/;
  for (let m = s.match(innerIf); m; m = s.match(innerIf)) {
    const parts = m[1].split(/\{:else[^}]*\}/);
    s = s.replace(m[0], parts.length > 1 ? parts[parts.length - 1] : ' ');
  }
  const words = (t: string) =>
    stripExprs(t)
      .replace(/<[^>]*>/g, ' ')
      .replace(/&amp;/g, '&')
      .replace(/\s+/g, ' ')
      .trim();
  let bare = s;
  for (let prev = ''; prev !== bare; ) {
    prev = bare;
    bare = bare.replace(/<([a-z][\w-]*)\b[^>]*>[^<]*<\/\1\s*>/g, ' ');
  }
  return /\p{L}/u.test(words(bare)) ? words(bare) : words(s);
}

/** `attr={…}`'s code, brace-matched. */
export function attrCode(attrs: string, attr: string): string {
  const at = attrs.search(new RegExp(`\\b${attr}=\\{`));
  if (at < 0) return '';
  let i = attrs.indexOf('{', at);
  const from = i + 1;
  let depth = 0;
  for (; i < attrs.length; i++) {
    if (attrs[i] === '{') depth++;
    else if (attrs[i] === '}' && --depth === 0) break;
  }
  return attrs.slice(from, i);
}

/** Function bodies by name: `function f(…) {…}` and `const f = (…) => …`,
 *  brace-matched (an arrow without braces runs to the end of its line). */
export function functionsOf(script: string): Map<string, string> {
  const out = new Map<string, string>();
  const re =
    /(?:function\s+([A-Za-z_]\w*)\s*(?:<[^>]*>)?\s*\(|(?:const|let)\s+([A-Za-z_]\w*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_]\w*)\s*(?::[^=;]*?)?=>)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(script))) {
    const name = m[1] ?? m[2];
    let i = m.index + m[0].length;
    // A `function`'s parameter list, then its body.
    if (m[1]) {
      let d = 1;
      for (; i < script.length && d > 0; i++) {
        if (script[i] === '(') d++;
        else if (script[i] === ')') d--;
      }
    }
    while (i < script.length && /\s/.test(script[i])) i++;
    const brace = script.indexOf('{', i);
    const eol = script.indexOf('\n', i);
    if (m[2] && (brace < 0 || (eol >= 0 && eol < brace))) {
      out.set(name, script.slice(i, eol < 0 ? script.length : eol));
      continue;
    }
    let d = 0;
    let j = brace;
    for (; j < script.length; j++) {
      if (script[j] === '{') d++;
      else if (script[j] === '}' && --d === 0) break;
    }
    out.set(name, script.slice(brace, j + 1));
  }
  return out;
}

const FALSY = String.raw`\s*(?:(?:false|null|undefined|0)(?![\w$])|''|""|\[\s*\]|$)`;

/** Whether `re` matches `code` unconditionally: at the top level of a
 *  function body (`{…}`) or of an inline handler, on a line that is not an
 *  `if`, an `else` or a ternary. A button whose handler opens a dialog only
 *  sometimes (a confirm for one value of a setting) is the action, not a
 *  dialog opener, and keeps its plain label. */
function always(code: string, re: RegExp): boolean {
  const body = code.trim();
  const base = body.startsWith('{') ? 1 : 0;
  const g = new RegExp(re.source, re.flags.includes('g') ? re.flags : re.flags + 'g');
  for (const m of body.matchAll(g)) {
    let depth = 0;
    for (let i = 0; i < m.index; i++) {
      if (body[i] === '{') depth++;
      else if (body[i] === '}') depth--;
    }
    if (depth !== base) continue;
    const line = body.slice(body.lastIndexOf('\n', m.index) + 1, m.index);
    if (/^\s*(?:if\b|else\b|\}\s*else\b)|\?|&&|\|\|/.test(line)) continue;
    return true;
  }
  return false;
}

/** `code` makes `flag` truthy, unconditionally: `flag = x` / `flag.set(x)` /
 *  `flag.update(…)` for an `x` that is not a falsy literal. */
export function setsFlag(code: string, flag: Flag): boolean {
  const f = flag.name;
  if (flag.store)
    return (
      always(code, new RegExp(String.raw`(?<![\w.$])\$?${f}\.(?:set\((?!${FALSY})|update\()`)) ||
      always(code, new RegExp(String.raw`(?<![\w.])\$${f}\s*=(?![=>])(?!${FALSY})`))
    );
  return always(code, new RegExp(String.raw`(?<![\w.$])${f}\s*=(?![=>])(?!${FALSY})`));
}

function calls(code: string, fn: string): boolean {
  return always(code, new RegExp(String.raw`(?<![\w.$])${fn}\b(?!\s*=)`));
}

export interface DialogIndex {
  /** Components that are a dialog. */
  dialogs: Set<string>;
  /** Store flags, app-wide (`settingsOpen`, `shareSheetFor`). */
  storeFlags: Set<string>;
  /** Exported `.ts` functions that set a store flag (`openSettingsAt`). */
  openers: Set<string>;
  /** Self-gated dialogs `selfGated` does not name a store for. */
  unnamed: string[];
}

const nameOf = (path: string) => path.replace(/^.*\//, '').replace(/\.svelte$/, '');

/** The components a dialog draws its frame with: `Modal`, and `DialogSheet`
 *  (step 5.10), which wraps it. */
const FRAMES = ['Modal', 'DialogSheet'];

/** Index the dialogs, their store flags and the module functions that open
 *  them. `selfGated` names the store a self-gated dialog's local flag follows
 *  (`ShareSheet`'s `id` is `$shareSheetFor`), for the ones whose `{#if}` reads
 *  a local. */
/** The frames a dialog renders: `Modal`, or `DialogSheet` (step 5.10's one
 *  dialog pattern, itself a `Modal`). */
const FRAMES = ['Modal', 'DialogSheet'];

export function indexDialogs(
  svelte: Record<string, string>,
  ts: Record<string, string>,
  selfGated: Record<string, string | null> = {},
): DialogIndex {
  const dialogs = new Set<string>(FRAMES);
  const storeFlags = new Set<string>();
  const unnamed: string[] = [];
  for (const [path, src] of Object.entries(svelte)) {
    const markup = markupOf(src);
    const at = markup.search(/<(?:Modal|DialogSheet)[\s>]/);
    if (at < 0) continue;
    const ifs = openIfs(markup, at);
    // A dialog: its Modal is top-level, or under one `{#if}` its markup
    // starts with or `selfGated` names (it shows itself). A pane that opens a Modal of its own
    // (SessionDetails) is not one; its flags are collected per file.
    if (ifs.length === 0 || (ifs.length === 1 && (/^\s*\{#if/.test(markup) || nameOf(path) in selfGated))) {
      dialogs.add(nameOf(path));
      if (ifs.length) {
        // Its own `{#if}` reads a local or an alias, so the store it follows
        // is named by the caller; an unnamed one is reported, not guessed.
        const name = nameOf(path);
        if (!(name in selfGated)) unnamed.push(name);
        else if (selfGated[name]) storeFlags.add(selfGated[name] as string);
      }
    }
  }
  for (const [path, src] of Object.entries(svelte)) {
    const markup = markupOf(src);
    for (const m of markup.matchAll(/<([A-Z]\w*)[\s/>]/g)) {
      if (!dialogs.has(m[1]) || (FRAMES.includes(m[1]) && dialogs.has(nameOf(path)))) continue;
      const flag = gateAt(markup, m.index);
      if (flag?.store) storeFlags.add(flag.name);
    }
  }
  const openers = new Set<string>();
  const fns = new Map<string, string>();
  for (const src of Object.values(ts)) for (const [n, b] of functionsOf(src)) if (/\bexport\s/.test(src)) fns.set(n, b);
  // To a fixed point: a function calling an opener opens too.
  for (let grew = true; grew; ) {
    grew = false;
    for (const [n, body] of fns) {
      if (openers.has(n)) continue;
      const opens =
        [...storeFlags].some((s) => setsFlag(body, { name: s, store: true })) ||
        [...openers].some((o) => calls(body, o));
      if (opens) {
        openers.add(n);
        grew = true;
      }
    }
  }
  return { dialogs, storeFlags, openers, unnamed };
}

export interface DialogButton {
  line: number;
  text: string;
  attrs: string;
  opens: boolean;
  /** What opens it: the flag, or the function called. */
  why: string;
}

/** Every `<button>` in a file, with whether its `onclick` opens a dialog. */
export function buttonsOf(src: string, index: DialogIndex, name = ''): DialogButton[] {
  const markup = markupOf(src);
  const local: Flag[] = [];
  for (const m of markup.matchAll(/<([A-Z]\w*)[\s/>]/g)) {
    // A dialog's own Modal is gated by the dialog's own flag, not a button's.
    if (!index.dialogs.has(m[1]) || (FRAMES.includes(m[1]) && index.dialogs.has(name))) continue;
    const flag = gateAt(markup, m.index);
    if (flag && !flag.store && !local.some((f) => f.name === flag.name)) local.push(flag);
  }
  const flags: Flag[] = [...local, ...[...index.storeFlags].map((name) => ({ name, store: true }))];
  const fns = functionsOf(scriptOf(src));
  const opening = new Set<string>();
  const opensCode = (code: string): string =>
    flags.find((f) => setsFlag(code, f))?.name ??
    [...index.openers, ...opening].find((o) => calls(code, o)) ??
    '';
  for (let grew = true; grew; ) {
    grew = false;
    for (const [n, body] of fns) {
      if (!opening.has(n) && opensCode(body)) {
        opening.add(n);
        grew = true;
      }
    }
  }
  return tagsOf(markup, 'button').map((t) => {
    const click = attrCode(t.attrs, 'onclick');
    const why = click === '' ? '' : opensCode(click);
    return { line: lineOf(markup, t.at), text: literalText(t.body), attrs: t.attrs, opens: why !== '', why };
  });
}

/** Lint one `.svelte` file. */
export function lintSvelte(file: string, src: string, index: DialogIndex): CopyFinding[] {
  const out: CopyFinding[] = [];
  for (const b of buttonsOf(src, index, nameOf(file))) {
    if (/\.\.\.$/.test(b.text)) out.push({ rule: 'three-dots', file, line: b.line, text: b.text });
    // A `.link` button sits inside a sentence ("It still waits in Settings ›
    // Review."): it reads as a link to a place, and an ellipsis would break
    // the sentence.
    if (/\bclass="[^"]*\blink\b/.test(b.attrs)) continue;
    if (b.opens && /\p{L}/u.test(b.text) && !b.text.endsWith('…')) out.push({ rule: 'dialog-ellipsis', file, line: b.line, text: b.text });
  }
  const markup = markupOf(src);
  // Status surfaces: an element with a status class says one of the six
  // words in its literal text, and draws any computed text from the status
  // vocabulary (`STATUS_SOURCES`), never from a raw state.
  for (const t of tagsOf(markup, 'span')) {
    const cls = t.attrs.match(/\bclass="([^"]*)"/)?.[1] ?? '';
    if (!cls.split(/\s+/).some((c) => STATUS_CLASSES.includes(c))) continue;
    const line = lineOf(markup, t.at);
    const body = t.body.replace(/<[A-Z][\s\S]*?\/>/g, ' ');
    const text = literalText(body);
    const exprs = [...body.matchAll(/\{(?![#:/@])([^}]*)\}/g)].map((m) => m[1]);
    if (exprs.length === 0) {
      if (text && !isStatusCopy(text)) out.push({ rule: 'status-word', file, line, text });
    } else if (!STATUS_SOURCES.test(exprs[0])) {
      out.push({ rule: 'status-word', file, line, text: `{${exprs[0].trim()}}` });
    } else if (text && !/^·/.test(text)) {
      out.push({ rule: 'status-word', file, line, text });
    }
  }
  for (const t of tagsOf(markup, 'StatusChip')) {
    const state = t.attrs.match(/\bstate="(waiting|working|failed|done|idle)"/);
    const label = t.attrs.match(/\blabel="([^"]*)"/);
    if (state && label && !isStatusCopy(label[1]))
      out.push({ rule: 'status-word', file, line: lineOf(markup, t.at), text: label[1] });
  }
  return out;
}
