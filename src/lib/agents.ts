// The built-in agents (the PR reviewer, the support agent; design
// docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md,
// step 5): their standing rules, newest jobs and the actions waiting for a
// person. The Automation screen's Agents tab lists them; the Inbox's Needs
// you carries the pending actions, each with Approve and Reject.
// `agents_status` / `agents_decide` route to the hub's `agents` tool on a
// paired desktop (the owner's own device only). Mirrors
// `fleet_core::service::agents::admin` and `fleet_core::store::Agent*Row`.
import { derived, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { LoopHealth } from './ipc';

/** `fleet`: an instance of a catalog fleet agent (configurable agents,
 *  step 2); the Agents tab lists them from step 5. */
export type AgentKind = 'review' | 'support' | 'fleet';
export type AgentLevel = 'watch' | 'propose' | 'act';

export interface AgentRule {
  id: number;
  agent: AgentKind | string;
  /** `gh:<owner>/<repo>`, `jira:<tracker id>:<PROJECT>`, or a fleet
   *  agent's instance name. */
  scope: string;
  level: AgentLevel | string;
  host_alias: string;
  project_id: number | null;
  /** The agent's options, JSON text. */
  config: string;
  granted_by: string;
  granted_at: number;
  expires_at: number | null;
  /** `fleet`: the catalog definition the rule runs. */
  definition?: string;
  /** `fleet`: the store bindings, JSON text (`[]` otherwise). */
  bindings: string;
  /** Not expired. */
  active: boolean;
}

export interface AgentJob {
  id: number;
  rule_id: number | null;
  agent: AgentKind | string;
  /** `gh:o/r#4`, `jira:L2S-12`. */
  subject: string;
  trigger: string;
  dedup_key: string;
  state: 'pending' | 'running' | 'done' | 'failed' | 'skipped' | 'needs_person' | string;
  attempts: number;
  next_attempt_at: number;
  lease_until: number | null;
  error_kind: string | null;
  error: string | null;
  /** The run's answer, JSON text. */
  result: string | null;
  cost_micros: number;
  created_at: number;
  updated_at: number;
}

export interface AgentAction {
  id: number;
  job_id: number;
  /** `post_review`, `pr_labels`, `pr_description`, `debate_reply`, `jira_comment`, `jira_create_bug`, `jira_link`, `kb_learn`;
   *  a fleet agent's `tracker_comment`, `tracker_issue`, `tracker_link`, `record`, `mcp_call`. */
  kind: string;
  /** Exactly what will be sent, JSON text. */
  payload: string;
  state: 'pending' | 'approved' | 'rejected' | 'executed' | 'failed' | 'expired' | string;
  decided_by: string | null;
  decided_at: number | null;
  executed_at: number | null;
  error: string | null;
  created_at: number;
}

export interface AgentsStatus {
  rules: AgentRule[];
  jobs: AgentJob[];
  /** Actions waiting for a person. */
  pending: AgentAction[];
}

export function agentsStatus(agent?: AgentKind, limit?: number): Promise<Result<AgentsStatus>> {
  return invokeCmd<AgentsStatus>('agents_status', { agent: agent ?? null, limit: limit ?? null });
}

export function decideAgentAction(id: number, approve: boolean): Promise<Result<AgentAction>> {
  return invokeCmd<AgentAction>('agents_decide', { id, approve });
}

// ---- debugging (`service::agents::debug`; the `agents_debug` command) -------

export interface AgentPollError {
  at: number;
  message: string;
}

/** One rule, as `explain` reads it. */
export interface AgentRuleExplain {
  rule: Omit<AgentRule, 'active'>;
  active: boolean;
  state: 'ok' | 'warn' | 'block';
  summary: string;
  polled_at?: number;
  next_poll_at?: number;
  cursor?: string;
  poll_error?: AgentPollError;
  /** Its jobs counted by state. */
  jobs: Record<string, number>;
  next_job?: AgentJob;
  last_failed?: AgentJob;
}

export interface AgentsExplain {
  automation_paused: boolean;
  poll_interval_secs: number;
  max_concurrent: number;
  max_retries: number;
  loops: LoopHealth[];
  rules: AgentRuleExplain[];
}

export interface AgentJobDetail {
  job: AgentJob;
  rule?: Omit<AgentRule, 'active'>;
  actions: AgentAction[];
  summary: string;
}

export function explainAgents(): Promise<Result<AgentsExplain>> {
  return invokeCmd<AgentsExplain>('agents_debug', { action: 'explain', id: null });
}

export function agentJob(id: number): Promise<Result<AgentJobDetail>> {
  return invokeCmd<AgentJobDetail>('agents_debug', { action: 'job', id });
}

/** "2 pending · 1 failed · 5 done", in a fixed order. */
export function jobCountsLine(jobs: Record<string, number>): string {
  const order = ['pending', 'running', 'needs_person', 'failed', 'skipped', 'done'];
  const known = order.filter((k) => (jobs[k] ?? 0) > 0).map((k) => `${jobs[k]} ${k.replace('_', ' ')}`);
  const other = Object.keys(jobs)
    .filter((k) => !order.includes(k) && jobs[k] > 0)
    .map((k) => `${jobs[k]} ${k}`);
  const all = [...known, ...other];
  return all.length ? all.join(' · ') : 'no jobs yet';
}

/** The last status read; null before the first one or when it failed (a
 *  hub that does not serve `agents` to this device). */
export const agents = writable<AgentsStatus | null>(null);

/** The actions waiting for a person: the Inbox's Needs you carries them. */
export const pendingAgentActions = derived(agents, ($a) => $a?.pending ?? []);
export const pendingAgentCount = derived(pendingAgentActions, ($p) => $p.length);

export async function loadAgents(): Promise<void> {
  const r = await agentsStatus(undefined, 20);
  agents.set(r.ok ? r.value : null);
}

/** Approve or reject, then read the status again. */
export async function answerAgentAction(id: number, approve: boolean): Promise<Result<AgentAction>> {
  const r = await decideAgentAction(id, approve);
  await loadAgents();
  return r;
}

export const AGENTS_EVERY_MS = 60_000;

/** Keep `agents` fresh while the window shows, as `trackFailingRoutines`
 *  keeps the failed routines; a hub that refuses this device stops it. */
export function trackAgents(
  deps: { load?: () => Promise<Result<AgentsStatus>>; doc?: Document | null; every?: number } = {},
): () => void {
  const load = deps.load ?? (() => agentsStatus(undefined, 20));
  const doc = deps.doc === undefined ? (typeof document === 'undefined' ? null : document) : deps.doc;
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const beat = async () => {
    timer = null;
    if (stopped) return;
    if (doc?.visibilityState !== 'hidden') {
      const r = await load();
      if (stopped) return;
      agents.set(r.ok ? r.value : null);
      if (!r.ok && (r.error.code === 'E_FORBIDDEN' || r.error.code === 'E_INVALID')) return;
    }
    timer = setTimeout(() => void beat(), deps.every ?? AGENTS_EVERY_MS);
  };
  void beat();
  return () => {
    stopped = true;
    if (timer !== null) clearTimeout(timer);
  };
}

function parse(text: string | null): Record<string, unknown> | null {
  if (!text) return null;
  try {
    const v: unknown = JSON.parse(text);
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** "Post a review on o/r#4: request changes, 2 comments", "Comment on
 *  L2S-7" — what an action will do, from its payload. */
export function actionTitle(a: Pick<AgentAction, 'kind' | 'payload'>): string {
  const p = parse(a.payload);
  if (a.kind === 'post_review' && p) {
    const event = String(p.event ?? 'COMMENT').toLowerCase().replace('_', ' ');
    const n = Array.isArray(p.comments) ? p.comments.length : 0;
    return `Post a review on ${p.repo}#${p.number}: ${event}, ${n} comment${n === 1 ? '' : 's'}`;
  }
  if (a.kind === 'pr_labels' && p) {
    const add = Array.isArray(p.add) ? p.add.map((l) => `+${l}`) : [];
    const remove = Array.isArray(p.remove) ? p.remove.map((l) => `−${l}`) : [];
    return `Label ${p.repo}#${p.number}: ${[...add, ...remove].join(' ')}`;
  }
  if (a.kind === 'pr_description' && p) return `Describe ${p.repo}#${p.number}`;
  if (a.kind === 'debate_reply' && p) return `Answer the test debate on ${p.repo}#${p.number}: ${p.decision}`;
  if (a.kind === 'jira_comment' && p) return `Comment on ${p.key}`;
  if (a.kind === 'jira_create_bug' && p)
    return p.source_key
      ? `File a bug in ${p.project} for ${p.source_key}: ${p.summary}`
      : `File a bug in ${p.project} for a logged error: ${p.summary}`;
  if (a.kind === 'jira_link' && p) return `Link ${p.bug_key} to ${p.source_key} (${String(p.link_type ?? '').toLowerCase()})`;
  if (a.kind === 'kb_learn' && p) return `Teach the support KB: ${p.path}`;
  // A fleet agent's (configurable agents, step 4).
  if (a.kind === 'tracker_comment' && p) return `Comment on ${p.key}`;
  if (a.kind === 'tracker_issue' && p) {
    const to = (p.link as { to?: string } | null | undefined)?.to;
    return `File a ${String(p.issue_type ?? 'Task').toLowerCase()} in ${p.project}${to ? ` for ${to}` : ''}: ${p.summary}`;
  }
  if (a.kind === 'tracker_link' && p) return `Link ${p.from} to ${p.to} (${String(p.link_type ?? '').toLowerCase()})`;
  if (a.kind === 'record' && p) return `Remember ${p.key} in ${p.store}`;
  if (a.kind === 'mcp_call' && p) return `Call ${p.binding}.${p.tool}`;
  return a.kind.replace(/_/g, ' ');
}

/** The body an action will post, when it carries one. */
export function actionBody(a: Pick<AgentAction, 'payload'>): string | null {
  const b = parse(a.payload)?.body;
  return typeof b === 'string' ? b : null;
}

/** "gh:o/r at propose on mercury", with "(expired)" when it is. */
export function ruleLine(r: Pick<AgentRule, 'scope' | 'level' | 'host_alias' | 'active'>): string {
  return `${r.scope} at ${r.level} on ${r.host_alias}${r.active ? '' : ' (expired)'}`;
}

/** A job in one line: its subject, what it came to and, for a review, the
 *  verdict; for an investigation, its severity and confidence. */
export function jobLine(j: Pick<AgentJob, 'subject' | 'state' | 'error' | 'result'>): string {
  const r = parse(j.result);
  const verdict =
    typeof r?.verdict === 'string'
      ? ` · ${r.verdict.toLowerCase().replace('_', ' ')}`
      : typeof r?.severity === 'string'
        ? ` · ${r.severity}, ${String(r.confidence ?? '?')} confidence`
        : '';
  const why = j.error && (j.state === 'failed' || j.state === 'skipped') ? `: ${j.error}` : '';
  return `${j.subject} · ${j.state.replace('_', ' ')}${verdict}${why}`;
}

/** What one agent is doing now, for the Agents tab's row. */
export function agentState(kind: AgentKind, status: AgentsStatus | null): string {
  if (!status) return 'not reported';
  const rules = status.rules.filter((r) => r.agent === kind && r.active);
  if (rules.length === 0) return 'off · no rule';
  const pending = status.pending.length;
  const jobs = status.jobs.filter((j) => j.agent === kind);
  const working = jobs.filter((j) => j.state === 'running' || j.state === 'pending').length;
  const parts = [`${rules.length} rule${rules.length === 1 ? '' : 's'}`];
  if (working > 0) parts.push(`${working} working`);
  if (kind === 'review' && pending > 0) parts.push(`${pending} waiting for you`);
  return parts.join(' · ');
}
