import type { SessionRow } from './sessions';

/**
 * Per-session predicate for the sidebar search box. `needle` is expected
 * already lower-cased by the caller (Sidebar.svelte's `matchesSearch`, which
 * also covers the owner/repo part of the query).
 */
export function sessionMatchesSearch(s: SessionRow, needle: string): boolean {
  if (s.tmux_name.toLowerCase().includes(needle)) return true;
  if (s.host_alias.toLowerCase().includes(needle)) return true;
  if (s.friendly_name?.toLowerCase().includes(needle)) return true;
  if (s.last_prompt?.toLowerCase().includes(needle)) return true;
  return s.tags?.some((t) => t.toLowerCase().includes(needle)) ?? false;
}
