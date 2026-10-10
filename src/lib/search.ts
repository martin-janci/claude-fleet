import type { SessionRow } from './sessions';
import { foldedIncludes } from './text_fold';

/**
 * Per-session predicate for the sidebar search box. `needle` is expected
 * already folded (`text_fold.ts`) by the caller (Sidebar.svelte's `matchesSearch`, which
 * also covers the owner/repo part of the query).
 */
export function sessionMatchesSearch(s: SessionRow, needle: string): boolean {
  if (foldedIncludes(s.tmux_name, needle)) return true;
  if (foldedIncludes(s.host_alias, needle)) return true;
  if (foldedIncludes(s.friendly_name, needle)) return true;
  if (foldedIncludes(s.last_prompt, needle)) return true;
  return s.tags?.some((t) => foldedIncludes(t, needle)) ?? false;
}
