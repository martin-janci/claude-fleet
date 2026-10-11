// G7.10: the verdict line of a review run ("approve · no blocking findings ·
// 2 nits", SessionDetails board). Read from what the reviewer wrote last,
// never measured by Fleet: the default review prompt (sessions.ts) asks the reviewer to
// end on a `Verdict:` line, and a run without one shows no line.
import type { Conversation } from './conversation';

export type ReviewVerdictWord = 'approve' | 'approve-with-fixes' | 'needs-rework';

export interface ReviewVerdict {
  verdict: ReviewVerdictWord;
  /** Counts the reviewer gave; null when the line did not say. */
  blocking: number | null;
  nits: number | null;
}

const WORDS: ReviewVerdictWord[] = ['approve-with-fixes', 'needs-rework', 'approve'];

/** The last `Verdict:` line in `text`, or null. Markdown emphasis and
 *  backticks around it are ignored; "approve with fixes" reads as the
 *  hyphenated word. */
export function parseReviewVerdict(text: string): ReviewVerdict | null {
  const lines = text.split('\n').reverse();
  for (const raw of lines) {
    const line = raw.replace(/[*_`>#]/g, '').trim().toLowerCase();
    const m = /^(?:overall\s+)?verdict\s*[:\-–—]\s*(.+)$/.exec(line);
    if (!m) continue;
    const rest = m[1].replace(/\s+/g, ' ');
    const flat = rest.replace(/approve with fixes/g, 'approve-with-fixes').replace(/needs rework/g, 'needs-rework');
    const verdict = WORDS.find((w) => new RegExp(`(^|[^a-z-])${w}($|[^a-z-])`).test(flat));
    if (!verdict) continue;
    const count = (re: RegExp) => {
      const c = re.exec(flat);
      if (!c) return null;
      return c[1] === 'no' ? 0 : Number(c[1]);
    };
    return {
      verdict,
      blocking: count(/(\d+|no) blocking/),
      nits: count(/(\d+|no) nits?\b/),
    };
  }
  return null;
}

/** The reviewer's last written text in `conv`, or null. */
export function lastReplyText(conv: Conversation | null): string | null {
  const turns = conv?.turns ?? [];
  for (let t = turns.length - 1; t >= 0; t--) {
    const items = turns[t].items;
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === 'text' && it.text.trim()) return it.text;
    }
  }
  return null;
}

const VERDICT_WORDS: Record<ReviewVerdictWord, string> = {
  approve: 'approve',
  'approve-with-fixes': 'approve with fixes',
  'needs-rework': 'needs rework',
};

/** "approve with fixes · no blocking findings · 2 nits". */
export function reviewVerdictLine(v: ReviewVerdict): string {
  const parts = [VERDICT_WORDS[v.verdict]];
  if (v.blocking !== null) {
    parts.push(
      v.blocking === 0 ? 'no blocking findings' : `${v.blocking} blocking finding${v.blocking === 1 ? '' : 's'}`,
    );
  }
  if (v.nits !== null) parts.push(v.nits === 0 ? 'no nits' : `${v.nits} nit${v.nits === 1 ? '' : 's'}`);
  return parts.join(' · ');
}

/** The kit tone of a verdict. */
export function reviewVerdictTone(v: ReviewVerdict): 'ok' | 'warn' | 'bad' {
  if (v.verdict === 'needs-rework' || (v.blocking ?? 0) > 0) return 'bad';
  return v.verdict === 'approve-with-fixes' ? 'warn' : 'ok';
}
