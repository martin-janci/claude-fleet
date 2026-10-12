// Gap plan G3.9: "Suggested from your fleet", the cards above Control's
// composer. Each one is read from the session rows the app already holds
// (no new backend, no model) and offers the action that exists for it:
//
// - a live session whose PR's checks fail: open the session, or ask Control
//   to finish CI (the request is put in the box; nothing is sent until
//   Enter);
// - a host that lost several restorable sessions at once (the sidebar's
//   fold, `lost_fold.ts`): open the host, whose "Restore n lost sessions"
//   is the restore with its dry run and confirm, or ask Control which are
//   worth restoring.
//
// A card is a suggestion, so it says why it is there and can be dismissed;
// the person's press is what runs anything.
import { writable } from 'svelte/store';
import type { SessionRow } from './sessions';
import { lostFolds } from './lost_fold';

/** Cards the person dismissed, for this run of the app. */
export const dismissedSuggestions = writable<ReadonlySet<string>>(new Set());

export type Suggestion =
  | {
      kind: 'ci';
      id: string;
      title: string;
      why: string;
      sessionId: number;
      label: string;
      /** What "Ask Control" puts in the box. */
      ask: string;
    }
  | {
      kind: 'restore';
      id: string;
      title: string;
      why: string;
      host: string;
      sessionIds: number[];
      ask: string;
    };

export const MAX_SUGGESTIONS = 3;

function labelOf(s: SessionRow): string {
  return s.friendly_name || s.tmux_name;
}

/** `#478` from a PR URL, else the URL. */
export function prShort(url: string): string {
  const m = /\/pull\/(\d+)/.exec(url);
  return m ? `#${m[1]}` : url;
}

/** The cards to show, most pressing first: failing CI, then mass losses. */
export function fleetSuggestions(rows: readonly SessionRow[], dismissed: ReadonlySet<string> = new Set()): Suggestion[] {
  const out: Suggestion[] = [];
  for (const s of rows) {
    if (s.lost_at != null || !s.pr_url || s.ci_status !== 'failing') continue;
    const pr = prShort(s.pr_url);
    const failing = s.pr_evidence?.checks?.failing?.map((c) => c.name).filter(Boolean) ?? [];
    const label = labelOf(s);
    out.push({
      kind: 'ci',
      id: `ci:${s.id}:${s.pr_url}`,
      title: `Finish CI on PR ${pr}`,
      why: failing.length > 0 ? `${failing.slice(0, 2).join(', ')} failing · ${label} on ${s.host_alias}` : `checks failing · ${label} on ${s.host_alias}`,
      sessionId: s.id,
      label,
      ask: `Finish CI on PR ${pr} (${s.pr_url}): its checks fail in ${label} on ${s.host_alias}.`,
    });
  }
  for (const f of lostFolds(rows)) {
    out.push({
      kind: 'restore',
      id: `restore:${f.host}:${f.rows.length}`,
      title: `Restore ${f.rows.length} stopped sessions on ${f.host}`,
      why: 'stopped together, by a reboot or a tmux restart',
      host: f.host,
      sessionIds: f.rows.map((r) => r.id),
      ask: `${f.rows.length} sessions stopped together on ${f.host}. Which are worth restoring, and which should be tidied?`,
    });
  }
  return out.filter((x) => !dismissed.has(x.id)).slice(0, MAX_SUGGESTIONS);
}
