import { describe, it, expect } from 'vitest';
import { splitPaths } from './paths';

describe('splitPaths', () => {
  it('finds repo-relative paths with an optional line, leaving the rest as text', () => {
    expect(splitPaths('see src/lib/foo.ts:42 and src-tauri/src/a.rs.')).toEqual([
      { t: 'text', v: 'see ' },
      { t: 'path', v: 'src/lib/foo.ts:42', path: 'src/lib/foo.ts', line: 42 },
      { t: 'text', v: ' and ' },
      { t: 'path', v: 'src-tauri/src/a.rs', path: 'src-tauri/src/a.rs', line: null },
      { t: 'text', v: '.' },
    ]);
  });

  it('accepts a ./ prefix and links the path without it', () => {
    expect(splitPaths('run ./scripts/build.sh:3 now')).toEqual([
      { t: 'text', v: 'run ' },
      { t: 'path', v: './scripts/build.sh:3', path: 'scripts/build.sh', line: 3 },
      { t: 'text', v: ' now' },
    ]);
  });

  it('leaves URLs, home paths, bare file names and words alone', () => {
    expect(splitPaths('https://example.com/x/y.ts')).toEqual([{ t: 'text', v: 'https://example.com/x/y.ts' }]);
    expect(splitPaths('~/.claude/skills/a.md')).toEqual([{ t: 'text', v: '~/.claude/skills/a.md' }]);
    expect(splitPaths('/etc/hosts and foo.ts')).toEqual([{ t: 'text', v: '/etc/hosts and foo.ts' }]);
    expect(splitPaths('either/or')).toEqual([{ t: 'text', v: 'either/or' }]);
    expect(splitPaths('')).toEqual([]);
  });

  it('refuses parent segments and a zero line, which the backend or viewer would reject', () => {
    expect(splitPaths('see ../foo/bar.ts here')).toEqual([{ t: 'text', v: 'see ../foo/bar.ts here' }]);
    expect(splitPaths('src/../lib/a.ts')).toEqual([{ t: 'text', v: 'src/../lib/a.ts' }]);
    expect(splitPaths('foo/bar.ts:0')).toEqual([
      { t: 'path', v: 'foo/bar.ts', path: 'foo/bar.ts', line: null },
      { t: 'text', v: ':0' },
    ]);
  });

  it('two calls in a row see every path (no regex state carried over)', () => {
    expect(splitPaths('a/b.ts and c/d.ts').filter((p) => p.t === 'path')).toHaveLength(2);
    expect(splitPaths('src/lib/a.ts x').filter((p) => p.t === 'path')).toHaveLength(1);
  });
});
