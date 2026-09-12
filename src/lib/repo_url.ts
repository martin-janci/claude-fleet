/**
 * TypeScript mirror of `src-tauri/src/repo_url.rs`. Twins that MUST stay in
 * sync: any rule change on one side needs the identical change on the other.
 * The point of this mirror is that the Add-project dialog never accepts
 * input the backend would refuse, so parity with the Rust file is the whole
 * requirement — not just "looks similar".
 *
 * Ported pitfalls worth remembering:
 * - `.length` on a JS string counts UTF-16 code units. That is identical to
 *   Rust's byte-length check ONLY because `is_component`'s character class is
 *   ASCII-only — every accepted character is one UTF-16 unit / one byte, so
 *   the two length checks agree. A non-ASCII string still gets its `.length`
 *   taken (matching Rust, which would have already rejected it on `s.len()`
 *   for a multi-byte character), but it is rejected by the character-class
 *   test below regardless.
 * - The character test MUST be an ASCII-only regex, `/^[A-Za-z0-9._-]+$/`.
 *   A Unicode-aware class like `\w` would incorrectly accept non-ASCII word
 *   characters that Rust's `is_ascii_alphanumeric` rejects.
 */

export interface RepoRef {
  owner: string;
  repo: string;
}

/**
 * `{ owner, repo }` from `owner/repo`, an https URL, or an SSH URL. `null`
 * for anything else — including a host other than github.com and any
 * component that is not a safe path component, so a parsed pair is always
 * safe to interpolate into a path.
 */
export function parseRepoUrl(input: string): RepoRef | null {
  const s = input.trim();
  if (s.length === 0) return null;

  let rest: string;
  if (s.startsWith('git@github.com:')) {
    rest = s.slice('git@github.com:'.length);
  } else if (s.startsWith('ssh://git@github.com/')) {
    rest = s.slice('ssh://git@github.com/'.length);
  } else if (s.startsWith('https://github.com/')) {
    rest = s.slice('https://github.com/'.length);
  } else if (s.startsWith('http://github.com/')) {
    rest = s.slice('http://github.com/'.length);
  } else if (s.includes('://') || s.includes('@')) {
    // Some other host, or an SSH form we do not accept.
    return null;
  } else {
    rest = s;
  }

  rest = rest.replace(/\/+$/, '');
  rest = rest.endsWith('.git') ? rest.slice(0, -'.git'.length) : rest;

  const parts = rest.split('/');
  if (parts.length !== 2) return null;
  const [owner, repo] = parts;

  // GitHub's own limits: an owner (user/org) name is capped at 39
  // characters, a repo name at 100.
  if (!isComponent(owner, 39) || !isComponent(repo, 100)) return null;

  return { owner, repo };
}

/**
 * A safe single path component. Beyond being non-empty and free of `/`,
 * this rejects a component made entirely of dots (`.`, `..`, `...`, ...,
 * which would otherwise let a segment resolve to the current or parent
 * directory), the literal component `.git` case-insensitively, a leading
 * `-` (which a command-line parser would read as an option instead of a
 * name), and anything longer than `maxLen`.
 */
export function isComponent(s: string, maxLen: number): boolean {
  return (
    s.length > 0 &&
    s.length <= maxLen &&
    !s.startsWith('-') &&
    !/^\.+$/.test(s) &&
    s.toLowerCase() !== '.git' &&
    /^[A-Za-z0-9._-]+$/.test(s)
  );
}

/** The URL fleet clones with, for a pair from {@link parseRepoUrl}. */
export function cloneUrlFor(owner: string, repo: string): string {
  return `git@github.com:${owner}/${repo}.git`;
}
