// The Work view's read side (work graph M14.2): the sidebar tree
// org → group → task → session occurrences, built from `work { tree }`
// pages, with its filters, saved views (applied, never written here),
// per-view expansion / selection prefs and the live patching.
//
// Contract: only the typed wrappers of `work_view.ts` and the batched
// `work:changed` / `session:*` handlers of `events.ts`. Section headers come
// from a page's `groups` (they count the whole filtered result); each
// section then pages its own tasks with `filters.group` (+ its org) and the
// keyset cursor. A `work:changed` naming a task patches that task in place
// (`mergeOne` / `removeOne` over the loaded sections) and re-reads only the
// header counts; a `resync` (`needsFullReload`), a rule change or a change
// without an id reloads the whole view. Session rows keep occurrence states
// current without a read; a change in a row's links re-reads only the
// sections that show that session. Everything is debounced, so a reconcile
// burst costs one read, not one per row.

import { get, writable, type Readable } from 'svelte/store';
import type { IpcError, Result } from './result';
import type { SessionEvent, SessionRow } from './sessions';
import type { ProjectTreeRow } from './projects';
import type { TicketRow } from './trackers';
import { placeForTicket } from './quick_switcher';
import { readPref, writePref } from './prefs';
import {
  needsFullReload,
  needsNewerHub,
  workTask,
  workTree,
  workViews,
  type GroupRef,
  type TaskDetail,
  type TreeOpts,
  type TreePage,
  type WorkChanged,
  type WorkTask,
  type WorkTaskLink,
  type WorkTreeFilters,
  type WorkView,
} from './work_view';

// ── pure helpers ────────────────────────────────────────────────────────────

/** Tasks per section page. */
export const SECTION_PAGE = 25;
/** A section re-read keeps what was loaded, up to the hub's page cap. */
export const MAX_PAGE = 200;
/** Occurrences per task in the tree (the task detail shows all). */
export const PER_TASK = 8;

/** The filters as the bar edits them; `filtersToRequest` drops defaults. */
export interface UiFilters {
  org: number | 'none' | null;
  tracker: number | 'local' | 'ref' | null;
  status: 'any' | 'open' | 'todo' | 'in_progress' | 'done';
  mine: boolean;
  has: 'any' | 'active' | 'past_only' | 'none' | 'suggested';
  review: boolean;
  query: string;
}

export const EMPTY_FILTERS: UiFilters = {
  org: null,
  tracker: null,
  status: 'any',
  mine: false,
  has: 'any',
  review: false,
  query: '',
};

/** The bar's filters → the contract's `WorkTreeFilters`, defaults left out
 *  so an unfiltered view sends `{}` (and a cursor stays bound to exactly
 *  what was asked). */
export function filtersToRequest(f: UiFilters): WorkTreeFilters {
  const out: WorkTreeFilters = {};
  if (f.org !== null) out.org = f.org;
  if (f.tracker !== null) out.tracker = f.tracker;
  if (f.status !== 'any') out.status = f.status;
  if (f.mine) out.mine = true;
  if (f.has !== 'any') out.has = f.has;
  if (f.review) out.review = true;
  const q = f.query.trim();
  if (q) out.query = q;
  return out;
}

/** A saved view's filters → the bar's (a view's `group` is not a bar
 *  filter: it would hide every other section). Unknown values fall back. */
export function uiFiltersOf(v: WorkTreeFilters): UiFilters {
  const statuses = ['any', 'open', 'todo', 'in_progress', 'done'];
  const hases = ['any', 'active', 'past_only', 'none', 'suggested'];
  return {
    org: typeof v.org === 'number' || v.org === 'none' ? v.org : null,
    tracker:
      typeof v.tracker === 'number' || v.tracker === 'local' || v.tracker === 'ref' ? v.tracker : null,
    status: v.status && statuses.includes(v.status) ? v.status : 'any',
    mine: v.mine === true,
    has: v.has && hases.includes(v.has) ? v.has : 'any',
    review: v.review === true,
    query: typeof v.query === 'string' ? v.query : '',
  };
}

export function sameFilters(a: UiFilters, b: UiFilters): boolean {
  return JSON.stringify(filtersToRequest(a)) === JSON.stringify(filtersToRequest(b));
}

/** One section: an org's group. Its key is stable across reloads. */
export function sectionKey(orgId: number | null | undefined, groupId: string): string {
  return `${orgId ?? 'none'}|${groupId}`;
}

/** The request of one section's pages: the view's filters narrowed to the
 *  section's org and group. */
export function sectionFilters(base: WorkTreeFilters, orgId: number | null | undefined, groupId: string): WorkTreeFilters {
  return { ...base, org: orgId ?? 'none', group: groupId };
}

export interface Section {
  key: string;
  orgId: number | null;
  group: GroupRef;
  /** The whole filtered count, from `groups`. */
  count: number;
  tasks: WorkTask[];
  /** `undefined`: never loaded; `null`: the last page is loaded. */
  cursor: string | null | undefined;
  loading: boolean;
  error: string | null;
}

export interface OrgSection {
  key: string;
  orgId: number | null;
  name: string;
  color?: string;
  count: number;
  sections: Section[];
}

/** A page's `groups` → org sections of group sections, in the hub's order
 *  (named orgs by name, unassigned last; groups by label, `none` last).
 *  Tasks already loaded under a section that still exists are kept. */
export function buildTree(page: Pick<TreePage, 'groups' | 'orgs'>, prev: readonly OrgSection[] = []): OrgSection[] {
  const kept = new Map<string, Section>();
  for (const o of prev) for (const s of o.sections) kept.set(s.key, s);
  const colors = new Map(page.orgs.map((o) => [o.id, o.color]));
  const out: OrgSection[] = [];
  const byOrg = new Map<string, OrgSection>();
  for (const g of page.groups) {
    const orgId = g.org_id ?? null;
    const ok = orgId === null ? 'none' : String(orgId);
    let org = byOrg.get(ok);
    if (!org) {
      org = {
        key: ok,
        orgId,
        name: orgId === null ? 'Unassigned' : (g.org_name ?? `org ${orgId}`),
        color: orgId === null ? undefined : colors.get(orgId),
        count: 0,
        sections: [],
      };
      byOrg.set(ok, org);
      out.push(org);
    }
    const key = sectionKey(orgId, g.group.id);
    const old = kept.get(key);
    org.count += g.count;
    org.sections.push(
      old
        ? { ...old, group: g.group, count: g.count }
        : { key, orgId, group: g.group, count: g.count, tasks: [], cursor: undefined, loading: false, error: null },
    );
  }
  return out;
}

/** `mergeOne` for tasks: replace in place by `task_id`, else append. */
export function mergeTask(list: readonly WorkTask[], t: WorkTask): WorkTask[] {
  const i = list.findIndex((x) => x.task_id === t.task_id);
  if (i < 0) return [...list, t];
  const next = list.slice();
  next[i] = t;
  return next;
}

/** `removeOne` for tasks. */
export function removeTask(list: readonly WorkTask[], taskId: string): WorkTask[] {
  return list.some((x) => x.task_id === taskId) ? list.filter((x) => x.task_id !== taskId) : (list as WorkTask[]);
}

/** A page appended under a section: never a task twice. */
export function appendPage(list: readonly WorkTask[], page: readonly WorkTask[]): WorkTask[] {
  let out = list as WorkTask[];
  for (const t of page) out = mergeTask(out, t);
  return out;
}

export type OccurrenceKind = 'primary' | 'secondary' | 'suggested' | 'past' | 'rejected';

/** How one occurrence is drawn: primary ★, secondary, suggested (dashed),
 *  past (dimmed, never "active"). An unknown state (a newer hub) is past:
 *  it must never pass for a live link. */
export function occurrenceKind(l: Pick<WorkTaskLink, 'state' | 'primary'>): OccurrenceKind {
  switch (l.state) {
    case 'active':
      return l.primary ? 'primary' : 'secondary';
    case 'suggested':
      return 'suggested';
    case 'rejected':
      return 'rejected';
    default:
      return 'past';
  }
}

/** The occurrences the tree shows: rejected links only in details. */
export function treeOccurrences(t: WorkTask): WorkTaskLink[] {
  return t.sessions.filter((l) => occurrenceKind(l) !== 'rejected');
}

/** `1 active / 2 past` (suggestions are the `?` and the dashed rows). */
export function countsLabel(t: Pick<WorkTask, 'counts'>): string {
  return `${t.counts.active} active / ${t.counts.ended} past`;
}

/** The tracker is failing (not merely not configured yet): its data is as
 *  of the last good sync, and "no sessions" may not be true. */
export function trackerDown(state: string | null | undefined): boolean {
  return !!state && state !== 'ok' && state !== 'unconfigured';
}

/** Where a task's org comes from, in words. */
export function orgSourceLabel(t: Pick<WorkTask, 'org_source' | 'org_fenced' | 'org_mixed'>): string {
  const base: Record<string, string> = {
    tracker: "the tracker's org",
    item: 'set on the task',
    sessions: 'from its sessions',
    none: 'no org',
  };
  const s = base[t.org_source] ?? t.org_source;
  if (t.org_mixed) return `${s} (sessions span orgs)`;
  return t.org_fenced ? `${s} · a boundary` : s;
}

/** Where a task's group comes from, in words. */
export function groupSourceLabel(g: Pick<GroupRef, 'source' | 'rule_id'>): string {
  switch (g.source) {
    case 'manual':
      return 'placed by a person';
    case 'rule':
      return g.rule_id != null ? `placement rule #${g.rule_id}` : 'a placement rule';
    case 'tracker':
      return "the tracker's project";
    case 'repo':
      return 'the repository of its latest session';
    case 'key':
      return 'the key prefix';
    case 'none':
      return 'nothing known';
    default:
      return g.source;
  }
}

/** A session row's live facts, patched onto its occurrences without a read. */
export function patchOccurrences(t: WorkTask, row: Pick<SessionRow, 'id' | 'claude_status'>): WorkTask {
  let changed = false;
  const sessions = t.sessions.map((l) => {
    if (l.session_id !== row.id || l.claude_status === (row.claude_status ?? undefined)) return l;
    changed = true;
    return { ...l, claude_status: row.claude_status ?? undefined };
  });
  return changed ? { ...t, sessions } : t;
}

/** A killed session: its live occurrences become past (never shown as
 *  active once the session is gone), and the counts follow. */
export function endOccurrences(t: WorkTask, sessionId: number): WorkTask {
  let ended = 0;
  let dropped = 0;
  const sessions = t.sessions.flatMap((l) => {
    if (l.session_id !== sessionId) return [l];
    if (l.state === 'active') {
      ended++;
      return [{ ...l, state: 'ended', primary: false, session_id: undefined, claude_status: undefined }];
    }
    if (l.state === 'suggested') {
      dropped++;
      return [];
    }
    return [l];
  });
  if (ended === 0 && dropped === 0) return t;
  return {
    ...t,
    sessions,
    counts: {
      active: Math.max(0, t.counts.active - ended),
      ended: t.counts.ended + ended,
      suggested: Math.max(0, t.counts.suggested - dropped),
    },
  };
}

/** What in a session row says its links changed: the primary and the top
 *  suggestion. (`work_rev`, which would also cover secondary links, is not
 *  built; see the spec's Revisions.) */
export function linkSignature(r: Pick<SessionRow, 'work' | 'work_suggested'>): string {
  const w = r.work;
  const s = r.work_suggested;
  return [w?.link_id ?? '', w?.state ?? '', s?.link_id ?? '', s?.suggestions ?? ''].join('|');
}

/** Where *Start new* opens the new-session dialog: the project (and host)
 *  of the task's newest session still in the list, else the project named
 *  by its repository, else where sessions on the same key family run, else
 *  the most recently used project. `null` when there are no projects. */
export function projectForTask(
  t: Pick<WorkTask, 'key' | 'repos' | 'sessions'>,
  sessionRows: readonly SessionRow[],
  projectRows: readonly ProjectTreeRow[],
): { project: ProjectTreeRow; host?: string } | null {
  const pickable = projectRows.filter((p) => !p.project.system);
  const byId = new Map(pickable.map((p) => [p.project.id, p]));
  const rows = t.sessions
    .map((l) => (l.session_id == null ? undefined : sessionRows.find((r) => r.id === l.session_id)))
    .filter((r): r is SessionRow => !!r && r.project_id != null && byId.has(r.project_id))
    .sort((a, b) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0));
  if (rows[0]) return { project: byId.get(rows[0].project_id!)!, host: rows[0].host_alias };
  for (const repo of t.repos ?? []) {
    const p = pickable.find((x) => `${x.project.owner}/${x.project.repo}` === repo);
    if (p) return { project: p };
  }
  if (t.key) {
    const place = placeForTicket(t.key, sessionRows, pickable, (s) => s.work?.key ?? null);
    if (place) return place;
  }
  const recent = [...pickable].sort((a, b) => (b.project.last_session_at ?? 0) - (a.project.last_session_at ?? 0))[0];
  return recent ? { project: recent } : null;
}

/** A work item as the new-session dialog's ticket (it links the new
 *  session and offers the brief); a bare key has none. */
export function ticketOf(t: WorkTask): TicketRow | undefined {
  if (t.item_id == null) return undefined;
  return {
    id: t.item_id,
    source: t.kind === 'local' ? 'local' : (t.provider ?? 'tracker'),
    key: t.key ?? null,
    title: t.title,
    url: t.url ?? null,
    status_category: t.status_category ?? 'todo',
    status_name: t.status_name ?? null,
    tracker_id: t.tracker_id ?? null,
    assignees: t.assignees ?? [],
    created_at: 0,
    updated_at: 0,
    live_session_ids: t.sessions.filter((l) => l.state === 'active' && l.session_id != null).map((l) => l.session_id!),
  };
}

// ── prefs (per view) ────────────────────────────────────────────────────────

/** `adhoc` when no saved view is applied. */
export type ViewId = number | 'adhoc';

interface ViewPrefs {
  expanded: string[];
  collapsedOrgs: string[];
  task: string | null;
}

const isViewPrefs = (v: unknown): v is ViewPrefs =>
  !!v &&
  typeof v === 'object' &&
  Array.isArray((v as ViewPrefs).expanded) &&
  Array.isArray((v as ViewPrefs).collapsedOrgs) &&
  ((v as ViewPrefs).task === null || typeof (v as ViewPrefs).task === 'string');

export function prefKey(view: ViewId): string {
  return `work.view.${view}`;
}

export function readViewPrefs(view: ViewId): ViewPrefs | null {
  return readPref<ViewPrefs | null>(prefKey(view), null, (v): v is ViewPrefs | null => isViewPrefs(v));
}

// ── the sidebar mode and cross-component requests ───────────────────────────

export type SidebarMode = 'sessions' | 'work';
const isMode = (v: unknown): v is SidebarMode => v === 'sessions' || v === 'work';

/** `Sessions | Work`, kept across restarts. */
export const sidebarMode = writable<SidebarMode>(readPref<SidebarMode>('ui.sidebarMode', 'sessions', isMode));
sidebarMode.subscribe((v) => writePref('ui.sidebarMode', v));

/** The task shown in the center pane (Details), or null. Opening a session
 *  from anywhere clears it (Details). */
export const openTaskId = writable<string | null>(null);

// ── the store ───────────────────────────────────────────────────────────────

export type LoadStatus = 'idle' | 'loading' | 'ready' | 'error' | 'needs_hub';

export interface WorkTreeState {
  status: LoadStatus;
  error: string | null;
  orgs: OrgSection[];
  orgList: TreePage['orgs'];
  trackers: TreePage['trackers'];
  total: number;
  filters: UiFilters;
  view: ViewId;
  views: WorkView[];
  viewsError: string | null;
  expanded: Set<string>;
  collapsedOrgs: Set<string>;
  /** The last task picked in this view (highlighted; kept per view). */
  selected: string | null;
}

export interface WorkTreeDeps {
  tree: (opts: TreeOpts) => Promise<Result<TreePage>>;
  task: (taskId: string) => Promise<Result<TaskDetail>>;
  views: () => Promise<Result<WorkView[]>>;
  readPrefs: (view: ViewId) => ViewPrefs | null;
  writePrefs: (view: ViewId, p: ViewPrefs) => void;
  /** Debounce for event-driven reads, ms. */
  debounceMs: number;
}

const DEFAULT_DEPS: WorkTreeDeps = {
  tree: workTree,
  task: workTask,
  views: workViews,
  readPrefs: readViewPrefs,
  writePrefs: (view, p) => writePref(prefKey(view), p),
  debounceMs: 400,
};

/** With no pref, the first sections open so the view is not a wall of
 *  closed headers. */
const AUTO_EXPAND = 3;

export interface WorkTreeStore extends Readable<WorkTreeState> {
  /** Load (or reload) everything: header counts, then the open sections. */
  reload(): Promise<void>;
  /** Mount-time: load once, later calls are no-ops until `reload`. */
  ensureLoaded(): Promise<void>;
  setFilters(f: UiFilters): void;
  applyView(v: WorkView | null): void;
  loadViews(): Promise<void>;
  toggleSection(key: string): void;
  toggleOrg(key: string): void;
  loadMore(key: string): Promise<void>;
  selectTask(taskId: string | null): void;
  /** Open the section holding `taskId` and select it (*Show in Work view*). */
  reveal(taskId: string): Promise<void>;
  onWorkChanged(changes: readonly WorkChanged[]): void;
  onSessionEvents(events: readonly SessionEvent[]): void;
  /** Stop timers (tests, teardown). */
  dispose(): void;
}

function errText(e: IpcError): string {
  return `${e.code}: ${e.message}`;
}

/** A page this build can read; anything else (a backend that answers the
 *  command with something else) is treated as no Work view. */
function isPage(p: unknown): p is TreePage {
  return !!p && typeof p === 'object' && Array.isArray((p as TreePage).tasks) && Array.isArray((p as TreePage).groups);
}
const NOT_A_PAGE: IpcError = { code: 'E_INVALID', message: 'unknown work action (not a tree page)' };

export function createWorkTreeStore(overrides: Partial<WorkTreeDeps> = {}): WorkTreeStore {
  const deps: WorkTreeDeps = { ...DEFAULT_DEPS, ...overrides };
  const state = writable<WorkTreeState>({
    status: 'idle',
    error: null,
    orgs: [],
    orgList: [],
    trackers: [],
    total: 0,
    filters: { ...EMPTY_FILTERS },
    view: 'adhoc',
    views: [],
    viewsError: null,
    expanded: new Set(),
    collapsedOrgs: new Set(),
    selected: null,
  });
  let hasPrefs = false;
  let generation = 0;
  let loadedOnce = false;
  const lastSig = new Map<number, string>();

  function hydratePrefs(view: ViewId) {
    const p = deps.readPrefs(view);
    hasPrefs = p !== null;
    state.update((s) => ({
      ...s,
      selected: p?.task ?? null,
      expanded: new Set(p?.expanded ?? []),
      collapsedOrgs: new Set(p?.collapsedOrgs ?? []),
    }));
  }
  function persist() {
    const s = get(state);
    deps.writePrefs(s.view, { expanded: [...s.expanded], collapsedOrgs: [...s.collapsedOrgs], task: s.selected });
    hasPrefs = true;
  }

  function requestFilters(): WorkTreeFilters {
    return filtersToRequest(get(state).filters);
  }

  function patchSection(key: string, fn: (s: Section) => Section) {
    state.update((st) => ({
      ...st,
      orgs: st.orgs.map((o) =>
        o.sections.some((s) => s.key === key)
          ? { ...o, sections: o.sections.map((s) => (s.key === key ? fn(s) : s)) }
          : o,
      ),
    }));
  }
  function patchAllTasks(fn: (t: WorkTask) => WorkTask) {
    state.update((st) => {
      let any = false;
      const orgs = st.orgs.map((o) => {
        let oc = false;
        const sections = o.sections.map((s) => {
          let sc = false;
          const tasks = s.tasks.map((t) => {
            const n = fn(t);
            if (n !== t) sc = true;
            return n;
          });
          if (!sc) return s;
          oc = true;
          return { ...s, tasks };
        });
        if (!oc) return o;
        any = true;
        return { ...o, sections };
      });
      return any ? { ...st, orgs } : st;
    });
  }
  function findSection(key: string): Section | undefined {
    for (const o of get(state).orgs) for (const s of o.sections) if (s.key === key) return s;
    return undefined;
  }

  function failed(e: IpcError): boolean {
    if (needsNewerHub(e)) {
      state.update((s) => ({ ...s, status: 'needs_hub', error: null }));
      return true;
    }
    return false;
  }

  /** Header counts only: sections, orgs, trackers, total. */
  async function loadHeader(gen: number): Promise<boolean> {
    const got = await deps.tree({ filters: requestFilters(), limit: 1, perTask: 0 });
    if (gen !== generation) return false;
    const r: Result<TreePage> = got.ok && !isPage(got.value) ? { ok: false, error: NOT_A_PAGE } : got;
    if (!r.ok) {
      if (!failed(r.error)) state.update((s) => ({ ...s, status: 'error', error: errText(r.error) }));
      return false;
    }
    const page = r.value;
    state.update((s) => {
      const orgs = buildTree(page, s.orgs);
      let expanded = s.expanded;
      if (!hasPrefs && expanded.size === 0) {
        expanded = new Set(orgs.flatMap((o) => o.sections.map((x) => x.key)).slice(0, AUTO_EXPAND));
      }
      return {
        ...s,
        status: 'ready',
        error: null,
        orgs,
        orgList: page.orgs,
        trackers: page.trackers,
        total: page.total,
        expanded,
      };
    });
    return true;
  }

  /** Read a section's first page(s) again, keeping as many tasks as were
   *  loaded (up to the page cap). */
  async function readSection(key: string, gen: number, keep = true): Promise<void> {
    const sec = findSection(key);
    if (!sec) return;
    const limit = keep ? Math.min(MAX_PAGE, Math.max(SECTION_PAGE, sec.tasks.length)) : SECTION_PAGE;
    patchSection(key, (s) => ({ ...s, loading: true, error: null }));
    const got = await deps.tree({
      filters: sectionFilters(requestFilters(), sec.orgId, sec.group.id),
      limit,
      perTask: PER_TASK,
    });
    if (gen !== generation) return;
    const r: Result<TreePage> = got.ok && !isPage(got.value) ? { ok: false, error: NOT_A_PAGE } : got;
    if (!r.ok) {
      if (!failed(r.error)) patchSection(key, (s) => ({ ...s, loading: false, error: errText(r.error) }));
      return;
    }
    patchSection(key, (s) => ({
      ...s,
      loading: false,
      error: null,
      tasks: r.value.tasks,
      cursor: r.value.next_cursor ?? null,
    }));
  }

  async function reload(): Promise<void> {
    const gen = ++generation;
    if (!loadedOnce) hydratePrefs(get(state).view);
    loadedOnce = true;
    state.update((s) => ({ ...s, status: s.status === 'ready' ? 'ready' : 'loading', error: null }));
    if (!(await loadHeader(gen))) return;
    const open = [...get(state).expanded];
    await Promise.all(
      open.filter((k) => findSection(k) && !isOrgCollapsed(k)).map((k) => readSection(k, gen)),
    );
  }

  function isOrgCollapsed(sectionKeyStr: string): boolean {
    const s = get(state);
    const org = s.orgs.find((o) => o.sections.some((x) => x.key === sectionKeyStr));
    return !!org && s.collapsedOrgs.has(org.key);
  }

  async function loadMore(key: string): Promise<void> {
    const sec = findSection(key);
    if (!sec || sec.loading || sec.cursor === null) return;
    if (sec.cursor === undefined) return readSection(key, generation, false);
    const gen = generation;
    patchSection(key, (s) => ({ ...s, loading: true, error: null }));
    const got = await deps.tree({
      filters: sectionFilters(requestFilters(), sec.orgId, sec.group.id),
      cursor: sec.cursor,
      limit: SECTION_PAGE,
      perTask: PER_TASK,
    });
    if (gen !== generation) return;
    const r: Result<TreePage> = got.ok && !isPage(got.value) ? { ok: false, error: NOT_A_PAGE } : got;
    if (!r.ok) {
      if (!failed(r.error)) patchSection(key, (s) => ({ ...s, loading: false, error: errText(r.error) }));
      return;
    }
    patchSection(key, (s) => ({
      ...s,
      loading: false,
      tasks: appendPage(s.tasks, r.value.tasks),
      cursor: r.value.next_cursor ?? null,
    }));
  }

  function toggleSection(key: string) {
    let opened = false;
    state.update((s) => {
      const expanded = new Set(s.expanded);
      if (expanded.has(key)) expanded.delete(key);
      else {
        expanded.add(key);
        opened = true;
      }
      return { ...s, expanded };
    });
    persist();
    const sec = findSection(key);
    if (opened && sec && sec.cursor === undefined && !sec.loading) void readSection(key, generation, false);
  }

  function toggleOrg(key: string) {
    let opened = false;
    state.update((s) => {
      const collapsedOrgs = new Set(s.collapsedOrgs);
      if (collapsedOrgs.has(key)) {
        collapsedOrgs.delete(key);
        opened = true;
      } else collapsedOrgs.add(key);
      return { ...s, collapsedOrgs };
    });
    persist();
    if (!opened) return;
    const s = get(state);
    const org = s.orgs.find((o) => o.key === key);
    for (const sec of org?.sections ?? []) {
      if (s.expanded.has(sec.key) && sec.cursor === undefined && !sec.loading) void readSection(sec.key, generation, false);
    }
  }

  function resetSections() {
    // Explicitly loading, never a flash of "no tasks match".
    state.update((s) => ({ ...s, orgs: [], status: s.status === 'needs_hub' ? s.status : 'loading' }));
  }

  function setFilters(f: UiFilters) {
    const s = get(state);
    if (sameFilters(s.filters, f)) return;
    // Hand-edited filters leave the saved view: its prefs stay its own.
    if (s.view !== 'adhoc') {
      state.update((x) => ({ ...x, view: 'adhoc' }));
      hydratePrefs('adhoc');
    }
    state.update((x) => ({ ...x, filters: { ...f } }));
    resetSections();
    void reload();
  }

  function applyView(v: WorkView | null) {
    const id: ViewId = v ? v.id : 'adhoc';
    state.update((x) => ({ ...x, view: id, filters: v ? uiFiltersOf(v.filters) : { ...EMPTY_FILTERS } }));
    hydratePrefs(id);
    resetSections();
    void reload();
  }

  async function loadViews(): Promise<void> {
    const r = await deps.views();
    if (r.ok) state.update((s) => ({ ...s, views: Array.isArray(r.value) ? r.value : [], viewsError: null }));
    else if (!failed(r.error)) state.update((s) => ({ ...s, viewsError: errText(r.error) }));
  }

  function selectTask(taskId: string | null) {
    state.update((s) => ({ ...s, selected: taskId }));
    openTaskId.set(taskId);
    persist();
  }

  async function reveal(taskId: string): Promise<void> {
    if (!loadedOnce) await reload();
    selectTask(taskId);
    const r = await deps.task(taskId);
    if (!r.ok) {
      failed(r.error);
      return;
    }
    const t = r.value?.task;
    if (!t?.group) return;
    const key = sectionKey(t.org_id, t.group.id);
    if (!findSection(key)) return;
    state.update((s) => {
      const collapsedOrgs = new Set(s.collapsedOrgs);
      collapsedOrgs.delete(t.org_id == null ? 'none' : String(t.org_id));
      const expanded = new Set(s.expanded);
      expanded.add(key);
      return { ...s, expanded, collapsedOrgs };
    });
    persist();
    const sec = findSection(key);
    if (sec && sec.cursor === undefined) await readSection(key, generation, false);
    // Not on the first page: put it where the person can see it.
    const now = findSection(key);
    if (now && !now.tasks.some((x) => x.task_id === t.task_id)) {
      patchSection(key, (s) => ({ ...s, tasks: [t, ...s.tasks] }));
    }
  }

  // ── live updates ──────────────────────────────────────────────────────────

  let timer: ReturnType<typeof setTimeout> | undefined;
  let pendingFull = false;
  const pendingTasks = new Set<string>();
  const pendingSections = new Set<string>();
  let pendingViews = false;

  function schedule() {
    clearTimeout(timer);
    timer = setTimeout(() => void flush(), deps.debounceMs);
  }

  async function flush() {
    timer = undefined;
    const full = pendingFull;
    const tasks = [...pendingTasks];
    const sections = [...pendingSections];
    const views = pendingViews;
    pendingFull = false;
    pendingViews = false;
    pendingTasks.clear();
    pendingSections.clear();
    if (views) void loadViews();
    if (!loadedOnce) return;
    if (full) {
      await reload();
      return;
    }
    const gen = generation;
    for (const id of tasks) await patchTask(id, gen);
    for (const k of sections) if (get(state).expanded.has(k)) await readSection(k, gen);
    if (tasks.length > 0 && gen === generation) await loadHeader(gen);
  }

  /** One task moved (placement, org): take it out of every section and put
   *  it where it now belongs, if that section is loaded and it still
   *  matches the view's org filter. */
  async function patchTask(taskId: string, gen: number) {
    const r = await deps.task(taskId);
    if (gen !== generation) return;
    if (r.ok && !r.value?.task?.group) return;
    if (!r.ok) {
      if (r.error.code === 'E_NOTFOUND') patchSections((s) => ({ ...s, tasks: removeTask(s.tasks, taskId) }));
      else failed(r.error);
      return;
    }
    const t = r.value.task;
    // The tree carries at most PER_TASK occurrences; the detail has them all.
    const slim: WorkTask = t.sessions.length > PER_TASK
      ? { ...t, sessions: t.sessions.slice(0, PER_TASK), sessions_more: t.sessions.length - PER_TASK + (t.sessions_more ?? 0) }
      : t;
    const f = get(state).filters;
    const fits = f.org === null || (f.org === 'none' ? t.org_id == null : f.org === t.org_id);
    const target = sectionKey(t.org_id, t.group.id);
    patchSections((s) => {
      if (s.key === target && fits && s.cursor !== undefined) return { ...s, tasks: mergeTask(s.tasks, slim) };
      return { ...s, tasks: removeTask(s.tasks, taskId) };
    });
  }

  function patchSections(fn: (s: Section) => Section) {
    state.update((st) => ({ ...st, orgs: st.orgs.map((o) => ({ ...o, sections: o.sections.map(fn) })) }));
  }

  function onWorkChanged(changes: readonly WorkChanged[]) {
    if (changes.length === 0) return;
    if (needsFullReload(changes)) pendingFull = true;
    for (const c of changes) {
      if (c.what === 'view') pendingViews = true;
      else if (c.what === 'rule') pendingFull = true;
      else if (c.what === 'placement' || c.what === 'org') {
        if (c.task_id) pendingTasks.add(c.task_id);
        else pendingFull = true;
      }
    }
    schedule();
  }

  function sectionsShowing(sessionId: number): string[] {
    const out: string[] = [];
    for (const o of get(state).orgs)
      for (const s of o.sections)
        if (s.tasks.some((t) => t.sessions.some((l) => l.session_id === sessionId))) out.push(s.key);
    return out;
  }

  function onSessionEvents(events: readonly SessionEvent[]) {
    if (!loadedOnce) {
      // Keep signatures current so the first change after opening is seen.
      for (const ev of events) if (ev.type !== 'killed') lastSig.set(ev.row.id, linkSignature(ev.row));
      return;
    }
    let reread = false;
    for (const ev of events) {
      if (ev.type === 'killed') {
        lastSig.delete(ev.id);
        patchAllTasks((t) => endOccurrences(t, ev.id));
        continue;
      }
      const row = ev.row;
      patchAllTasks((t) => patchOccurrences(t, row));
      const sig = linkSignature(row);
      const prev = lastSig.get(row.id);
      lastSig.set(row.id, sig);
      if (prev === undefined ? ev.type === 'created' && sig !== '|||' : prev !== sig) {
        const showing = sectionsShowing(row.id);
        // A link to a task in a section we have not drawn: its counts
        // changed, and so may the section it lands in.
        if (showing.length === 0) pendingFull = true;
        for (const k of showing) pendingSections.add(k);
        reread = true;
      }
    }
    if (reread) schedule();
  }

  async function ensureLoaded() {
    if (loadedOnce) return;
    void loadViews();
    await reload();
  }

  return {
    subscribe: state.subscribe,
    reload,
    ensureLoaded,
    setFilters,
    applyView,
    loadViews,
    toggleSection,
    toggleOrg,
    loadMore,
    selectTask,
    reveal,
    onWorkChanged,
    onSessionEvents,
    dispose: () => clearTimeout(timer),
  };
}

/** The app's one Work view. */
export const workTreeStore: WorkTreeStore = createWorkTreeStore();

/** *Show in Work view* from anywhere (session detail's Tasks section). */
export function showInWorkView(taskId: string): void {
  sidebarMode.set('work');
  void workTreeStore.reveal(taskId);
}
