// A finished mission (Orbit Fleet G3.7, the Finish board): what it did, the
// checks it finished on, its pull requests, and the sessions it leaves
// running, which "Archive N sessions" ends.
//
// Archive is not a new kind of kill. Each session goes through the session's
// own Clean up (`kill_check.cleanUp`, the Kill dialog's): a clean, pushed
// worktree is removed with its pane at once, which frees its disk; one with
// uncommitted or unpushed work goes through the agent's Safe remove (commit,
// push, then remove). Transcripts stay either way. Each session's own grant
// decides, so a session this person may not end is left as it was, with why.

import { checkWork, cleanUp, cleanUpBlocked, type WorkCheck } from './kill_check';
import { durationWords, wavesOf, type CondCheck, type FinishSession, type MissionDetail } from './missions';
import type { SessionRow } from './sessions';
import { sizeText } from './hosts_table';

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "Archive 3 sessions". */
export function archiveLabel(n: number): string {
  return `Archive ${plural(n, 'session')}`;
}

/** The finish block's live sessions, or none. */
export function finishSessions(detail: Pick<MissionDetail, 'finish'>): FinishSession[] {
  return detail.finish?.sessions ?? [];
}

/** A span of days and hours: "3 d 4 h", "5 h", "45 min". */
export function spanWords(secs: number): string {
  if (secs < 86_400) return durationWords(secs);
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3600);
  return h ? `${d} d ${h} h` : `${d} d`;
}

/** The header's one line: "All 12 tasks verified in 4 waves over 3 d 4 h."
 *  A mission that ended without all its tasks says how many were done. */
export function finishSummary(detail: MissionDetail): string {
  const m = detail.mission;
  const members = (detail.items ?? []).filter((i) => i.id !== m.root_item_id);
  const nodes = (detail.graph?.nodes ?? []).filter((n) => n.item_id !== m.root_item_id);
  const waves = new Set(nodes.map((n) => n.wave)).size;
  const done = nodes.filter((n) => n.state === 'done').length;
  const verified = nodes.filter((n) => n.verification?.state === 'verified').length;
  const span = m.started_at != null && m.finished_at != null && m.finished_at > m.started_at ? ` over ${spanWords(m.finished_at - m.started_at)}` : '';
  const inWaves = waves > 0 ? ` in ${plural(waves, 'wave')}` : '';
  const total = members.length;
  if (total === 0) return `No tasks${span}.`;
  if (verified === total) return `All ${plural(total, 'task')} verified${inWaves}${span}.`;
  if (done === total) return `All ${plural(total, 'task')} done${inWaves}${span}.`;
  return `${done} of ${plural(total, 'task')} done${inWaves}${span}.`;
}

/** One check of the mission's finish, with the task it belongs to. */
export interface FinishCheck extends CondCheck {
  item_id: number;
}

/** The mission-level finish checks: every member's done_when answers, in
 *  graph order, with how many passed. */
export function finishChecks(detail: MissionDetail): { passed: number; checks: FinishCheck[] } {
  const checks: FinishCheck[] = [];
  for (const n of detail.graph?.nodes ?? []) {
    for (const c of n.verification?.checks ?? []) checks.push({ ...c, item_id: n.item_id });
  }
  return { passed: checks.filter((c) => c.state === 'pass').length, checks };
}

/** A person's or a run's mark on a check: "Martin · 14:20", "run · 14:02". */
export function checkByLine(c: Pick<CondCheck, 'by' | 'at'>, names: (personId: number) => string | null = () => null): string {
  const time = c.at != null ? new Date(c.at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '';
  let who = '';
  const by = c.by ?? '';
  if (by.startsWith('person:')) who = names(Number(by.slice(7))) ?? 'a person';
  else if (by.startsWith('task:')) who = `run ${by.slice(5)}`;
  else if (by) who = by;
  return [who, time].filter(Boolean).join(' · ');
}

/** The waves, each with its task titles: "✓ Wave 1 · 3 tasks". */
export function waveSummary(detail: MissionDetail): { wave: number; count: number; done: boolean; titles: string[] }[] {
  const title = new Map((detail.items ?? []).map((i) => [i.id, i.title] as const));
  const root = detail.mission.root_item_id;
  return wavesOf(detail)
    .map(({ wave, nodes }) => {
      const ns = nodes.filter((n) => n.item_id !== root);
      return {
        wave,
        count: ns.length,
        done: ns.every((n) => n.state === 'done'),
        titles: ns.map((n) => title.get(n.item_id) ?? `#${n.item_id}`),
      };
    })
    .filter((w) => w.count > 0);
}

/** The measured worktrees of `sessions`, in kB, or `null` when none is. */
export function finishFreedKb(sessions: readonly FinishSession[]): number | null {
  let total: number | null = null;
  for (const s of sessions) if (s.worktree_kb != null) total = (total ?? 0) + s.worktree_kb;
  return total;
}

/** "After archiving: moves to Missions › Completed (5). Spent $31.40 of $40 · 3 d 4 h." */
export function afterArchiveLine(detail: MissionDetail, completed: number, dollars: (micros: number) => string): string {
  const m = detail.mission;
  const parts: string[] = [];
  if (m.cost_micros != null && m.cost_micros > 0) {
    parts.push(m.budget_micros ? `Spent ${dollars(m.cost_micros)} of ${dollars(m.budget_micros)}` : `Spent ${dollars(m.cost_micros)}`);
  }
  if (m.started_at != null && m.finished_at != null && m.finished_at > m.started_at) parts.push(spanWords(m.finished_at - m.started_at));
  const tail = parts.length ? ` ${parts.join(' · ')}.` : '';
  return `After archiving: moves to Missions › Completed (${completed}).${tail}`;
}

/** One session's worktree state in a few words: "clean · pushed". */
export function checkWords(c: WorkCheck | undefined): string {
  if (!c || c.state === 'checking') return 'checking…';
  if (c.state === 'clean') return 'clean · pushed';
  if (c.state === 'unknown') return `not checked: ${c.why}`;
  const parts: string[] = [];
  parts.push(c.files.length > 0 ? `${plural(c.files.length, 'uncommitted file')}` : 'clean');
  parts.push(c.unpushed > 0 ? `${c.unpushed} not pushed` : 'pushed');
  return parts.join(' · ');
}

/** Read each session's worktree, as the Kill dialog does. */
export async function checkSessions(rows: readonly SessionRow[]): Promise<Map<number, WorkCheck>> {
  const got = await Promise.all(rows.map((r) => checkWork(r)));
  return new Map(rows.map((r, i) => [r.id, got[i]] as const));
}

/** What one Archive press did. */
export interface MissionArchiveOutcome {
  /** Ended with their worktree removed. */
  removed: number[];
  /** Handed to the agent's Safe remove: commits and pushes, then removes. */
  asked: number[];
  /** Left as they were, with why. */
  skipped: { session_id: number; why: string }[];
  /** The measured size of the worktrees removed now, kB, or `null`. */
  freedKb: number | null;
}

/**
 * Archive a finished mission's sessions: each one's Clean up, one after
 * another. A session this view has no row for (not loaded, or not this
 * person's to see) and one this person may not clean up are skipped.
 */
export async function archiveMissionSessions(
  sessions: readonly FinishSession[],
  rowOf: (id: number) => SessionRow | undefined,
  checks: ReadonlyMap<number, WorkCheck>,
): Promise<MissionArchiveOutcome> {
  const out: MissionArchiveOutcome = { removed: [], asked: [], skipped: [], freedKb: null };
  for (const s of sessions) {
    const row = rowOf(s.session_id);
    if (!row) {
      out.skipped.push({ session_id: s.session_id, why: 'not yours to clean up' });
      continue;
    }
    const blocked = cleanUpBlocked(row);
    if (blocked !== null) {
      out.skipped.push({ session_id: s.session_id, why: blocked });
      continue;
    }
    let check = checks.get(s.session_id);
    if (!check || check.state === 'checking') check = await checkWork(row);
    const r = await cleanUp(row, check);
    if (!r.ok) {
      out.skipped.push({ session_id: s.session_id, why: r.error.message });
    } else if (r.value === 'removed') {
      out.removed.push(s.session_id);
      if (s.worktree_kb != null) out.freedKb = (out.freedKb ?? 0) + s.worktree_kb;
    } else {
      out.asked.push(s.session_id);
    }
  }
  return out;
}

/** The toast after an Archive press. */
export function archiveOutcomeLine(o: MissionArchiveOutcome): string {
  const parts: string[] = [];
  if (o.removed.length) parts.push(`Archived ${plural(o.removed.length, 'session')}${o.freedKb != null ? ` · freed about ${sizeText(o.freedKb)}` : ''}`);
  if (o.asked.length) parts.push(`${plural(o.asked.length, 'agent')} committing and pushing first`);
  if (o.skipped.length) parts.push(`${o.skipped.length} left as ${o.skipped.length === 1 ? 'it was' : 'they were'} (${[...new Set(o.skipped.map((x) => x.why))].join(', ')})`);
  return parts.length ? parts.join(' · ') : 'Nothing to archive';
}
