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

// ── triage ranking (P13) ──

export interface AttentionOptions {
  /** Work sessions idle at least this long (seconds) need a nudge. 0 = off. */
  idleSecs: number;
  /** Unix seconds "now" (injected so tests are deterministic). */
  now: number;
}

export const DEFAULT_ATTENTION_IDLE_MINUTES = 30;

/** Triage buckets, most urgent first. `classify()` puts a row in exactly one,
 *  and everything that orders sessions reads this one list — the sidebar's
 *  "Needs you" queue, the project sort below, later the quick switcher and the
 *  digest — so those orderings cannot drift apart.
 *
 *  Two buckets are reachable but stay empty until Wave 1 A2 lands its columns:
 *   - `waiting` is driven by `claude_status === 'blocked'` alone. A2's
 *     `waiting_for` will separate a permission prompt from a question and let
 *     the age weighting apply per kind.
 *   - `done_unread` needs `last_viewed_at` and its `touch_session_viewed`
 *     writer, so nothing matches it today. */
export const TRIAGE_BUCKETS = [
  'waiting',
  'stuck',
  'failed',
  'done_unread',
  'lifecycle',
  'idle_long',
  'working',
  'idle',
] as const;

export type TriageBucket = (typeof TRIAGE_BUCKETS)[number];

/** Buckets the "Needs you" FILTER shows. `idle_long` is in: the toggle is the
 *  only surface for the operator-configured idle nudge, so leaving it out
 *  would delete that reach and reduce `attentionIdleMinutes` to a sort knob.
 *  `working` and `idle` are never in it. */
export const NEEDS_YOU_BUCKETS: readonly TriageBucket[] = TRIAGE_BUCKETS.slice(0, 6);

/** Buckets the "Needs you" COUNTER reports — deliberately one narrower than
 *  the filter, excluding `idle_long`.
 *
 *  The divergence is intentional, not an oversight. The pill answers "which
 *  sessions need me NOW"; on a fleet of ~60 sessions most are idle, so
 *  counting them would read "Needs you (34)" and the number would stop
 *  meaning anything. The rows are still one toggle away, because the filter
 *  above does include them. Do not "reconcile" these two sets. */
export const NEEDS_YOU_COUNTED_BUCKETS: readonly TriageBucket[] = TRIAGE_BUCKETS.slice(0, 5);

const NEEDS_YOU = new Set<TriageBucket>(NEEDS_YOU_BUCKETS);
const NEEDS_YOU_COUNTED = new Set<TriageBucket>(NEEDS_YOU_COUNTED_BUCKETS);

/** Age is capped so that no wait, however long, lets a row jump its bucket. */
const AGE_CAP_SECS = 1_000_000;

export interface TriageRank {
  bucket: TriageBucket;
  /** Index into TRIAGE_BUCKETS; 0 is the most urgent. */
  order: number;
  /** Seconds spent in this state; 0 when the row carries no usable stamp. */
  ageSecs: number;
  /** Sort weight, higher = more urgent. The bucket dominates, age breaks ties. */
  score: number;
}

/** A2: `waiting_for` will distinguish permission, question and elicitation.
 *  Until then a blocked session is the only thing known to await the user. */
function isWaiting(s: SessionRow): boolean {
  return s.claude_status === 'blocked';
}

/** A2: needs `last_viewed_at`, so nothing is done-unread yet. */
function isDoneUnread(_s: SessionRow): boolean {
  return false;
}

function isLifecycleBroken(s: SessionRow): boolean {
  if (s.safe_kill_state === 'failed' || s.safe_kill_state === 'requested') return true;
  return s.status === 'ghost' || s.lost_at !== null;
}

function isIdleLong(s: SessionRow, opts: AttentionOptions): boolean {
  if (opts.idleSecs <= 0) return false;
  if (s.kind !== 'work' && s.kind !== 'review') return false;
  if (s.idle_since === null) return false;
  return opts.now - s.idle_since >= opts.idleSecs;
}

/** The single classifier: the order of these checks IS the bucket order. */
export function classify(s: SessionRow, opts: AttentionOptions): TriageBucket {
  if (isWaiting(s)) return 'waiting';
  if (s.stuck_kind) return 'stuck';
  if (s.claude_status === 'failed') return 'failed';
  if (isDoneUnread(s)) return 'done_unread';
  if (isLifecycleBroken(s)) return 'lifecycle';
  if (isIdleLong(s, opts)) return 'idle_long';
  if (s.claude_status === 'working') return 'working';
  return 'idle';
}

/** When the row entered the state its bucket describes, best effort. */
function bucketSince(s: SessionRow, bucket: TriageBucket): number {
  switch (bucket) {
    case 'stuck':
      return s.stuck_since ?? s.last_activity_at;
    case 'lifecycle':
      return s.lost_at ?? s.safe_kill_requested_at ?? s.last_activity_at;
    case 'done_unread':
      return s.last_stop_at ?? s.last_turn_at ?? s.last_activity_at;
    case 'working':
      return s.last_activity_at;
    default:
      return s.idle_since ?? s.last_activity_at;
  }
}

/** Where a row sits in the triage queue. Pure, and `now` is injected, so the
 *  sidebar, the tests and later the digest all agree. */
export function rank(s: SessionRow, opts: AttentionOptions): TriageRank {
  const bucket = classify(s, opts);
  const order = TRIAGE_BUCKETS.indexOf(bucket);
  const ageSecs = Math.min(AGE_CAP_SECS - 1, Math.max(0, opts.now - bucketSince(s, bucket)));
  return { bucket, order, ageSecs, score: (TRIAGE_BUCKETS.length - order) * AGE_CAP_SECS + ageSecs };
}

export function needsYou(s: SessionRow, opts: AttentionOptions): boolean {
  return NEEDS_YOU.has(classify(s, opts));
}

/** How many rows are waiting on the operator right now. Narrower than
 *  `needsYou()` on purpose — see NEEDS_YOU_COUNTED_BUCKETS. */
export function countNeedsYou(rows: readonly SessionRow[], opts: AttentionOptions): number {
  let n = 0;
  for (const s of rows) if (NEEDS_YOU_COUNTED.has(classify(s, opts))) n++;
  return n;
}

/** Rows worst-first: bucket, then the longest wait, then id — so the order is
 *  stable across ticks and never reshuffles under the cursor. */
export function byTriage(rows: readonly SessionRow[], opts: AttentionOptions): SessionRow[] {
  return rows
    .map((s) => ({ s, score: rank(s, opts).score }))
    .sort((a, b) => b.score - a.score || a.s.id - b.s.id)
    .map((x) => x.s);
}

// ── severity (for sorting projects by worst child) ──

/** Higher = worse, derived from the triage buckets so the project tree and the
 *  Needs-you queue can never disagree. Classified with the idle rule off, which
 *  keeps severity a pure function of the row with no clock to inject. */
export function severity(s: SessionRow): number {
  return TRIAGE_BUCKETS.length - TRIAGE_BUCKETS.indexOf(classify(s, { idleSecs: 0, now: 0 }));
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
