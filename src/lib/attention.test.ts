import { describe, it, expect } from 'vitest';
import {
  attentionReason,
  ciStatusLabel,
  claudeStatusColor,
  claudeStatusLabel,
  contextLevel,
  displayName,
  formatElapsed,
  isClaudeStatus,
  isStuckKind,
  needsAttention,
  newlyStuck,
  promptPreview,
  sessionStart,
  severity,
  stuckKindLabel,
  stuckMessage,
  stuckSnapshot,
  worstSeverityByProject,
} from './attention';
import { CLAUDE_STATUSES, STUCK_KINDS, type SessionRow } from './sessions';

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
    ci_status: null,
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
  it('is amber at 70 and red at 90', () => {
    expect(contextLevel(null)).toBeNull();
    expect(contextLevel(0)).toBe('ok');
    expect(contextLevel(69.9)).toBe('ok');
    expect(contextLevel(70)).toBe('warn');
    expect(contextLevel(89.9)).toBe('warn');
    expect(contextLevel(90)).toBe('crit');
    expect(contextLevel(100)).toBe('crit');
    expect(contextLevel(Number.NaN)).toBeNull();
  });
});

describe('attentionReason', () => {
  const opts = { idleSecs: 1800, now: 10_000 };

  it('returns null for a healthy working session', () => {
    expect(attentionReason(row({ claude_status: 'working' }), opts)).toBeNull();
    expect(needsAttention(row({ claude_status: 'working' }), opts)).toBe(false);
  });

  it('ranks stuck above everything else', () => {
    const r = row({ stuck_kind: 'oom', safe_kill_state: 'failed', status: 'ghost' });
    expect(attentionReason(r, opts)).toBe('stuck');
  });

  it('flags safe-kill pending/failed, ghosts and failed agents', () => {
    expect(attentionReason(row({ safe_kill_state: 'requested' }), opts)).toBe('safe_kill');
    expect(attentionReason(row({ safe_kill_state: 'failed' }), opts)).toBe('safe_kill');
    expect(attentionReason(row({ safe_kill_state: 'ready' }), opts)).toBeNull();
    expect(attentionReason(row({ status: 'ghost' }), opts)).toBe('ghost');
    expect(attentionReason(row({ lost_at: 5 }), opts)).toBe('ghost');
    expect(attentionReason(row({ claude_status: 'failed' }), opts)).toBe('failed');
  });

  it('flags work sessions idle past the threshold only', () => {
    const idle = row({ claude_status: 'idle', idle_since: 10_000 - 1800 });
    expect(attentionReason(idle, opts)).toBe('idle');
    const fresh = row({ claude_status: 'idle', idle_since: 10_000 - 1799 });
    expect(attentionReason(fresh, opts)).toBeNull();
    // Shell / bg sessions are not nudged for being idle.
    expect(attentionReason(row({ kind: 'shell', idle_since: 0 }), opts)).toBeNull();
    expect(attentionReason(row({ kind: 'bg', idle_since: 0 }), opts)).toBeNull();
    // Threshold 0 disables the rule.
    expect(attentionReason(idle, { idleSecs: 0, now: 10_000 })).toBeNull();
    // No idle stamp ⇒ not idle.
    expect(attentionReason(row({ claude_status: 'idle' }), opts)).toBeNull();
  });
});

describe('severity', () => {
  it('orders stuck > blocked > lost > failed > working > idle > unknown', () => {
    const order = [
      row({ stuck_kind: 'press_enter' }),
      row({ claude_status: 'blocked' }),
      row({ status: 'ghost' }),
      row({ claude_status: 'failed' }),
      row({ claude_status: 'working' }),
      row({ claude_status: 'idle' }),
      row(),
    ].map(severity);
    for (let i = 1; i < order.length; i++) expect(order[i - 1]).toBeGreaterThan(order[i]);
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
