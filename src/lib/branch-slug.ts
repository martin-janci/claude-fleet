// Convert free-form user input ("Fix the login bug!") into a git-safe branch
// / worktree name ("fix-the-login-bug"). Designed for live use in an input:
// it preserves a trailing `-` while the user is mid-word so typing stays
// natural. Run `finalizeBranchSlug` on blur / submit to strip the trailing
// dash and other tail debris.

const ALLOWED = /[a-z0-9./-]/;

export function slugifyBranch(raw: string): string {
  // 1. Normalize accents, lowercase, swap underscores + whitespace for dashes.
  const lowered = raw
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '') // strip combining diacritics
    .toLowerCase()
    .replace(/[\s_]+/g, '-');
  // 2. Drop anything outside the allowed set.
  let out = '';
  for (const ch of lowered) {
    if (ALLOWED.test(ch)) out += ch;
  }
  // 3. Collapse runs and forbidden sequences. Git rejects `..`, `//`, and
  //    leading `-` / `.` — squash them here, but keep the trailing `-` if
  //    any so live typing feels responsive.
  out = out
    .replace(/-{2,}/g, '-')
    .replace(/\.{2,}/g, '.')
    .replace(/\/{2,}/g, '/')
    .replace(/^[-./]+/, '');
  // Cap at a sensible branch length so a pasted paragraph doesn't blow up
  // the worktree dir name.
  if (out.length > 60) out = out.slice(0, 60).replace(/-+$/, '');
  return out;
}

export function finalizeBranchSlug(raw: string): string {
  return slugifyBranch(raw).replace(/[-./]+$/, '');
}

/**
 * Mirror git's `check-ref-format --branch` rules closely enough to catch the
 * common typos in the "New branch" prompt before the round-trip to the host.
 * Returns a human-readable reason, or null when the name is acceptable.
 */
export function validateBranchName(name: string): string | null {
  if (name.trim() === '') return 'Branch name is required.';
  if (/\s/.test(name)) return 'Branch names cannot contain whitespace.';
  if (/[~^:?*[\\]/.test(name)) return 'Branch names cannot contain ~ ^ : ? * [ or \\.';
  if (/[\x00-\x1f\x7f]/.test(name)) return 'Branch names cannot contain control characters.';
  if (name.startsWith('-')) return 'Branch names cannot start with -.';
  if (name.startsWith('/') || name.endsWith('/')) return 'Branch names cannot start or end with /.';
  if (name.endsWith('.') || name.endsWith('.lock')) return 'Branch names cannot end with . or .lock.';
  if (name.includes('..') || name.includes('//') || name.includes('@{')) {
    return 'Branch names cannot contain .. // or @{.';
  }
  if (name.split('/').some((seg) => seg.startsWith('.'))) {
    return 'Branch name components cannot start with a dot.';
  }
  if (name === '@') return '"@" is not a valid branch name.';
  return null;
}
