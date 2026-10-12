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
import attentionTable from './attention_states.json';

// ── status vocabulary ──

export function isClaudeStatus(v: unknown): v is ClaudeStatus {
  return typeof v === 'string' && (CLAUDE_STATUSES as readonly string[]).includes(v);
}

export function isStuckKind(v: unknown): v is StuckKind {
  return typeof v === 'string' && (STUCK_KINDS as readonly string[]).includes(v);
}

// The theme's status tokens (app.css), so both themes clear the text floor
// and the desktop reads like the phone: working blue, waiting amber, done
// green. Callers tint with `color-mix`, never by appending hex alpha.
const STATUS_COLOR: Record<ClaudeStatus, string> = {
  working: 'var(--status-working)',
  blocked: 'var(--status-waiting)', // needs input
  completed: 'var(--status-done)',
  failed: 'var(--status-failed)',
  stopped: 'var(--status-idle)', // stopped by hook or user
  idle: 'var(--status-idle)',
};

// The manual's status words (`kit/status.ts::STATUS_WORDS`): one plain word,
// no glyph; the colour carries the tone. A session waiting on the person is
// "Needs you"; one stopped by a hook or the user sits idle like any other.
const STATUS_LABEL: Record<ClaudeStatus, string> = {
  working: 'Working',
  blocked: 'Needs you',
  completed: 'Done',
  failed: 'Failed',
  stopped: 'Idle',
  idle: 'Idle',
};

export function claudeStatusColor(status: ClaudeStatus | null): string {
  return status && isClaudeStatus(status) ? STATUS_COLOR[status] : 'transparent';
}

/** The status word for a `claude_status`; '' for anything else (it reads
 *  any string, so a caller holding a wider state need not narrow it). */
export function claudeStatusLabel(status: ClaudeStatus | string | null): string {
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

/** A stuck session's status: "Failed · press Enter". */
export function stuckStatus(kind: StuckKind | null): string {
  const why = stuckKindLabel(kind);
  return why ? `Failed · ${why}` : 'Failed';
}

/** A session's one status word: stuck, then its claude_status, else Idle
 *  (a pane with no Claude state, or a stopped one, sits idle). */
export function sessionStatusWord(s: Pick<SessionRow, 'stuck_kind' | 'claude_status'>): string {
  if (s.stuck_kind) return stuckStatus(s.stuck_kind);
  return claudeStatusLabel(s.claude_status) || 'Idle';
}

/** Red, always — the stuck chip outranks whatever claude_status says. */
export const STUCK_COLOR = 'var(--status-failed)';

// ── context pressure ──

export type ContextLevel = 'ok' | 'warn' | 'crit';

/** The hub's one context threshold (`health.context_red_pct`, sent as
 *  `fleet_health.context_red_pct`). 85 until the first health read; a hub
 *  too old to send it (0 / undefined) leaves it alone. */
let contextRedPct = 85;
/** `warn` starts this many points below `crit`. */
const CONTEXT_WARN_MARGIN = 15;

export function setContextRedPct(pct: number | undefined): void {
  if (typeof pct === 'number' && Number.isFinite(pct) && pct > 0 && pct <= 100) contextRedPct = pct;
}

export function contextLevel(pct: number | null): ContextLevel | null {
  if (pct === null || !Number.isFinite(pct)) return null;
  if (pct >= contextRedPct) return 'crit';
  if (pct >= contextRedPct - CONTEXT_WARN_MARGIN) return 'warn';
  return 'ok';
}

/** A CSS colour for the context meter. Every level uses the theme tokens
 *  shared with the usage bars (`--usage-ok` / `--usage-warn` / `--usage-crit`
 *  in app.css), so they keep their contrast in both themes. */
export function contextColor(level: ContextLevel | null): string {
  switch (level) {
    case 'crit':
      return 'var(--usage-crit)';
    case 'warn':
      return 'var(--usage-warn)';
    case 'ok':
      return 'var(--usage-ok)';
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
  /** What the fleet knows beyond the row (step 2.4); none when absent. */
  facts?: AttentionFacts;
}

/** A usage window at its limit. Mirrors `attention::Limit`. */
export interface AttentionLimit {
  window: 'five_hour' | 'weekly';
  resets_at: number | null;
}

/** Mirrors `attention::Facts` (step 2.4): which hosts are down and which
 *  accounts are at a limit or have no usable login. The three Blocked
 *  buckets come from here; `attention_facts.ts` builds it from the stores. */
export interface AttentionFacts {
  down_hosts?: readonly string[];
  limited_accounts?: Readonly<Record<string, AttentionLimit>>;
  uncredentialed_accounts?: readonly string[];
}

/** Triage buckets, most urgent first. `classify()` puts a row in exactly one,
 *  and everything that orders sessions reads this one list — the sidebar's
 *  "Needs you" queue, the project sort below, later the quick switcher and the
 *  digest — so those orderings cannot drift apart.
 *
 *  `waiting` is driven by `claude_status === 'blocked'` (or a pending form)
 *  alone. A2's `waiting_for` will separate a permission prompt from a
 *  question and let the age weighting apply per kind. `done_unread` reads
 *  `last_viewed_at` (redesign 2.3), which `session_viewed.ts` stamps while a
 *  session is on screen. */
export const TRIAGE_BUCKETS = [
  'waiting',
  'stuck',
  'host_down',
  'account_limit',
  'no_credentials',
  'stop_failed',
  'failed',
  'context_full',
  'stale_working',
  'ci_failing',
  'probably_waiting',
  'done_unread',
  'lifecycle',
  'idle_long',
  'working',
  'idle',
] as const;

export type TriageBucket = (typeof TRIAGE_BUCKETS)[number];

/** Buckets the "Needs you" FILTER shows (never `probably_waiting`). `idle_long` is in: the toggle is the
 *  only surface for the operator-configured idle nudge, so leaving it out
 *  would delete that reach and reduce `attentionIdleMinutes` to a sort knob.
 *  `working` and `idle` are never in it. */
export const NEEDS_YOU_BUCKETS: readonly TriageBucket[] = TRIAGE_BUCKETS.filter(
  // G1.6: Jev's "probably waiting" is kept apart from Needs you, in its own
  // Inbox section, so the filter leaves it out as the badge does.
  (b) => b !== 'working' && b !== 'idle' && b !== 'probably_waiting',
);

// ── the seven attention states (redesign step 0.4) ──

/** The one attention model shared with the hub: the twelve buckets fold into
 *  seven states, and only Action required, Failed and Blocked raise the
 *  badge. The table is `attention_states.json`, which
 *  `crates/fleet-core/src/service/attention.rs` checks against its own
 *  `State` and `BUCKET_STATES`; `attention.test.ts` checks it here. */
export type AttentionState =
  | 'action_required'
  | 'failed'
  | 'blocked'
  | 'proposed'
  | 'working'
  | 'paused'
  | 'done'
  | 'idle';

export const ATTENTION_STATES: readonly AttentionState[] = attentionTable.states.map(
  (s) => s.id as AttentionState,
);

const COUNTED_STATES = new Set<AttentionState>(
  attentionTable.states.filter((s) => s.counted).map((s) => s.id as AttentionState),
);

const BUCKET_STATE = new Map<string, AttentionState>(
  attentionTable.buckets.map(([b, st]) => [b, st as AttentionState]),
);

/** The state a triage bucket folds into. */
export function bucketState(bucket: TriageBucket): AttentionState {
  const st = BUCKET_STATE.get(bucket);
  if (!st) throw new Error(`attention_states.json has no row for bucket ${bucket}`);
  return st;
}

/** A row's attention state. */
export function attentionState(s: SessionRow, opts: AttentionOptions): AttentionState {
  return bucketState(classify(s, opts));
}

/** Whether a state raises the Needs you badge. */
export function countsTowardBadge(state: AttentionState): boolean {
  return COUNTED_STATES.has(state);
}

/** Buckets the "Needs you" COUNTER reports: the buckets whose state is
 *  counted (step 0.4). Narrower than the filter: it leaves out `idle_long`,
 *  and `lifecycle` (ghost, lost and pending-kill rows, which nobody can
 *  answer) and `done_unread` (Done) since the seven-state model.
 *
 *  The divergence is intentional, not an oversight. The pill answers "which
 *  sessions need me NOW"; on a fleet of ~60 sessions most are idle, so
 *  counting them would read "Needs you (34)" and the number would stop
 *  meaning anything. The rows are still one toggle away, because the filter
 *  above does include them. Do not "reconcile" these two sets. */
export const NEEDS_YOU_COUNTED_BUCKETS: readonly TriageBucket[] = TRIAGE_BUCKETS.filter((b) =>
  countsTowardBadge(bucketState(b)),
);

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
  return s.claude_status === 'blocked' || s.pending_form != null;
}

/** J2 (step 5.11): what Jev read a silent turn's end as, on a row still idle
 *  after it. Every hook clears `turn_outcome`, so it never outvotes one;
 *  the same rule as the hub's `attention::needs_attention_in`. */
function jevSays(s: SessionRow, outcome: 'asked' | 'stuck'): boolean {
  return isIdleStatus(s.claude_status) && s.turn_outcome === outcome;
}

/** The J2 reading on a row, if any: the row says so (review r15 F17), since
 *  the person, not Jev, decides what to do next. `stuck` puts it in Needs
 *  you; `asked` makes it "probably waiting", kept apart (G1.6). */
export function jevOutcome(s: SessionRow): 'asked' | 'stuck' | null {
  if (jevSays(s, 'asked')) return 'asked';
  if (jevSays(s, 'stuck')) return 'stuck';
  return null;
}

/** A turn ended after the session was last viewed (redesign 2.3, migration
 *  123) — or, for a session fleet started and nobody has opened yet, after
 *  it started. A row reconcile found on a host has neither stamp and is
 *  never unread. Seconds on both sides: a turn that ends in the second it
 *  is viewed counts as seen. */
export function isUnread(s: Pick<SessionRow, 'last_viewed_at' | 'started_at' | 'last_stop_at'>): boolean {
  const seen = s.last_viewed_at ?? s.started_at;
  return seen != null && s.last_stop_at != null && s.last_stop_at > seen;
}

/** Done and unread: the last turn finished (the session is idle) on a live
 *  row nobody has looked at since. */
function isDoneUnread(s: SessionRow): boolean {
  if (s.status === 'ghost' || s.lost_at !== null) return false;
  return isIdleStatus(s.claude_status) && isUnread(s);
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

/** The single classifier: the order of these checks IS the bucket order.
 *  A Claude session running outside fleet entirely (`external`) is read-only
 *  and cannot be acted on from here, so it never lands in a needs-you bucket:
 *  it is `working` or `idle`, whatever its fields say. */
export function classify(s: SessionRow, opts: AttentionOptions): TriageBucket {
  if (s.kind === 'external') return s.claude_status === 'working' ? 'working' : 'idle';
  if (s.kind === 'shell') return 'idle';
  if (isWaiting(s)) return 'waiting';
  if (s.stuck_kind || jevSays(s, 'stuck')) return 'stuck';
  // A dead row keeps its last context reading; a lost one reads `lifecycle`.
  const live = s.status !== 'ghost' && s.lost_at === null;
  // Step 2.4: Blocked on something outside the session. Live rows only: a
  // host's mass loss is one Restore row, not a badge per session.
  const blocked = live ? blockedBy(s, opts) : null;
  if (blocked) return blocked;
  if (s.claude_status === 'failed') return s.kind === 'bg' ? 'failed' : 'stop_failed';
  if (live && contextLevel(s.context_pct) === 'crit') return 'context_full';
  if (live && (s.stale_working_at ?? null) !== null) return 'stale_working';
  if (s.ci_status === 'failing' && isIdleStatus(s.claude_status)) return 'ci_failing';
  // G1.6: Jev's reading of a silent turn's end as a question is a proposal,
  // after every reason a person must act on, and never on a dead row.
  if (live && jevSays(s, 'asked')) return 'probably_waiting';
  if (isDoneUnread(s)) return 'done_unread';
  if (isLifecycleBroken(s)) return 'lifecycle';
  if (isIdleLong(s, opts)) return 'idle_long';
  if (s.claude_status === 'working') return 'working';
  return 'idle';
}

/** The Blocked bucket the fleet's facts put a live row in, if any: its host
 *  is down; or, while it is not working, its account is at a limit that has
 *  not reset yet ("Paused · limit"), or has no usable login. */
function blockedBy(s: SessionRow, opts: AttentionOptions): TriageBucket | null {
  const f = opts.facts;
  if (!f) return null;
  if (f.down_hosts?.includes(s.host_alias)) return 'host_down';
  if (s.claude_status === 'working' || !s.account_uuid) return null;
  const limit = f.limited_accounts?.[s.account_uuid];
  if (limit && (limit.resets_at == null || limit.resets_at > opts.now)) return 'account_limit';
  if (f.uncredentialed_accounts?.includes(s.account_uuid)) return 'no_credentials';
  return null;
}

function isIdleStatus(status: ClaudeStatus | null): boolean {
  return status === 'idle' || status === 'completed' || status === 'stopped';
}

/** When the row entered `bucket`, best effort — the same field per reason as the hub's
 *  `attention::since_for`. */
function bucketSince(s: SessionRow, bucket: TriageBucket): number {
  switch (bucket) {
    case 'stuck':
      return s.stuck_since ?? s.last_activity_at;
    case 'stop_failed':
      return s.last_stop_at ?? s.last_activity_at;
    case 'failed':
      return s.last_activity_at;
    case 'context_full':
      return s.context_at ?? s.last_activity_at;
    case 'stale_working':
      return s.stale_working_at ?? s.last_activity_at;
    case 'ci_failing':
    case 'probably_waiting':
    case 'account_limit':
    case 'no_credentials':
      return s.idle_since ?? s.last_activity_at;
    case 'host_down':
      return s.last_activity_at;
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
 *  keeps severity a pure function of the row with no clock to inject. An
 *  `external` row always sits at the bottom: read-only, never worth floating
 *  a project to the top for. */
export function severity(s: SessionRow): number {
  if (s.kind === 'external') return 0;
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

/** session.id → stuck_kind for every currently-stuck row. `external` rows
 *  are read-only and excluded even if the backend ever set a stuck_kind on
 *  one. */
export function stuckSnapshot(sessions: readonly SessionRow[]): Map<number, StuckKind> {
  const m = new Map<number, StuckKind>();
  for (const s of sessions) if (s.kind !== 'external' && s.stuck_kind) m.set(s.id, s.stuck_kind);
  return m;
}

/** Rows that became stuck (or changed stuck kind) since `prev`. Clearing is
 *  not a transition worth announcing. `external` rows never announce. */
export function newlyStuck(
  prev: ReadonlyMap<number, StuckKind>,
  sessions: readonly SessionRow[],
): SessionRow[] {
  const out: SessionRow[] = [];
  for (const s of sessions) {
    if (s.kind === 'external') continue;
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
      return 'var(--status-done)';
    case 'failing':
      return 'var(--status-failed)';
    case 'pending':
      return 'var(--status-waiting)';
    default:
      return 'transparent';
  }
}
