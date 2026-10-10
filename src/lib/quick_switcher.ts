// Pure model for QuickSwitcher.svelte: which rows exist, how a query ranks
// them, and the MRU list that puts recently opened sessions first.
//
// Rows are sessions (Enter attaches), projects (Enter opens the
// new-session dialog for that project), hosts (`host: <alias>`, Enter opens
// the Hosts view on that host) and — with a tracker (work graph M3) — tickets
// and the person's own tasks (TASK-n), with or without a tracker
// (Enter jumps to the live session, or opens the dialog prefilled; ⌘↵ starts
// with the defaults) plus a lookup row for a pasted URL or an unknown exact
// key, like VS Code's quick open mixing "recently opened" with "create new". Host rows always rank below every session row so
// they never displace a session result. Ranking is `fuzzy.ts` over every
// searchable facet (friendly name, tmux name, project, host, branch,
// status) so `"blue mef"` finds the blue-sirius session on mefistos.
import { matchShortcut, shortcutLabel, type KeyEventLike } from './shortcuts';
import { get, writable } from 'svelte/store';
import { fuzzyMatchFields } from './fuzzy';
import { sessionSearchFields } from './search';
import type { PrefixMode } from './commands';
import type { ProjectTreeRow } from './projects';
import { readPref, writePref } from './prefs';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import { catalogOf, type AssetListing } from './assets';
import { displayKey, keyFamily, type TicketRow } from './trackers';
import type { WorkTreeFacets, WorkTreeFilters } from './work_view';
import type { SearchHit } from './search_api';
import { rowMatches, sessionFilterRow, type FilterRow } from './sidebar_index';

/** A cached tracker ticket or own task and the section it is listed under. */
export interface SwitcherTicket {
  ticket: TicketRow;
  /** One of `TICKET_SECTIONS`. */
  section: string;
}

/** Section order for tickets: the tracker views, the person's own tasks,
 *  then what a query found in the rest of the cache. */
export const TICKET_SECTIONS = ['My work', 'My tasks', 'Current sprint', 'Recent', 'Search'] as const;

/** A query this long (after a prefix) also searches the hub's whole ticket
 *  cache, once typing pauses for the debounce. */
export const TICKET_SEARCH_MIN = 2;
export const TICKET_SEARCH_DEBOUNCE_MS = 200;

export interface SwitcherEntry {
  /** `ticket`: a cached tracker ticket (work graph M3); `lookup`: resolve
   *  the pasted URL / typed key through the tracker. */
  kind: 'session' | 'project' | 'host' | 'ticket' | 'lookup' | 'asset' | 'command' | 'setting' | 'planning' | 'found';
  /** `session:<id>`, `project:<id>`, `host:<alias>`, `ticket:<KEY>`,
   *  `lookup:<query>`, `asset:<catalog>:<kind>/<name>` (the Assets
   *  workspace's own selection key) or `command:<rescan|sync|propose>`. */
  key: string;
  label: string;
  description: string;
  meta: string;
  /** Everything a query token may hit. */
  fields: string[];
  session?: SessionRow;
  project?: ProjectTreeRow;
  host?: HostRow;
  /** Assets: the workspace selection key to open. */
  asset?: { key: string };
  /** Commands: which Assets command to run. */
  command?: AssetsCommand;
  /** Palette commands (step 3.9, `commands.ts`): the command id. */
  action?: string;
  /** A plain-words settings change (step 3.9): applied on Enter. */
  setting?: { key: string; value: string; label: string; words: string; confirm: boolean };
  ticket?: TicketRow;
  /** Tickets and palette commands: the section heading. */
  section?: string;
  /** Lookup: what to resolve. */
  lookup?: string;
  /** Tickets: the tracker's provider badge (work graph M6). */
  badge?: { icon: string; title: string };
  /** Planning: the Work view's filters Enter opens it with (a sprint, an
   *  epic). */
  planning?: WorkTreeFilters;
  /** Found: a hit of the hub's full-text search (search phase 3). */
  found?: SearchHit;
  /** `[start, end)` of the label the query matched, when the hub said. */
  marks?: [number, number][];
}

const FOUND_KIND_LABELS: Record<string, string> = {
  item: 'Task',
  session: 'Session',
  conversation: 'Conversation',
  transcript: 'Said in',
  pr: 'Pull request',
  journal: 'Journal',
};

/** Rows for the hub's full-text hits, in the hub's order. A session hit
 *  for a session ⌘K already lists, and a task hit for a ticket row it
 *  already has, are left out: the row above is the same thing. */
export function foundEntries(
  hits: readonly SearchHit[],
  listedSessionIds: ReadonlySet<number>,
  listedItemIds: ReadonlySet<number>,
): SwitcherEntry[] {
  const out: SwitcherEntry[] = [];
  for (const h of hits) {
    if (h.kind === 'session' && h.session_id != null && listedSessionIds.has(h.session_id)) continue;
    const itemId = h.task_id?.startsWith('item:') ? Number(h.task_id.slice(5)) : null;
    if (h.kind === 'item' && itemId != null && listedItemIds.has(itemId)) continue;
    const where = [h.session_name, h.host_alias].filter(Boolean).join(' · ');
    const label = h.title || (h.session_name ?? h.snippet);
    out.push({
      kind: 'found',
      key: `found:${h.kind}:${h.ref}`,
      label,
      description: [FOUND_KIND_LABELS[h.kind] ?? h.kind, where, h.snippet].filter(Boolean).join(' · '),
      meta: h.kind === 'item' ? 'task' : h.session_id != null ? 'open' : '',
      // What the hub matched, so ⌘K's own filter keeps the row.
      fields: [label, h.snippet, h.session_name ?? '', h.key ?? ''].filter(Boolean),
      found: h,
      marks: h.title ? h.title_marks : undefined,
    });
  }
  return out;
}

/** Rows that open the Work view on a sprint or an epic (search phase 2):
 *  the current sprint, each sprint, each epic, from the tree's `facets`.
 *  Shown only for a query that matches them. */
export function planningEntries(facets: WorkTreeFacets | null | undefined): SwitcherEntry[] {
  const out: SwitcherEntry[] = [];
  const iterations = facets?.iterations ?? [];
  const active = iterations.filter((i) => i.active);
  if (iterations.length > 0) {
    out.push({
      kind: 'planning',
      key: 'planning:sprint:current',
      label: 'Current sprint',
      description: active.length > 0 ? `${active.map((i) => i.name).join(', ')} · ${active.reduce((n, i) => n + i.count, 0)} tasks` : 'the active sprint',
      meta: 'Work',
      fields: ['current sprint', 'sprint', 'iteration', 'cycle', ...active.map((i) => i.name)],
      planning: { iteration: 'current' },
    });
  }
  for (const it of iterations) {
    out.push({
      kind: 'planning',
      key: `planning:sprint:${it.name}`,
      label: `Sprint: ${it.name}`,
      description: `${it.active ? 'active · ' : ''}${it.count} task${it.count === 1 ? '' : 's'}`,
      meta: 'Work',
      fields: [it.name, 'sprint'],
      planning: { iteration: it.name },
    });
  }
  for (const e of facets?.epics ?? []) {
    const ref = e.key ?? e.task_id;
    out.push({
      kind: 'planning',
      key: `planning:epic:${ref}`,
      label: `Epic: ${e.key ? `${displayKey(e.key)} ` : ''}${e.title}`,
      description: `${e.count} task${e.count === 1 ? '' : 's'} under it`,
      meta: 'Work',
      fields: [e.key ?? '', e.title, 'epic'].filter(Boolean),
      planning: { epic: ref },
    });
  }
  return out;
}


/** What a command row asks the Assets panel to run (`app_views.ts`). */
export type AssetsCommand = 'rescan' | 'sync' | 'propose';

const TICKET_KEY_RE = /^[A-Za-z][A-Za-z0-9_]{1,9}-\d{1,7}$/;

/** Ticket rows, one per key (the first section a key appears in wins). */
export function ticketEntries(
  tickets: readonly SwitcherTicket[],
  /** tracker id → its provider badge, when badges are shown (M6). */
  badges: ReadonlyMap<number, { icon: string; title: string }> = new Map(),
): SwitcherEntry[] {
  const seen = new Set<string>();
  const out: SwitcherEntry[] = [];
  for (const { ticket: t, section } of tickets) {
    const key = t.key ?? `#${t.id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const live = (t.live_session_ids ?? []).length;
    const assignee = (t.assignees ?? []).join(', ');
    const shown = displayKey(key);
    out.push({
      kind: 'ticket',
      key: `ticket:${key}`,
      label: t.title ? `${shown} ${t.title}` : shown,
      description: [t.status_name, assignee, live > 0 ? `${live} live` : null]
        .filter((x): x is string => !!x)
        .join(' · '),
      meta: live > 0 ? 'jump' : 'start',
      fields: [key, t.title, t.status_name ?? '', assignee, ...(t.aliases ?? [])].filter(Boolean),
      ticket: t,
      section,
      badge: t.tracker_id != null ? badges.get(t.tracker_id) : undefined,
    });
  }
  return out;
}

/** One row per catalog asset (Assets M6, R19); Enter opens it in the Assets
 *  workspace. The key is the workspace's own selection key. */
export function assetEntries(listing: AssetListing | null): SwitcherEntry[] {
  return (listing?.assets ?? []).map((a) => {
    const catalog = catalogOf(a);
    const label = `${a.kind}/${a.name}`;
    return {
      kind: 'asset',
      key: `asset:${catalog}:${label}`,
      label,
      description: a.description ? `${catalog} · ${a.description}` : catalog,
      meta: 'Assets',
      fields: [label, a.description, catalog].filter(Boolean),
      asset: { key: `asset:${catalog}:${label}` },
    };
  });
}

const COMMANDS: { command: AssetsCommand; label: string; synonyms: string[] }[] = [
  { command: 'rescan', label: 'Rescan assets', synonyms: ['scan'] },
  { command: 'sync', label: 'Sync fleet', synonyms: ['sync', 'rollout'] },
  { command: 'propose', label: 'Propose cards', synonyms: ['propose', 'cards', 'layers'] },
];

/** The Assets workspace's three commands (R19). */
export function commandEntries(): SwitcherEntry[] {
  return COMMANDS.map((c) => ({
    kind: 'command',
    key: `command:${c.command}`,
    label: c.label,
    description: 'Assets',
    meta: 'Commands',
    fields: [c.label, 'assets', ...c.synonyms],
    command: c.command,
  }));
}

/** A `lookup` row for a pasted ticket URL or an exact key the cache does
 *  not hold; null otherwise. */
export function lookupEntry(query: string, knownKeys: ReadonlySet<string>): SwitcherEntry | null {
  const q = query.trim();
  if (!q) return null;
  const isUrl = /^https:\/\/\S+$/i.test(q);
  // A ticket key, or a GitHub `owner/repo#n` (work graph M6).
  const isKey = TICKET_KEY_RE.test(q) || /^[\w.-]+\/[\w.-]+#\d{1,9}$/.test(q);
  if (!isUrl && !isKey) return null;
  if (isKey && knownKeys.has(q.toUpperCase())) return null;
  return {
    kind: 'lookup',
    key: `lookup:${q}`,
    label: isUrl ? `Look up ${q}` : `Look up ${TICKET_KEY_RE.test(q) ? q.toUpperCase() : q}`,
    description: 'fetch the ticket from its tracker',
    meta: '↵',
    fields: [q],
    lookup: q,
  };
}

/** Stable identity used for the MRU list (ids churn on re-discovery). */
export function sessionMruKey(s: { host_alias: string; tmux_name: string }): string {
  return `${s.host_alias}/${s.tmux_name}`;
}

const RECENT_PREF = 'quick-switcher.recent';
export const RECENT_MAX = 20;
const isStringArray = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((x) => typeof x === 'string');

/** Most-recently-selected sessions, newest first. Persisted across restarts. */
export const recentSessions = writable<string[]>(readPref(RECENT_PREF, [], isStringArray));
recentSessions.subscribe((v) => writePref(RECENT_PREF, v));

const RECENT_QUERIES_PREF = 'switcher.recent_queries';
const RECENT_QUERIES_MAX = 5;
/** The last few queries a row was picked with, newest first. */
export const recentQueries = writable<string[]>(readPref(RECENT_QUERIES_PREF, [], isStringArray));
recentQueries.subscribe((v) => writePref(RECENT_QUERIES_PREF, v));

/** Remember `q` (two letters or more) at the head of the recent queries. */
export function noteQuery(q: string): void {
  const v = q.trim();
  if (v.length < 2) return;
  const cur = get(recentQueries);
  if (cur[0] === v) return;
  recentQueries.set([v, ...cur.filter((x) => x !== v)].slice(0, RECENT_QUERIES_MAX));
}

/** Move `s` to the head of the MRU list (no-op when already there). */
export function noteRecent(s: { host_alias: string; tmux_name: string }): void {
  const key = sessionMruKey(s);
  const cur = get(recentSessions);
  if (cur[0] === key) return;
  recentSessions.set([key, ...cur.filter((k) => k !== key)].slice(0, RECENT_MAX));
}

function statusLabel(s: SessionRow): string {
  if (s.status === 'ghost') return 'ghost';
  return s.claude_status ?? s.status;
}

function worktreeLabel(s: SessionRow, p: ProjectTreeRow | undefined): string | null {
  if (!p) return s.worktree_key;
  const wt = p.worktrees.find((w) => w.id === s.worktree_id);
  return wt?.branch ?? wt?.name ?? s.worktree_key;
}

export function buildEntries(
  sessions: readonly SessionRow[],
  projects: readonly ProjectTreeRow[],
  hosts: readonly HostRow[] = [],
): SwitcherEntry[] {
  const byId = new Map(projects.map((p) => [p.project.id, p]));
  const out: SwitcherEntry[] = [];
  for (const s of sessions) {
    const p = s.project_id == null ? undefined : byId.get(s.project_id);
    const projectName = p ? `${p.project.owner}/${p.project.repo}` : null;
    const branch = worktreeLabel(s, p);
    const label = s.friendly_name || s.tmux_name;
    const parts = [projectName, s.host_alias, branch].filter((x): x is string => !!x);
    out.push({
      kind: 'session',
      key: `session:${s.id}`,
      label,
      description: parts.join(' · '),
      meta: statusLabel(s),
      // The Sessions list's fields (`search.ts`), plus what only ⌘K
      // ranks by: the repo alone, the branch, the status and the kind.
      fields: [
        label,
        ...sessionSearchFields(s, projectName),
        p?.project.repo ?? '',
        branch ?? '',
        statusLabel(s),
        s.kind,
      ].filter(Boolean),
      session: s,
    });
  }
  for (const p of projects) {
    // A system project is fleet's own working directory, not one of yours —
    // it labels the sessions above, but "New session in fleet/operator" is
    // never an offer worth making. See `ProjectRow.system`.
    if (p.project.system) continue;
    const name = `${p.project.owner}/${p.project.repo}`;
    out.push({
      kind: 'project',
      key: `project:${p.project.id}`,
      label: `New session in ${name}`,
      description: p.project.base_path,
      meta: '+',
      fields: [name, p.project.repo, 'new session'],
      project: p,
    });
  }
  for (const h of hosts) {
    const count = sessions.filter((s) => s.host_alias === h.alias).length;
    const state = h.reachable ? 'online' : 'offline';
    out.push({
      kind: 'host',
      key: `host:${h.alias}`,
      label: `host: ${h.alias}`,
      description: `${state} · ${count} session${count === 1 ? '' : 's'}${h.hidden ? ' · hidden' : ''}`,
      meta: 'Hosts',
      fields: [h.alias, `host ${h.alias}`, h.ssh_alias ?? '', 'hosts'].filter(Boolean),
      host: h,
    });
  }
  return out;
}

/**
 * Filter + order for a query. Empty query: sessions in MRU order (then most
 * recent activity), projects after (most recently used first). Non-empty:
 * by fuzzy score, ties broken by the same recency order.
 */
export function rankEntries(
  entries: readonly SwitcherEntry[],
  query: string,
  recent: readonly string[],
): SwitcherEntry[] {
  // Assets and commands never displace a session, project, host or ticket:
  // they merge after all of them, assets first. Two exceptions (step 3.9):
  // a settings change typed in plain words is what the query asked for, so
  // it leads, and a command on the open session that the query matches
  // comes next.
  const isTail = (e: SwitcherEntry) =>
    e.kind === 'asset' || e.kind === 'command' || e.kind === 'setting' || e.kind === 'planning' || e.kind === 'found';
  const head = rankHead(
    entries.filter((e) => !isTail(e)),
    query,
    recent,
  );
  const q0 = query.trim();
  const tailRows = (kind: SwitcherEntry['kind']) =>
    entries
      .filter((e) => e.kind === kind)
      .map((e) => ({ e, score: q0 ? fuzzyMatchFields(q0, e.fields) : 0 }))
      .filter((x): x is { e: SwitcherEntry; score: number } => x.score !== null)
      .sort((a, b) => b.score - a.score)
      .map((x) => x.e);
  const settings = entries.filter((e) => e.kind === 'setting');
  const commands = tailRows('command');
  const lead = q0 ? commands.filter((e) => e.section === 'This session') : [];
  // Planning rows (a sprint, an epic) only answer a query; so do the hub's
  // full-text hits, which keep the hub's order (its rank, not ⌘K's).
  const planning = q0 ? tailRows('planning') : [];
  const found = q0 ? entries.filter((e) => e.kind === 'found') : [];
  const tail = [...planning, ...found, ...tailRows('asset'), ...commands.filter((e) => !lead.includes(e))];
  return [...settings, ...lead, ...head, ...tail];
}

function rankHead(
  entries: readonly SwitcherEntry[],
  query: string,
  recent: readonly string[],
): SwitcherEntry[] {
  const isTicket = (e: SwitcherEntry) => e.kind === 'ticket' || e.kind === 'lookup';
  const base = rankBase(
    entries.filter((e) => !isTicket(e)),
    query,
    recent,
  );
  const tickets = entries.filter(isTicket);
  if (tickets.length === 0) return base;
  // Tickets rank below sessions — except an exact key match (or the lookup
  // row for what was typed or pasted), which ranks first.
  const q = query.trim();
  const exactKey = q.toUpperCase();
  const sectionRank = (e: SwitcherEntry) => {
    const i = (TICKET_SECTIONS as readonly string[]).indexOf(e.section ?? '');
    return i === -1 ? TICKET_SECTIONS.length : i;
  };
  const scored = tickets
    .map((e) => ({ e, score: e.kind === 'lookup' ? 0 : q ? fuzzyMatchFields(q, e.fields) : 0 }))
    .filter((x): x is { e: SwitcherEntry; score: number } => x.score !== null);
  const exact = scored.filter(
    (x) =>
      x.e.kind === 'lookup' ||
      (x.e.ticket?.key ?? '').toUpperCase() === exactKey ||
      (x.e.ticket?.aliases ?? []).includes(exactKey),
  );
  const rest = scored
    .filter((x) => !exact.includes(x))
    .sort((a, b) => b.score - a.score || sectionRank(a.e) - sectionRank(b.e));
  let lastSession = -1;
  base.forEach((e, i) => {
    if (e.kind === 'session') lastSession = i;
  });
  return [
    ...exact.map((x) => x.e),
    ...base.slice(0, lastSession + 1),
    ...rest.map((x) => x.e),
    ...base.slice(lastSession + 1),
  ];
}

function rankBase(
  entries: readonly SwitcherEntry[],
  query: string,
  recent: readonly string[],
): SwitcherEntry[] {
  const mru = new Map(recent.map((k, i) => [k, i]));
  const recency = (e: SwitcherEntry): number => {
    if (e.kind === 'session' && e.session) {
      const i = mru.get(sessionMruKey(e.session));
      return i === undefined ? RECENT_MAX + 1 : i;
    }
    return RECENT_MAX + 2;
  };
  const activity = (e: SwitcherEntry): number =>
    e.kind === 'session'
      ? (e.session?.last_activity_at ?? 0)
      : (e.project?.project.last_session_at ?? 0);
  const q = query.trim();
  type Scored = { e: SwitcherEntry; score: number };
  const all = entries
    .map((e) => ({ e, score: q ? fuzzyMatchFields(q, e.fields) : 0 }))
    .filter((x): x is Scored => x.score !== null);
  const scored = all.filter((x) => x.e.kind !== 'host');
  scored.sort((a, b) => {
    if (b.score !== a.score) return b.score - a.score;
    // Sessions before projects when nothing else separates them.
    if (a.e.kind !== b.e.kind) return a.e.kind === 'session' ? -1 : 1;
    const ra = recency(a.e);
    const rb = recency(b.e);
    if (ra !== rb) return ra - rb;
    return activity(b.e) - activity(a.e);
  });
  // Host rows never precede a session row: they merge by score into what
  // follows the last session, ahead of an equally scored project.
  const hostRows = all
    .filter((x) => x.e.kind === 'host')
    .sort((a, b) => b.score - a.score || a.e.label.localeCompare(b.e.label));
  let lastSession = -1;
  scored.forEach((x, i) => {
    if (x.e.kind === 'session') lastSession = i;
  });
  const out: Scored[] = scored.slice(0, lastSession + 1);
  const rest = scored.slice(lastSession + 1);
  let h = 0;
  for (const x of rest) {
    while (h < hostRows.length && hostRows[h].score >= x.score) out.push(hostRows[h++]);
    out.push(x);
  }
  out.push(...hostRows.slice(h));
  return out.map((x) => x.e);
}

/**
 * Which project a "new session with this name" (Cmd/Ctrl+Enter) targets:
 * the selected session's project, else the top-ranked session row's, else
 * the most recently used project. `null` when there are no projects.
 */
export function contextProject(
  ranked: readonly SwitcherEntry[],
  selected: SessionRow | null,
  projects: readonly ProjectTreeRow[],
): ProjectTreeRow | null {
  // System projects are excluded at every step: this function's answer is
  // "where would a new session go", and the agent's own directory is not an
  // answer to that — not even when the agent's session is the selected one.
  const pickable = projects.filter((p) => !p.project.system);
  const byId = new Map(pickable.map((p) => [p.project.id, p]));
  if (selected?.project_id != null) {
    const p = byId.get(selected.project_id);
    if (p) return p;
  }
  const top = ranked.find((e) => e.kind === 'session' && e.session?.project_id != null);
  if (top?.session?.project_id != null) {
    const p = byId.get(top.session.project_id);
    if (p) return p;
  }
  const topProject = ranked.find((e) => e.kind === 'project');
  if (topProject?.project && !topProject.project.project.system) return topProject.project;
  return (
    [...pickable].sort(
      (a, b) => (b.project.last_session_at ?? 0) - (a.project.last_session_at ?? 0),
    )[0] ?? null
  );
}

/**
 * True for the open-switcher chords. Platform-correct, following
 * TerminalView's copy/paste convention: on macOS Cmd+K / Cmd+P (Cmd is
 * reserved for the app, never sent to the pty); elsewhere Ctrl+Shift+K /
 * Ctrl+Shift+P, so plain Ctrl+K (readline kill-line) and Ctrl+P (previous
 * history) keep reaching the terminal.
 */
export function isSwitcherChord(e: KeyEventLike, isMac: boolean): boolean {
  return matchShortcut('global', e, isMac) === 'switcher';
}

/** Human label for the open chord, for hints and docs. */
export function chordLabel(isMac: boolean): string {
  return shortcutLabel('switcher', isMac);
}

/** The New session picker's chord (project picker spec v2): ⌘N on macOS,
 *  Ctrl+Shift+N elsewhere — plain Ctrl+N stays readline's next-history. */
export function isNewSessionChord(e: KeyEventLike, isMac: boolean): boolean {
  return matchShortcut('global', e, isMac) === 'new-session';
}

/** "Start from work": My work tickets nobody has a session on, at most `cap`. */
export function workBlock(tickets: readonly SwitcherTicket[], cap = 3): SwitcherTicket[] {
  return tickets
    .filter((t) => t.section === 'My work' && (t.ticket.live_session_ids ?? []).length === 0)
    .slice(0, cap);
}

/**
 * Where a ticket's new session most likely belongs: the project and host of
 * the most recently active session working on a key of the same family
 * (`ABC-*`; a GitHub issue the same `owner/repo`, see `keyFamily`), else null
 * — the dialog then falls back to `contextProject`. The hub's `start` makes
 * the same choice from its links.
 */
export function placeForTicket(
  key: string,
  sessions: readonly SessionRow[],
  projects: readonly ProjectTreeRow[],
  keyOf: (s: SessionRow) => string | null,
): { project: ProjectTreeRow; host: string } | null {
  const family = keyFamily(key);
  if (!family) return null;
  const byId = new Map(projects.filter((p) => !p.project.system).map((p) => [p.project.id, p]));
  const hits = sessions
    .filter((s) => s.project_id != null && byId.has(s.project_id))
    .filter((s) => keyFamily(keyOf(s)) === family)
    .sort((a, b) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0));
  const s = hits[0];
  if (!s || s.project_id == null) return null;
  return { project: byId.get(s.project_id)!, host: s.host_alias };
}

/** A ticket as a filter row (work graph M5.5): its tracker's org as its
 *  scope, `*` for an unassigned tracker (no scope hides it). */
export function ticketFilterRow(
  t: TicketRow,
  trackerOrg: ReadonlyMap<number, number | null | undefined>,
): FilterRow {
  const org = t.tracker_id != null ? trackerOrg.get(t.tracker_id) : undefined;
  return {
    host: null,
    scope: org != null ? `org:${org}` : '*',
    trackerId: t.tracker_id ?? null,
    statusCategory: t.status_category ?? null,
    statusName: t.status_name ?? null,
    assignees: t.assignees ?? [],
    live: (t.live_session_ids ?? []).length > 0,
    archived: false,
  };
}

/** ⌘K under the sidebar's scope: sessions and tickets through the same
 *  `rowMatches` the sidebar uses; hosts, projects and actions pass. */
export function scopeEntries(
  entries: readonly SwitcherEntry[],
  scope: string,
  sessionScope: (s: SessionRow) => string,
  trackerOrg: ReadonlyMap<number, number | null | undefined>,
): SwitcherEntry[] {
  if (scope === 'all') return [...entries];
  return entries.filter((e) => {
    if (e.kind === 'session' && e.session) {
      return rowMatches(sessionFilterRow(e.session, sessionScope), { scope });
    }
    if (e.kind === 'ticket' && e.ticket) {
      return rowMatches(ticketFilterRow(e.ticket, trackerOrg), { scope });
    }
    return true;
  });
}

/** The switcher's empty list, in the words of what was searched: a `#`
 *  query names tasks and tickets, `@` hosts, `>` commands; a plain one
 *  offers ⌘↵ to create a session with that name. */
export function switcherEmptyText(
  prefix: { mode: PrefixMode; rest: string },
  hasTrackers: boolean,
  modKey: string,
): string {
  const q = prefix.rest.trim();
  switch (prefix.mode) {
    case 'work':
      if (!q) return hasTrackers ? 'No tasks or tickets yet.' : 'No tasks yet.';
      return hasTrackers
        ? `No task or ticket matches “${q}”.`
        : `No task matches “${q}”. Connect a tracker in Settings → Work to search tickets too.`;
    case 'hosts':
      return q ? `No host matches “${q}”.` : 'No hosts yet.';
    case 'commands':
      return q ? `No command matches “${q}”.` : 'No commands here.';
    case 'all':
      return q ? `Nothing matches “${q}”. ${modKey}↵ creates a session with that name.` : 'No sessions yet.';
  }
}
