import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { linkHref } from './open_external';

// A link's target is data in most of the app: the pull request a host's `gh`
// reported, a tracker item's URL, a failing check. Only http(s) and mailto
// may become a target (`safeHref`), and only `open_external.ts` hands one to
// the system browser. Held on the source, so a new link cannot skip it.

function sources(ext: RegExp): string[] {
  return readdirSync('src', { recursive: true })
    .map((rel) => `src/${rel.replaceAll('\\', '/')}`)
    .filter((file) => ext.test(file));
}

// `href={expr}`, `href="{expr}…"` and the `{href}` shorthand.
const HREF_EXPR = /\bhref=(?:\{([^}]*)\}|"([^"]*\{[^"]*)")|\{href\}/g;

// The Markdown renderer's links: `parseInline` already put every target
// through `safeHref` (`null` when refused), and the click goes to
// `openExternal`, which checks again.
const PARSED = new Set(['src/lib/MarkdownInline.svelte: n.href']);

describe('link targets', () => {
  it('linkHref keeps http(s) and mailto and drops everything else', () => {
    expect(linkHref('https://github.com/o/r/pull/7')).toBe('https://github.com/o/r/pull/7');
    expect(linkHref('http://jira.local/browse/A-1')).toBe('http://jira.local/browse/A-1');
    expect(linkHref('mailto:a@b.c')).toBe('mailto:a@b.c');
    for (const bad of [
      'javascript:alert(1)',
      'file:///etc/passwd',
      'smb://host/share',
      'ms-msdt:/id x',
      'data:text/html,<b>x</b>',
      '//host/path',
      '/relative',
      'https://a b',
      '',
      null,
      undefined,
    ]) {
      expect(linkHref(bad), String(bad)).toBeUndefined();
    }
  });

  it('every href built from an expression goes through linkHref', () => {
    const loose: string[] = [];
    for (const file of sources(/\.svelte$/)) {
      const src = readFileSync(file, 'utf8');
      for (const m of src.matchAll(HREF_EXPR)) {
        const expr = (m[1] ?? m[2] ?? '{href}').trim();
        if (expr.startsWith('linkHref(')) continue;
        // A fragment on this page (`href="#{item.id}"`) names no scheme.
        if (m[2]?.startsWith('#')) continue;
        if (PARSED.has(`${file}: ${expr}`)) continue;
        loose.push(`${file}: ${expr}`);
      }
    }
    expect(loose).toEqual([]);
  });

  it('only open_external.ts hands a URL to the system browser', () => {
    const callers = sources(/\.(svelte|ts)$/)
      .filter((file) => !/\.test\.ts$/.test(file) && file !== 'src/lib/open_external.ts')
      .filter((file) => /\bopenUrl\s*\(/.test(readFileSync(file, 'utf8')));
    expect(callers).toEqual([]);
  });
});
