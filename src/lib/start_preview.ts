// The start preview (task → session spec P-1): where a start would land,
// what it would send and what is in the way — nothing is made. The Work
// button asks for it first: a clean preview starts at once, anything else
// opens the start popover with the conflicts and the choices.
import { invokeCmd, type IpcError, type Result } from './result';
import { startWork, type StartWorkArgs } from './trackers';
import type { SessionRow } from './sessions';
import type { WorkTask } from './work_view';

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
}

/** Preview a start: the same arguments `startWork` takes. */
export function previewStartWork(args: StartWorkArgs): Promise<Result<StartPreview>> {
  return invokeCmd<StartPreview>('preview_start_work', { args });
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
    project_id: p.plan?.project_id ?? null,
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
