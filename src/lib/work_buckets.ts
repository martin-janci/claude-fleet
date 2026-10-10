// Sprints and releases (design `docs/superpowers/specs/2026-09-28-sprints-releases-epics-design.md`):
// the reads (`work_buckets`, `work_bucket`), a person's membership
// (`add_work_to_bucket`, `remove_work_from_bucket`, both routed) and the
// buckets themselves (`work_bucket_admin`: create, update, close, delete —
// `work_admin` standalone, `work_link`'s `bucket_admin` on a paired desktop,
// where the hub decides who may plan and a bucket of no org is personal).
import { writable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';
import { bumpWorkChanged, setWorkParent } from './work';
import { readPref, writePref } from './prefs';
import type { WorkItemRow } from './trackers';
import { workTree, type WorkTreeFilters } from './work_view';

export type BucketKind = 'sprint' | 'release';

/** Mirrors `store::BucketRow`. */
export interface BucketRow {
  id: number;
  kind: BucketKind | string;
  name: string;
  org_id?: number | null;
  /** A personal bucket's person: theirs alone. Absent: the team's. */
  owner_person_id?: number | null;
  /** sprint: planned | active | closed; release: planned | released */
  state: string;
  /** Unix seconds. */
  starts_at?: number | null;
  /** A sprint's end; a release's target date. Unix seconds. */
  ends_at?: number | null;
  shipped_at?: number | null;
  shipped_ref?: string | null;
  goal?: string | null;
  created_at: number;
  updated_at: number;
  version: number;
  /** Current members. */
  total?: number;
  /** Current members that are done. */
  done?: number;
}

/** Mirrors `store::BucketMemberRow`. */
export interface BucketMemberRow {
  item: WorkItemRow;
  /** manual | adopted */
  source: string;
  added_at: number;
  removed_at?: number | null;
}

/** Mirrors `service::work::buckets::BucketDetail`. */
export interface BucketDetail {
  bucket: BucketRow;
  members?: BucketMemberRow[];
}

/** Mirrors `service::work::buckets::BucketAnswer`. */
export interface BucketAnswer {
  bucket: BucketRow;
  warning?: string | null;
}

/** Mirrors `store::SprintClosed`. */
export interface SprintClosed {
  bucket: BucketRow;
  ended: number[];
  carried: number[];
  carry_to?: number | null;
}

export function workBuckets(kind?: BucketKind): Promise<Result<BucketRow[]>> {
  return invokeCmd<BucketRow[]>('work_buckets', { args: kind ? { kind } : {} });
}

export function workBucket(bucketId: number): Promise<Result<BucketDetail>> {
  return invokeCmd<BucketDetail>('work_bucket', { args: { bucket_id: bucketId } });
}

export function addToBucket(bucketId: number, itemId: number): Promise<Result<BucketRow>> {
  return invokeCmd<BucketRow>('add_work_to_bucket', { args: { bucket_id: bucketId, item_id: itemId } });
}

export function removeFromBucket(bucketId: number, itemId: number): Promise<Result<BucketRow>> {
  return invokeCmd<BucketRow>('remove_work_from_bucket', { args: { bucket_id: bucketId, item_id: itemId } });
}

/** What a person types for a new bucket. Dates are `YYYY-MM-DD`. */
export interface BucketInput {
  kind: BucketKind;
  name: string;
  org_id?: number | null;
  starts_on?: string;
  ends_on?: string;
  goal?: string;
}

/** `YYYY-MM-DD` → Unix seconds at UTC noon (a date reads the same day in
 *  every zone); `''` / absent → absent. */
export function dayToUnix(day: string | undefined): number | undefined {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec((day ?? '').trim());
  if (!m) return undefined;
  return Math.floor(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), 12) / 1000);
}

/** Unix seconds → `YYYY-MM-DD` (UTC, the inverse of `dayToUnix`). */
export function unixToDay(secs: number | null | undefined): string {
  if (typeof secs !== 'number') return '';
  return new Date(secs * 1000).toISOString().slice(0, 10);
}

async function admin<T>(args: Record<string, unknown>): Promise<Result<T>> {
  const r = await invokeCmd<T>('work_bucket_admin', { args });
  if (r.ok) bumpWorkChanged();
  return r;
}

export function createBucket(b: BucketInput): Promise<Result<BucketAnswer>> {
  const args: Record<string, unknown> = { action: 'bucket_create', kind: b.kind, name: b.name.trim() };
  if (b.org_id != null) args.org_id = b.org_id;
  const starts = b.kind === 'sprint' ? dayToUnix(b.starts_on) : undefined;
  const ends = dayToUnix(b.ends_on);
  if (starts !== undefined) args.starts_at = starts;
  if (ends !== undefined) args.ends_at = ends;
  if (b.goal?.trim()) args.goal = b.goal.trim();
  return admin<BucketAnswer>(args);
}

/** Start a planned sprint, or mark a planned release released. */
export function advanceBucket(b: BucketRow): Promise<Result<BucketAnswer>> {
  return admin<BucketAnswer>({
    action: 'bucket_update',
    bucket_id: b.id,
    expected_version: b.version,
    state: b.kind === 'sprint' ? 'active' : 'released',
  });
}

/** Close a sprint (E9): `carry` (the unfinished members a person kept
 *  ticked) moves to `carryTo`; with no `carryTo` nothing is carried. */
export function closeSprint(b: BucketRow, carryTo: number | null, carry: number[]): Promise<Result<SprintClosed>> {
  const args: Record<string, unknown> = { action: 'bucket_close', bucket_id: b.id, expected_version: b.version };
  if (carryTo != null) {
    args.carry_to = carryTo;
    args.carry = carry;
  }
  return admin<SprintClosed>(args);
}

export function deleteBucket(b: BucketRow): Promise<Result<{ removed: number }>> {
  return admin<{ removed: number }>({ action: 'bucket_delete', bucket_id: b.id });
}

/** `sprint:12` / `release:3` (a Work view section grouped by sprint or
 *  release) → the bucket id; anything else → null. */
export function bucketIdOfGroup(groupId: string): number | null {
  const m = /^(?:sprint|release):(\d+)$/.exec(groupId);
  return m ? Number(m[1]) : null;
}

const shortDate = (secs: number) =>
  new Date(secs * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', timeZone: 'UTC' });

/** A section header's roll-up: "Active · 4/9 done · ends 20 Oct". */
export function bucketSummary(b: BucketRow): string {
  const parts: string[] = [b.state.charAt(0).toUpperCase() + b.state.slice(1)];
  if (typeof b.total === 'number') parts.push(`${b.done ?? 0}/${b.total} done`);
  if (b.kind === 'release' && b.shipped_at) parts.push(`shipped ${shortDate(b.shipped_at)}`);
  else if (b.ends_at) parts.push(`${b.kind === 'sprint' ? 'ends' : 'target'} ${shortDate(b.ends_at)}`);
  return parts.join(' · ');
}

/** The buckets a task can still be planned into: no closed sprint. Active
 *  sprints first, then planned, then by name. */
export function openBuckets(all: readonly BucketRow[], kind: BucketKind): BucketRow[] {
  const rank = (b: BucketRow) => (b.state === 'active' ? 0 : b.state === 'planned' ? 1 : 2);
  return all
    .filter((b) => b.kind === kind && b.state !== 'closed')
    .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}

export interface BulkOutcome {
  done: number;
  failed: { itemId: number; error: IpcError }[];
}

/** Put every item in `bucket`. An item already in another sprint is MOVED
 *  (taken out of that sprint first): picking a sprint for it says where it
 *  belongs now. One item's refusal does not stop the rest. */
export async function planInto(bucket: BucketRow, itemIds: readonly number[]): Promise<BulkOutcome> {
  const out: BulkOutcome = { done: 0, failed: [] };
  for (const itemId of itemIds) {
    let r = await addToBucket(bucket.id, itemId);
    const other = !r.ok && r.error.code === 'E_CONFLICT' ? (r.error.details as { sprint_id?: unknown } | null)?.sprint_id : undefined;
    if (!r.ok && typeof other === 'number' && other !== bucket.id) {
      const off = await removeFromBucket(other, itemId);
      r = off.ok ? await addToBucket(bucket.id, itemId) : off;
    }
    if (r.ok) out.done++;
    else out.failed.push({ itemId, error: r.error });
  }
  if (out.done > 0) bumpWorkChanged();
  return out;
}

/** Take every item out of `bucket`. */
export async function unplanFrom(bucket: BucketRow, itemIds: readonly number[]): Promise<BulkOutcome> {
  const out: BulkOutcome = { done: 0, failed: [] };
  for (const itemId of itemIds) {
    const r = await removeFromBucket(bucket.id, itemId);
    if (r.ok) out.done++;
    else out.failed.push({ itemId, error: r.error });
  }
  if (out.done > 0) bumpWorkChanged();
  return out;
}

/** What the board shows (sprints design §6c): every task the Work view's
 *  filters match (`all`), one sprint's tasks (its id), or the tasks in no
 *  sprint (`none`, the backlog a sprint is planned from). */
export type BoardScope = 'all' | 'none' | number;

const isBoardScope = (v: unknown): v is BoardScope =>
  v === 'all' || v === 'none' || (typeof v === 'number' && Number.isInteger(v) && v > 0);

/** The board's scope, kept per machine. */
export const boardScope = writable<BoardScope>(readPref('work.board.scope', 'all', isBoardScope));
boardScope.subscribe((v) => writePref('work.board.scope', v));

/** The board's read: the Work view's filters (their own grouping and
 *  section dropped) narrowed to one sprint's section, or the no-sprint one,
 *  of a group by sprint. */
export function boardFilters(view: WorkTreeFilters, scope: BoardScope): WorkTreeFilters {
  const { status: _s, ...rest } = view;
  if (scope === 'all') return { ...rest, archived: true };
  const { group: _g, group_by: _b, ...narrow } = rest;
  return { ...narrow, archived: true, group_by: 'sprint', group: scope === 'none' ? 'none' : `sprint:${scope}` };
}

/** The scope to show: a sprint that is gone or closed (its tasks have left
 *  it) falls back to every task. Before the sprints are read, as chosen. */
export function liveScope(scope: BoardScope, sprints: readonly BucketRow[] | null): BoardScope {
  if (typeof scope !== 'number' || sprints === null) return scope;
  const b = sprints.find((x) => x.id === scope && x.kind === 'sprint');
  return b && b.state !== 'closed' ? scope : 'all';
}

/** An epic a task can be filed under: a section of a group by epic. */
export interface EpicChoice {
  itemId: number;
  label: string;
  orgId: number | null;
}

/** Every epic this reader sees (sprints design §3, §6b), from one
 *  `work_tree` read grouped by epic: an epic is its own section, so each
 *  one is there even with nothing filed under it yet. */
export async function workEpics(): Promise<Result<EpicChoice[]>> {
  const r = await workTree({ filters: { group_by: 'epic', archived: true }, limit: 1 });
  if (!r.ok) return r;
  const out: EpicChoice[] = [];
  for (const g of Array.isArray(r.value?.groups) ? r.value.groups : []) {
    const m = /^epic:(\d+)$/.exec(g.group.id);
    if (m) out.push({ itemId: Number(m[1]), label: g.group.label, orgId: g.org_id ?? null });
  }
  out.sort((a, b) => a.label.localeCompare(b.label));
  return { ok: true, value: out };
}

/** File every item under `parentItemId`, or take each out to the top
 *  (`null`). One item's refusal does not stop the rest. */
export async function fileUnder(parentItemId: number | null, itemIds: readonly number[]): Promise<BulkOutcome> {
  const out: BulkOutcome = { done: 0, failed: [] };
  for (const itemId of itemIds) {
    if (itemId === parentItemId) continue;
    const r = await setWorkParent(itemId, parentItemId);
    if (r.ok) out.done++;
    else out.failed.push({ itemId, error: r.error });
  }
  return out;
}
