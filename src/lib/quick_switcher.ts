// Pure model for QuickSwitcher.svelte: which rows exist, how a query ranks
// them, and the MRU list that puts recently opened sessions first.
//
// Rows are sessions (Enter attaches) and projects (Enter opens the
// new-session dialog for that project), like VS Code's quick open mixing
// "recently opened" with "create new". Ranking is `fuzzy.ts` over every
// searchable facet (friendly name, tmux name, project, host, branch,
// status) so `"blue mef"` finds the blue-sirius session on mefistos.
import { get, writable } from 'svelte/store';
import { fuzzyMatchFields } from './fuzzy';
import type { ProjectTreeRow } from './projects';
import { readPref, writePref } from './prefs';
import type { SessionRow } from './sessions';

export interface SwitcherEntry {
  kind: 'session' | 'project';
  /** `session:<id>` or `project:<id>`. */
  key: string;
  label: string;
  description: string;
  meta: string;
  /** Everything a query token may hit. */
  fields: string[];
  session?: SessionRow;
  project?: ProjectTreeRow;
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
  const scored = entries
    .map((e) => ({ e, score: q ? fuzzyMatchFields(q, e.fields) : 0 }))
    .filter((x): x is { e: SwitcherEntry; score: number } => x.score !== null);
  scored.sort((a, b) => {
    if (b.score !== a.score) return b.score - a.score;
    // Sessions before projects when nothing else separates them.
    if (a.e.kind !== b.e.kind) return a.e.kind === 'session' ? -1 : 1;
    const ra = recency(a.e);
    const rb = recency(b.e);
    if (ra !== rb) return ra - rb;
    return activity(b.e) - activity(a.e);
  });
  return scored.map((x) => x.e);
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
  const byId = new Map(projects.map((p) => [p.project.id, p]));
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
  if (topProject?.project) return topProject.project;
  return (
    [...projects].sort(
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
