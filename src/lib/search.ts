import type { SessionRow } from './sessions';
import { fold } from './text_fold';

// One rule for finding a session, wherever a person types (the Sessions
// list, Control, ⌘K's ranking, and fleet-mobile's list): every word of the
// query must be in one of the session's fields, in any order, case and
// accents ignored.

/** What a session is found by: its names, host, branch, project, tags,
 *  last prompt and the task it works on. */
export function sessionSearchFields(s: SessionRow, projectLabel?: string | null): string[] {
  return [
    s.friendly_name,
    s.tmux_name,
    s.host_alias,
    s.worktree_key,
    projectLabel,
    ...(s.tags ?? []),
    s.last_prompt,
    s.work?.key,
    s.work?.title,
  ].filter((x): x is string => !!x);
}

/** Every word of `query` is in one of `fields`; an empty query matches. */
export function matchesAllWords(fields: readonly (string | null | undefined)[], query: string): boolean {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const hay = fields.filter((f): f is string => !!f).map(fold);
  return words.every((w) => hay.some((h) => h.includes(w)));
}

/** The Sessions list's search: [`sessionSearchFields`] under the
 *  every-word rule. `projectLabel` (`owner/repo`) lets a query name the
 *  project and the session at once ("api login"). */
export function sessionMatchesSearch(s: SessionRow, query: string, projectLabel?: string | null): boolean {
  return matchesAllWords(sessionSearchFields(s, projectLabel), query);
}
