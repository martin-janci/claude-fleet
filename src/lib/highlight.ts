// highlight.ts — a tiny, dependency-free syntax highlighter.
//
// This is deliberately *not* a real parser. It is a single-pass character
// scanner that colours comments, strings, numbers and a per-language keyword
// set — enough to make a file readable in the viewer. Edge cases (regex
// literals, nested template expressions, here-docs) are intentionally not
// handled; the goal is a useful approximation, not correctness.

export type TokClass = 'kw' | 'str' | 'com' | 'num' | 'txt' | 'head' | 'code';

export interface Tok {
  text: string;
  cls: TokClass;
}

interface LangCfg {
  /** Line-comment marker, e.g. `//` or `#`. */
  line?: string;
  blockOpen?: string;
  blockClose?: string;
  /** String delimiters; the backtick is treated as multi-line. */
  strings: string[];
  /** Allow tripled delimiters as multi-line strings (Python """ / '''). */
  triple?: boolean;
  keywords: Set<string>;
}

const words = (s: string): Set<string> => new Set(s.split(/\s+/));

// One over-inclusive keyword set covers the whole C-family. A keyword that is
// really an identifier in some member language is a rare, harmless mis-colour.
const CLIKE = words(
  'abstract any as async await break case catch class const continue debugger ' +
    'declare default defer delete do else enum export extends extern false final ' +
    'finally fn for from func function go goto if impl implements import in ' +
    'instanceof interface let loop match mod module mut namespace new null of ' +
    'override package private protected pub public readonly return satisfies ' +
    'select self static struct super switch this throw throws trait true try ' +
    'type typeof undefined union unsafe use var void where while with yield ' +
    'bool boolean byte char double float int long number short str string ' +
    'usize isize uint i8 i16 i32 i64 u8 u16 u32 u64 f32 f64',
);

const PY = words(
  'and as assert async await break case class continue def del elif else except ' +
    'False finally for from global if import in is lambda match None nonlocal not ' +
    'or pass raise return self True try while with yield',
);

const SHELL = words(
  'if then elif else fi for while until do done case esac function in select ' +
    'return local export readonly declare set unset source alias',
);

const LANGS: Record<string, LangCfg> = {
  clike: {
    line: '//',
    blockOpen: '/*',
    blockClose: '*/',
    strings: ['"', "'", '`'],
    keywords: CLIKE,
  },
  python: { line: '#', strings: ['"', "'"], triple: true, keywords: PY },
  shell: { line: '#', strings: ['"', "'"], keywords: SHELL },
  yaml: { line: '#', strings: ['"', "'"], keywords: words('true false null yes no on off') },
  css: { blockOpen: '/*', blockClose: '*/', strings: ['"', "'"], keywords: new Set() },
  html: { blockOpen: '<!--', blockClose: '-->', strings: ['"', "'"], keywords: new Set() },
  json: { strings: ['"'], keywords: words('true false null') },
};

const EXT: Record<string, string> = {
  js: 'clike', jsx: 'clike', mjs: 'clike', cjs: 'clike',
  ts: 'clike', tsx: 'clike', mts: 'clike', cts: 'clike',
  c: 'clike', h: 'clike', cpp: 'clike', cc: 'clike', cxx: 'clike', hpp: 'clike', hh: 'clike',
  cs: 'clike', java: 'clike', kt: 'clike', kts: 'clike', scala: 'clike',
  go: 'clike', rs: 'clike', swift: 'clike', dart: 'clike', php: 'clike',
  py: 'python', pyi: 'python', rb: 'python',
  sh: 'shell', bash: 'shell', zsh: 'shell', fish: 'shell',
  css: 'css', scss: 'css', sass: 'css', less: 'css',
  html: 'html', htm: 'html', xml: 'html', svg: 'html', vue: 'html', svelte: 'html',
  json: 'json', jsonc: 'json',
  yaml: 'yaml', yml: 'yaml', toml: 'yaml',
  md: 'md', markdown: 'md', mdx: 'md',
};

const NAME: Record<string, string> = {
  dockerfile: 'shell',
  makefile: 'shell',
};

/** Pick a highlighter language for a path, or `''` (plain text) if unknown. */
export function langForPath(path: string | null | undefined): string {
  if (!path) return '';
  const base = (path.split('/').pop() ?? '').toLowerCase();
  if (NAME[base]) return NAME[base];
  const dot = base.lastIndexOf('.');
  if (dot <= 0) return '';
  return EXT[base.slice(dot + 1)] ?? '';
}

// Colour inline Markdown spans (code, emphasis, links) within one line,
// appending tokens to `out`. Spans are not nested — the first match wins.
function mdInline(s: string, out: Tok[]): void {
  const re =
    /(`+)([^`]*?)\1|(\*\*|__)(.+?)\3|(\*|_)(.+?)\5|(!?)\[([^\]]*)\]\(([^)]*)\)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) {
    if (m.index > last) out.push({ text: s.slice(last, m.index), cls: 'txt' });
    if (m[1] !== undefined) {
      out.push({ text: m[0], cls: 'code' }); // `inline code`
    } else if (m[3] !== undefined || m[5] !== undefined) {
      out.push({ text: m[0], cls: 'kw' }); // **bold** / *italic*
    } else {
      out.push({ text: `${m[7]}[${m[8]}]`, cls: 'str' }); // [link text]
      out.push({ text: `(${m[9]})`, cls: 'com' }); // (url)
    }
    last = re.lastIndex;
  }
  if (last < s.length) out.push({ text: s.slice(last), cls: 'txt' });
}

// Markdown is structural rather than token-based, so it gets a dedicated
// line-oriented pass instead of the generic scanner.
function highlightMarkdown(content: string): Tok[][] {
  const lines: Tok[][] = [];
  let inFence = false;
  for (const raw of content.split('\n')) {
    if (/^\s*(```|~~~)/.test(raw)) {
      inFence = !inFence;
      lines.push([{ text: raw, cls: 'code' }]);
      continue;
    }
    if (inFence) {
      lines.push(raw === '' ? [] : [{ text: raw, cls: 'code' }]);
      continue;
    }
    if (raw === '') {
      lines.push([]);
      continue;
    }
    if (/^\s{0,3}#{1,6}(\s|$)/.test(raw)) {
      lines.push([{ text: raw, cls: 'head' }]); // # heading
      continue;
    }
    if (/^\s{0,3}>/.test(raw)) {
      lines.push([{ text: raw, cls: 'com' }]); // > blockquote
      continue;
    }
    const toks: Tok[] = [];
    // A list marker (-, *, +, 1.) is keyword-coloured; the rest is inline.
    const marker = raw.match(/^(\s*)([-*+]|\d+[.)])(\s+)/);
    if (marker) {
      if (marker[1]) toks.push({ text: marker[1], cls: 'txt' });
      toks.push({ text: marker[2], cls: 'kw' });
      toks.push({ text: marker[3], cls: 'txt' });
      mdInline(raw.slice(marker[0].length), toks);
    } else {
      mdInline(raw, toks);
    }
    lines.push(toks);
  }
  return lines;
}

/**
 * Tokenise `content` for `lang` (a key from {@link langForPath}). Returns one
 * token array per source line; an empty array is an empty line. An unknown
 * `lang` yields a single plain-text token per line.
 */
export function highlight(content: string, lang: string): Tok[][] {
  if (lang === 'md') return highlightMarkdown(content);

  const lines: Tok[][] = [];
  let cur: Tok[] = [];
  // Append `text` (which may contain newlines) as `cls` tokens, breaking the
  // current line wherever a newline appears.
  const push = (text: string, cls: TokClass): void => {
    const parts = text.split('\n');
    for (let i = 0; i < parts.length; i++) {
      if (i > 0) {
        lines.push(cur);
        cur = [];
      }
      if (parts[i] !== '') cur.push({ text: parts[i], cls });
    }
  };

  const cfg = LANGS[lang];
  if (!cfg) {
    for (const ln of content.split('\n')) lines.push(ln === '' ? [] : [{ text: ln, cls: 'txt' }]);
    return lines;
  }

  const kw = cfg.keywords;
  const word = /[A-Za-z_$][\w$]*|\d[\w.]*/g;
  // Split a run of non-comment, non-string text into identifier/number tokens.
  const emitPlain = (s: string): void => {
    word.lastIndex = 0;
    let last = 0;
    let m: RegExpExecArray | null;
    while ((m = word.exec(s)) !== null) {
      if (m.index > last) push(s.slice(last, m.index), 'txt');
      const w = m[0];
      if (w[0] >= '0' && w[0] <= '9') push(w, 'num');
      else push(w, kw.has(w) ? 'kw' : 'txt');
      last = word.lastIndex;
    }
    if (last < s.length) push(s.slice(last), 'txt');
  };

  const n = content.length;
  let i = 0;
  let plainStart = 0;
  const flush = (upto: number): void => {
    if (upto > plainStart) emitPlain(content.slice(plainStart, upto));
  };

  while (i < n) {
    const ch = content[i];

    if (cfg.line && content.startsWith(cfg.line, i)) {
      flush(i);
      let j = content.indexOf('\n', i);
      if (j === -1) j = n;
      push(content.slice(i, j), 'com');
      i = plainStart = j;
      continue;
    }

    if (cfg.blockOpen && content.startsWith(cfg.blockOpen, i)) {
      flush(i);
      const close = cfg.blockClose!;
      const found = content.indexOf(close, i + cfg.blockOpen.length);
      const j = found === -1 ? n : found + close.length;
      push(content.slice(i, j), 'com');
      i = plainStart = j;
      continue;
    }

    if (cfg.strings.includes(ch)) {
      flush(i);
      const triple = !!cfg.triple && content[i + 1] === ch && content[i + 2] === ch;
      const delim = triple ? ch + ch + ch : ch;
      const multiline = triple || ch === '`';
      let j = i + delim.length;
      let end = n;
      while (j < n) {
        const c = content[j];
        if (c === '\\' && !triple) {
          j += 2;
          continue;
        }
        if (content.startsWith(delim, j)) {
          end = j + delim.length;
          break;
        }
        // An unterminated single-line string stops at the line end.
        if (!multiline && c === '\n') {
          end = j;
          break;
        }
        j++;
      }
      push(content.slice(i, end), 'str');
      i = plainStart = end;
      continue;
    }

    i++;
  }
  flush(n);
  lines.push(cur);
  return lines;
}
