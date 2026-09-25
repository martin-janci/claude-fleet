// Work graph M7.3: the tidy-up vocabulary and the pure helpers the sheet,
// the sidebar and the settings dry run share.
import { describe, it, expect } from 'vitest';
import {
  applyItems,
  candidatesFor,
  requestedTicks,
  tidyRequestLive,
  requestedOnly,
  TIDY_REQUEST_TTL_MS,
  autoTidyPreview,
  choicesFor,
  defaultChoice,
  formatIdle,
  groupByReason,
  newlyReopened,
  preselected,
  reopenedBadge,
  reopenedByKey,
  splitArchived,
  tidyEvidence,
  tidyReasonLabel,
  type ReopenedWork,
  type TidyCandidate,
} from './tidy';
import { parseAutoTidyReasons, toggleAutoTidyReason } from './fleet_settings';

const cand = (id: number, over: Partial<TidyCandidate> = {}): TidyCandidate => ({
  session_id: id,
  link_id: 100 + id,
  host_alias: 'h',
  tmux_name: `s${id}`,
  reason: 'done_idle',
  action: 'safe_kill',
  since: 0,
  idle_secs: 5 * 3600,
  ...over,
});

describe('tidy choices', () => {
  it('offers safe kill by default for the kill reasons, plus archive, snooze and never', () => {
    const c = cand(1);
    expect(choicesFor(c)).toEqual(['safe_kill', 'archive', 'snooze', 'never']);
    expect(defaultChoice(c)).toBe('safe_kill');
    expect(preselected(c)).toBe(true);
  });

  it('offers a plain kill only where the backend chose one', () => {
    const dup = cand(2, { reason: 'duplicate_worktree', action: 'kill' });
    expect(choicesFor(dup)).toEqual(['kill', 'archive', 'snooze', 'never']);
    expect(choicesFor(cand(1))).not.toContain('kill');
  });

  it('a lost session is not preselected and never offers a kill', () => {
    const ghost = cand(3, { reason: 'ghost_expiring', action: 'resume_or_expire' });
    expect(choicesFor(ghost)).toEqual(['snooze', 'never']);
    expect(defaultChoice(ghost)).toBe('snooze');
    expect(preselected(ghost)).toBe(false);
  });

  it('an unlinked candidate offers keep instead of snooze, never or archive; an archived one no archive', () => {
    expect(choicesFor(cand(4, { link_id: null, action: 'kill' }))).toEqual(['kill', 'keep']);
    expect(choicesFor(cand(5, { archived: true }))).toEqual(['safe_kill', 'snooze', 'never']);
  });

  it('groups by reason in the backend ranking', () => {
    const g = groupByReason([
      cand(1, { reason: 'duplicate_worktree', action: 'kill' }),
      cand(2, { reason: 'pr_merged_idle' }),
      cand(3),
      cand(4, { reason: 'something_newer' }),
    ]);
    expect(g.map((x) => x.reason)).toEqual([
      'done_idle',
      'pr_merged_idle',
      'duplicate_worktree',
      'something_newer',
    ]);
  });

  it('sends only the ticked rows, with their chosen action', () => {
    const cands = [cand(1), cand(2), cand(3, { link_id: null, action: 'kill', reason: 'duplicate_worktree' })];
    const items = applyItems(cands, new Set([1, 2, 3]), new Map([[2, 'snooze' as const]]));
    expect(items).toEqual([
      { session_id: 1, action: 'safe_kill', link_id: 101 },
      { session_id: 2, action: 'snooze', link_id: 102, days: 7 },
      { session_id: 3, action: 'kill' },
    ]);
    expect(applyItems(cands, new Set(), new Map())).toEqual([]);
  });

  it('formats idle times', () => {
    expect(formatIdle(5 * 3600)).toBe('5 h');
    expect(formatIdle(3 * 86_400)).toBe('3 d');
    expect(formatIdle(30)).toBe('1 min');
  });
});

describe('the dry run and the auto-tidy reasons', () => {
  it('previews only safe kills and archives of the allowed reasons', () => {
    const cands = [
      cand(1),
      cand(2, { reason: 'pr_merged_idle' }),
      cand(3, { reason: 'duplicate_worktree', action: 'kill' }),
    ];
    expect(autoTidyPreview(cands, new Set(['done_idle'])).map((c) => c.session_id)).toEqual([1]);
    expect(
      autoTidyPreview(cands, new Set(['done_idle', 'pr_merged_idle', 'duplicate_worktree'])).map(
        (c) => c.session_id,
      ),
    ).toEqual([1, 2]);
  });

  it('toggles a reason in the comma list, in the backend order', () => {
    expect(toggleAutoTidyReason('done_idle,pr_merged_idle', 'done_idle')).toBe('pr_merged_idle');
    expect(toggleAutoTidyReason('pr_merged_idle', 'not_planned')).toBe('pr_merged_idle,not_planned');
    expect(toggleAutoTidyReason('pr_merged_idle', 'done_idle')).toBe('done_idle,pr_merged_idle');
    expect([...parseAutoTidyReasons(' done_idle , bogus')]).toEqual(['done_idle']);
  });
});

describe('archived and reopened work', () => {
  const row = (id: number, archived: number | null) => ({
    id,
    work: archived === null ? null : { archived_at: archived },
  });

  it('collapses archived sessions, but never one that needs you', () => {
    const rows = [row(1, 10), row(2, null), row(3, 20)];
    const { live, archived } = splitArchived(rows, (r) => r.id === 3);
    expect(live.map((r) => r.id)).toEqual([2, 3]);
    expect(archived.map((r) => r.id)).toEqual([1]);
  });

  it('badges reopened work by key and toasts only what is new', () => {
    const w: ReopenedWork[] = [
      { item_id: 1, key: 'ABC-1', title: 'Login', reopened_at: 5, past_sessions: 2 },
      { item_id: 2, key: null, title: 'Untitled', reopened_at: 6, past_sessions: 1 },
    ];
    const byKey = reopenedByKey(w);
    expect(byKey.size).toBe(1);
    expect(reopenedBadge(byKey.get('ABC-1')!)).toBe('reopened · 2 past sessions');
    expect(reopenedBadge(w[1])).toBe('reopened · 1 past session');
    expect(newlyReopened(new Set([1]), w).map((x) => x.item_id)).toEqual([2]);
  });
});

describe('the sidebar scope (work graph M5)', () => {
  it('keeps only candidates whose session is in the chosen scope', async () => {
    const { inScope } = await import('./tidy');
    const rows = [{ id: 1 }, { id: 2 }] as unknown as import('./sessions').SessionRow[];
    const scopeOf = (r: { id: number }) => (r.id === 1 ? 'org:1' : 'org:2');
    const cands = [cand(1), cand(2), cand(3)];
    expect(inScope(cands, rows, 'all', scopeOf).map((c) => c.session_id)).toEqual([1, 2, 3]);
    expect(inScope(cands, rows, 'org:1', scopeOf).map((c) => c.session_id)).toEqual([1, 3]);
  });
});

describe('tidy requests (work graph M9)', () => {
  const c = (id: number, action = 'safe_kill', reason = 'done_idle') =>
    ({ session_id: id, host_alias: 'h', tmux_name: `s${id}`, reason, action, since: 0 }) as TidyCandidate;

  it('tick the requested rows it can act on, else the preselection', () => {
    const cands = [c(1), c(2, 'resume_or_expire', 'ghost_expiring'), c(3)];
    expect([...requestedTicks(cands, { sessionIds: [3, 9], at: 0 })]).toEqual([3]);
    expect([...requestedTicks(cands, { sessionIds: [], at: 0 })].sort()).toEqual(
      cands.filter(preselected).map((x) => x.session_id).sort(),
    );
    expect(candidatesFor(cands, [1, 3]).map((x) => x.session_id)).toEqual([1, 3]);
  });

  it('a request lives for a short while only', () => {
    const now = 1_000_000;
    expect(tidyRequestLive({ sessionIds: [], at: now - TIDY_REQUEST_TTL_MS }, now)).toBe(true);
    expect(tidyRequestLive({ sessionIds: [], at: now - TIDY_REQUEST_TTL_MS - 1 }, now)).toBe(false);
    expect(tidyRequestLive(null, now)).toBe(false);
  });
});

describe('requestedOnly (work graph M10.4)', () => {
  const c = (id: number) => ({ session_id: id, host_alias: 'h', tmux_name: `s${id}`, reason: 'done_idle', action: 'safe_kill', since: 0 }) as TidyCandidate;
  it('narrows to the requested candidates, or shows all', () => {
    const cands = [c(1), c(2), c(3)];
    expect([...(requestedOnly(cands, [3, 1, 9]) ?? [])].sort()).toEqual([1, 3]);
    expect(requestedOnly(cands, [])).toBeNull();
    expect(requestedOnly(cands, [9])).toBeNull();
  });
});

describe('idle_unlinked (work graph M11.3)', () => {
  const NOW = 2_000_000_000;
  const lonely = cand(7, {
    link_id: null,
    reason: 'idle_unlinked',
    action: 'safe_kill',
    since: NOW - 9 * 86_400,
    idle_secs: 12 * 86_400,
  });

  it('is labelled, offers Safe kill and Keep, and is never preselected', () => {
    expect(tidyReasonLabel('idle_unlinked')).toBe('Idle, no work linked');
    expect(choicesFor(lonely)).toEqual(['safe_kill', 'keep']);
    expect(defaultChoice(lonely)).toBe('safe_kill');
    expect(preselected(lonely)).toBe(false);
    expect([...requestedTicks([lonely, cand(1)], { sessionIds: [7, 1], at: 0 })]).toEqual([1]);
  });

  it('shows its evidence: quiet since its last use, no work linked', () => {
    expect(tidyEvidence(lonely, NOW)).toBe('idle 9 d · no work linked');
    expect(tidyEvidence(cand(1), NOW)).toBe('idle 5 h');
  });

  it('keeps for 7 days per session, with no link', () => {
    expect(applyItems([lonely], new Set([7]), new Map([[7, 'keep' as const]]))).toEqual([
      { session_id: 7, action: 'keep', days: 7 },
    ]);
  });

  it('ranks last, and is never in the auto-tidy dry run', () => {
    expect(groupByReason([lonely, cand(1, { reason: 'ghost_expiring' })]).map((g) => g.reason)).toEqual([
      'ghost_expiring',
      'idle_unlinked',
    ]);
    expect(autoTidyPreview([lonely], new Set(['idle_unlinked', 'done_idle']))).toEqual([]);
  });
});
