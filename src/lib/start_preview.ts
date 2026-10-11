// The start preview (task → session spec P-1): where a start would land,
// what it would send and what is in the way — nothing is made. The Work
// button asks for it first: a clean preview starts at once, anything else
// opens the start popover with the conflicts and the choices.
import { writable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';
import { startWork, type StartWorkArgs } from './trackers';
import type { DecisionProposal } from './proposals';
import type { SessionRow } from './sessions';
import type { WorkTask } from './work_view';
import { preselect, type ProposalLike } from './ai_proposal';
import type { StartRule } from './start_rules';

/** What would stop or change a start. */
export interface StartConflict {
  /** `live_session` | `proposal` | `cross_org` | `worktree_busy` | `done`. */
  kind: string;
  message: string;
  /** The session in the way, when it may be named. */
  session_id?: number | null;
}

export interface StartPlanView {
  key: string;
  title: string;
  item_id?: number | null;
  project_id: number;
  host_alias: string;
  branch: string;
  worktree_id?: number | null;
  name: string;
  parallel?: boolean;
  /** The start rule that picked the repository (redesign 8.11). */
  rule_id?: number | null;
  /** How the rules say it runs (G7.1): the account, model, effort, agent. */
  profile?: string | null;
  model?: string | null;
  effort?: string | null;
  agent?: string | null;
  /** The rule's host, offline, when the start took its fallback. */
  fell_back_from?: string | null;
  /** The placement rule whose "its sessions start here" applied. */
  placement_rule_id?: number | null;
}

/** "account work · opus · effort high · Codex · mac is offline": what the
 *  rules set on a planned start beyond the repository and host. Empty when
 *  nothing. */
export function planLaunchLine(p: Pick<StartPlanView, 'profile' | 'model' | 'effort' | 'agent' | 'fell_back_from'>): string {
  const parts: string[] = [];
  if (p.agent === 'codex') parts.push('Codex');
  if (p.profile) parts.push(`account ${p.profile}`);
  if (p.model) parts.push(p.model);
  if (p.effort) parts.push(`effort ${p.effort}`);
  if (p.fell_back_from) parts.push(`${p.fell_back_from} is offline`);
  return parts.join(' · ');
}

export interface StartPreview {
  key: string;
  title: string;
  item_id?: number | null;
  /** `null` while `missing` names what to pick. */
  plan: StartPlanView | null;
  missing?: 'project' | 'host' | string | null;
  projects: { id: number; owner: string; repo: string }[];
  hosts: { alias: string; reachable: boolean }[];
  conflicts: StartConflict[];
  brief?: string | null;
  checkout?: { exists: boolean; busy_by?: number | null } | null;
  /** With `missing: 'project'`: the repository Jev proposes (K1, assist).
   *  A pre-selection only; the person still presses Start. */
  suggested_project?: { project_id: number; confidence_pct?: number | null; run_id?: number | null } | null;
  /** The same pre-selection as a proposal (redesign 2.8): feature
   *  `start_project`, value `p<id>`. */
  proposal?: DecisionProposal | null;
  /** With a planned project: the sibling repository (one the key ran in
   *  before) Jev proposes the task also needs (N3, assist). A pre-tick
   *  only; the person still presses Start. */
  suggested_sibling?: { project_id: number; confidence_pct?: number | null; run_id?: number | null } | null;
  /** The rule fleet offers after five identical starts of the key's
   *  prefix in one repository (redesign 8.11): "Add rule PD-* → acme/pos?". */
  rule_offer?: StartRule | null;
  /** The brief was drafted by a model (redesign 6.10); absent for the
   *  template. */
  brief_draft?: BriefDraft | null;
}

/** What a drafted brief says about itself. */
export interface BriefDraft {
  model: string;
  host_alias: string;
  /** Notes from earlier sessions the model read besides the ticket. */
  notes: number;
  truncated?: boolean;
}

/** Jev's K1 answer as the shared chip reads it (redesign 3.12), when the
 *  preview still lacks a project and the answer is one of its candidates. */
export function projectProposal(p: StartPreview): ProposalLike | null {
  const s = p.suggested_project;
  if (s == null || p.missing !== 'project' || !p.projects.some((x) => x.id === s.project_id)) return null;
  return { value: String(s.project_id), source: 'jev', confidence_pct: s.confidence_pct ?? null };
}

/** Jev's N3 answer (redesign 3.12): the sibling repository the same task
 *  also needs, as the shared chip reads it. Only with a planned project. */
export function siblingProposal(p: StartPreview): ProposalLike | null {
  const s = p.suggested_sibling;
  if (s == null || p.missing != null) return null;
  return { value: String(s.project_id), source: 'jev', confidence_pct: s.confidence_pct ?? null };
}

/** The project a preview pre-selects: Jev's answer, only above the floor
 *  (ai_proposal `preselect`), so the field and the chip always agree. */
export function suggestedProjectId(p: StartPreview): number | null {
  const v = preselect('project', projectProposal(p));
  return v == null ? null : Number(v);
}

/** Preview a start: the same arguments `startWork` takes. */
export function previewStartWork(args: StartWorkArgs): Promise<Result<StartPreview>> {
  return invokeCmd<StartPreview>('preview_start_work', { args });
}

/** Draft the brief from the ticket and the task's earlier work (redesign
 *  6.10): one model call on the host the start would land on. The draft is
 *  only text for the person to edit; nothing starts. A hub older than this
 *  ignores the ask and answers the template, which is said, not shown as a
 *  draft. */
export async function draftBrief(args: StartWorkArgs): Promise<Result<{ brief: string; draft: BriefDraft }>> {
  const r = await previewStartWork({ ...args, with_brief: true, brief: undefined, draft_brief: true });
  if (!r.ok) return r;
  const { brief, brief_draft } = r.value;
  if (!brief || !brief_draft) {
    const message = r.value.missing
      ? `Pick a ${r.value.missing} first.`
      : 'This hub cannot draft a brief yet; the ticket brief stays.';
    return { ok: false, error: { code: 'E_UNSUPPORTED', message } };
  }
  return { ok: true, value: { brief, draft: brief_draft } };
}

/** A brief drafted in the task page (G7.6, "Draft brief from the ticket"),
 *  held per task in this window: every start of that task from here sends
 *  it until it is cleared. Nothing is written to the task. */
export interface HeldBrief {
  brief: string;
  draft: BriefDraft;
}
export const taskBriefDrafts = writable<ReadonlyMap<string, HeldBrief>>(new Map());

/** Hold `held` for `taskId`; `null` lets it go. */
export function holdTaskBrief(taskId: string, held: HeldBrief | null): void {
  taskBriefDrafts.update((m) => {
    const next = new Map(m);
    if (held && held.brief.trim()) next.set(taskId, held);
    else next.delete(taskId);
    return next;
  });
}

/** What a draft read: "from the ticket and 3 earlier notes". */
export function draftSource(d: BriefDraft): string {
  const notes = d.notes === 1 ? '1 earlier note' : `${d.notes} earlier notes`;
  return d.notes > 0 ? `from the ticket and ${notes}` : 'from the ticket';
}

/** A hub older than the preview has no `preview_start` action: it answers
 *  `E_INVALID` naming it (or "unknown work_link action") and makes nothing.
 *  The caller then starts as it did before there was a preview. */
export function previewUnsupported(e: IpcError): boolean {
  return e.code === 'E_INVALID' && /\bpreview_start\b|unknown work_link action/.test(e.message);
}

/** What the Work button's primary half does for a task. */
export type PrimaryAction = 'open' | 'continue' | 'start';

/** Open a live session, else continue a past one, else start. */
export function primaryAction(o: { live: boolean; resumable: boolean }): PrimaryAction {
  if (o.live) return 'open';
  if (o.resumable) return 'continue';
  return 'start';
}

export const PRIMARY_LABEL: Record<PrimaryAction, string> = {
  open: 'Open',
  continue: 'Continue',
  start: 'Start',
};

/** A preview a plain click may act on without asking: everything resolved,
 *  nothing in the way. */
export function previewIsClean(p: StartPreview): boolean {
  return p.plan != null && !p.missing && p.conflicts.length === 0;
}

/** The start's arguments for a task, before any choice: by item when it has
 *  one, else by key. The brief is on: a ticket's text, or a native task's
 *  notes, is what the new session should read first. */
export function baseStartArgs(t: Pick<WorkTask, 'item_id' | 'key' | 'project_id'>): StartWorkArgs {
  const base: StartWorkArgs = t.item_id != null ? { item_id: t.item_id } : { reference: t.key ?? '' };
  if (t.project_id != null) base.project_id = t.project_id;
  base.with_brief = true;
  return base;
}

/** "Start with last settings" (G7.6, the Continue menu): the host and
 *  repository of the task's most recent session, newest link first, that
 *  named a host. The repository is that session's when the row is still
 *  known, else the task's own. `null` with no such session. */
export function lastStartSettings(
  t: Pick<WorkTask, 'project_id' | 'sessions'>,
  rows: readonly Pick<SessionRow, 'id' | 'project_id'>[],
): { host_alias: string; project_id?: number } | null {
  const links = (t.sessions ?? []).filter((l) => !!l.host && l.state !== 'suggested' && l.state !== 'rejected');
  if (links.length === 0) return null;
  const last = links.reduce((a, b) => ((b.created_at ?? 0) > (a.created_at ?? 0) ? b : a));
  const row = last.session_id != null ? rows.find((r) => r.id === last.session_id) : undefined;
  const project = row?.project_id ?? t.project_id ?? null;
  return project != null ? { host_alias: last.host!, project_id: project } : { host_alias: last.host! };
}

/** The choices the popover holds, over the base arguments. */
export interface StartChoice {
  project_id?: number | null;
  host_alias?: string | null;
  worktree?: string | null;
  with_brief: boolean;
  force_cross_org: boolean;
  parallel: boolean;
}

/** The popover's first choices, from a preview. A live session means a
 *  parallel start; a cross-org one is never chosen for the person. */
export function choiceFromPreview(p: StartPreview, withBrief = true): StartChoice {
  return {
    project_id: p.plan?.project_id ?? suggestedProjectId(p),
    host_alias: p.plan?.host_alias ?? null,
    worktree: null,
    with_brief: withBrief,
    force_cross_org: false,
    parallel: p.conflicts.some((c) => c.kind === 'live_session') || !!p.plan?.parallel,
  };
}

/** Base arguments plus the person's choices: what the preview and the start
 *  both send. */
export function argsWithChoice(base: StartWorkArgs, c: StartChoice): StartWorkArgs {
  const out: StartWorkArgs = { ...base, with_brief: c.with_brief };
  if (c.project_id != null) out.project_id = c.project_id;
  if (c.host_alias) out.host_alias = c.host_alias;
  const w = c.worktree?.trim();
  if (w) out.worktree = w;
  if (c.force_cross_org) out.force_cross_org = true;
  if (c.parallel) out.parallel = true;
  return out;
}

/** May the popover's Start go ahead with these choices? `null` when it may,
 *  else why not, in words. */
export function startBlockedBy(p: StartPreview, c: StartChoice): string | null {
  if (p.missing === 'project' && c.project_id == null) return 'Pick a repository.';
  if (p.missing === 'host' && !c.host_alias) return 'Pick a host.';
  if (p.conflicts.some((x) => x.kind === 'proposal')) return 'Accept the proposal first.';
  if (p.conflicts.some((x) => x.kind === 'cross_org') && !c.force_cross_org)
    return 'This crosses organisations: tick "Start across organisations" if it is meant.';
  const host = p.hosts.find((h) => h.alias === (c.host_alias ?? p.plan?.host_alias));
  if (host && !host.reachable) return `${host.alias} is offline: pick another host.`;
  return null;
}

/** A start from the arguments a clean preview resolved: exactly where the
 *  preview said, so a plain click and the popover land in the same place. */
export function startFromPreview(base: StartWorkArgs, p: StartPreview): Promise<Result<SessionRow>> {
  return startWork(
    argsWithChoice(base, {
      project_id: p.plan?.project_id,
      host_alias: p.plan?.host_alias,
      worktree: null,
      with_brief: base.with_brief ?? true,
      force_cross_org: false,
      parallel: !!p.plan?.parallel,
    }),
  );
}

/** `owner/repo`, or `repo` for a local owner. */
export function projectLabel(p: { owner: string; repo: string }): string {
  return p.owner && p.owner !== 'local' ? `${p.owner}/${p.repo}` : p.repo;
}

/** What the list's keyboard asks of a task's Work button. */
export interface WorkButtonHandle {
  /** The primary half: Open, Continue or Start. */
  primary(): void;
  /** Start new…: the popover, whatever the preview says. */
  ask(): void;
}

const buttons = new Set<{ id: () => string; h: WorkButtonHandle }>();

/** A mounted Work button, found by its task for `s` / ⇧S. Returns the
 *  unregister. */
export function registerWorkButton(id: () => string, h: WorkButtonHandle): () => void {
  const entry = { id, h };
  buttons.add(entry);
  return () => buttons.delete(entry);
}

/** The Work button of `taskId`, if one is mounted. */
export function workButtonFor(taskId: string): WorkButtonHandle | undefined {
  for (const e of buttons) if (e.id() === taskId) return e.h;
  return undefined;
}
