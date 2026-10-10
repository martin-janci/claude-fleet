// Lost and found with proposals (Orbit Fleet redesign step 4.12, board
// HostDetail "Lost & found"): Adopt into for a live pane fleet did not
// start (N4), Restore into for a conversation whose pane is gone (J10).
// Each form is prefilled with a project: the rule's (the directory is in
// the project), else Jev's when `decide.jev.adopt_target` /
// `decide.jev.restore_target` is at assist, else nothing. A person still
// presses the button and confirms. Pure but for the invoke wrappers;
// LostTargetForm.svelte renders.
import { invokeCmd, type Result } from './result';
import { readPref, writePref } from './prefs';
import type { ProposalLike } from './ai_proposal';
import type { ProjectRow } from './projects';
import type { LostCandidate, SessionRow } from './sessions';

/** Mirrors `fleet_core::service::decide::lost_target::LostTarget`. */
export interface LostTarget {
  project_id?: number | null;
  source?: 'rule' | 'jev' | null;
  reason?: string | null;
  confidence_pct?: number | null;
  run_id?: number | null;
  /** Jev was asked and named no project: the form stays blank, and says so. */
  unsure?: boolean;
  /** J10's other half (a found conversation only): the ticket its branch names. */
  ticket?: LostTicket | null;
}

/** Mirrors `fleet_core::service::decide::lost_target::LostTicket`. */
export interface LostTicket {
  key: string;
  title?: string | null;
  source: 'rule';
  reason: string;
}

/** Mirrors `fleet_core::service::sessions::LostTargetArgs`. */
export type LostTargetArgs =
  | { session_id: number }
  | { host_alias: string; claude_session_id: string; cwd: string; git_branch?: string | null };

/** Mirrors `fleet_core::service::sessions::PlacedTranscript`. */
export interface PlacedTranscript {
  project_id: number;
  tmux_name: string;
  copied: boolean;
}

/** The line a blank form shows when Jev was asked and was unsure. */
export const UNSURE_NOTE = 'Jev was unsure, so nothing is filled in';

/**
 * Whether `row` is a pane fleet did not start and may adopt: mirrors
 * `fleet_core::service::sessions::is_outside_fleet`.
 */
export function isOutsideFleet(row: SessionRow): boolean {
  return (
    row.started_at === null &&
    row.status !== 'ghost' &&
    row.lost_at === null &&
    row.kind !== 'bg' &&
    row.kind !== 'external'
  );
}

/**
 * Whether a found conversation needs Restore into a project: it has no row
 * yet, and resuming where it ran is not possible (outside every project, or
 * not a registered checkout). The resumable ones keep their plain Resume.
 */
export function needsRestoreInto(c: LostCandidate): boolean {
  return c.existing_session_id === null && !(c.resumable && c.project_id !== null && c.derived_tmux_name !== null);
}

/** The projects a form offers: the person's own, most recently used first. */
export function pickableProjects(rows: readonly ProjectRow[]): ProjectRow[] {
  return rows
    .filter((p) => !p.system)
    .slice()
    .sort((a, b) => (b.last_session_at ?? 0) - (a.last_session_at ?? 0));
}

/** `acme/papaya-pos`. */
export function projectLabel(p: Pick<ProjectRow, 'owner' | 'repo'>): string {
  return `${p.owner}/${p.repo}`;
}

/**
 * The proposal as ProposedBy reads it, with the project id as its value;
 * `null` when the target proposes nothing (or names a project this list
 * does not hold).
 */
export function proposalOf(t: LostTarget | null, projects: readonly ProjectRow[]): ProposalLike | null {
  if (!t || t.project_id == null || !t.source) return null;
  if (!projects.some((p) => p.id === t.project_id)) return null;
  return {
    value: String(t.project_id),
    source: t.source,
    reason: t.reason ?? null,
    confidence_pct: t.confidence_pct ?? null,
  };
}

/** `PD-2412 · Receipt totals`, or the bare key when no title is cached. */
export function ticketLabel(t: LostTicket): string {
  return t.title ? `${t.key} · ${t.title}` : t.key;
}

/** The ticket proposal as ProposedBy reads it. */
export function ticketProposalOf(t: LostTicket): ProposalLike {
  return { value: t.key, source: t.source, reason: t.reason, confidence_pct: null };
}

/** Whether the blank form says Jev was unsure. */
export function showsUnsure(t: LostTarget | null): boolean {
  return !!t && t.unsure === true && t.project_id == null;
}

/** The confirm dialog's title: `Adopt scratch into acme/papaya-pos?`. */
export function confirmTitle(action: 'Adopt' | 'Restore', entry: string, project: ProjectRow | null): string {
  return project ? `${action} ${entry} into ${projectLabel(project)}?` : `${action} ${entry} without a project?`;
}

// ---- the backend ---------------------------------------------------------

export function lostTarget(args: LostTargetArgs): Promise<Result<LostTarget>> {
  return invokeCmd<LostTarget>('lost_target', { args });
}

export function placeTranscript(args: {
  host_alias: string;
  claude_session_id: string;
  project_id: number;
}): Promise<Result<PlacedTranscript>> {
  return invokeCmd<PlacedTranscript>('place_transcript', { args });
}

// ── Ignore (gap plan step G2.7, the FormsSession board's "Adopt a lost
// session · Ignore") ──
//
// A found conversation a person does not want back is left out of the list
// on later searches. Nothing on the host changes and the hub keeps nothing:
// the choice is this device's (a pref), so it is undone with Undo or "Show
// ignored", and another device still lists it.

const IGNORED_PREF = 'lost.ignored-conversations';
const isIgnoredMap = (v: unknown): v is Record<string, string[]> =>
  typeof v === 'object' &&
  v !== null &&
  !Array.isArray(v) &&
  Object.values(v).every((ids) => Array.isArray(ids) && ids.every((id) => typeof id === 'string'));

/** The conversations ignored on `host`, by `claude_session_id`. */
export function ignoredConversations(host: string): ReadonlySet<string> {
  return new Set(readPref(IGNORED_PREF, {}, isIgnoredMap)[host] ?? []);
}

/** Ignore (`ignored`) or bring back one found conversation on `host`. */
export function setConversationIgnored(host: string, claudeSessionId: string, ignored: boolean): void {
  const all = readPref(IGNORED_PREF, {}, isIgnoredMap);
  const ids = new Set(all[host] ?? []);
  if (ignored) ids.add(claudeSessionId);
  else ids.delete(claudeSessionId);
  const next = { ...all };
  if (ids.size > 0) next[host] = [...ids];
  else delete next[host];
  writePref(IGNORED_PREF, next);
}
