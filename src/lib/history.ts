// IPC wrappers for the History & Branches views. Like files.ts, there is no
// long-lived store — FilesPanel holds component-local state.

import { invokeCmd, type Result } from './result';
import type { ChangedFile, FileDiff } from './files';

export interface GitRef {
  name: string;
  /** branch | remote | tag | head */
  kind: string;
}

export interface Commit {
  hash: string;
  shortHash: string;
  parents: string[];
  refs: GitRef[];
  author: string;
  date: string;
  subject: string;
}

export interface Branch {
  name: string;
  isCurrent: boolean;
  isRemote: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  tipHash: string;
  /** The base branch (origin/HEAD, else main or master) already contains
   *  this tip, so deleting it loses nothing. Never set on the base, its
   *  local twin or the checked-out branch. */
  merged: boolean;
}

/** What "Delete merged" did: each name re-checked on the host first. */
export interface DeleteMergedResult {
  deleted: string[];
  /** Asked for but kept (no longer merged, checked out somewhere, gone). */
  kept: string[];
}

export interface CommitDetail {
  hash: string;
  subject: string;
  body: string;
  author: string;
  date: string;
  files: ChangedFile[];
  /** A remote-tracking branch contains it; absent from an older hub. */
  pushed?: boolean;
}

export function repoLog(
  sessionId: number,
  opts: { all?: boolean; limit?: number; skip?: number } = {},
): Promise<Result<Commit[]>> {
  return invokeCmd<Commit[]>('repo_log', {
    args: {
      session_id: sessionId,
      all: opts.all ?? true,
      limit: opts.limit ?? 0,
      skip: opts.skip ?? 0,
    },
  });
}

export function repoBranches(sessionId: number): Promise<Result<Branch[]>> {
  return invokeCmd<Branch[]>('repo_branches', { args: { session_id: sessionId } });
}

export function repoCommit(sessionId: number, hash: string): Promise<Result<CommitDetail>> {
  return invokeCmd<CommitDetail>('repo_commit', { args: { session_id: sessionId, hash } });
}

export function repoCommitDiff(
  sessionId: number,
  hash: string,
  path: string,
): Promise<Result<FileDiff>> {
  return invokeCmd<FileDiff>('repo_commit_diff', {
    args: { session_id: sessionId, hash, path },
  });
}

// ─── mutations (Phase 4 backend) ──────────────────────────────────────────

export function repoCheckout(sessionId: number, branch: string): Promise<Result<null>> {
  return invokeCmd<null>('repo_checkout', { args: { session_id: sessionId, branch } });
}

export function repoCheckoutCommit(sessionId: number, hash: string): Promise<Result<null>> {
  return invokeCmd<null>('repo_checkout_commit', { args: { session_id: sessionId, hash } });
}

export function repoCreateBranch(
  sessionId: number,
  name: string,
  opts: { startPoint?: string | null; checkout?: boolean } = {},
): Promise<Result<null>> {
  return invokeCmd<null>('repo_create_branch', {
    args: {
      session_id: sessionId,
      name,
      start_point: opts.startPoint ?? null,
      checkout: opts.checkout ?? false,
    },
  });
}

export function repoDeleteBranch(
  sessionId: number,
  name: string,
  force = false,
): Promise<Result<null>> {
  return invokeCmd<null>('repo_delete_branch', {
    args: { session_id: sessionId, name, force },
  });
}

export function repoDeleteMergedBranches(
  sessionId: number,
  names: string[],
): Promise<Result<DeleteMergedResult>> {
  return invokeCmd<DeleteMergedResult>('repo_delete_merged_branches', {
    args: { session_id: sessionId, names },
  });
}

export function repoStage(sessionId: number, paths: string[]): Promise<Result<null>> {
  return invokeCmd<null>('repo_stage', { args: { session_id: sessionId, paths } });
}

export function repoUnstage(sessionId: number, paths: string[]): Promise<Result<null>> {
  return invokeCmd<null>('repo_unstage', { args: { session_id: sessionId, paths } });
}

/** A commit message drafted from the staged changes (redesign 5.12). */
export interface CommitDraft {
  message: string;
  model: string;
  host_alias: string;
  /** How many staged files it was written from. */
  files: number;
}

/** Draft a commit message on the session's own host and account; nothing
 *  is committed. */
export function draftCommitMessage(sessionId: number): Promise<Result<CommitDraft>> {
  return invokeCmd<CommitDraft>('draft_commit_message', { args: { session_id: sessionId } });
}

export function repoCommitCreate(
  sessionId: number,
  message: string,
  amend = false,
): Promise<Result<null>> {
  return invokeCmd<null>('repo_commit_create', {
    args: { session_id: sessionId, message, amend },
  });
}

export function repoFetch(sessionId: number): Promise<Result<null>> {
  return invokeCmd<null>('repo_fetch', { args: { session_id: sessionId } });
}

export function repoPull(sessionId: number): Promise<Result<null>> {
  return invokeCmd<null>('repo_pull', { args: { session_id: sessionId } });
}

export function repoPush(sessionId: number, setUpstream = false): Promise<Result<null>> {
  return invokeCmd<null>('repo_push', {
    args: { session_id: sessionId, set_upstream: setUpstream },
  });
}
