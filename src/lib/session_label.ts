// The Label field of "Rename and label" (Orbit Fleet gap plan step G2.7,
// the FormsSession board): a short word that groups sessions, e.g.
// "release". Labels are the session's tags (`set_session_tags`, rule 6 of
// the gap plan: labels use session tags), so the field edits the whole list
// as words separated by spaces or commas. The rule mirrors
// `service::sessions::normalize_session_tags`; the backend checks again.

export const MAX_TAGS = 16;
export const MAX_TAG_CHARS = 32;
const TAG_RE = /^[A-Za-z0-9_.:-]+$/;

export interface ParsedLabel {
  tags: string[];
  /** Why the text cannot be saved; null when it can. */
  error: string | null;
}

/** "release, wip" → ["release", "wip"]: split, trimmed, de-duplicated in
 *  order. Empty text is no label at all. */
export function parseLabel(text: string): ParsedLabel {
  const words = text.split(/[\s,]+/).filter((w) => w !== '');
  const tags: string[] = [];
  for (const w of words) if (!tags.includes(w)) tags.push(w);
  if (tags.length > MAX_TAGS) return { tags, error: `At most ${MAX_TAGS} labels per session.` };
  for (const t of tags) {
    if (t.length > MAX_TAG_CHARS) return { tags, error: `"${t.slice(0, 12)}…" is longer than ${MAX_TAG_CHARS} characters.` };
    if (!TAG_RE.test(t)) return { tags, error: `"${t}" may only use letters, digits and _ . : -` };
  }
  return { tags, error: null };
}

/** How a tag list reads in the field. */
export function labelText(tags: readonly string[] | null | undefined): string {
  return (tags ?? []).join(' ');
}

export function sameTags(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((t, i) => t === b[i]);
}
