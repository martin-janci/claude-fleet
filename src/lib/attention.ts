// Pure triage helpers for the sidebar, the details pane and the attention
// strip: status vocabulary → label/colour, context-pressure levels, the
// "needs attention" predicate, per-row severity for sorting projects by their
// worst child, stuck-transition detection between two store snapshots, and
// the small display formatters for the outcome fields (elapsed, last prompt).
//
// Everything here is side-effect free so it is unit-testable without
// mounting a component, and so Sidebar.svelte can run each helper ONCE per
// store change inside a $derived rather than per row.
import {
  CLAUDE_STATUSES,
  STUCK_KINDS,
  type CiStatus,
  type ClaudeStatus,
  type SessionRow,
  type StuckKind,
} from './sessions';

// ── status vocabulary ──

export function isClaudeStatus(v: unknown): v is ClaudeStatus {
  return typeof v === 'string' && (CLAUDE_STATUSES as readonly string[]).includes(v);
}

export function isStuckKind(v: unknown): v is StuckKind {
  return typeof v === 'string' && (STUCK_KINDS as readonly string[]).includes(v);
}

const STATUS_COLOR: Record<ClaudeStatus, string> = {
  working: '#50c86e', // green — active
  blocked: '#f0b429', // yellow — needs input
  completed: '#6c8ebf', // blue — done
  failed: '#e64a4a', // red
  stopped: '#888', // grey — stopped by hook or user
  idle: '#888', // grey
};

const STATUS_LABEL: Record<ClaudeStatus, string> = {
  working: '⚡ working',
  blocked: '⏸ blocked',
  completed: '✓ done',
  failed: '✗ failed',
  stopped: '■ stopped',
  idle: '· idle',
};

export function claudeStatusColor(status: ClaudeStatus | null): string {
  return status && isClaudeStatus(status) ? STATUS_COLOR[status] : 'transparent';
}

export function claudeStatusLabel(status: ClaudeStatus | null): string {
  return status && isClaudeStatus(status) ? STATUS_LABEL[status] : '';
}

const STUCK_LABEL: Record<StuckKind, string> = {
  auth_menu: 'auth menu',
  reconnect: 'reconnecting',
  trust_prompt: 'trust prompt',
  oom: 'out of memory',
  press_enter: 'press Enter',
};

/** Human label for a stuck kind, e.g. `press_enter` → "press Enter". */
export function stuckKindLabel(kind: StuckKind | null): string {
  return kind && isStuckKind(kind) ? STUCK_LABEL[kind] : '';
}

/** Red, always — the stuck chip outranks whatever claude_status says. */
export const STUCK_COLOR = '#e64a4a';

// ── context pressure ──

export type ContextLevel = 'ok' | 'warn' | 'crit';

export const CONTEXT_WARN_PCT = 70;
export const CONTEXT_CRIT_PCT = 90;

export function contextLevel(pct: number | null): ContextLevel | null {
  if (pct === null || !Number.isFinite(pct)) return null;
  if (pct >= CONTEXT_CRIT_PCT) return 'crit';
  if (pct >= CONTEXT_WARN_PCT) return 'warn';
  return 'ok';
}

export function contextColor(level: ContextLevel | null): string {
  switch (level) {
    case 'crit':
      return '#e64a4a';
    case 'warn':
      return '#d29b4a';
    case 'ok':
      return '#50c86e';
    default:
      return 'transparent';
  }
}

// ── needs attention ──

export interface AttentionOptions {
  /** Work sessions idle at least this long (seconds) need a nudge. 0 = off. */
  idleSecs: number;
  /** Unix seconds "now" (injected so tests are deterministic). */
  now: number;
}

export const DEFAULT_ATTENTION_IDLE_MINUTES = 30;

export type AttentionReason = 'stuck' | 'safe_kill' | 'ghost' | 'failed' | 'idle';

/** Why a row needs the operator, or null when it does not. Checked in
 *  priority order so the strongest reason wins. */
export function attentionReason(s: SessionRow, opts: AttentionOptions): AttentionReason | null {
  if (s.stuck_kind) return 'stuck';
  if (s.safe_kill_state === 'failed' || s.safe_kill_state === 'requested') return 'safe_kill';
  if (s.status === 'ghost' || s.lost_at !== null) return 'ghost';
  if (s.claude_status === 'failed') return 'failed';
  if (opts.idleSecs > 0 && (s.kind === 'work' || s.kind === 'review') && s.idle_since !== null) {
    if (opts.now - s.idle_since >= opts.idleSecs) return 'idle';
  }
  return null;
}

export function needsAttention(s: SessionRow, opts: AttentionOptions): boolean {
  return attentionReason(s, opts) !== null;
}

// ── severity (for sorting projects by worst child) ──

/** Higher = worse. stuck > blocked > lost > failed > working > idle > rest. */
export function severity(s: SessionRow): number {
  if (s.stuck_kind) return 6;
  if (s.claude_status === 'blocked') return 5;
  if (s.status === 'ghost' || s.lost_at !== null) return 4;
  if (s.claude_status === 'failed' || s.safe_kill_state === 'failed') return 3;
  if (s.claude_status === 'working') return 2;
  if (s.claude_status === 'idle' || s.claude_status === 'completed' || s.claude_status === 'stopped')
    return 1;
  return 0;
}

/** project_id → max severity over its sessions. */
export function worstSeverityByProject(sessions: readonly SessionRow[]): Map<number, number> {
  const out = new Map<number, number>();
  for (const s of sessions) {
    if (s.project_id == null) continue;
    const sev = severity(s);
    if (sev > (out.get(s.project_id) ?? -1)) out.set(s.project_id, sev);
  }
  return out;
}

// ── stuck transitions ──

/** session.id → stuck_kind for every currently-stuck row. */
export function stuckSnapshot(sessions: readonly SessionRow[]): Map<number, StuckKind> {
  const m = new Map<number, StuckKind>();
  for (const s of sessions) if (s.stuck_kind) m.set(s.id, s.stuck_kind);
  return m;
}

/** Rows that became stuck (or changed stuck kind) since `prev`. Clearing is
 *  not a transition worth announcing. */
export function newlyStuck(
  prev: ReadonlyMap<number, StuckKind>,
  sessions: readonly SessionRow[],
): SessionRow[] {
  const out: SessionRow[] = [];
  for (const s of sessions) {
    if (!s.stuck_kind) continue;
    if (prev.get(s.id) !== s.stuck_kind) out.push(s);
  }
  return out;
}

/** The label a row shows in the sidebar / notifications. */
export function displayName(s: SessionRow, friendly: boolean): string {
  return friendly && s.friendly_name ? s.friendly_name : s.tmux_name;
}

/** One-line announcement for a stuck transition. */
export function stuckMessage(s: SessionRow, friendly = true): string {
  return `${displayName(s, friendly)} on ${s.host_alias} is stuck: ${stuckKindLabel(s.stuck_kind)}`;
}

// ── outcome display ──

/** Compact elapsed duration: "42s", "5m", "3h 12m", "2d 4h". */
export function formatElapsed(fromUnix: number | null, nowUnix: number): string {
  if (fromUnix === null) return '—';
  const secs = Math.max(0, nowUnix - fromUnix);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ${mins % 60}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

/** The instant a session's clock starts: fleet's `started_at`, else tmux's
 *  `created_at`. */
export function sessionStart(s: Pick<SessionRow, 'started_at' | 'created_at'>): number {
  return s.started_at ?? s.created_at;
}

/** First line of the last prompt, ellipsised to `max` chars, for a row's
 *  secondary text. */
export function promptPreview(prompt: string | null, max = 60): string {
  if (!prompt) return '';
  const line = prompt.split('\n').find((l) => l.trim().length > 0)?.trim() ?? '';
  const chars = Array.from(line);
  return chars.length > max ? chars.slice(0, max - 1).join('') + '…' : line;
}

export function ciStatusLabel(status: CiStatus | null): string {
  switch (status) {
    case 'passing':
      return '✓ CI';
    case 'failing':
      return '✗ CI';
    case 'pending':
      return '… CI';
    default:
      return '';
  }
}

export function ciStatusColor(status: CiStatus | null): string {
  switch (status) {
    case 'passing':
      return '#50c86e';
    case 'failing':
      return '#e64a4a';
    case 'pending':
      return '#d29b4a';
    default:
      return 'transparent';
  }
}
