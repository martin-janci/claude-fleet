// IPC wrappers for the Files & Diff viewer (iter 5). Each call resolves the
// session's worktree on the backend; there is no long-lived store — the
// FilesPanel holds its own component-local state and caches results.

import { invokeCmd, type Result } from './result';

/** One entry from `git status` for a session's worktree. */
export interface ChangedFile {
  path: string;
  /** modified | added | deleted | renamed | copied | untracked | conflict */
  status: string;
  staged: boolean;
  orig_path: string | null;
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

/** Statuses for which a diff against HEAD is meaningful (not untracked). */
export function hasDiff(status: string | undefined): boolean {
  return status !== undefined && status !== 'untracked';
}
