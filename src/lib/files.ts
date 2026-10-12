// IPC wrappers for the Files & Diff viewer (iter 5). Each call resolves the
// session's worktree on the backend; there is no long-lived store — the
// FilesPanel holds its own component-local state and caches results.

import { invokeCmd, type Result } from './result';
import type { Commit } from './history';

/** One entry from `git status` for a session's worktree. */
export interface ChangedFile {
  path: string;
  /** modified | added | deleted | renamed | copied | untracked | conflict */
  status: string;
  staged: boolean;
  orig_path: string | null;
  /** Lines added / removed (M15 G1.10); absent for a binary or untracked
   *  file, and from an older hub. */
  added?: number;
  removed?: number;
}

/** Flat worktree listing — tracked + untracked, gitignore respected. */
export interface RepoTree {
  entries: string[];
  truncated: boolean;
}

/** The content of one worktree file. */
export interface FileContent {
  path: string;
  content: string;
  truncated: boolean;
  binary: boolean;
  /** True when the path is a directory (e.g. an embedded git repo). */
  is_dir: boolean;
  /** Byte size when fully read; null when truncated. */
  size: number | null;
}

/** A unified diff for one worktree file. */
export interface FileDiff {
  path: string;
  diff: string;
  binary: boolean;
  truncated: boolean;
}

/** One run of consecutive lines last changed by the same commit. */
export interface BlameHunk {
  /** 1-based first line in the current worktree file. */
  start: number;
  lines: number;
  hash: string;
  author: string;
  /** Author time, Unix seconds. */
  time: number;
  summary: string;
  /** Changed in the worktree and not committed yet. */
  uncommitted: boolean;
}

/** `git blame` of one worktree file. */
export interface FileBlame {
  path: string;
  hunks: BlameHunk[];
  truncated: boolean;
}

/** Which committed range a Changed-section diff covers (redesign step 5.6):
 *  the branch's commits no remote has, or the branch against its base. */
export type DiffRange = 'unpushed' | 'base';

/** What the session's branch carries beyond the worktree: commits not
 *  pushed yet, and what it changes against the base branch. */
export interface BranchDiff {
  /** The checked-out branch; null when HEAD is detached. */
  branch: string | null;
  /** Its upstream (`origin/feat`); null when it was never pushed. */
  upstream: string | null;
  /** Commits no remote has, newest first. */
  unpushed: Commit[];
  /** The files those commits change, as one diff. */
  unpushedFiles: ChangedFile[];
  /** More unpushed commits than the backend lists. */
  truncated: boolean;
  /** The base branch (`origin/main`), or null without one. */
  base: string | null;
  /** Commits on the branch since it left the base. */
  aheadOfBase: number;
  /** The files the branch changes against its merge base with the base. */
  baseFiles: ChangedFile[];
  /** Commits on the base since the branch left it ("N behind main");
   *  absent without a base, and from an older hub. */
  behindBase?: number;
}

export function repoChanges(sessionId: number): Promise<Result<ChangedFile[]>> {
  return invokeCmd<ChangedFile[]>('repo_changes', { args: { session_id: sessionId } });
}

export function repoTree(sessionId: number): Promise<Result<RepoTree>> {
  return invokeCmd<RepoTree>('repo_tree', { args: { session_id: sessionId } });
}

export function repoFile(sessionId: number, path: string): Promise<Result<FileContent>> {
  return invokeCmd<FileContent>('repo_file', { args: { session_id: sessionId, path } });
}

export function repoDiff(sessionId: number, path: string): Promise<Result<FileDiff>> {
  return invokeCmd<FileDiff>('repo_diff', { args: { session_id: sessionId, path } });
}

export function repoBlame(sessionId: number, path: string): Promise<Result<FileBlame>> {
  return invokeCmd<FileBlame>('repo_blame', { args: { session_id: sessionId, path } });
}

export function repoBranchDiff(sessionId: number): Promise<Result<BranchDiff>> {
  return invokeCmd<BranchDiff>('repo_branch_diff', { args: { session_id: sessionId } });
}

export function repoRangeDiff(sessionId: number, path: string, range: DiffRange): Promise<Result<FileDiff>> {
  return invokeCmd<FileDiff>('repo_range_diff', { args: { session_id: sessionId, path, range } });
}

/** Blame needs the file in a commit: an untracked file has no history. */
export function canBlame(status: string | undefined): boolean {
  return status !== 'untracked';
}

/**
 * One gutter entry per line (index 0 is line 1): the hunk that line belongs
 * to, and whether it is the hunk's first line (the only one that shows its
 * label, so a run of lines from one commit reads as one block). Lines past
 * the blame (a truncated blame, or a file read after it) get null.
 */
export function blameGutter(
  hunks: BlameHunk[],
  lineCount: number,
): ({ hunk: BlameHunk; first: boolean } | null)[] {
  const out: ({ hunk: BlameHunk; first: boolean } | null)[] = new Array(lineCount).fill(null);
  for (const h of hunks) {
    for (let i = 0; i < h.lines; i++) {
      const idx = h.start - 1 + i;
      if (idx >= 0 && idx < lineCount) out[idx] = { hunk: h, first: i === 0 };
    }
  }
  return out;
}

/** Statuses for which a diff against HEAD is meaningful (not untracked). */
export function hasDiff(status: string | undefined): boolean {
  return status !== undefined && status !== 'untracked';
}

/**
 * True when a repo command failed because the session's worktree directory no
 * longer exists on disk (backend `E_NO_WORKTREE`). The session may still be
 * running, but its files can't be shown — callers should switch to a "worktree
 * gone" state rather than surfacing the raw git error.
 */
export function isWorktreeGone(r: Result<unknown>): boolean {
  return !r.ok && r.error.code === 'E_NO_WORKTREE';
}

/**
 * Go to file (redesign step 5.6, ⌥⌘P): the worktree paths matching `query`,
 * best first. Every query character must appear in order (a subsequence,
 * case-insensitive); a match inside the file name beats one spread over the
 * folders, a run of consecutive characters beats scattered ones, and a
 * shorter path wins a tie. An empty query lists the first `limit` paths.
 */
export function goToFileMatches(entries: readonly string[], query: string, limit = 50): string[] {
  const q = query.trim().toLowerCase().replace(/\s+/g, '');
  if (q === '') return entries.slice(0, limit);
  const scored: { path: string; score: number }[] = [];
  for (const path of entries) {
    const s = subsequenceScore(path, q);
    if (s !== null) scored.push({ path, score: s });
  }
  scored.sort((a, b) => b.score - a.score || a.path.length - b.path.length || a.path.localeCompare(b.path));
  return scored.slice(0, limit).map((m) => m.path);
}

function subsequenceScore(path: string, q: string): number | null {
  const lc = path.toLowerCase();
  const base = lc.lastIndexOf('/') + 1;
  // The whole query inside the file name is the strongest signal.
  const inName = lc.indexOf(q, base);
  if (inName >= 0) return 1000 - (inName - base) * 2 - (lc.length - base);
  let score = 0;
  let at = -1;
  let prev = -2;
  for (const ch of q) {
    at = lc.indexOf(ch, at + 1);
    if (at < 0) return null;
    score += at === prev + 1 ? 5 : 1;
    if (at >= base) score += 2;
    if (at === 0 || '/._-'.includes(lc[at - 1])) score += 3;
    prev = at;
  }
  return score;
}
