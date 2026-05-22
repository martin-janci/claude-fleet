import { describe, it, expect } from 'vitest';
import { highlight, langForPath } from './highlight';

describe('langForPath', () => {
  it('maps extensions and special names to families', () => {
    expect(langForPath('src/a.ts')).toBe('clike');
    expect(langForPath('x.py')).toBe('python');
    expect(langForPath('deploy.sh')).toBe('shell');
    expect(langForPath('Dockerfile')).toBe('shell');
    expect(langForPath('notes.unknownext')).toBe('');
    expect(langForPath(null)).toBe('');
  });
});

describe('highlight', () => {
  it('returns one token row per source line', () => {
    expect(highlight('a\nb\nc', 'clike').length).toBe(3);
  });

  it('colours keywords, strings and line comments', () => {
    const [row] = highlight('const x = "hi"; // note', 'clike');
    const of = (c: string) => row.filter((t) => t.cls === c).map((t) => t.text);
    expect(of('kw')).toContain('const');
    expect(of('str')).toContain('"hi"');
    expect(of('com')).toContain('// note');
  });

  it('keeps a block comment spanning multiple lines', () => {
    const rows = highlight('/* one\ntwo */ x', 'clike');
    expect(rows[0].some((t) => t.cls === 'com')).toBe(true);
    expect(rows[1].some((t) => t.cls === 'com')).toBe(true);
  });

  it('tags numeric literals', () => {
    const [row] = highlight('let n = 42;', 'clike');
    expect(row.some((t) => t.cls === 'num' && t.text === '42')).toBe(true);
  });

  it('falls back to plain text for unknown languages', () => {
    expect(highlight('hello world', '')).toEqual([[{ text: 'hello world', cls: 'txt' }]]);
  });
});
