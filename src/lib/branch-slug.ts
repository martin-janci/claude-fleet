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
