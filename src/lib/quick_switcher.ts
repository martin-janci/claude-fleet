// Pure model for QuickSwitcher.svelte: which rows exist, how a query ranks
// them, and the MRU list that puts recently opened sessions first.
//
// Rows are sessions (Enter attaches), projects (Enter opens the
// new-session dialog for that project), hosts (`host: <alias>`, Enter opens
// the Hosts view on that host) and — with a tracker (work graph M3) — tickets
// (Enter jumps to the live session, or opens the dialog prefilled; ⌘↵ starts
// with the defaults) plus a lookup row for a pasted URL or an unknown exact
// key, like VS Code's quick open mixing "recently opened" with "create new". Host rows always rank below every session row so
// they never displace a session result. Ranking is `fuzzy.ts` over every
// searchable facet (friendly name, tmux name, project, host, branch,
// status) so `"blue mef"` finds the blue-sirius session on mefistos.
import { get, writable } from 'svelte/store';
import { fuzzyMatchFields } from './fuzzy';
import type { ProjectTreeRow } from './projects';
import { readPref, writePref } from './prefs';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import { displayKey, keyFamily, type TicketRow } from './trackers';
import { rowMatches, sessionFilterRow, type FilterRow } from './sidebar_index';

/** A cached tracker ticket and the section it is listed under. */
export interface SwitcherTicket {
  ticket: TicketRow;
  /** `My work` | `Current sprint` | `Recent`. */
  section: string;
}

/** Section order for tickets on an empty query. */
export const TICKET_SECTIONS = ['My work', 'Current sprint', 'Recent'] as const;

export interface SwitcherEntry {
  /** `ticket`: a cached tracker ticket (work graph M3); `lookup`: resolve
   *  the pasted URL / typed key through the tracker. */
  kind: 'session' | 'project' | 'host' | 'ticket' | 'lookup';
  /** `session:<id>`, `project:<id>`, `host:<alias>`, `ticket:<KEY>` or
   *  `lookup:<query>`. */
  key: string;
  label: string;
  description: string;
  meta: string;
  /** Everything a query token may hit. */
  fields: string[];
  session?: SessionRow;
  project?: ProjectTreeRow;
  host?: HostRow;
  ticket?: TicketRow;
  /** Tickets: the section heading. */
  section?: string;
  /** Lookup: what to resolve. */
  lookup?: string;
  /** Tickets: the tracker's provider badge (work graph M6). */
  badge?: { icon: string; title: string };
}

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
      fields: [
        label,
        s.tmux_name,
        projectName ?? '',
        p?.project.repo ?? '',
        s.host_alias,
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
export function isSwitcherChord(
  e: {
    key: string;
    metaKey: boolean;
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
  },
  isMac: boolean,
): boolean {
  const k = e.key.toLowerCase();
  if (k !== 'k' && k !== 'p') return false;
  if (e.altKey) return false;
  if (isMac) return e.metaKey && !e.ctrlKey && !e.shiftKey;
  return e.ctrlKey && e.shiftKey && !e.metaKey;
}

/** Human label for the open chord, for hints and docs. */
export function chordLabel(isMac: boolean): string {
  return isMac ? '⌘K' : 'Ctrl+Shift+K';
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
