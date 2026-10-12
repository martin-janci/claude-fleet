import { afterEach, describe, it, expect } from 'vitest';
import {
  byTriage,
  jevOutcome,
  ciStatusLabel,
  classify,
  claudeStatusColor,
  claudeStatusLabel,
  contextLevel,
  contextColor,
  countNeedsYou,
  displayName,
  formatElapsed,
  isClaudeStatus,
  isUnread,
  isStuckKind,
  needsYou,
  newlyStuck,
  NEEDS_YOU_BUCKETS,
  NEEDS_YOU_COUNTED_BUCKETS,
  promptPreview,
  rank,
  sessionStart,
  setContextRedPct,
  severity,
  TRIAGE_BUCKETS,
  ATTENTION_STATES,
  attentionState,
  bucketState,
  countsTowardBadge,
  stuckKindLabel,
  stuckMessage,
  stuckSnapshot,
  worstSeverityByProject,
  type AttentionFacts,
} from './attention';
import { CLAUDE_STATUSES, STUCK_KINDS, type SessionRow } from './sessions';
import attentionTable from './attention_states.json';

let nextId = 1;
function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    tmux_name: 'dev-x',
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1000,
    last_activity_at: 1000,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  };
}

describe('status vocabulary', () => {
  it('matches the backend enums exactly', () => {
    expect([...CLAUDE_STATUSES]).toEqual(['working', 'blocked', 'completed', 'failed', 'stopped', 'idle']);
    expect([...STUCK_KINDS]).toEqual(['auth_menu', 'reconnect', 'trust_prompt', 'oom', 'press_enter']);
    for (const s of CLAUDE_STATUSES) expect(isClaudeStatus(s)).toBe(true);
    for (const k of STUCK_KINDS) expect(isStuckKind(k)).toBe(true);
    expect(isClaudeStatus('running')).toBe(false);
    expect(isStuckKind('stuck')).toBe(false);
  });

  it('every status has a label and a colour; unknown falls back', () => {
    for (const s of CLAUDE_STATUSES) {
      expect(claudeStatusLabel(s)).not.toBe('');
      expect(claudeStatusColor(s)).not.toBe('transparent');
    }
    expect(claudeStatusLabel(null)).toBe('');
    expect(claudeStatusColor(null)).toBe('transparent');
    for (const k of STUCK_KINDS) expect(stuckKindLabel(k)).not.toBe('');
    expect(stuckKindLabel('press_enter')).toBe('press Enter');
    expect(stuckKindLabel(null)).toBe('');
  });
});

describe('contextLevel', () => {
  afterEach(() => setContextRedPct(85));

  it('reads the hub threshold: red at 85, amber 15 points below', () => {
    expect(contextLevel(null)).toBeNull();
    expect(contextLevel(0)).toBe('ok');
    expect(contextLevel(69.9)).toBe('ok');
    expect(contextLevel(70)).toBe('warn');
    expect(contextLevel(84.9)).toBe('warn');
    expect(contextLevel(85)).toBe('crit');
    expect(contextLevel(100)).toBe('crit');
    expect(contextLevel(Number.NaN)).toBeNull();
  });

  it('follows fleet_health.context_red_pct and ignores an older hub that sends none', () => {
    setContextRedPct(95);
    expect(contextLevel(90)).toBe('warn');
    expect(contextLevel(95)).toBe('crit');
    setContextRedPct(undefined);
    expect(contextLevel(95)).toBe('crit');
    setContextRedPct(0);
    expect(contextLevel(95)).toBe('crit');
  });
});

describe('contextColor', () => {
  it('uses the shared usage theme tokens for warn and crit', () => {
    expect(contextColor('crit')).toBe('var(--usage-crit)');
    expect(contextColor('warn')).toBe('var(--usage-warn)');
    expect(contextColor('ok')).toBe('var(--usage-ok)');
    expect(contextColor(null)).toBe('transparent');
  });
});

describe('severity', () => {
  // Derived from TRIAGE_BUCKETS, so this order is P13's, not the old one:
  // a session waiting on the user now outranks a stuck one.
  it('follows the triage bucket order: waiting > stuck > failed > lifecycle > working > idle', () => {
    const order = [
      row({ claude_status: 'blocked' }),
      row({ stuck_kind: 'press_enter' }),
      row({ claude_status: 'failed' }),
      row({ kind: 'bg', claude_status: 'failed' }),
      row({ context_pct: 90 }),
      row({ claude_status: 'idle', stale_working_at: 5 }),
      row({ claude_status: 'idle', ci_status: 'failing' }),
      row({ status: 'ghost' }),
      row({ claude_status: 'working' }),
      row({ claude_status: 'idle' }),
    ].map(severity);
    for (let i = 1; i < order.length; i++) expect(order[i - 1]).toBeGreaterThan(order[i]);
    // A row with no signals at all ranks with the idle ones, not below them.
    expect(severity(row())).toBe(severity(row({ claude_status: 'idle' })));
  });

  it('external rows sit below everything, even a signal-less row, regardless of status', () => {
    const quiet = severity(row());
    for (const s of [
      row({ kind: 'external', claude_status: 'blocked' }),
      row({ kind: 'external', stuck_kind: 'oom' }),
      row({ kind: 'external', claude_status: 'failed' }),
    ]) {
      expect(severity(s)).toBe(0);
      expect(severity(s)).toBeLessThan(quiet);
    }
  });

  it('worstSeverityByProject takes the max per project and skips orphans', () => {
    const m = worstSeverityByProject([
      row({ project_id: 1, claude_status: 'idle' }),
      row({ project_id: 1, stuck_kind: 'oom' }),
      row({ project_id: 2, claude_status: 'working' }),
      row({ project_id: null, stuck_kind: 'oom' }),
    ]);
    expect(m.get(1)).toBe(severity(row({ stuck_kind: 'oom' })));
    expect(m.get(2)).toBe(severity(row({ claude_status: 'working' })));
    expect(m.size).toBe(2);
  });
});

describe('stuck transitions', () => {
  it('reports rows that became stuck or changed kind, not ones that cleared', () => {
    const a = row({ stuck_kind: 'oom' });
    const b = row({ stuck_kind: null });
    const c = row({ stuck_kind: 'press_enter' });
    const prev = stuckSnapshot([a, row({ id: b.id, stuck_kind: 'oom' }), c]);
    const next = [
      { ...a, stuck_kind: 'auth_menu' as const }, // changed kind ⇒ announced
      b, // cleared ⇒ silent
      c, // unchanged ⇒ silent
      row({ stuck_kind: 'reconnect' }), // new ⇒ announced
    ];
    const fresh = newlyStuck(prev, next);
    expect(fresh.map((r) => r.stuck_kind)).toEqual(['auth_menu', 'reconnect']);
  });

  it('ignores external rows in the stuck map and in new-stuck detection', () => {
    const ext = row({ kind: 'external', stuck_kind: 'oom' });
    expect(stuckSnapshot([ext]).size).toBe(0);
    expect(newlyStuck(new Map(), [ext])).toEqual([]);
  });

  it('stuckMessage uses the friendly name when asked and available', () => {
    const r = row({ tmux_name: 'dev-x', friendly_name: 'Fix login', host_alias: 'mefistos', stuck_kind: 'trust_prompt' });
    expect(stuckMessage(r, true)).toBe('Fix login on mefistos is stuck: trust prompt');
    expect(stuckMessage(r, false)).toBe('dev-x on mefistos is stuck: trust prompt');
    expect(displayName(row({ tmux_name: 'raw' }), true)).toBe('raw');
  });
});

describe('outcome display', () => {
  it('formatElapsed picks the right unit', () => {
    expect(formatElapsed(null, 100)).toBe('—');
    expect(formatElapsed(100, 142)).toBe('42s');
    expect(formatElapsed(0, 5 * 60)).toBe('5m');
    expect(formatElapsed(0, 3 * 3600 + 12 * 60)).toBe('3h 12m');
    expect(formatElapsed(0, 2 * 86400 + 4 * 3600)).toBe('2d 4h');
    expect(formatElapsed(200, 100)).toBe('0s');
  });

  it('sessionStart prefers started_at over created_at', () => {
    expect(sessionStart({ started_at: 5, created_at: 1 })).toBe(5);
    expect(sessionStart({ started_at: null, created_at: 1 })).toBe(1);
  });

  it('promptPreview takes the first non-empty line and ellipsises', () => {
    expect(promptPreview(null)).toBe('');
    expect(promptPreview('\n\n  hello world  \nsecond')).toBe('hello world');
    expect(promptPreview('a'.repeat(80), 10)).toBe('a'.repeat(9) + '…');
  });

  it('ciStatusLabel covers the vocabulary', () => {
    expect(ciStatusLabel('passing')).toContain('CI');
    expect(ciStatusLabel('failing')).toContain('CI');
    expect(ciStatusLabel('pending')).toContain('CI');
    expect(ciStatusLabel(null)).toBe('');
  });
});

describe('triage rank', () => {
  const opts = { idleSecs: 1800, now: 10_000 };

  it('a session waiting on a form is waiting, even while its tool call runs', () => {
    const r = row({ claude_status: 'working', pending_form: { form_id: 'f_a', title: 'T' } });
    expect(classify(r, opts)).toBe('waiting');
  });

  it('classifies every bucket reachable from today\'s fields', () => {
    expect(classify(row({ claude_status: 'blocked' }), opts)).toBe('waiting');
    expect(classify(row({ stuck_kind: 'oom' }), opts)).toBe('stuck');
    expect(classify(row({ claude_status: 'failed' }), opts)).toBe('stop_failed');
    expect(classify(row({ kind: 'bg', claude_status: 'failed' }), opts)).toBe('failed');
    expect(classify(row({ context_pct: 85 }), opts)).toBe('context_full');
    expect(classify(row({ context_pct: 84.9 }), opts)).toBe('idle');
    expect(classify(row({ claude_status: 'idle', stale_working_at: 5 }), opts)).toBe('stale_working');
    expect(classify(row({ claude_status: 'idle', ci_status: 'failing' }), opts)).toBe('ci_failing');
    expect(classify(row({ claude_status: 'working', ci_status: 'failing' }), opts)).toBe('working');
    expect(classify(row({ kind: 'shell', claude_status: 'idle', context_pct: 99 }), opts)).toBe('idle');
    expect(classify(row({ safe_kill_state: 'requested' }), opts)).toBe('lifecycle');
    expect(classify(row({ safe_kill_state: 'failed' }), opts)).toBe('lifecycle');
    expect(classify(row({ status: 'ghost' }), opts)).toBe('lifecycle');
    // A dead row's last context reading or stale stamp is not actionable.
    expect(classify(row({ status: 'ghost', context_pct: 99, stale_working_at: 5 }), opts)).toBe('lifecycle');
    expect(classify(row({ lost_at: 5, context_pct: 99 }), opts)).toBe('lifecycle');
    expect(classify(row({ lost_at: 5 }), opts)).toBe('lifecycle');
    expect(classify(row({ idle_since: 0 }), opts)).toBe('idle_long');
    expect(classify(row({ claude_status: 'working' }), opts)).toBe('working');
    expect(classify(row(), opts)).toBe('idle');
  });

  it('keeps the rules the pill it replaces had', () => {
    // A safe-kill that is merely 'ready' is not a lifecycle problem.
    expect(classify(row({ safe_kill_state: 'ready' }), opts)).toBe('idle');
    // Only work and review sessions get the idle nudge...
    expect(classify(row({ kind: 'shell', idle_since: 0 }), opts)).toBe('idle');
    expect(classify(row({ kind: 'bg', idle_since: 0 }), opts)).toBe('idle');
    // ...a fresh idle stamp is not long enough...
    expect(classify(row({ idle_since: 9_000 }), opts)).toBe('idle');
    // ...and a threshold of 0 disables the rule.
    expect(classify(row({ idle_since: 0 }), { idleSecs: 0, now: 10_000 })).toBe('idle');
  });

  it('done_unread: a finished turn nobody has viewed (redesign 2.3)', () => {
    const done = { claude_status: 'idle' as const, last_stop_at: 9_000 };
    // Viewed before the turn ended, or never viewed since fleet started it.
    expect(classify(row({ ...done, last_viewed_at: 8_000 }), opts)).toBe('done_unread');
    expect(classify(row({ ...done, started_at: 5_000 }), opts)).toBe('done_unread');
    expect(classify(row({ ...done, claude_status: 'completed', last_viewed_at: 1 }), opts)).toBe(
      'done_unread',
    );
    // Viewed since (the same second counts as seen).
    expect(classify(row({ ...done, last_viewed_at: 9_000 }), opts)).not.toBe('done_unread');
    // A row fleet only found on a host has no stamp to compare: never unread.
    expect(classify(row(done), opts)).not.toBe('done_unread');
    // Still working, or a dead row: not "done".
    expect(classify(row({ ...done, claude_status: 'working', last_viewed_at: 1 }), opts)).toBe(
      'working',
    );
    expect(classify(row({ ...done, status: 'ghost', last_viewed_at: 1 }), opts)).toBe('lifecycle');
    // A failing CI is the more urgent reason for the same row.
    expect(classify(row({ ...done, ci_status: 'failing', last_viewed_at: 1 }), opts)).toBe(
      'ci_failing',
    );
  });

  it('isUnread compares the last turn with the last view, else the start', () => {
    expect(isUnread({ last_stop_at: 10, last_viewed_at: 9, started_at: null })).toBe(true);
    expect(isUnread({ last_stop_at: 10, last_viewed_at: 10, started_at: 1 })).toBe(false);
    expect(isUnread({ last_stop_at: 10, last_viewed_at: null, started_at: 5 })).toBe(true);
    expect(isUnread({ last_stop_at: null, last_viewed_at: null, started_at: 5 })).toBe(false);
    expect(isUnread({ last_stop_at: 10, last_viewed_at: undefined, started_at: null })).toBe(false);
  });

  it('needsYou covers every bucket above working, and nothing below', () => {
    expect([...NEEDS_YOU_BUCKETS]).toEqual([...TRIAGE_BUCKETS].slice(0, 14).filter((b) => b !== 'probably_waiting'));
    expect(needsYou(row({ claude_status: 'blocked' }), opts)).toBe(true);
    expect(needsYou(row({ stuck_kind: 'oom' }), opts)).toBe(true);
    expect(needsYou(row({ idle_since: 0 }), opts)).toBe(true);
    expect(needsYou(row({ claude_status: 'working' }), opts)).toBe(false);
    expect(needsYou(row(), opts)).toBe(false);
    expect(
      countNeedsYou([row({ stuck_kind: 'oom' }), row({ claude_status: 'working' }), row({ status: 'ghost' })], opts),
    ).toBe(1);
  });

  // The pill and the filter answer different questions, so they cover
  // different buckets. If this test ever "fails" because the two were made to
  // agree, read NEEDS_YOU_COUNTED_BUCKETS before changing it.
  it('counts narrower than it filters: idle_long and lifecycle are shown, not counted', () => {
    expect([...NEEDS_YOU_COUNTED_BUCKETS]).toEqual(
      [...TRIAGE_BUCKETS].slice(0, 13).filter((b) => b !== 'done_unread' && b !== 'lifecycle' && b !== 'probably_waiting'),
    );
    const idleRows = Array.from({ length: 6 }, () => row({ idle_since: 0 }));
    const blocked = row({ claude_status: 'blocked' });
    const rows = [...idleRows, blocked];
    // A fleet of idle sessions plus one thing actually waiting on the user:
    // the filter surfaces all seven...
    expect(rows.filter((s) => needsYou(s, opts))).toHaveLength(7);
    // ...while the pill reads 1, so idle rows cannot inflate the number.
    expect(countNeedsYou(rows, opts)).toBe(1);
    // And an idle_long row alone is filtered in but never counted.
    expect(needsYou(idleRows[0], opts)).toBe(true);
    expect(countNeedsYou([idleRows[0]], opts)).toBe(0);
  });

  it('orders by bucket first, then by the longest wait', () => {
    const oldStuck = row({ stuck_kind: 'oom', stuck_since: 1_000 });
    const newStuck = row({ stuck_kind: 'oom', stuck_since: 9_000 });
    const blocked = row({ claude_status: 'blocked', idle_since: 9_500 });
    const working = row({ claude_status: 'working' });
    const ordered = byTriage([working, newStuck, blocked, oldStuck], opts);
    expect(ordered.map((s) => s.id)).toEqual([blocked.id, oldStuck.id, newStuck.id, working.id]);
  });

  it('is stable: equal rank falls back to the session id, whatever the input order', () => {
    const a = row({ stuck_kind: 'oom', stuck_since: 1_000 });
    const b = row({ stuck_kind: 'oom', stuck_since: 1_000 });
    expect(byTriage([b, a], opts).map((s) => s.id)).toEqual([a.id, b.id]);
    expect(byTriage([a, b], opts).map((s) => s.id)).toEqual([a.id, b.id]);
  });

  it('caps age so no wait lets a row jump its bucket', () => {
    const ancientWorking = row({ claude_status: 'working', last_activity_at: -5_000_000 });
    const freshStuck = row({ stuck_kind: 'oom', stuck_since: 10_000 });
    expect(rank(freshStuck, opts).score).toBeGreaterThan(rank(ancientWorking, opts).score);
    expect(rank(ancientWorking, opts).ageSecs).toBeLessThan(1_000_000);
  });

  it('never puts an external row in a needs-you bucket, however alarming its fields', () => {
    for (const s of [
      row({ kind: 'external', stuck_kind: 'oom' }),
      row({ kind: 'external', claude_status: 'blocked' }),
      row({ kind: 'external', claude_status: 'failed' }),
      row({ kind: 'external', status: 'ghost' }),
    ]) {
      expect(needsYou(s, opts)).toBe(false);
      expect(classify(s, opts)).toBe('idle');
    }
    expect(classify(row({ kind: 'external', claude_status: 'working' }), opts)).toBe('working');
    expect(countNeedsYou([row({ kind: 'external', stuck_kind: 'oom' }), row({ stuck_kind: 'oom' })], opts)).toBe(1);
  });
});

// Redesign step 0.4: the shared attention table. attention.rs checks the same
// file against its own `State` / `BUCKET_STATES` and runs the same cases.
describe('the seven attention states (shared fixture with attention.rs)', () => {
  const opts = { idleSecs: 0, now: 1000 };

  it('maps every triage bucket, in order, to one of eight states', () => {
    expect(attentionTable.buckets.map(([b]) => b)).toEqual([...TRIAGE_BUCKETS]);
    expect(ATTENTION_STATES).toEqual(['action_required', 'failed', 'blocked', 'proposed', 'working', 'paused', 'done', 'idle']);
    for (const b of TRIAGE_BUCKETS) expect(ATTENTION_STATES).toContain(bucketState(b));
  });

  it('the badge counts only Action required, Failed and Blocked', () => {
    expect(ATTENTION_STATES.filter(countsTowardBadge)).toEqual(['action_required', 'failed', 'blocked']);
  });

  for (const c of attentionTable.cases) {
    it(c.name, () => {
      const r = row({
        claude_status: 'working',
        last_activity_at: 100,
        host_alias: 'alpha',
        ...(c.row as Partial<SessionRow>),
      });
      const o = { ...opts, facts: (c as { facts?: AttentionFacts }).facts };
      expect(classify(r, o)).toBe(c.bucket);
      expect(attentionState(r, o)).toBe(c.state);
      expect(countNeedsYou([r], o)).toBe(c.counted ? 1 : 0);
    });
  }
});

describe('G1.6: Jev\'s "probably waiting"', () => {
  const opts = { idleSecs: 0, now: 1000 };
  it('is its own bucket and state, out of the Needs you filter and the count', () => {
    const r = row({ claude_status: 'idle', turn_outcome: 'asked', host_alias: 'alpha' });
    expect(classify(r, opts)).toBe('probably_waiting');
    expect(attentionState(r, opts)).toBe('proposed');
    expect(needsYou(r, opts)).toBe(false);
    expect(countNeedsYou([r], opts)).toBe(0);
    // Jev's reading still says who read it.
    expect(jevOutcome(r)).toBe('asked');
  });
});
