import { timeAgo } from './session_status';
import { invokeCmd, type Result } from './result';
import type { ClaudeStatus, StuckKind, SessionRow } from './sessions';
import { stuckKindLabel, contextLevel, type ContextLevel } from './attention';
import type { SessionEvent } from './timeline';
import { promptFirstLine, type TaskRow } from './tasks';

export type ConvItem =
  | { kind: 'text'; text: string }
  | {
      kind: 'tool';
      summary: string;
      error?: boolean;
      id: string | null;
      name: string;
      target: string | null;
      at: string | null;
      ended_at: string | null;
      done: boolean;
    }
  | {
      kind: 'subagent';
      id: string | null;
      name: string;
      agent_type: string | null;
      description: string | null;
      result: string | null;
      error: boolean;
      at: string | null;
      ended_at: string | null;
      done: boolean;
    }
  | { kind: 'compact'; trigger: string | null; pre_tokens: number | null; summary: string | null }
  | { kind: 'command'; name: string; args: string | null; output: string | null }
  | {
      kind: 'notification';
      task_id: string | null;
      tool_use_id: string | null;
      status: string | null;
      summary: string | null;
      result: string | null;
      output_file: string | null;
      event: string | null;
      at: string | null;
    }
  | { kind: 'interrupt'; during_tool: boolean };

export interface ConvTurn {
  prompt: string | null;
  at: string | null;
  /** When the reply last advanced; null for a prompt with no reply yet. */
  ended_at: string | null;
  items: ConvItem[];
}

/** Current-conversation context size, read from the same transcript tail. */
export interface ContextView {
  tokens: number;
  window: number;
  pct: number;
  stale: boolean;
}

export interface Conversation {
  turns: ConvTurn[];
  truncated: boolean;
  context: ContextView | null;
  /** This conversation's timeline events, oldest first. */
  events: SessionEvent[];
}

/** Poll cadence for the Conversation tab while it is visible (spec §6). */
export const CONVERSATION_POLL_MS = 5_000;

/** Scroll is considered "pinned to bottom" within this many px (spec §6). */
export const PIN_THRESHOLD_PX = 40;

/** Default turn window the backend serves; "Load older" grows it by this. */
export const CONV_TURNS_STEP = 10;
/** The backend clamps a requested window here (`transcript::CONV_MAX_TURNS`). */
export const CONV_MAX_TURNS = 100;
/** How long an on-demand pane probe outranks the row's own status. */
export const PROBE_TTL_MS = 10_000;

/** One Claude Code conversation a session has run (`session_conversations`). */
export interface ConversationSummary {
  id: number;
  session_id: number;
  claude_session_id: string;
  transcript_path: string | null;
  started_at: number;
  ended_at: number | null;
  start_source: 'startup' | 'resume' | 'clear' | 'compact' | 'fork' | 'fleet' | 'unknown';
  end_reason: string | null;
  model: string | null;
  first_prompt: string | null;
  turns: number;
  compactions: number;
  current: boolean;
}

/** `claudeSessionId` reads an earlier conversation of the session instead of
 *  the current one (from {@link listConversations}). */
export function sessionConversation(
  sessionId: number,
  turns?: number,
  claudeSessionId?: string,
): Promise<Result<Conversation>> {
  const args: { session_id: number; turns?: number; claude_session_id?: string } = { session_id: sessionId };
  if (turns !== undefined) args.turns = turns;
  if (claudeSessionId !== undefined) args.claude_session_id = claudeSessionId;
  return invokeCmd<Conversation>('session_conversation', { args });
}

/** The session's conversations, newest first (backend caps `limit` at 500). */
export function listConversations(sessionId: number, limit = 50): Promise<Result<ConversationSummary[]>> {
  return invokeCmd<ConversationSummary[]>('session_conversations', {
    args: { session_id: sessionId, limit },
  });
}

/** The file change of an Edit / MultiEdit / Write call (`session_tool_detail`). */
export interface EditDetail {
  file_path: string;
  old: string;
  new: string;
}

/** One tool call's input and result, read on demand (`session_tool_detail`).
 *  Every text is capped at 8 000 chars ("…" when cut). */
export interface ToolDetail {
  id: string;
  name: string;
  /** Pretty JSON of the input. */
  input: string;
  /** Edit / MultiEdit / Write only. */
  edit: EditDetail | null;
  /** Bash only: the full command. */
  command: string | null;
  /** Null until the result arrives. */
  result: string | null;
  is_error: boolean;
}

/** The input and result of tool call `toolUseId`; `claudeSessionId` looks in
 *  an earlier conversation of the session. */
export function toolDetail(
  sessionId: number,
  toolUseId: string,
  claudeSessionId?: string,
): Promise<Result<ToolDetail>> {
  const args: { session_id: number; tool_use_id: string; claude_session_id?: string } = {
    session_id: sessionId,
    tool_use_id: toolUseId,
  };
  if (claudeSessionId !== undefined) args.claude_session_id = claudeSessionId;
  return invokeCmd<ToolDetail>('session_tool_detail', { args });
}

/** Deep (JSON) equality — used to decide whether a poll result actually changed. */
export function sameConversation(a: Conversation | null, b: Conversation): boolean {
  if (a === null) return false;
  return JSON.stringify(a) === JSON.stringify(b);
}

export function isPinned(scrollTop: number, clientHeight: number, scrollHeight: number): boolean {
  return scrollHeight - scrollTop - clientHeight <= PIN_THRESHOLD_PX;
}

/**
 * The Conversation tab's empty-state message, or null when turns should be
 * rendered instead. Missing Claude session id takes priority over any error
 * code (there was nothing to fetch in the first place).
 */
export function emptyStateText(code: string | null, hasId: boolean): string | null {
  if (!hasId) return 'No Claude session id yet';
  if (code === 'E_NO_TRANSCRIPT') return 'No conversation yet';
  return null;
}

/** The second line of an empty state: what the user can do about it. Null
 *  wherever `emptyStateText` is null, so the two stay in step. */
export function emptyStateHint(code: string | null, hasId: boolean, canPrompt: boolean): string | null {
  if (!hasId) return 'It starts one as soon as Claude runs in this session.';
  if (code !== 'E_NO_TRANSCRIPT') return null;
  return canPrompt
    ? 'Send a prompt below to start it.'
    : 'This agent runs outside tmux, so it can only be prompted where it was started.';
}

/** Pure relative-time formatter for an ISO timestamp against a reference clock. */
export function relativeTime(iso: string, nowMs: number): string {
  return timeAgo(new Date(iso).getTime() / 1000, nowMs);
}

/** One tool call line; `error` when its result came back as an error. */
export interface ToolLine {
  summary: string;
  error: boolean;
  id: string | null;
  name: string;
  target: string | null;
  at: string | null;
  ended_at: string | null;
  done: boolean;
}

/** A reply item after folding: prose, a run of consecutive tool calls, or one
 *  of the standalone event kinds (each of which also breaks a tool run). */
export type ConvGroup =
  | { kind: 'text'; text: string }
  | { kind: 'tools'; tools: ToolLine[] }
  | {
      kind: 'subagent';
      id: string | null;
      name: string;
      agent_type: string | null;
      description: string | null;
      result: string | null;
      error: boolean;
      at: string | null;
      ended_at: string | null;
      done: boolean;
    }
  | { kind: 'compact'; trigger: string | null; pre_tokens: number | null; summary: string | null }
  | { kind: 'command'; name: string; args: string | null; output: string | null }
  | {
      kind: 'notification';
      task_id: string | null;
      tool_use_id: string | null;
      status: string | null;
      summary: string | null;
      result: string | null;
      output_file: string | null;
      event: string | null;
      at: string | null;
    }
  | { kind: 'interrupt'; during_tool: boolean };

/** Fold consecutive tool one-liners into one group; text items stay apart;
 *  a subagent (and compact/command/interrupt) items are each their own
 *  group and close any open tool run. */
export function groupItems(items: ConvItem[]): ConvGroup[] {
  const out: ConvGroup[] = [];
  for (const item of items) {
    if (item.kind === 'text') {
      out.push({ kind: 'text', text: item.text });
      continue;
    }
    if (item.kind === 'tool') {
      const line: ToolLine = {
        summary: item.summary,
        error: item.error === true,
        id: item.id,
        name: item.name,
        target: item.target,
        at: item.at,
        ended_at: item.ended_at,
        done: item.done,
      };
      const last = out[out.length - 1];
      if (last?.kind === 'tools') last.tools.push(line);
      else out.push({ kind: 'tools', tools: [line] });
      continue;
    }
    out.push(item);
  }
  return out;
}

/** The tool name of a one-liner such as `Bash(command=ls)`. */
export function toolName(summary: string): string {
  const paren = summary.indexOf('(');
  return paren > 0 ? summary.slice(0, paren) : summary;
}

/** `"7 tool calls · Bash, Read, Edit +2"` for a folded group, with
 *  `" · 1 failed"` appended when any call errored. */
export function toolGroupLabel(tools: ToolLine[]): string {
  const names = [...new Set(tools.map((t) => t.name || toolName(t.summary)))];
  const shown = names.slice(0, 3).join(', ');
  const more = names.length > 3 ? ` +${names.length - 3}` : '';
  const failed = tools.filter((t) => t.error).length;
  const suffix = failed > 0 ? ` · ${failed} failed` : '';
  return `${tools.length} tool calls · ${shown}${more}${suffix}`;
}

// ─── Background work: notifications (spec §"Frontend — the thread") ─────────

/** One `<task-notification>` as the backend parsed it. */
export type NotificationItem = Extract<ConvItem, { kind: 'notification' }>;

export type NotificationTone = 'info' | 'warn' | 'error';

/** A notification's tone, from its status. A mid-stream Monitor event has no
 *  status at all: it is progress, so it reads as plain info. */
export function notificationTone(status: string | null): NotificationTone {
  if (status === 'failed' || status === 'killed') return 'error';
  if (status === 'stopped') return 'warn';
  return 'info';
}

/** The glyph in front of the row. Keyed off the status rather than the tone,
 *  so a status-less event is a bullet rather than a tick. */
export function notificationMark(status: string | null): string {
  if (status === null) return '•';
  if (status === 'failed' || status === 'killed') return '✕';
  if (status === 'stopped') return '⏸';
  return '✓';
}

/** The row's single line: the harness's own sentence, plus the streamed
 *  event's first line when there is one. */
export function notificationLabel(n: { summary: string | null; event: string | null }): string {
  const head = n.summary?.trim() || 'Background task reported';
  const tail = n.event?.trim().split('\n')[0].trim();
  return tail ? `${head}: ${tail}` : head;
}

// ─── Background work: the session's own list (task 4) ───────────────────────

/** How a background entry currently stands. `running` covers "launched and
 *  has not reported back" as well as a queued fleet task. */
export type BackgroundStatus = 'running' | 'done' | 'failed' | 'stopped';

/** One report a background task filed. A resumed agent files several. */
export interface BackgroundReport {
  at: string | null;
  status: string | null;
  summary: string | null;
  result: string | null;
}

/** One background thing that belongs to a session: something this
 *  conversation launched, or a fleet row/task spawned from it. */
export interface BackgroundEntry {
  /** Stable across renders: `task:<task-id>` or `tool:<tool_use id>` for a
   *  transcript entry, `session:<id>` / `fleettask:<id>` for a fleet one. */
  key: string;
  source: 'transcript' | 'fleet_task' | 'fleet_session';
  /** `Agent` | `Bash` | `Monitor` | … for a transcript entry; the session
   *  row's `kind`, or `task`, for a fleet one. */
  kind: string;
  label: string;
  status: BackgroundStatus;
  /** When it was launched; ISO for a transcript entry, null for fleet rows
   *  whose own list already shows their age. */
  at: string | null;
  result: string | null;
  error: string | null;
  outputFile: string | null;
  /** The fleet session to switch to, when there is one. */
  sessionId: number | null;
  taskId: number | null;
  history: BackgroundReport[];
}

/** Tools whose calls can be backgrounded and then report in. A foreground
 *  call of the same tool never gets a notification, which is exactly how the
 *  two are told apart — the launch input carries no flag to read. */
const BACKGROUND_TOOLS = new Set(['Bash', 'Monitor', 'Workflow', 'SendMessage']);

function statusFromReports(reports: BackgroundReport[]): BackgroundStatus {
  const last = [...reports].reverse().find((r) => r.status !== null);
  if (!last) return 'running';
  if (last.status === 'completed') return 'done';
  if (last.status === 'stopped') return 'stopped';
  return 'failed';
}

/** Running first, then newest launch first. */
function byRunningThenNewest(a: BackgroundEntry, b: BackgroundEntry): number {
  const run = (e: BackgroundEntry) => (e.status === 'running' ? 0 : 1);
  if (run(a) !== run(b)) return run(a) - run(b);
  return (b.at ?? '').localeCompare(a.at ?? '');
}

/** The last report to carry a non-null value for `k`, or null. */
function lastNonNull<K extends keyof BackgroundReport>(rs: BackgroundReport[], k: K): BackgroundReport[K] | null {
  for (let i = rs.length - 1; i >= 0; i--) {
    const v = rs[i][k];
    if (v !== null) return v;
  }
  return null;
}

/** The background work this conversation launched, from its own turns.
 *
 *  A subagent block is listed when a notification named it, or while it is
 *  still open — a finished call with no notification was a foreground one.
 *  A tool line is listed only when a notification named it.
 *
 *  Notifications that arrive back to back are coalesced into one turn, so
 *  the turn's own `at` is only the first one's — each report's finish time
 *  (and the task's output file) is read from the notification item's own
 *  `at`/`output_file`, captured once while walking the turns, not re-derived
 *  per entry afterwards. */
export function transcriptBackground(turns: ConvTurn[]): BackgroundEntry[] {
  const reports = new Map<string, BackgroundReport[]>();
  const taskIds = new Map<string, string>();
  const outputFiles = new Map<string, string>();
  for (const t of turns) {
    for (const item of t.items) {
      if (item.kind !== 'notification' || item.tool_use_id === null) continue;
      const list = reports.get(item.tool_use_id) ?? [];
      list.push({ at: item.at ?? t.at, status: item.status, summary: item.summary, result: item.result });
      reports.set(item.tool_use_id, list);
      if (item.task_id !== null) taskIds.set(item.tool_use_id, item.task_id);
      if (item.output_file !== null) outputFiles.set(item.tool_use_id, item.output_file);
    }
  }

  const out: BackgroundEntry[] = [];
  for (const t of turns) {
    for (const item of t.items) {
      if (item.kind === 'subagent') {
        const rs = (item.id !== null && reports.get(item.id)) || [];
        if (rs.length === 0 && item.done) continue;
        out.push({
          key: item.id !== null && taskIds.has(item.id) ? `task:${taskIds.get(item.id)}` : `tool:${item.id ?? ''}`,
          source: 'transcript',
          kind: item.name || 'Agent',
          label: item.description ?? item.agent_type ?? 'subagent',
          status: statusFromReports(rs),
          at: item.at,
          result: lastNonNull(rs, 'result') ?? item.result,
          error: null,
          outputFile: (item.id !== null && outputFiles.get(item.id)) || null,
          sessionId: null,
          taskId: null,
          history: rs,
        });
      } else if (item.kind === 'tool' && item.id !== null && BACKGROUND_TOOLS.has(item.name)) {
        const rs = reports.get(item.id) ?? [];
        if (rs.length === 0) continue;
        out.push({
          key: taskIds.has(item.id) ? `task:${taskIds.get(item.id)}` : `tool:${item.id}`,
          source: 'transcript',
          kind: item.name,
          label: item.target ?? item.summary,
          status: statusFromReports(rs),
          at: item.at,
          result: lastNonNull(rs, 'result'),
          error: null,
          outputFile: outputFiles.get(item.id) ?? null,
          sessionId: null,
          taskId: null,
          history: rs,
        });
      }
    }
  }
  return out.sort(byRunningThenNewest);
}

/** The fleet rows and tasks this session spawned. A worker session appears
 *  both as a session and as its task: they are different things — one is a
 *  place to go, the other a unit of work with a result. */
export function fleetBackground(sessions: SessionRow[], tasks: TaskRow[], sessionId: number): BackgroundEntry[] {
  const out: BackgroundEntry[] = [];
  for (const s of sessions) {
    if (s.parent_session_id !== sessionId) continue;
    out.push({
      key: `session:${s.id}`,
      source: 'fleet_session',
      kind: s.kind,
      label: s.friendly_name || s.tmux_name,
      status:
        s.claude_status === 'working'
          ? 'running'
          : s.claude_status === 'failed'
            ? 'failed'
            : s.claude_status === 'stopped'
              ? 'stopped'
              : 'done',
      at: null,
      result: null,
      error: null,
      outputFile: null,
      sessionId: s.id,
      taskId: null,
      history: [],
    });
  }
  for (const t of tasks) {
    if (t.requester_session_id !== sessionId) continue;
    out.push({
      key: `fleettask:${t.id}`,
      source: 'fleet_task',
      kind: 'task',
      label: promptFirstLine(t.prompt) || `task #${t.id}`,
      status:
        t.state === 'queued' || t.state === 'running'
          ? 'running'
          : t.state === 'done'
            ? 'done'
            : t.state === 'cancelled'
              ? 'stopped'
              : 'failed',
      at: null,
      result: t.result,
      error: t.error,
      outputFile: null,
      sessionId: t.worker_session_id,
      taskId: t.id,
      history: [],
    });
  }
  return out;
}

// ─── Tool lines / subagents / doing now (phase 3) ───────────────────────────

const TOOL_VERBS: Record<string, string> = {
  Read: 'Read',
  Edit: 'Edit',
  MultiEdit: 'Edit',
  Write: 'Write',
  Bash: 'Run',
  Grep: 'Search',
  Glob: 'Find',
  WebFetch: 'Fetch',
  WebSearch: 'Search web',
  TodoWrite: 'Update todos',
};

/** The short verb a tool line leads with; `mcp__srv__tool` → `srv · tool`. */
export function toolVerb(name: string): string {
  const known = TOOL_VERBS[name];
  if (known) return known;
  if (name.startsWith('mcp__')) {
    const [server, ...rest] = name.slice('mcp__'.length).split('__');
    if (server && rest.length > 0) return `${server} · ${rest.join('__')}`;
  }
  return name;
}

/** A path target cut to its last two segments (`…/store/reconcile.rs`),
 *  relative to `cwdHint` when it lies under it; anything that is not a path
 *  (a command, a pattern with spaces, a URL) is returned unchanged. */
export function shortTarget(target: string | null, cwdHint?: string | null): string | null {
  if (target === null) return null;
  if (!target.includes('/') || /\s/.test(target) || /^[a-z][a-z0-9+.-]*:\/\//i.test(target)) return target;
  let path = target;
  if (cwdHint) {
    const base = cwdHint.endsWith('/') ? cwdHint : `${cwdHint}/`;
    if (path.startsWith(base) && path.length > base.length) path = path.slice(base.length);
  }
  const segs = path.split('/').filter((p) => p.length > 0);
  if (segs.length <= 2) return path;
  return `…/${segs.slice(-2).join('/')}`;
}

/** `0.4s` under a second, `12s` under a minute, else `3m 05s`. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${(Math.max(0, ms) / 1000).toFixed(1)}s`;
  const secs = Math.floor(ms / 1000);
  if (secs < 60) return `${secs}s`;
  return `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, '0')}s`;
}

/** How long a tool call took, or (when `endedAt` is null and `nowMs` is
 *  given) how long it has been running; null when it cannot be told. */
export function toolDurationMs(at: string | null, endedAt: string | null, nowMs: number | null): number | null {
  if (!at) return null;
  const start = Date.parse(at);
  const end = endedAt ? Date.parse(endedAt) : nowMs;
  if (end === null || !Number.isFinite(start) || !Number.isFinite(end)) return null;
  return Math.max(0, end - start);
}

/** What the running turn is doing right now, for the activity indicator. */
export interface DoingNow {
  label: string;
  /** How long it has been running; null when its start is unknown. */
  sinceMs: number | null;
}

/** `Run cargo test` for a tool line (verb + short target). */
export function toolLineLabel(t: { name: string; summary: string; target: string | null }): string {
  const verb = toolVerb(t.name || toolName(t.summary));
  const target = shortTarget(t.target);
  return target ? `${verb} ${target}` : verb;
}

/** The last turn's last unfinished tool call or subagent (after its last
 *  interrupt), while the session is working; null otherwise. */
export function doingNow(conv: Conversation | null, working: boolean, nowMs: number): DoingNow | null {
  if (!working || !conv || conv.turns.length === 0) return null;
  const items = conv.turns[conv.turns.length - 1].items;
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    // Anything before an interrupt was cut off, not running.
    if (it.kind === 'interrupt') return null;
    if (it.kind === 'tool' && !it.done) {
      return { label: toolLineLabel(it), sinceMs: toolDurationMs(it.at, null, nowMs) };
    }
    if (it.kind === 'subagent' && !it.done) {
      const who = it.agent_type ?? 'subagent';
      const label = it.description ? `${who} · ${it.description}` : who;
      return { label, sinceMs: toolDurationMs(it.at, null, nowMs) };
    }
  }
  return null;
}

/** The last turn has a tool call or subagent still waiting for its result.
 *  Only a live turn's pending call has a running clock (see ToolLine). */
export function hasPendingCall(conv: Conversation | null): boolean {
  if (!conv || conv.turns.length === 0) return false;
  return conv.turns[conv.turns.length - 1].items.some((it) => (it.kind === 'tool' || it.kind === 'subagent') && !it.done);
}

export interface DiffLine {
  kind: 'del' | 'add' | 'ctx';
  text: string;
}

/** Context lines kept on each side of an edit's changed block. */
const DIFF_CONTEXT = 2;

/** A line diff of an edit: the shared leading / trailing lines as context
 *  (at most two each side), the changed middle as deletions then additions. */
export function editDiffLines(old: string, next: string): DiffLine[] {
  const a = old === '' ? [] : old.split('\n');
  const b = next === '' ? [] : next.split('\n');
  let pre = 0;
  while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre++;
  let suf = 0;
  while (suf < a.length - pre && suf < b.length - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++;
  const out: DiffLine[] = [];
  for (const text of a.slice(Math.max(0, pre - DIFF_CONTEXT), pre)) out.push({ kind: 'ctx', text });
  for (const text of a.slice(pre, a.length - suf)) out.push({ kind: 'del', text });
  for (const text of b.slice(pre, b.length - suf)) out.push({ kind: 'add', text });
  for (const text of a.slice(a.length - suf, a.length - suf + DIFF_CONTEXT)) out.push({ kind: 'ctx', text });
  return out;
}

/** Prompts longer than this are clamped behind "Show more". */
export const PROMPT_CLAMP_LINES = 6;
const PROMPT_CLAMP_CHARS = 600;

export function isLongPrompt(prompt: string): boolean {
  return prompt.length > PROMPT_CLAMP_CHARS || prompt.split('\n').length > PROMPT_CLAMP_LINES;
}

/** A prompt sent from the composer, shown as its own turn until the
 *  transcript carries it. `seen` is how many turns already had this exact
 *  text when it was sent, so re-sending an earlier prompt ("continue") is
 *  not mistaken for the transcript having caught up. */
export interface PendingPrompt {
  prompt: string;
  at: string;
  seen: number;
}

/** True once a fetched conversation has more turns with the pending text
 *  than there were when it was sent. */
export function transcriptCarries(conv: Conversation, pending: PendingPrompt): boolean {
  return conv.turns.filter((t) => t.prompt === pending.prompt).length > pending.seen;
}

/** The note under the composer's Send button, or null when the session is
 *  ready to take a prompt. A stuck session may never read what is typed; a
 *  working one queues it until the turn ends. */
export function composerStatus(s: { claude_status: string | null; stuck_kind: string | null }): string | null {
  if (s.stuck_kind) return `Session is stuck (${stuckKindLabel(s.stuck_kind as StuckKind) || s.stuck_kind}). The prompt may not be read until that is cleared.`;
  if (s.claude_status === 'working') return 'Claude is working. The prompt is queued until the current turn ends.';
  return null;
}

/** A Claude Code built-in slash command offered by the composer's menu. */
export interface SlashCommand {
  name: string;
  description: string;
  /** Completes with a trailing space so the user can type the argument. */
  args?: boolean;
}

/**
 * Claude Code's built-in commands (the REPL runs them when the line is sent
 * exactly as typed, verified against v2.1 through tmux send-keys). Kept
 * short and stable: this is a hint list, not a spec of the CLI.
 */
export const SLASH_COMMANDS: readonly SlashCommand[] = [
  { name: 'clear', description: 'Clear the conversation and start fresh' },
  { name: 'compact', description: 'Summarise the context to free space (optional focus text)', args: true },
  { name: 'context', description: 'Show what is using the context window' },
  { name: 'cost', description: 'Show token usage and cost for this session' },
  { name: 'usage', description: 'Show plan usage and rate limits' },
  { name: 'status', description: 'Show version, model, account and working directory' },
  { name: 'model', description: 'Switch the model', args: true },
  { name: 'effort', description: 'Set the reasoning effort level', args: true },
  { name: 'rc', description: 'Remote Control: drive this session from claude.ai' },
  { name: 'resume', description: 'Resume an earlier conversation' },
  { name: 'rewind', description: 'Rewind the conversation and files to a checkpoint' },
  { name: 'review', description: 'Review the current changes' },
  { name: 'memory', description: 'Edit the memory files loaded into context' },
  { name: 'config', description: 'Open settings' },
  { name: 'permissions', description: 'Manage tool permissions' },
  { name: 'mcp', description: 'Manage MCP servers' },
  { name: 'agents', description: 'Manage subagent definitions' },
  { name: 'hooks', description: 'Manage hooks' },
  { name: 'doctor', description: 'Check the installation' },
  { name: 'init', description: 'Write a CLAUDE.md for this project' },
  { name: 'export', description: 'Export the conversation to a file' },
  { name: 'help', description: 'List commands and shortcuts' },
  { name: 'exit', description: 'Quit Claude Code (the tmux session stays)' },
];

/**
 * Commands whose name starts with the draft's slash token. Empty unless the
 * whole draft is one token that begins with `/` (no spaces or newlines): once
 * an argument or a second line is being typed, the menu gets out of the way.
 */
export function matchSlashCommands(draft: string): SlashCommand[] {
  if (!draft.startsWith('/') || /\s/.test(draft)) return [];
  const prefix = draft.slice(1).toLowerCase();
  return SLASH_COMMANDS.filter((c) => c.name.startsWith(prefix));
}

/** The draft text that accepting a menu item yields. */
export function completeSlashCommand(c: SlashCommand): string {
  return c.args ? `/${c.name} ` : `/${c.name}`;
}

// ─── Live indicator ──────────────────────────────────────────────────────────

/** `session_activity`: what the pane shows right now, same vocabularies as
 *  the session row so it can be laid over it. */
export interface ActivityProbe {
  claude_status: ClaudeStatus | null;
  current_activity: string | null;
  stuck_kind: StuckKind | null;
  waiting_for: 'permission' | 'input' | null;
  spinner: string | null;
}

export function sessionActivity(sessionId: number): Promise<Result<ActivityProbe>> {
  return invokeCmd<ActivityProbe>('session_activity', { args: { session_id: sessionId } });
}

/** Probe cadence while the indicator is live (working / sent / blocked). */
export const ACTIVITY_POLL_MS = 2_000;
/** Transcript re-read cadence for a quiet session (nothing is changing). */
export const QUIET_POLL_MS = 15_000;

/** Statuses under which the transcript cannot be growing. `null` (not yet
 *  classified) is NOT quiet: an unknown session keeps the 5 s cadence. */
export function isQuietStatus(s: ClaudeStatus | null): boolean {
  return s === 'idle' || s === 'completed' || s === 'stopped' || s === 'failed';
}

/** Whether a poll tick should re-read the transcript. A quiet session is
 *  read only when its turn counter moved or the quiet cadence elapsed. */
export function shouldFetchTranscript(a: { quiet: boolean; sinceLastFetchMs: number; turnSeqChanged: boolean }): boolean {
  return !a.quiet || a.turnSeqChanged || a.sinceLastFetchMs >= QUIET_POLL_MS;
}

/** `Cooking… (3s · ↓ 306 tokens · esc to interrupt)` → `Cooking… 3s · ↓ 306 tokens`. */
export function spinnerLabel(spinner: string): string {
  const parts = spinner
    .replace(/[()]/g, ' ')
    .split('·')
    .map((p) => p.trim())
    .filter((p) => p.length > 0 && !/esc to interrupt/i.test(p));
  return parts.join(' · ').replace(/\s+/g, ' ').trim();
}

export type Indicator =
  | { kind: 'working'; label: string }
  | { kind: 'sent' }
  | { kind: 'blocked'; detail: string | null; waiting: 'permission' | 'input' | null }
  | null;

/**
 * What to show under the last turn. A stuck session yields nothing here
 * (the composer's status note and the Press Enter chip cover it). `pending`
 * is a prompt sent from the composer the transcript has not carried yet;
 * `optimistic` is the window after our own send in which no probe has yet
 * reported the session idle and its turn counter has not moved.
 */
export function indicatorFor(a: {
  status: ClaudeStatus | null;
  stuckKind: StuckKind | null;
  waitingFor: 'permission' | 'input' | null;
  activity: string | null;
  spinner: string | null;
  pending: boolean;
  optimistic: boolean;
}): Indicator {
  if (a.stuckKind) return null;
  if (a.status === 'blocked') return { kind: 'blocked', detail: a.activity, waiting: a.waitingFor };
  if (a.status === 'working') return { kind: 'working', label: a.spinner ? spinnerLabel(a.spinner) : 'Working…' };
  if (a.pending) return { kind: 'sent' };
  if (a.optimistic) return { kind: 'working', label: 'Working…' };
  return null;
}

/** `2m 14s` / `35s` between a turn's prompt and its latest reply entry;
 *  null when either end is missing, unparsable, or under a second. */
export function turnDuration(at: string | null, endedAt: string | null): string | null {
  if (!at || !endedAt) return null;
  const ms = new Date(endedAt).getTime() - new Date(at).getTime();
  if (!Number.isFinite(ms) || ms < 1000) return null;
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${secs % 60}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}

/**
 * Unsent composer text per session id, kept for the life of the app. The
 * panel unmounts on every tab switch, so without this a half-typed prompt
 * would vanish when the user glances at Files or the terminal.
 */
export const composerDrafts = new Map<number, string>();

export function rememberDraft(sessionId: number, text: string): void {
  if (text.length === 0) composerDrafts.delete(sessionId);
  else composerDrafts.set(sessionId, text);
}

/** Prompts plus reply items, for "N new" while the user is scrolled up. */
export function countItems(conv: Conversation | null): number {
  if (!conv) return 0;
  return conv.turns.reduce((n, t) => n + (t.prompt !== null ? 1 : 0) + t.items.length, 0);
}

/** How many items a poll added. Never negative: a re-read that trimmed
 *  older turns is not "new" content. */
export function newItemCount(prev: Conversation | null, next: Conversation): number {
  return Math.max(0, countItems(next) - countItems(prev));
}

/**
 * Prompts to recall with ArrowUp in the composer, oldest first: the
 * conversation's prompts plus the one just sent (if the transcript has not
 * carried it yet). Slash commands are skipped, and a prompt repeated back
 * to back appears once.
 */
export function promptHistory(conv: Conversation | null, pending: PendingPrompt | null): string[] {
  const out: string[] = [];
  const push = (p: string | null) => {
    if (!p || p.startsWith('/')) return;
    if (out[out.length - 1] !== p) out.push(p);
  };
  for (const t of conv?.turns ?? []) push(t.prompt);
  push(pending?.prompt ?? null);
  return out;
}

// ─── Header / thread helpers ────────────────────────────────────────────────

export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`;
  const m = n / 1_000_000;
  return `${Number.isInteger(m) ? m : m.toFixed(2).replace(/0+$/, '').replace(/\.$/, '')}M`;
}

export interface ContextMeter {
  pct: number;
  level: ContextLevel;
  label: string;
  title: string;
  stale: boolean;
}

export function contextMeter(
  s: Pick<SessionRow, 'context_pct' | 'context_tokens' | 'context_window' | 'context_stale'>,
): ContextMeter | null {
  const pct =
    s.context_pct ??
    (s.context_tokens != null && s.context_window ? (s.context_tokens * 100) / s.context_window : null);
  const level = contextLevel(pct);
  if (pct === null || level === null) return null;
  const rounded = Math.round(pct);
  const label =
    s.context_tokens != null && s.context_window
      ? `${formatTokens(s.context_tokens)} / ${formatTokens(s.context_window)} · ${rounded}%`
      : `ctx ${rounded}%`;
  const title = s.context_stale
    ? 'Context size from before the last compaction or resume — it updates with the next reply'
    : `Context window ${rounded}% used`;
  return { pct, level, label, title, stale: !!s.context_stale };
}

export const SOURCE_LABELS: Record<ConversationSummary['start_source'], string> = {
  startup: 'started',
  resume: '/resume',
  clear: '/clear',
  compact: '/compact',
  fork: 'fork',
  fleet: 'started by fleet',
  unknown: 'new conversation',
};

/** Switcher rows: drop empty non-current conversations (a `/clear` right
 *  after a `/clear`), keep order (newest first). */
export function switcherEntries(list: ConversationSummary[]): ConversationSummary[] {
  return list.filter((c) => c.current || c.turns > 0 || c.first_prompt !== null);
}

function clock(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

export function conversationTitle(c: ConversationSummary): string {
  const when = c.current ? 'Current' : clock(c.started_at);
  const turns = `${c.turns} turn${c.turns === 1 ? '' : 's'}`;
  return `${when} · ${SOURCE_LABELS[c.start_source]} · ${turns}`;
}

export function statusChip(s: Pick<SessionRow, 'claude_status' | 'current_activity'>): string | null {
  if (s.current_activity === 'compacting') return 'compacting';
  return s.claude_status ?? null;
}

export interface InlineEvent {
  id: number;
  at: number;
  label: string;
  detail: string | null;
  tone: 'info' | 'warn' | 'error';
}

const PERMISSION_KINDS = new Set(['permission_prompt', 'elicitation_dialog', 'elicitation_url_dialog']);

function humanError(detail: string | null): { head: string; rest: string | null } {
  if (!detail) return { head: 'unknown error', rest: null };
  const i = detail.indexOf(':');
  const head = (i < 0 ? detail : detail.slice(0, i)).replace(/_/g, ' ').trim();
  const rest = i < 0 ? null : detail.slice(i + 1).trim() || null;
  return { head, rest };
}

/** The inline row for a timeline event, or null for events the thread does
 *  not show (turn_done, status changes, prompts and compactions — the
 *  transcript already carries those). */
export function inlineEventFor(
  e: SessionEvent,
  live: { latestId: number | null; blocked: boolean },
): InlineEvent | null {
  switch (e.kind) {
    case 'conversation_started':
      return e.detail === 'resume'
        ? { id: e.id, at: e.at, label: 'Resumed conversation', detail: null, tone: 'info' }
        : null;
    case 'stop_failure': {
      const { head, rest } = humanError(e.detail);
      return { id: e.id, at: e.at, label: `Turn failed: ${head}`, detail: rest, tone: 'error' };
    }
    case 'notification': {
      if (!e.detail || !PERMISSION_KINDS.has(e.detail)) return null;
      const waiting = live.blocked && live.latestId === e.id;
      return {
        id: e.id,
        at: e.at,
        label: waiting ? 'Waiting for permission' : 'Asked for permission',
        detail: null,
        tone: waiting ? 'warn' : 'info',
      };
    }
    case 'conversation_ended':
      return { id: e.id, at: e.at, label: `Conversation ended (${e.detail ?? 'unknown'})`, detail: null, tone: 'info' };
    default:
      return null;
  }
}

export type ThreadRow =
  | { kind: 'turn'; turn: ConvTurn; index: number }
  | { kind: 'event'; event: InlineEvent };

/** Interleave turns (ISO `at`) and inline events (unix secs) by time. An
 *  event goes after the last turn that started at or before it. When the
 *  tail is truncated, events older than the first loaded turn are dropped
 *  (their turns are not on screen). */
export function buildThread(
  turns: ConvTurn[],
  events: SessionEvent[],
  live: { blocked: boolean },
  truncated: boolean,
): ThreadRow[] {
  const latestId = events.length ? events[events.length - 1].id : null;
  const inline = events
    .map((e) => inlineEventFor(e, { latestId, blocked: live.blocked }))
    .filter((e): e is InlineEvent => e !== null)
    .sort((a, b) => a.at - b.at || a.id - b.id);
  const starts: number[] = [];
  let prev = -Infinity;
  for (const t of turns) {
    const ms = t.at ? Date.parse(t.at) : NaN;
    prev = Number.isNaN(ms) ? prev : ms / 1000;
    starts.push(prev);
  }
  const rows: ThreadRow[] = [];
  let k = 0;
  const firstStart = starts.length ? starts[0] : Infinity;
  while (k < inline.length && inline[k].at < firstStart) {
    if (!truncated) rows.push({ kind: 'event', event: inline[k] });
    k++;
  }
  turns.forEach((turn, i) => {
    rows.push({ kind: 'turn', turn, index: i });
    const next = i + 1 < starts.length ? starts[i + 1] : Infinity;
    while (k < inline.length && inline[k].at < next) {
      rows.push({ kind: 'event', event: inline[k] });
      k++;
    }
  });
  return rows;
}

const LAST_EVENT_KINDS: Record<string, (d: string | null) => string | null> = {
  compact_done: () => '/compact',
  compact_started: () => 'compacting',
  stop_failure: (d) => humanError(d).head,
  notification: (d) => (d && PERMISSION_KINDS.has(d) ? 'permission asked' : null),
  conversation_started: (d) => (d === 'resume' ? '/resume' : d === 'clear' ? '/clear' : null),
};

/** "`/compact` 3m ago" for the newest notable event, else null. */
export function lastEventLabel(events: SessionEvent[], nowMs: number): string | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const f = LAST_EVENT_KINDS[events[i].kind];
    const label = f ? f(events[i].detail) : null;
    if (label) return `${label} ${timeAgo(events[i].at, nowMs)}`;
  }
  return null;
}

/** Union by id, oldest first (the panel appends pushed events to the ones
 *  the fetch returned). */
export function mergeEvents(base: SessionEvent[], extra: SessionEvent[]): SessionEvent[] {
  const byId = new Map<number, SessionEvent>();
  for (const e of [...base, ...extra]) byId.set(e.id, e);
  return [...byId.values()].sort((a, b) => a.at - b.at || a.id - b.id);
}
