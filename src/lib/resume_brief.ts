// Review r15 F22: "What changed" on Resume is the person's to edit, and the
// brief carries it. The hub writes the past session's summary into the brief
// under one header line (`handover.rs`, `past_summary`), its lines trimmed and
// blank lines dropped; this swaps that block for the edited text, or drops it
// when the person cleared the field.

/** The header line the hub writes above a past session's summary. */
export const SUMMARY_HEADER = 'Summary of a past session, written after it ended';

/** The summary's lines as the hub writes them: trimmed at the end, no blanks. */
function blockLines(s: string): string[] {
  return s
    .split('\n')
    .map((l) => l.trimEnd())
    .filter((l) => l !== '');
}

/**
 * PURE: `brief` with the summary block that holds `was` replaced by `next`
 * (header and block gone when `next` is blank), or `null` when the brief no
 * longer holds `was` as written (the person edited that part of the brief, or
 * the hub cut it short), so the caller says the brief was left alone.
 */
export function spliceSummary(brief: string, was: string, next: string): string | null {
  const lines = brief.split('\n');
  const old = blockLines(was);
  const h = lines.findIndex((l) => l.startsWith(SUMMARY_HEADER));
  if (h < 0 || old.length === 0) return null;
  const n = old.length;
  for (let i = 0; i < n; i++) if (lines[h + 1 + i] !== old[i]) return null;
  const fresh = blockLines(next);
  const replaced = fresh.length === 0 ? [] : [lines[h], ...fresh];
  return [...lines.slice(0, h), ...replaced, ...lines.slice(h + 1 + n)].join('\n');
}
