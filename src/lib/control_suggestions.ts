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
//   worth restoring;
// - finished sessions the tidy-up report lists (done or PR merged, and
//   idle): review them in the tidy sheet, or ask Control to `/tidy`.
//
// A card is a suggestion, so it says why it is there and can be dismissed;
// the person's press is what runs anything. A dismissal holds across
// restarts for a week (prefs, as Control's escalations' Later does), and
// with the escalation scope on "Control's threads" a card about a session
// is raised only for Control's own (review w04-14).
import { writable } from 'svelte/store';
import type { SessionRow } from './sessions';
import { lostFolds } from './lost_fold';
import { pruneLater } from './control_escalations';
import { readPref, writePref } from './prefs';
import type { TidyCandidate } from './tidy';

/** The pref dismissals are kept in: `[id, unix seconds]` pairs. */
export const DISMISSED_PREF = 'control.suggestionsDismissed';

type Dismissal = [string, number];
const isDismissals = (v: unknown): v is Dismissal[] =>
  Array.isArray(v) && v.every((e) => Array.isArray(e) && e.length === 2 && typeof e[0] === 'string' && typeof e[1] === 'number');
const nowSecs = () => Math.floor(Date.now() / 1000);
let dismissals: Dismissal[] = pruneLater(readPref<Dismissal[]>(DISMISSED_PREF, [], isDismissals), nowSecs());

/** Cards the person dismissed, kept a week across restarts. */
export const dismissedSuggestions = writable<ReadonlySet<string>>(new Set(dismissals.map(([k]) => k)));

/** Dismiss the card `id` (✕). */
export function dismissSuggestion(id: string): void {
  dismissals = pruneLater([...dismissals.filter(([k]) => k !== id), [id, nowSecs()]], nowSecs());
  writePref(DISMISSED_PREF, dismissals);
  dismissedSuggestions.set(new Set(dismissals.map(([k]) => k)));
}

/** Test seam: nothing dismissed, nothing remembered. */
export function resetSuggestionsForTests(): void {
  dismissals = [];
  writePref(DISMISSED_PREF, dismissals);
  dismissedSuggestions.set(new Set());
}

/** The tidy-up reasons a `/tidy` card is raised for: finished work. */
const TIDY_CARD_REASONS: ReadonlySet<string> = new Set(['done_idle', 'pr_merged_idle']);

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
      kind: 'tidy';
      id: string;
      title: string;
      why: string;
      sessionIds: number[];
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

export interface SuggestionInput {
  /** Control's sessions (`controlSessionIds`): a card about a session is
   *  raised only for these. `null`: the whole fleet. */
  scope?: ReadonlySet<number> | null;
  /** The tidy-up report's candidates (`tidyReport`). */
  tidy?: readonly TidyCandidate[];
}

/** The cards to show, most pressing first: failing CI, then mass losses,
 *  then finished sessions to tidy. */
export function fleetSuggestions(
  rows: readonly SessionRow[],
  dismissed: ReadonlySet<string> = new Set(),
  input: SuggestionInput = {},
): Suggestion[] {
  const scope = input.scope ?? null;
  const out: Suggestion[] = [];
  for (const s of rows) {
    if (s.lost_at != null || !s.pr_url || s.ci_status !== 'failing') continue;
    if (scope && !scope.has(s.id)) continue;
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
  const done = (input.tidy ?? []).filter((c) => TIDY_CARD_REASONS.has(c.reason) && (!scope || scope.has(c.session_id)));
  if (done.length > 0) {
    const ids = done.map((c) => c.session_id).sort((a, b) => a - b);
    const n = ids.length;
    out.push({
      kind: 'tidy',
      id: `tidy:${ids.join(',')}`,
      title: `Tidy ${n} finished session${n === 1 ? '' : 's'}`,
      why: 'done or their PR merged, and idle since',
      sessionIds: ids,
      ask: '/tidy',
    });
  }
  return out.filter((x) => !dismissed.has(x.id)).slice(0, MAX_SUGGESTIONS);
}
