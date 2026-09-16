import { describe, it, expect } from 'vitest';
import { parseMarkdown, parseInline, safeHref, fenceLang, type Block, type Inline } from './markdown';

const text = (v: string): Inline => ({ t: 'text', v });

describe('safeHref', () => {
  it('accepts http, https and mailto', () => {
    expect(safeHref('https://example.com/a?b=1')).toBe('https://example.com/a?b=1');
    expect(safeHref('http://x.y')).toBe('http://x.y');
    expect(safeHref('mailto:a@b.c')).toBe('mailto:a@b.c');
  });

  it('rejects script-capable, relative and malformed urls', () => {
    expect(safeHref('javascript:alert(1)')).toBeNull();
    expect(safeHref('JavaScript:alert(1)')).toBeNull();
    expect(safeHref(' javascript:alert(1)')).toBeNull();
    expect(safeHref('data:text/html,<b>x</b>')).toBeNull();
    expect(safeHref('/etc/passwd')).toBeNull();
    expect(safeHref('file:///etc/passwd')).toBeNull();
    expect(safeHref('https://')).toBeNull();
    expect(safeHref('')).toBeNull();
  });
});

describe('parseInline', () => {
  it('parses emphasis, strong, strikethrough and code', () => {
    expect(parseInline('a **b** *c* ~~d~~ `e`')).toEqual([
      text('a '),
      { t: 'strong', c: [text('b')] },
      text(' '),
      { t: 'em', c: [text('c')] },
      text(' '),
      { t: 'del', c: [text('d')] },
      text(' '),
      { t: 'code', v: 'e' },
    ]);
  });

  it('nests emphasis inside strong', () => {
    expect(parseInline('**a *b* c**')).toEqual([
      { t: 'strong', c: [text('a '), { t: 'em', c: [text('b')] }, text(' c')] },
    ]);
  });

  it('parses emphasis that starts or ends with a code span', () => {
    expect(parseInline('**`claude logs` needs the id**')).toEqual([
      { t: 'strong', c: [{ t: 'code', v: 'claude logs' }, text(' needs the id')] },
    ]);
    expect(parseInline('*see `x`*')).toEqual([{ t: 'em', c: [text('see '), { t: 'code', v: 'x' }] }]);
    expect(parseInline('****')).toEqual([text('****')]);
  });

  it('keeps markup inside code spans literal', () => {
    expect(parseInline('`**x** <b>`')).toEqual([{ t: 'code', v: '**x** <b>' }]);
    expect(parseInline('``a ` b``')).toEqual([{ t: 'code', v: 'a ` b' }]);
  });

  it('leaves unmatched delimiters and snake_case alone', () => {
    expect(parseInline('2 * 3 = 6')).toEqual([text('2 * 3 = 6')]);
    expect(parseInline('my_var_name and **open')).toEqual([text('my_var_name and **open')]);
    expect(parseInline('a ` b')).toEqual([text('a ` b')]);
  });

  it('honours backslash escapes', () => {
    expect(parseInline('\\*not em\\*')).toEqual([text('*not em*')]);
  });

  it('parses links, marking unsafe targets with a null href', () => {
    expect(parseInline('see [docs](https://d.io/x "t") now')).toEqual([
      text('see '),
      { t: 'link', href: 'https://d.io/x', c: [text('docs')] },
      text(' now'),
    ]);
    expect(parseInline('[x](javascript:alert(1))')).toEqual([{ t: 'link', href: null, c: [text('x')] }]);
  });

  it('autolinks bare and angle-bracket urls without trailing punctuation', () => {
    expect(parseInline('PR: https://github.com/a/b/pull/1.')).toEqual([
      text('PR: '),
      { t: 'link', href: 'https://github.com/a/b/pull/1', c: [text('https://github.com/a/b/pull/1')] },
      text('.'),
    ]);
    expect(parseInline('<https://x.io>')).toEqual([{ t: 'link', href: 'https://x.io', c: [text('https://x.io')] }]);
  });

  it('keeps raw HTML as text', () => {
    expect(parseInline('<script>alert(1)</script>')).toEqual([text('<script>alert(1)</script>')]);
  });

  it('turns newlines into line breaks', () => {
    expect(parseInline('a\nb')).toEqual([text('a'), { t: 'br' }, text('b')]);
  });
});

describe('parseMarkdown', () => {
  it('parses headings, paragraphs and rules', () => {
    const blocks = parseMarkdown('# Title\n\nFirst line\nsecond line\n\n---\n\n### Sub ###');
    expect(blocks).toEqual<Block[]>([
      { t: 'heading', level: 1, c: [text('Title')] },
      { t: 'para', c: [text('First line'), { t: 'br' }, text('second line')] },
      { t: 'hr' },
      { t: 'heading', level: 3, c: [text('Sub')] },
    ]);
  });

  it('parses fenced code with a language, keeping content verbatim', () => {
    const blocks = parseMarkdown('Run:\n```bash\necho "**hi**"\n  indented\n```\nafter');
    expect(blocks).toEqual<Block[]>([
      { t: 'para', c: [text('Run:')] },
      { t: 'code', lang: 'bash', v: 'echo "**hi**"\n  indented' },
      { t: 'para', c: [text('after')] },
    ]);
  });

  it('runs an unclosed fence to the end', () => {
    expect(parseMarkdown('~~~\nstill code')).toEqual([{ t: 'code', lang: '', v: 'still code' }]);
  });

  it('parses bullet lists with nesting and task items', () => {
    const blocks = parseMarkdown('- [x] done\n- [ ] todo\n  - child\n- plain');
    expect(blocks).toEqual<Block[]>([
      {
        t: 'list',
        ordered: false,
        start: 1,
        items: [
          { task: true, c: [{ t: 'para', c: [text('done')] }] },
          {
            task: false,
            c: [
              { t: 'para', c: [text('todo')] },
              { t: 'list', ordered: false, start: 1, items: [{ task: null, c: [{ t: 'para', c: [text('child')] }] }] },
            ],
          },
          { task: null, c: [{ t: 'para', c: [text('plain')] }] },
        ],
      },
    ]);
  });

  it('parses ordered lists with their start number', () => {
    expect(parseMarkdown('3. three\n4. four')).toEqual<Block[]>([
      {
        t: 'list',
        ordered: true,
        start: 3,
        items: [
          { task: null, c: [{ t: 'para', c: [text('three')] }] },
          { task: null, c: [{ t: 'para', c: [text('four')] }] },
        ],
      },
    ]);
  });

  it('keeps a lazy continuation line inside its list item', () => {
    expect(parseMarkdown('- first\ncontinued')).toEqual([
      {
        t: 'list',
        ordered: false,
        start: 1,
        items: [{ task: null, c: [{ t: 'para', c: [text('first'), { t: 'br' }, text('continued')] }] }],
      },
    ]);
  });

  it('parses blockquotes recursively', () => {
    expect(parseMarkdown('> **Note**\n> - a')).toEqual<Block[]>([
      {
        t: 'quote',
        c: [
          { t: 'para', c: [{ t: 'strong', c: [text('Note')] }] },
          { t: 'list', ordered: false, start: 1, items: [{ task: null, c: [{ t: 'para', c: [text('a')] }] }] },
        ],
      },
    ]);
  });

  it('parses tables with alignment and escaped pipes', () => {
    const blocks = parseMarkdown('| Name | Count | Note |\n|:---|---:|:-:|\n| a | 1 | x \\| y |\n| `b` | 2 |');
    expect(blocks).toEqual<Block[]>([
      {
        t: 'table',
        align: ['left', 'right', 'center'],
        head: [[text('Name')], [text('Count')], [text('Note')]],
        rows: [
          [[text('a')], [text('1')], [text('x | y')]],
          [[{ t: 'code', v: 'b' }], [text('2')], []],
        ],
      },
    ]);
  });

  it('treats a pipe line without a separator as a paragraph', () => {
    expect(parseMarkdown('a | b')).toEqual([{ t: 'para', c: [text('a | b')] }]);
  });

  it('keeps raw HTML blocks as paragraph text', () => {
    expect(parseMarkdown('<img src=x onerror=alert(1)>')).toEqual([
      { t: 'para', c: [text('<img src=x onerror=alert(1)>')] },
    ]);
  });

  it('handles CRLF and an empty source', () => {
    expect(parseMarkdown('')).toEqual([]);
    expect(parseMarkdown('a\r\nb')).toEqual([{ t: 'para', c: [text('a'), { t: 'br' }, text('b')] }]);
  });
});

describe('fenceLang', () => {
  it('maps common fence names onto highlighter languages', () => {
    expect(fenceLang('ts')).toBe('clike');
    expect(fenceLang('TypeScript')).toBe('clike');
    expect(fenceLang('rust')).toBe('clike');
    expect(fenceLang('bash')).toBe('shell');
    expect(fenceLang('console')).toBe('shell');
    expect(fenceLang('python')).toBe('python');
    expect(fenceLang('json')).toBe('json');
    expect(fenceLang('yml')).toBe('yaml');
    expect(fenceLang('diff')).toBe('');
    expect(fenceLang('')).toBe('');
  });
});

describe('pathological input', () => {
  it('stays fast on many unmatched delimiters', () => {
    const nasty = '*a _b `c [d '.repeat(6000);
    const t0 = performance.now();
    const blocks = parseMarkdown(nasty);
    expect(performance.now() - t0).toBeLessThan(1000);
    expect(blocks).toHaveLength(1);
  });

  it('does not overflow the stack on deeply nested quotes and lists', () => {
    expect(() => parseMarkdown('>'.repeat(5000) + ' x')).not.toThrow();
    expect(() => parseMarkdown(Array.from({ length: 400 }, (_, k) => ' '.repeat(k * 2) + '- x').join('\n'))).not.toThrow();
  });
});
