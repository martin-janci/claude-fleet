import { writable } from 'svelte/store';
import { createRowStore } from './row_store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';
import { readPref, writePref } from './prefs';
import type { DecisionProposal } from './proposals';
import { foldStartProgress, newStartToken, NO_START_STEPS, type StartProgressFrame, type StartSteps } from './start_steps';

/** The `claude_status` vocabulary (pane_intel `ClaudeStatus`). Anything the
 *  backend has not classified arrives as `null`. */
export const CLAUDE_STATUSES = [
  'working',
  'blocked',
  'completed',
  'failed',
  'stopped',
  'idle',
] as const;
export type ClaudeStatus = (typeof CLAUDE_STATUSES)[number];

/** The `stuck_kind` vocabulary (pane_intel `StuckKind`). */
export const STUCK_KINDS = ['auth_menu', 'reconnect', 'trust_prompt', 'oom', 'press_enter'] as const;
export type StuckKind = (typeof STUCK_KINDS)[number];

/** Reduced PR check status populated by reconcile (migration 019). */
export type CiStatus = 'passing' | 'failing' | 'pending';

/** One failing check of a PR, by the name GitHub shows (result evidence). */
export interface FailingCheck {
  name: string;
  url?: string;
}

/** The PR's check rollup, counted. Mirrors Rust `outcome::CheckSummary`. */
export interface CheckSummary {
  total: number;
  pending: number;
  skipped: number;
  failing?: FailingCheck[];
  failing_total: number;
}

/**
 * What the PR probe last read as evidence about a session's PR (migration
 * 082). Mirrors Rust `outcome::PrEvidence`; every optional field absent
 * means "not observed", never "fine".
 */
export interface PrEvidence {
  /** The commit GitHub's checks describe (`headRefOid`). */
  head_oid?: string;
  /** The worktree's HEAD when the probe ran. */
  local_head?: string;
  /** Commits not on the upstream; absent without an upstream. */
  ahead?: number;
  /** Tracked files differ from HEAD; absent when git could not tell. */
  dirty?: boolean;
  draft: boolean;
  review_decision?: string;
  merge_state?: string;
  /** OPEN | CLOSED | MERGED; absent from readings stored before it was added. */
  state?: string;
  checks: CheckSummary;
}

/** `sessions.turn_outcome` (migration 129). */
export type TurnOutcome = 'finished' | 'asked' | 'stuck' | 'working' | 'unsure';

/** `sessions.origin` (migration 124). */
export type SessionOrigin = 'person' | 'operator' | 'mission' | 'background' | 'token' | 'routine';

/** `sessions.agent` (migration 121). */
export type SessionAgent = 'claude' | 'codex' | 'agy' | 'shell';

/** The agent a row runs, reading an older hub's missing field as Claude
 *  Code (a shell row there still says `kind: 'shell'`). */
export function sessionAgent(row: Pick<SessionRow, 'agent' | 'kind'>): SessionAgent {
  // Kind wins for a shell: the desktop reads a hub's rows through the Rust
  // `SessionRow`, whose serde default fills `agent: "claude"` when an older
  // hub sends none, so a shell row would otherwise read as Claude Code.
  if (row.kind === 'shell') return 'shell';
  return row.agent ?? 'claude';
}

export interface SessionRow {
  id: number;
  tmux_name: string;
  host_alias: string;
  project_id: number | null;
  worktree_id: number | null;
  created_at: number;
  last_activity_at: number;
  status: string;
  notes: string | null;
  account_uuid: string | null;
  kind: string;
  reviews_session_id: number | null;
  worktree_key: string | null;
  lost_at: number | null;
  /** Why the row was marked lost: "host_reboot" | "tmux_server_gone" |
   *  "missing" | "killed" | null (never lost, or still live). */
  lost_reason?: string | null;
  // Claude agent fields — null when claude CLI not installed or session not managed by Claude Code
  claude_session_id: string | null;
  claude_status: ClaudeStatus | null;
  effort_level: string | null;
  pr_url: string | null;
  current_activity: string | null;
  // Pane-tail intel (migration 012): context window usage 0..100 and the
  // detected stuck state. Both are authoritative when the pane was observed
  // on the last reconcile pass and preserved otherwise.
  context_pct: number | null;
  stuck_kind: StuckKind | null;
  // Display label set by the in-session agent via the `set_friendly_name`
  // MCP tool. When the sidebar toggle is on, this is shown instead of
  // tmux_name; null falls back to tmux_name.
  friendly_name: string | null;
  // Safe-kill flow (migration 017): values "requested" | "ready" | "failed" | null.
  safe_kill_state: string | null;
  safe_kill_nonce: string | null;
  safe_kill_detail: string | null;
  safe_kill_requested_at: number | null;
  // Lifecycle + outcome fields (migration 019), unix seconds unless noted.
  /** When claude_status last entered idle/completed/stopped; null while working. */
  idle_since: number | null;
  /** When the current stuck_kind episode began; null when not stuck. */
  stuck_since: number | null;
  /** When a stuck playbook last acted on this session. */
  last_playbook_at: number | null;
  /** First 200 chars of the last prompt sent through fleet. */
  last_prompt: string | null;
  /** When fleet created the session (null for tmux-discovered rows). */
  started_at: number | null;
  /** Last Stop hook (turn completed). */
  last_turn_at: number | null;
  ci_status: CiStatus | null;
  /** The PR's evidence (migration 082); absent without a PR or from an older hub. */
  pr_evidence?: PrEvidence | null;
  /** When the probe last observed the PR (unix secs); absent without a PR. */
  pr_checked_at?: number | null;
  // Orchestration fields (migration 020).
  /** Completed turns, bumped by every Stop hook. */
  turn_seq: number;
  /** Unix secs of the last Stop hook. */
  last_stop_at: number | null;
  /** When the tick demoted a stale `working` row to idle (attention `stale_working`); absent from an older hub. */
  stale_working_at?: number | null;
  /** Requester session that dispatched the task this session works on. */
  parent_session_id: number | null;
  /** Labels set via `set_session_tags`; empty when none. */
  tags: string[];
  // Token usage + ESTIMATED cost (migration 025), summed from the Claude
  // transcript by the backend every `usage.interval_secs`. The backend
  // always sends them; they are optional here so rows built client-side
  // (tests, optimistic patches) need not spell out zeros — read them with
  // `?? 0` / the helpers below.
  usage_input_tokens?: number;
  usage_output_tokens?: number;
  usage_cache_write_tokens?: number;
  usage_cache_read_tokens?: number;
  /** Estimated cost in millionths of a USD (built-in per-model price table). */
  usage_cost_micros?: number;
  /** Model of the most recent counted message. */
  usage_model?: string | null;
  /** Unix secs the usage totals last changed. */
  usage_updated_at?: number | null;
  // Current-conversation context (migration 037), flattened on the wire.
  /** Model of the current conversation (SessionStart / transcript). */
  model: string | null;
  /** Prompt size of the latest request: input + cache read + cache write. */
  context_tokens: number | null;
  /** Context window of `model` (200 000 or 1 000 000). */
  context_window: number | null;
  /** Who wrote the context value last. */
  context_source: 'transcript' | 'hook' | 'pane' | null;
  /** Unix secs of the last context write. */
  context_at: number | null;
  /** True after a compaction or resume until the next usage line. */
  context_stale: boolean;
  /** tmux pane id (`%17`) reconcile last saw for this row. */
  tmux_pane_id: string | null;
  /** Bumped by the backend on every write (migration 042); orders a
   *  command's return value against a row event. Absent on rows built
   *  client-side and on rows from a hub older than the column. */
  row_version?: number;
  /** How many UserPromptSubmit hooks the backend has recorded for this row
   *  (migration 042): the composer's "Claude took it" receipt. Absent from
   *  a hub older than the field, which has no receipts to give. */
  prompt_submit_seq?: number;
  // Pane dialog (migration 040): the permission/question dialog a blocked
  // pane is showing, derived alongside current_activity. Null whenever the
  // pane shows no such dialog.
  pending_input: {
    kind: 'permission' | 'input';
    question: string | null;
    /** `checked`: ticked, on a multi-select (absent = false, and from a hub
     *  older than multi-select support). The label never carries the box. */
    options: { n: number; label: string; selected: boolean; checked?: boolean }[];
    /** A multi-select question: a digit TOGGLES an option, `Tab` moves on
     *  with the ticks kept (see `AnswerPrompt.svelte`). Absent = false. */
    multi?: boolean;
    /** What a permission dialog asks to run, from the tool-call line above
     *  it (`Bash(git push …)`). Absent when the pane shows none, and from a
     *  hub older than redesign 5.9. */
    detail?: string;
  } | null;
  // Chat forms (migration 119): the form this session's agent asked and is
  // waiting on. Optional: an older hub sends none.
  pending_form?: { form_id: string; title: string } | null;
  /** The form this session's agent is still writing (`ask { draft }`,
   *  redesign 10.12): the JSON so far, drawn in by ChatWizards. */
  form_draft?: { draft: string; why: string | null; updated_at: number } | null;
  /** The session's primary work link (migration 046), set through the work
   *  commands (`work.ts`). Absent from a hub older than the work graph. */
  work?: SessionWork | null;
  /** Keys the user said this session does NOT work on (sticky "Not this").
   *  Key recognition (`work_keys.ts`) must not show them. */
  work_rejected?: string[];
  /** The session's top link SUGGESTION (work graph M4): a guess nobody has
   *  decided. Never a work group — only `work` groups a session. */
  work_suggested?: SessionWork | null;
  /** The session's org (work graph M5): the most specific org rule, else
   *  its host's org. Absent = unassigned (or a hub older than M5). */
  org_id?: number | null;
  // ── Multi-user M1: who owns the row, and who may be shown it ────────────
  // Both are caller-INDEPENDENT facts about the session, which is the whole
  // reason they are on the row at all: the event bus serialises a bare
  // `SessionRow` with no caller, and `row_store.ts` replaces a held row
  // wholesale, so a per-caller field here would be erased by the next
  // routine `session:updated` (R6-j). What this client may DO with the row
  // is derived from these two plus its own identity, in `access.ts`.
  /** The person who started the session, absent on an `unclaimed` row and on
   *  a hub older than M1. Absent is NOT ownership: `strip_nulls` removes a
   *  null on the way out, so absent and unowned are indistinguishable here
   *  and `access.ts` treats both as "not mine". */
  owner_person_id?: number | null;
  /** `private` — the default for anything a person starts through fleet — or
   *  `unclaimed`, the safe holding state for a row fleet did not create (a
   *  tmux session reconcile found). There is deliberately **no `'org'`** in
   *  M1: team sharing needs memberships and arrives in M2, and a third value
   *  here would be a reader nobody defined. Absent from a hub older than M1. */
  visibility?: 'private' | 'unclaimed';
  /** The credential profile the session runs under (`CLAUDE_CONFIG_DIR` =
   *  `~/.claude-profiles/<name>` on its host, docs/accounts.md); absent =
   *  the host's own login. */
  claude_profile?: string | null;
  /** Which agent runs in the pane (migration 121): Claude Code, Codex, Agy,
   *  or none for a plain shell (`kind: 'shell'`). Absent from a hub older
   *  than the column, whose sessions all run Claude Code. */
  agent?: SessionAgent;
  /** Who or what started the session (migration 124); absent for a row
   *  fleet did not start (found on a host) or one older than the column. */
  origin?: SessionOrigin | null;
  /** What `origin` points at: a person id (`person`), a session id
   *  (`operator`, `background`, `token`), a mission id (`mission`). */
  origin_ref?: string | null;
  /** When a person last looked at the session (migration 125), unix
   *  seconds. A turn that ended after it is unread; absent for a row nobody
   *  has opened since fleet found it. */
  last_viewed_at?: number | null;
  /** What a finished turn came to when hooks said nothing (migration 129,
   *  J2 in step 5.11); a hook event always wins over it. */
  turn_outcome?: TurnOutcome | null;
  /** What a rule, Jev or an LLM proposes about the session, one per
   *  feature (redesign 2.8). Absent when nothing proposes anything. */
  proposals?: DecisionProposal[];
  /** A digest of the versions and ids of the session's live (non-ended)
   *  work links (work graph M14): it moves whenever any of them changes —
   *  added, removed, primary, state — a secondary link too. Absent = 0 (no
   *  live links, or a hub older than the Work view). */
  work_rev?: number;
}

/** `SessionRow.work`: the primary link's summary. `key` is the item's key or
 *  the link's own reference; null for an item named by title only. */
export interface SessionWork {
  link_id: number;
  item_id: number | null;
  key: string | null;
  title: string;
  /** `manual` | `started` | `agent` | `agent_started` … — tolerant: a newer hub may add more. */
  source: string;
  /** `tracker` | `local` | `ref` (native item status): which kind of task
   *  this is. Empty for a hub older than this field — do not read that as
   *  `ref`. */
  kind?: string;
  /** The tracker item's status (work graph M3); absent for a bare key, a
   *  local item, or a hub older than M3. `todo` | `in_progress` | `done`.
   *
   *  Deliberately NOT the live status (native item status task 4, fix
   *  round 2): `isLocal`-style checks elsewhere derive "this is a local
   *  item, not a ticket" from this being absent, so a local item's real
   *  status must not appear here — see `effective_status`. */
  status_category?: string | null;
  /** The item's status with the live precedence applied (design
   *  2026-09-28 §2): a person's setting or a stamped `done` is final;
   *  otherwise a confirmed link whose session is presently working lifts a
   *  LOCAL item to `in_progress`; otherwise the stored value — for a
   *  tracker item and a local item alike, unlike `status_category` above.
   *  Prefer this for display (a status chip, a filter, "is it stale");
   *  `status_category`'s only remaining job is "is this a ticket". Absent
   *  for a bare key, or a hub older than this field. */
  effective_status?: string | null;
  /** The tracker's own status name ("In Review"). */
  status_name?: string | null;
  url?: string | null;
  /** The tracker no longer answers for the item (deleted or not visible). */
  unavailable?: boolean;
  /** Work graph M4: `confirmed` | `suggested` (absent from older hubs). */
  state?: string;
  /** `explicit` | `strong` | `weak`. */
  strength?: string | null;
  /** The detection rule that made it (`R3`, `R5` …). */
  rule?: string | null;
  /** A suggestion shown pre-selected. */
  preselected?: boolean;
  /** Live suggestions still to decide. */
  suggestions?: number;
  /** The link's org (work graph M5): its tracker's, else the session's. */
  org_id?: number | null;
  /** Work graph M7: archived from the UI at this unix second — the session
   *  collapses into its group's Done while tmux keeps running. */
  archived_at?: number | null;
}

type UsageFields = Partial<
  Pick<
    SessionRow,
    'usage_input_tokens' | 'usage_output_tokens' | 'usage_cache_write_tokens' | 'usage_cache_read_tokens'
  >
>;

/** Every token counter of a session summed (0 for a row without usage). */
export function sessionUsageTokens(s: UsageFields): number {
  return (
    (s.usage_input_tokens ?? 0) +
    (s.usage_output_tokens ?? 0) +
    (s.usage_cache_write_tokens ?? 0) +
    (s.usage_cache_read_tokens ?? 0)
  );
}

/** Compact token count: 950 → "950", 1_234 → "1.2k", 123_456 → "123k", 4_560_000 → "4.56M". */
export function formatTokens(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n <= 0) return '0';
  if (n < 1_000) return String(Math.round(n));
  // Each step's bound is where its ROUNDED value would reach the next
  // format, so 999_600 reads "1.00M", never "1000k" (or 9_999 "10.0k").
  if (n < 9_950) return `${(n / 1_000).toFixed(1)}k`;
  if (n < 999_500) return `${Math.round(n / 1_000)}k`;
  if (n < 9_995_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n < 999_950_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}

/** Estimated cost from micro-USD: "$0.00", "<$0.01", "$1.23", "$1,234". */
export function formatCostMicros(micros: number | null | undefined): string {
  if (micros == null || !Number.isFinite(micros) || micros <= 0) return '$0.00';
  const usd = micros / 1_000_000;
  if (usd < 0.01) return '<$0.01';
  if (usd < 100) return `$${usd.toFixed(2)}`;
  return `$${Math.round(usd).toLocaleString('en-US')}`;
}

// Monotonic guard: a payload carrying a lower row_version than the row we
// hold is a stale snapshot (a command return value that raced a newer
// `session:updated`). Equal versions still apply. A payload without one is
// never rejected for it — only a KNOWN older version is. Shared by the
// `rows` store's `isStale` option and by `loadSessions`'s own list/event
// reconciliation below, so both use exactly the same rule.
function sessionIsStale(incoming: SessionRow, current: SessionRow): boolean {
  return (
    incoming.row_version !== undefined &&
    current.row_version !== undefined &&
    incoming.row_version < current.row_version
  );
}

/** Human label for a ghost row's `lost_reason`, or null when the reason has
 *  no dedicated wording (e.g. "missing", "killed", or none recorded). */
export function lostReasonLabel(reason: string | null | undefined): string | null {
  switch (reason) {
    case 'host_reboot':
      return 'host rebooted';
    case 'tmux_server_gone':
      return 'tmux server stopped';
    case 'local_disabled':
      return 'local host is off on this hub';
    default:
      return null;
  }
}

const rows = createRowStore<SessionRow, number>({
  key: (s) => s.id,
  // Both the optimistic `removeSession()` and the `session:killed` event
  // delete a row; a `session:updated` still in flight for that id would
  // otherwise re-insert the dead row ("ghost session").
  tombstoneMs: 5000,
  isStale: sessionIsStale,
  // `sessions.id` is an INTEGER PRIMARY KEY without AUTOINCREMENT, so a killed
  // highest id is handed to the next insert. A `session:created` whose row is
  // not the killed one gets past that id's tombstone (`mergeCreatedInto`).
  identity: (s) => `${s.host_alias}\u0000${s.tmux_name}\u0000${s.created_at}`,
});
export const sessions = rows.store;
export const resetTombstonesForTests = rows.resetTombstonesForTests;

/** True once the first successful `list_sessions` has populated the store.
 *  Consumers that react to *transitions* (Attention.svelte) treat everything
 *  before this as baseline, so a launch never replays every already-stuck
 *  row as a fresh alert. */
export const sessionsLoaded = writable<boolean>(false);

/** True once the first `list_sessions` has answered, either way. Until then
 *  the fleet is still arriving (redesign step 3.13: the empty pane shows the
 *  Particle swarm and ⌘K says which hosts it is still hearing from); a
 *  failed first load ends the wait too, so a loader never outlives it. */
export const sessionsAnswered = writable<boolean>(false);

// Sidebar filter — when false, background (`kind === 'bg'`) sessions are
// hidden from the tree. Defaults to true (shown). Persisted across restarts.
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
export const showBgAgents = writable<boolean>(readPref('show-bg-agents', true, isBool));
showBgAgents.subscribe((v) => writePref('show-bg-agents', v));

// Sidebar display toggle — when true, sessions render their agent-set
// `friendly_name` (falling back to `tmux_name` when unset) instead of the raw
// tmux_name. Defaults to true so a newly populated friendly_name is visible
// without the user having to discover the toggle. Persisted across restarts.
export const showFriendlyNames = writable<boolean>(
  readPref('show-friendly-names', true, isBool),
);
showFriendlyNames.subscribe((v) => writePref('show-friendly-names', v));

// Sidebar density toggle — when true, session rows show their second
// (details) line: host, tmux name / worktree, elapsed, badges, last prompt.
export const showRowDetails = writable<boolean>(readPref('rows.details', true, isBool));
showRowDetails.subscribe((v) => writePref('rows.details', v));

// Sidebar grouping — `project` (the tree by repository) or `work` (sessions
// that carry a work key grouped by it first, the rest still under their
// project; see work_keys.ts). Persisted across restarts.
/** Project and work are trees with their own headers; state, host, agent
 *  (redesign step 3.6) and organisation (the Sessions board) are flat
 *  groups, `row_groups.ts`. */
export type SidebarGroupBy = 'project' | 'work' | 'state' | 'host' | 'agent' | 'org';
const isGroupBy = (v: unknown): v is SidebarGroupBy =>
  v === 'project' || v === 'work' || v === 'state' || v === 'host' || v === 'agent' || v === 'org';
export const sidebarGroupBy = writable<SidebarGroupBy>(readPref('sidebar.group', 'project', isGroupBy));
sidebarGroupBy.subscribe((v) => writePref('sidebar.group', v));

// `force: true` (the sidebar Refresh button) makes the backend run a fleet
// reconcile pass now; the default returns stored rows while the last pass is
// within the configured interval, so window-focus reloads stay cheap.
export async function loadSessions(opts: { force?: boolean } = {}): Promise<Result<SessionRow[]>> {
  const token = rows.beginList();
  const r = await invokeCmd<SessionRow[]>('list_sessions', { force: opts.force ?? false });
  sessionsAnswered.set(true);
  if (r.ok) {
    // The list owns ORDER (the backend's `ORDER BY last_activity_at DESC`);
    // events own CONTENT. Rebuilding from the current store's position would
    // freeze every row at wherever it first landed — position has to be
    // taken from the list every time, and content still has to lose to a
    // `session:updated` that raced this call and is strictly newer, and a
    // session created or killed while it was in flight keeps that state.
    rows.applyList(r.value, token);
    sessionsLoaded.set(true);
  }
  return r;
}

export async function killSession(hostAlias: string, name: string): Promise<Result<number>> {
  const r = await invokeCmd<number>('kill_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) removeSession(r.value);
  return r;
}

/** Ask Claude to safely persist all work, then delete the worktree and kill
 *  the tmux session. The command returns the row with `safe_kill_state =
 *  "requested"`; subsequent transitions ("ready" → row will be killed via the
 *  Stop hook; "failed" → user must resolve) arrive via `session:updated`. */
export async function safeKillSession(
  hostAlias: string,
  tmuxName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('safe_kill_session', {
    args: { host_alias: hostAlias, tmux_name: tmuxName },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

export interface DirtyFile {
  status: string;
  path: string;
}

export interface SafeKillInspection {
  has_worktree: boolean;
  worktree_path: string | null;
  branch: string | null;
  upstream: string | null;
  dirty_files: DirtyFile[];
  unpushed_commits: number;
  safe_to_remove: boolean;
  error: string | null;
}

/** Pre-flight: cheap git inspect that drives the safe-remove dialog. */
export async function inspectSafeKill(
  hostAlias: string,
  tmuxName: string,
): Promise<Result<SafeKillInspection>> {
  return invokeCmd<SafeKillInspection>('inspect_safe_kill', {
    args: { host_alias: hostAlias, tmux_name: tmuxName },
  });
}

/** Direct remove. `force=false` errors out if anything is dirty — only safe
 *  when the inspection said `safe_to_remove`. `force=true` is the explicit
 *  "discard local work and kill" path. */
export async function discardKillSession(
  hostAlias: string,
  tmuxName: string,
  force: boolean,
): Promise<Result<number>> {
  const r = await invokeCmd<number>('discard_kill_session', {
    args: { host_alias: hostAlias, tmux_name: tmuxName },
    force,
  });
  if (r.ok) removeSession(r.value);
  return r;
}

export async function renameSession(
  hostAlias: string,
  oldName: string,
  newName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rename_session', {
    args: { host_alias: hostAlias, old_name: oldName, new_name: newName },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/**
 * Set the session's display label (`friendly_name`). An empty or
 * whitespace-only value clears it, so the row falls back to the tmux name.
 * Unlike `renameSession` this never touches tmux and keeps the row id.
 */
export async function setFriendlyName(
  hostAlias: string,
  tmuxName: string,
  friendlyName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('set_session_friendly_name', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, friendly_name: friendlyName },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Replace the session's tags (the Label field, M15 G2.7): the whole
 *  list, empty clears. Validated again by the backend (1–32 characters of
 *  letters, digits, `_ . : -`, at most 16). */
export async function setSessionTags(sessionId: number, tags: readonly string[]): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('set_session_tags', {
    args: { session_id: sessionId, tags: [...tags] },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Link or Not related on the session's `related_session` proposal (M15
 *  G4.3): Link keeps the other session listed as linked, Not related
 *  withdraws it. Answers the session's row. */
export async function decideRelatedSession(sessionId: number, runId: number, linked: boolean): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('decide_related_session', {
    args: { session_id: sessionId, run_id: runId, linked },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Mark the session viewed now (redesign 2.3): its finished turns read as
 *  seen and it leaves the `done_unread` bucket. A watcher's call is refused
 *  by the hub (the stamp is one per row); that is not an error to show. */
export async function touchSessionViewed(sessionId: number): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('touch_session_viewed', {
    args: { session_id: sessionId },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Restart the session in place. With `profile`, resume its conversation
 *  under that credential profile instead (`''` = the host's own login): a
 *  running `claude` cannot change its login, so a switch is a restart. */
export async function restartSession(
  hostAlias: string,
  name: string,
  profile?: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', {
    args: profile === undefined ? { host_alias: hostAlias, name } : { host_alias: hostAlias, name, profile },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Truncate this session's transcript into a new conversation.
 *
 *  `mode: 'rewind'` restarts THIS session on the copy; `'fork'` leaves it
 *  running and starts a new session on the copy. `anchorUuid` is the turn's
 *  `prompt_uuid` for a rewind, and the NEXT later turn's for a fork — `null`
 *  keeps the whole transcript, which is what forking the newest turn means.
 */
export async function rewindConversation(
  sessionId: number,
  mode: 'rewind' | 'fork',
  anchorUuid: string | null,
  newWorktree: string | null = null,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rewind_conversation', {
    args: {
      session_id: sessionId,
      mode,
      anchor_uuid: anchorUuid,
      new_worktree: newWorktree,
    },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** What `repair_session` found and did (mirrors `service::repair::RepairReport`). */
export interface RepairReport {
  session_id: number | null;
  host_alias: string;
  tmux_name: string;
  /** Project root the repair resolved on the host (shows which base path / setting was used). */
  project_root: string;
  /** The verified directory the pane runs in (user-facing form). */
  cwd: string;
  /** The same directory as the host resolves it (`pwd -P`), when known. */
  cwd_physical: string | null;
  /** True when nothing needed doing. */
  healthy: boolean;
  /** Ordered, human-readable actions that were applied. */
  actions: string[];
  warnings: string[];
  /** Automatic check only: the workspace needs an explicit repair (Repair
   *  workspace); nothing git-side was applied. `deferred` says what it would do. */
  needs_explicit_repair: boolean;
  deferred: string[];
  /** `branch_local` | `branch_remote` | `branch_from_base:<start>` when a worktree was (re)created. */
  branch_source: string | null;
  /** `created` | `respawned` when tmux was touched. */
  tmux: string | null;
  tmux_alive: boolean;
  /** The tmux session is confirmed gone (not merely unknown). */
  tmux_dead: boolean;
  /** A live pane's reported working directory no longer exists. */
  tmux_cwd_stale: boolean;
  worktree_row_updated: boolean;
  /** Alive sessions on the same host sharing this workspace. */
  sibling_session_ids: number[];
  /** Set when this worktree's registered directory was found missing: which
   *  conditions of the automatic stale-entry removal held. */
  vanished_guard?: VanishedGuard | null;
}

/** Mirrors `service::repair::VanishedGuard`: an automatic repair may drop the
 *  worktree's own stale registration only when every field is true. */
export interface VanishedGuard {
  dir_absent: boolean;
  parent_exists: boolean;
  same_filesystem: boolean;
  repo_ok: boolean;
  under_root: boolean;
  not_locked: boolean;
  no_other_session: boolean;
  /** No other worktree under the same parent is missing too. */
  siblings_present: boolean;
  /** The parent's dev:inode equals the one recorded while healthy. */
  fingerprint_matches: boolean;
  /** `match` | `mismatch` | `missing` | `stat_failed`. */
  fingerprint_check: string;
}

/** Make the session's directory a healthy git worktree on its branch and its
 *  tmux session run there (recreating tmux when it is gone). A no-op on a
 *  healthy session; the backend emits the row events for anything it fixed,
 *  so nothing is merged here. */
export async function repairSession(
  sessionId: number,
  opts: {
    /** `true` only for the Repair workspace button: an explicit repair that
     *  may unregister a stale entry, adopt a moved checkout, recreate the
     *  branch and respawn a live pane. Default `false`: the automatic check,
     *  which only creates what is confirmed missing. */
    explicit?: boolean;
  } = {},
): Promise<Result<RepairReport>> {
  return invokeCmd<RepairReport>('repair_session', {
    args: { session_id: sessionId, explicit: opts.explicit ?? false },
  });
}

/**
 * A read-only snapshot of a session's tmux pane, as plain text.
 *
 * Added to the desktop for the WATCHER (multi-user M1, R5-e): sharing never
 * confers a terminal, so a person a session is shared with has no `pty_open`
 * and needs some view of the live pane that the hub can actually revoke. This
 * is it — a routed read, enforced on the hub per request, with no SSH of its
 * own from this machine. The owner does not use it: they attach.
 *
 * `scrollback_lines` asks for history above the visible pane; the backend
 * clamps it and caps the reply, prefixing a `[capture_session: showing the
 * last N of M lines …]` note when it had to cut.
 */
export async function captureSession(
  sessionId: number,
  opts: { scrollback_lines?: number; max_lines?: number } = {},
): Promise<Result<string>> {
  const args: { session_id: number; scrollback_lines?: number; max_lines?: number } = {
    session_id: sessionId,
  };
  if (opts.scrollback_lines !== undefined) args.scrollback_lines = opts.scrollback_lines;
  if (opts.max_lines !== undefined) args.max_lines = opts.max_lines;
  return invokeCmd<string>('capture_session', { args });
}

// ── sharing a session (multi-user M1) ───────────────────────────────────────
//
// Three mutations and one read, all four routed to the hub's own tools. They
// live here, beside the other mutation wrappers, because three of them answer
// with the session row and so go through `acceptCommandRow` like every other
// mutation — the optimistic patch that keeps a surface from waiting a reconcile
// tick for its own click.
//
// What they deliberately do NOT patch is the GRANT LIST: that is
// `session_access`'s answer (for the owner, in the Share sheet) and
// `access.ts`'s own map (for the recipient, patched by `grant:changed`), and
// neither of those lives on a row. A grant mutates no `sessions` column, which
// is the whole reason `grant:changed` exists.
//
// `my_grants` is deliberately **not** here: it belongs to `src/lib/access.ts`,
// which owns this client's own identity and grant set and patches no row.

/**
 * One live grant on a session, as `session_access` lists them — the sharer's
 * view of who they have shared with.
 *
 * `level` is read as a plain string on purpose: a level this build does not
 * know must still be *shown* (the owner has to be able to see and revoke it),
 * which is the opposite of `access.ts`'s rule for the client's OWN grants,
 * where an unknown level is dropped so it can never be acted on.
 */
export interface SessionGrant {
  session_id: number;
  /** The recipient: a person, or (org administration phase D) an org. */
  person_id: number | null;
  /** The recipient's name, when the hub sends one; the id is the fallback. */
  person_name?: string | null;
  person_display_name?: string | null;
  /** An org recipient: its members and admins from when it was shared. */
  org_id?: number | null;
  org_name?: string | null;
  /** `watch` | `drive`, tolerantly. */
  level: string;
  /** The person who granted it, and when (unix seconds). */
  granted_by?: number | null;
  granted_at?: number | null;
}

/** Who a share is addressed to: a person's name, or an org's (org
 *  administration phase D — its members and admins from now on). */
export type ShareTo = string | { org: string };

function recipientArgs(to: ShareTo): { person: string } | { person: string; org: string } {
  return typeof to === 'string' ? { person: to } : { person: '', org: to.org };
}

/**
 * Share this session with one person, or an org you are in, at `watch`,
 * `answer` (Orbit Fleet 11.7: also answer its questions) or `drive`.
 *
 * Owner only, enforced on the hub (and in the store, against the row's own
 * `owner_person_id`); the Share sheet's own gate is the UI half of the same
 * rule. An org share reaches the members and admins who are in the org when
 * it is made — never someone who joins later, never a viewer.
 */
export async function shareSession(
  sessionId: number,
  to: ShareTo,
  level: 'watch' | 'answer' | 'drive',
): Promise<Result<SessionRow | null>> {
  const r = await invokeCmd<SessionRow | null>('session_share', {
    args: { session_id: sessionId, ...recipientArgs(to), level },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Revoke one person's (or org's) grant on this session. Owner only. */
export async function unshareSession(
  sessionId: number,
  to: ShareTo,
): Promise<Result<SessionRow | null>> {
  const r = await invokeCmd<SessionRow | null>('session_unshare', {
    args: { session_id: sessionId, ...recipientArgs(to) },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/**
 * Lower one person's grant to `watch`, from `drive` or `answer`.
 *
 * There is deliberately no wrapper that raises one, because there is no tool
 * that raises one: a grant only ever moves downward (spec §4.3 invariant 3),
 * and "re-home the grant to me" is a privacy bypass wearing a grant's clothes.
 * Widening is done by revoking and sharing again, which is a decision the
 * owner takes explicitly.
 */
export async function narrowShare(
  sessionId: number,
  to: ShareTo,
): Promise<Result<SessionRow | null>> {
  const r = await invokeCmd<SessionRow | null>('session_narrow', {
    args: { session_id: sessionId, ...recipientArgs(to) },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/**
 * The live grants on one session — the Share sheet's list.
 *
 * Named `fetchSessionAccess`, not `sessionAccess`: `access.ts` already exports
 * a `sessionAccess`, the pure per-row derivation, and the two are imported
 * into the same components. One is "what may I do with this row", the other is
 * "who has this owner shared it with"; sharing a name between them would be a
 * reader's trap and, in `SessionDetails.svelte`, an actual collision.
 *
 * Answers an array, never null: "shared with nobody" is the ordinary case and
 * an empty list is how it reads.
 */
export async function fetchSessionAccess(sessionId: number): Promise<Result<SessionGrant[]>> {
  const r = await invokeCmd<SessionGrant[] | null>('session_access', {
    args: { session_id: sessionId },
  });
  if (!r.ok) return r;
  return { ok: true, value: r.value ?? [] };
}

export interface NewSessionArgs {
  host_alias: string;
  project_id: number;
  worktree_id: number | null;
  name: string;
  new_worktree?: string | null;
  /** Branch to fork a new worktree from; null/empty = repo default branch. */
  base_branch?: string | null;
  /** "work" (default) runs Claude Code; "shell" runs a plain login shell. */
  kind?: 'work' | 'shell';
  /** Optional command run on start for a shell session (null = bare shell). */
  start_command?: string | null;
  /**
   * Optional user-supplied sidebar label. When omitted or empty, the backend
   * derives one from the branch name so the sidebar never shows the raw
   * `dev-<owner>-<repo>--…` slug.
   */
  friendly_name?: string | null;
  /**
   * Resume this Claude conversation id instead of minting a fresh one (from
   * discover_lost_sessions). Rejected for a shell session and when a session
   * on the host already holds that conversation.
   */
  resume_claude_session_id?: string;
  /** `claude --model` for the first launch (an alias or a model id); null =
   *  the host's default. Rejected for a shell session. */
  model?: string | null;
  /** `claude --effort` for the first launch (low … max); null = the host's
   *  default. Rejected for a shell session. */
  effort?: string | null;
  /** Credential profile for the first launch (`~/.claude-profiles/<name>`
   *  on the host, with its own `/login`); null = the host's login. Rejected
   *  for a shell session. */
  profile?: string | null;
  /** Which agent runs in the pane: `claude` (default), `codex`, or `shell`
   *  (the same as `kind: 'shell'`). `agy` is refused until fleet can launch
   *  it. A Codex session takes no `profile`. */
  agent?: SessionAgent | null;
  /** Opaque id the start reports its steps under (`start:progress`, step
   *  5.13); `newSessionAbortable` mints it. */
  start_token?: string | null;
  /** The person was asked about `accounts.pause_at` (step 4.4) and chose to
   *  start anyway; without it a hub refuses such a start (`E_ACCOUNT_LIMIT`). */
  over_limit_ok?: boolean;
}

/** A ⌘N start whose create command is in flight (redesign step 5.13): the
 *  Pulse sequence's worktree and tmux steps run inside it. */
export interface CreatingStart {
  host_alias: string;
  /** Empty when the backend mints the name. */
  name: string;
  kind: 'work' | 'shell';
  /** The `start_token` the command carries. */
  token: string;
  /** What the backend has reported so far (`start:progress`). */
  steps: StartSteps;
}

export const creatingStart = writable<CreatingStart | null>(null);

/** `start:progress` (step 5.13): move the in-flight start's step, if the
 *  frame is this window's own start. Frames of other starts — another
 *  window's, another device's on the same hub — are ignored. */
export function applyStartProgress(f: StartProgressFrame): void {
  creatingStart.update((c) => (c && c.token === f.token ? { ...c, steps: foldStartProgress(c.steps, f) } : c));
}

/** Rows a create returned before their agent was up; `session_starting.ts`
 *  follows them until it is. */
export const startedIds = writable<ReadonlySet<number>>(new Set());

function markStarting(row: Pick<SessionRow, 'id' | 'kind' | 'claude_status'> | null): void {
  if (!row || row.kind === 'shell' || row.claude_status !== null) return;
  startedIds.update((s) => new Set([...s, row.id]));
}

export async function newSessionAbortable(
  args: NewSessionArgs,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  // The Pulse sequence (5.13) follows the start: the command in flight is
  // its worktree and tmux steps, the row it returns waits on the agent.
  const token = args.start_token || newStartToken();
  creatingStart.set({
    host_alias: args.host_alias,
    name: args.name ?? '',
    kind: args.kind === 'shell' || args.agent === 'shell' ? 'shell' : 'work',
    token,
    steps: NO_START_STEPS,
  });
  let r: Result<SessionRow>;
  try {
    r = await invokeCmdAbortable<SessionRow>('new_session', { args: { ...args, start_token: token } }, signal);
  } finally {
    creatingStart.set(null);
  }
  if (r.ok) {
    acceptCommandRow(r.value);
    markStarting(r.value);
  }
  return r;
}

// ─── identity ────────────────────────────────────────────────────────────────

/** The stable identity of a session. `id` is the primary key; the
 *  host_alias + tmux_name pair is the fallback for rows whose id churned on
 *  re-discovery. A bare tmux_name is NOT an identity — default names are
 *  project-derived, so the same name on two hosts is the normal case. */
export interface SessionIdentity {
  id?: number | null;
  host_alias: string;
  tmux_name: string;
}

/** True when `a` and `b` denote the same session: same id, or (when either
 *  side has no usable id) same host_alias + tmux_name. */
export function sameSession(a: SessionIdentity, b: SessionIdentity): boolean {
  if (a.id != null && b.id != null) return a.id === b.id;
  return a.host_alias === b.host_alias && a.tmux_name === b.tmux_name;
}

/** Locate `ident` in `arr`: by id first, then by the host+name pair. */
export function findSession(arr: SessionRow[], ident: SessionIdentity): SessionRow | undefined {
  if (ident.id != null) {
    const byId = arr.find((s) => s.id === ident.id);
    if (byId) return byId;
  }
  return arr.find((s) => s.host_alias === ident.host_alias && s.tmux_name === ident.tmux_name);
}

export function mergeSession(row: SessionRow): void {
  rows.merge(row);
}

export function removeSession(id: number): void {
  rows.remove(id);
}

/** One backend row event, as delivered by `events.ts`. */
export type SessionEvent =
  | { type: 'created' | 'updated'; row: SessionRow }
  | { type: 'killed'; id: number };

/** Apply a burst of session events in ONE store update. The reconcile tick
 *  emits `session:updated` once per session, so without batching every tick
 *  costs N store flushes (and N sidebar re-derives). Events are applied in
 *  order, so a `killed` after an `updated` for the same id still removes the
 *  row, and an `updated` after a `killed` is dropped by the tombstone. */
export function applySessionEvents(events: readonly SessionEvent[]): void {
  if (events.length === 0) return;
  sessions.update((arr) => {
    let next = arr;
    for (const ev of events) {
      if (ev.type === 'killed') next = rows.removeFrom(next, ev.id);
      else if (ev.type === 'created') next = rows.mergeCreatedInto(next, ev.row);
      else next = rows.mergeInto(next, ev.row);
    }
    return next;
  });
}

/** Apply a row returned by a mutation command (rename/restart/new). Unlike an
 *  event, a command result is the authoritative response to a request the
 *  user just made, so it clears any tombstone for that id before merging —
 *  except a live tombstone for this same row (same host, tmux name and
 *  created_at): a command that resolved after `session:killed` does not
 *  resurrect the killed row (review r06). A new row reusing the id passes. */
export function acceptCommandRow(row: SessionRow | null | undefined): void {
  rows.accept(row);
}

/**
 * Type `prompt` into a session's REPL and submit it.
 *
 * `opts.keys` presses one key instead — `Enter`, `Escape`, `Tab`, `C-c`, or a
 * digit `1`-`9` that picks that option of a `pending_input` dialog (toggles
 * it, on a multi-select). A key is never
 * marked untrusted and is never recorded as a prompt, and `prompt` must be
 * empty alongside it (the backend refuses the pair with `E_VALIDATE`).
 * Answering a dialog has to go this way. The text path pastes through
 * `paste-buffer -p`, and the REPL has bracketed paste on (DECSET 2004), so
 * the pane receives `ESC [ 2 0 0 ~ 3 ESC [ 2 0 1 ~` — the first key a select
 * dialog sees is ESC, which cancels it. `send-keys 3` delivers one raw `3`.
 */
export async function sendPrompt(
  hostAlias: string,
  tmuxName: string,
  prompt: string,
  opts: { keys?: string } = {},
): Promise<Result<void>> {
  return invokeCmd<void>('send_prompt', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, prompt, keys: opts.keys ?? null },
  });
}

/**
 * Press one of a dialog's own keys (a numbered option, Enter, Escape, Tab) —
 * the `answer` tier's one write (Orbit Fleet 11.7). It is `send_prompt` with
 * an empty prompt and a key, which is the shape the hub admits at `answer`
 * (after a fresh read of the pane shows a dialog); any prompt text would make
 * it `drive`. Kept apart from {@link sendPrompt} so the share sweep can hold a
 * key-only write to the `answer` gate and every other `send_prompt` to
 * `drive`.
 */
export async function answerDialog(hostAlias: string, tmuxName: string, key: string): Promise<Result<void>> {
  return invokeCmd<void>('send_prompt', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, prompt: '', keys: key },
  });
}

/** What `queue_prompt` did with one prompt (step 5.10). */
export interface QueuePromptResult {
  session_id: number;
  /** Typed now: the session was idle. */
  delivered: boolean;
  /** Kept until the session is idle; `null` when delivered. */
  queued_id?: number | null;
}

/** A prompt waiting for its session to be idle, or one whose typing failed. */
export interface QueuedPrompt {
  id: number;
  session_id: number;
  body: string;
  created_at: number;
  delivered_at?: number | null;
  attempts?: number;
  failed_at?: number | null;
  error?: string | null;
  cancelled_at?: number | null;
  /** Send later: not typed before this unix second. */
  not_before?: number | null;
  /** Held while the session's account is at its usage limit. */
  until_limit_reset?: boolean;
  /** Dropped instead if the session is archived first. */
  skip_if_archived?: boolean;
  skipped_at?: number | null;
}

/** Send later's time choices (M15 G1.8). Every field is optional: none set
 *  is the plain "when it is idle". */
export interface SendLaterTiming {
  /** Unix seconds before which the prompt is not typed. */
  notBefore?: number;
  /** Wait until the session's account is under its usage limit again. */
  untilLimitReset?: boolean;
  /** Skip it if the session is archived first. */
  skipIfArchived?: boolean;
}

/** Send a prompt as a new turn: now when the session is idle, else once its
 *  turn ends (never into a dialog). `timing` holds it for later. */
export function queuePrompt(
  sessionId: number,
  prompt: string,
  timing: SendLaterTiming = {},
): Promise<Result<QueuePromptResult>> {
  const args: Record<string, unknown> = { session_id: sessionId, prompt };
  // Only what is set goes on the wire, so an older hub reads the call it knows.
  if (timing.notBefore !== undefined) args.not_before = Math.floor(timing.notBefore);
  if (timing.untilLimitReset) args.until_limit_reset = true;
  if (timing.skipIfArchived) args.skip_if_archived = true;
  return invokeCmd<QueuePromptResult>('queue_prompt', { args });
}

export function queuedPrompts(sessionId: number): Promise<Result<QueuedPrompt[]>> {
  return invokeCmd<QueuedPrompt[]>('queued_prompts', { args: { session_id: sessionId } });
}

/** Take back a waiting prompt; answers what is still waiting. */
export function cancelQueuedPrompt(sessionId: number, id: number): Promise<Result<QueuedPrompt[]>> {
  return invokeCmd<QueuedPrompt[]>('cancel_queued_prompt', { args: { session_id: sessionId, id } });
}

export const DEFAULT_REVIEW_PROMPT = `Review the work in this worktree. Run \`git diff\` and \`git log\` against the base branch to see what changed.

Pass 1 — correctness: does the code do what it should? Any bugs?
Pass 2 — code quality: clarity, structure, test coverage.
Pass 3 — risk: anything dangerous, security-sensitive, or destructive?

Cite file:line for every point. End with an overall verdict: approve / approve-with-fixes / needs-rework.`;

export async function spawnReview(
  sourceSessionId: number,
  prompt: string,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  const r = await invokeCmdAbortable<SessionRow>(
    'spawn_review',
    { args: { source_session_id: sourceSessionId, prompt } },
    signal,
  );
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function recreateSession(sessionId: number): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('recreate_session', {
    args: { session_id: sessionId },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Mirrors `service::sessions::restore::RestorePlanEntry`: one planned restore
 *  action. `action` is `"restore"` for a session the batch will attempt to
 *  resume, `"skip"` (with `reason` set) for one an explicit `sessionIds`
 *  request named that cannot be restored. */
export interface RestorePlanEntry {
  session_id: number;
  tmux_name: string | null;
  cwd: string | null;
  claude_session_id: string | null;
  friendly_name: string | null;
  action: 'restore' | 'skip';
  reason: string | null;
}

/** Mirrors `service::sessions::restore::RestoreOutcome`: the result of one
 *  restore attempt. */
export interface RestoreOutcome {
  session_id: number;
  tmux_name: string;
  ok: boolean;
  error: string | null;
}

/** Mirrors `service::sessions::restore::RestoreReport`. */
export interface RestoreReport {
  host_alias: string;
  dry_run: boolean;
  plan: RestorePlanEntry[];
  results: RestoreOutcome[];
}

/** Batch-restore a host's sessions lost to a reboot or a tmux server restart,
 *  over `recreate_session`. Pass `dryRun: true` first to get the plan (no
 *  ssh, no writes); `sessionIds` restricts the batch to those fleet session
 *  ids instead of every lost, resumable session on the host. The backend
 *  emits `session:updated` row events for anything it restores, so nothing
 *  is merged into the sessions store here. */
export async function restoreHostSessions(
  hostAlias: string,
  opts: { dryRun?: boolean; sessionIds?: number[] } = {},
): Promise<Result<RestoreReport>> {
  return invokeCmd<RestoreReport>('restore_host_sessions', {
    args: {
      host_alias: hostAlias,
      dry_run: opts.dryRun ?? false,
      session_ids: opts.sessionIds ?? null,
    },
  });
}

/** Mirrors `service::sessions::discover::LostCandidate`: one transcript the
 *  host has (possibly already held by a fleet row — `existing_session_id`),
 *  ranked and enriched from the store. `rank_hint` is relative to the host's
 *  last boot. `resumable` is true only when `new_session` would start the
 *  pane in exactly `cwd` (a registered worktree or the project root) —
 *  anywhere else `claude --resume` misses the transcript and a new, empty
 *  conversation starts instead. `derived_tmux_name` is set only when
 *  `resumable`, and is a hint for `new_session`'s `name` — it may already be
 *  taken by a second session on the same worktree. Restore a resumable
 *  candidate with `new_session({ hostAlias, projectId, worktreeId, name:
 *  derivedTmuxName, resumeClaudeSessionId: claudeSessionId })`. */
export interface LostCandidate {
  cwd: string;
  git_branch: string | null;
  claude_session_id: string;
  transcript_mtime: number;
  derived_tmux_name: string | null;
  project_id: number | null;
  worktree_id: number | null;
  existing_session_id: number | null;
  rank_hint: 'before_boot' | 'after_boot' | 'stale' | 'unknown';
  resumable: boolean;
}

/** Scan a host's Claude transcripts (`~/.claude/projects`) for lost
 *  conversations (one a fleet row already holds is flagged via
 *  `existing_session_id`) and rank/enrich them from the store. Read-only: no
 *  writes, nothing to merge into the sessions store. `limit` caps how many
 *  transcripts (newest first) are read; omit for the backend default (50). */
export async function discoverLostSessions(
  hostAlias: string,
  limit?: number,
): Promise<Result<LostCandidate[]>> {
  return invokeCmd<LostCandidate[]>('discover_lost_sessions', {
    args: { host_alias: hostAlias, limit: limit ?? null },
  });
}

export async function dismissGhostSession(sessionId: number): Promise<Result<void>> {
  const r = await invokeCmd<void>('dismiss_ghost_session', {
    args: { session_id: sessionId },
  });
  if (r.ok) removeSession(sessionId);
  return r;
}

/** Adopt a live tmux session fleet did not start (`started_at` null): fleet
 *  runs it from now on and the caller owns it when nobody did (Lost and
 *  found, redesign step 4.8). `projectId` adopts it into that project
 *  (Adopt into, step 4.12); omitted keeps the one reconcile found. */
export async function adoptSession(sessionId: number, projectId?: number | null): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('adopt_session', {
    args: projectId == null ? { session_id: sessionId } : { session_id: sessionId, project_id: projectId },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

// ─── background sessions ─────────────────────────────────────────────────────

export interface NewBgSessionResult {
  claude_session_id: string | null;
  /** The fleet row, registered by the post-launch reconcile (MCP-7). Null
   *  when the agent could not be matched yet — it appears on the next tick. */
  session?: SessionRow | null;
  warning?: string | null;
}

/** Launch a supervised Claude background session on `hostAlias`. */
export async function newBgSession(
  hostAlias: string,
  name: string,
  prompt: string,
  /** The session asking for this one; the new row becomes its child. Null
   *  from the desktop dialog — nobody asked for it from inside a session. */
  requesterSessionId: number | null = null,
): Promise<Result<NewBgSessionResult>> {
  const r = await invokeCmd<NewBgSessionResult>('new_bg_session', {
    args: {
      host_alias: hostAlias,
      name,
      prompt,
      requester_session_id: requesterSessionId,
    },
  });
  if (r.ok && r.value?.session) acceptCommandRow(r.value.session);
  return r;
}

/** True for a row with no attached tmux pane: a supervised background agent
 *  (`bg`) or an interactive Claude session running outside fleet entirely
 *  (`external`, e.g. Claude Desktop). Every "no PTY" check in the app should
 *  go through this instead of comparing `kind` directly. */
export function hasNoPane(s: Pick<SessionRow, 'kind'>): boolean {
  return s.kind === 'bg' || s.kind === 'external';
}

/** True for a background agent whose CLI process is gone (backend marks it
 *  `claude_status: 'stopped'` once its transcript has been quiet past
 *  `AGENT_INACTIVE_SECS`). Never true for `external` rows — those leave the
 *  list on their own when the process ends. */
export function isInactiveAgent(s: Pick<SessionRow, 'kind' | 'claude_status'>): boolean {
  return s.kind === 'bg' && s.claude_status === 'stopped';
}

/** Remove a `bg` agent row from the list without touching the underlying
 *  process (it does not use fleet). Refused by the backend for `external`
 *  rows and for a row still `working`. The row itself is removed by the
 *  `session:removed` event the backend emits, not by this call. */
export async function dismissAgentSession(sessionId: number): Promise<Result<null>> {
  return invokeCmd<null>('dismiss_agent_session', {
    args: { session_id: sessionId },
  });
}

/** Outcome of `purge_project` on one host (mirrors Rust `claude_cli::PurgeReport`). */
export interface PurgeReport {
  host_alias: string;
  /** The path fleet recorded from the projects scan. */
  logical_path: string;
  /** `pwd -P` on the target host; null when the directory no longer exists. */
  physical_path: string | null;
  /** Path forms whose Claude state was deleted. */
  purged: string[];
  /** Path forms Claude held no state for (not an error). */
  not_found: string[];
}

/** Delete Claude Code state for a project on every host in `hostAliases`
 *  (under both its physical and logical path forms). The backend removes the
 *  project from the DB only if every host succeeds; one report per host. */
export async function purgeProject(
  hostAliases: string[],
  projectPath: string,
  projectId: number,
): Promise<Result<PurgeReport[]>> {
  return invokeCmd<PurgeReport[]>('purge_project', {
    args: { host_aliases: hostAliases, project_path: projectPath, project_id: projectId },
  });
}
