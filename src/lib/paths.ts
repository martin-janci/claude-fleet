/**
 * Repo-relative file paths inside reply text, so the Conversation tab can
 * make `src/lib/foo.ts:42` open in the Files tab. Pure; the renderer maps
 * the pieces onto elements.
 */

export type PathPiece = { t: 'text'; v: string } | { t: 'path'; v: string; path: string; line: number | null };

// A path: an optional `./`, at least one directory segment, a file name
// with an extension, an optional `:line`. Not preceded by a path/URL char
// (so `https://x/y.ts`, `~/.claude/x.md` and absolute paths are left alone,
// as none of those is repo-relative) and not followed by one.
const PATH_RE = /(?<![\w/.:@~-])(?:\.\/)?((?:[\w.@-]+\/)+[\w.@-]+\.[A-Za-z0-9]{1,8})(?::(\d+))?(?![\w/])/g;

export function splitPaths(text: string): PathPiece[] {
  const out: PathPiece[] = [];
  let last = 0;
  for (const m of text.matchAll(PATH_RE)) {
    const start = m.index ?? 0;
    if (start > last) out.push({ t: 'text', v: text.slice(last, start) });
    out.push({ t: 'path', v: m[0], path: m[1], line: m[2] ? Number(m[2]) : null });
    last = start + m[0].length;
  }
  if (last < text.length) out.push({ t: 'text', v: text.slice(last) });
  return out;
}

/** True when the text holds at least one path (cheap pre-check). */
export function hasPath(text: string): boolean {
  PATH_RE.lastIndex = 0;
  const hit = PATH_RE.test(text);
  PATH_RE.lastIndex = 0;
  return hit;
}
