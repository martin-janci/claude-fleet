// Toolkit (Orbit Fleet redesign step 3.16, board Toolkit): the home for
// today's Assets workspace. Its own nav column (UX audit 2026-10-09, A2):
// Skills, MCP servers and Hooks, each the catalog's assets of that kind as a
// host matrix with drift per host, and the Assets catalog, the full
// workspace (layers and changesets) unchanged, and Prompts & snippets, the
// composer's quick-action chips (moved from Settings).
import { writable } from 'svelte/store';
import { hostOrderOf, catalogOf, type AssetKind, type AssetListing, type AssetSummary, type HostState } from './assets';
import { AGENT_LABELS } from './row_groups';
import { keyOf } from './assets_workspace';
import { readPref, writePref } from './prefs';

export type ToolkitTab = 'skills' | 'mcp' | 'hooks' | 'assets' | 'prompts';

const isToolkitTab = (v: unknown): v is ToolkitTab =>
  v === 'skills' || v === 'mcp' || v === 'hooks' || v === 'assets' || v === 'prompts';

/** The asset kind each matrix page lists. */
export const TOOLKIT_KIND: Record<Exclude<ToolkitTab, 'assets' | 'prompts'>, AssetKind> = {
  skills: 'skill',
  mcp: 'mcp_server',
  hooks: 'hook',
};

/** The tab Toolkit shows; the rail reopens the last one, the old Assets
 *  entry points (tab, sidebar, quick switcher) ask for `assets`. */
export const toolkitTab = writable<ToolkitTab>(readPref<ToolkitTab>('ui.toolkitTab', 'skills', isToolkitTab));
toolkitTab.subscribe((v) => writePref('ui.toolkitTab', v));

/** One host's cell: the worst state across that host's harnesses. */
export type SkillCellState = 'in_sync' | 'behind' | 'edited' | 'drifted' | 'missing' | 'none';

export interface SkillCell {
  state: SkillCellState;
  /** What the cell reads. */
  word: string;
  /** The hover text, per harness when a host has several. */
  title: string;
}

export interface SkillRow {
  /** The Assets workspace's row key (`keyOf`), so Edit selects it there. */
  key: string;
  name: string;
  description: string;
  version: string;
  agents: string;
  cells: Record<string, SkillCell>;
  outOfSync: boolean;
}

export interface SkillMatrix {
  hosts: string[];
  rows: SkillRow[];
  outOfSync: number;
}

// Higher wins when a host has several harnesses.
const RANK: Record<SkillCellState, number> = { none: 0, in_sync: 1, behind: 2, missing: 3, edited: 4, drifted: 5 };

function cellStateOf(h: HostState): SkillCellState {
  if (h.state === 'in_sync') return 'in_sync';
  if (h.state === 'missing') return 'missing';
  if (h.state === 'drifted') return h.drift_side === 'catalog' ? 'behind' : h.drift_side === 'host' ? 'edited' : 'drifted';
  return 'none';
}

const STATE_TITLE: Record<SkillCellState, string> = {
  in_sync: 'in sync',
  behind: 'behind the catalog',
  edited: 'edited on host',
  drifted: 'drifted',
  missing: 'missing',
  none: 'not installed',
};

function harnessLabel(h: string): string {
  return (AGENT_LABELS as Record<string, string>)[h] ?? h;
}

function cellOf(host: string, version: string, states: HostState[]): SkillCell {
  if (states.length === 0) return { state: 'none', word: '—', title: `${host}: not installed` };
  let state: SkillCellState = 'none';
  for (const h of states) {
    const s = cellStateOf(h);
    if (RANK[s] > RANK[state]) state = s;
  }
  const word =
    state === 'in_sync'
      ? `✓ ${version}`
      : state === 'behind'
        ? `${version} ↑`
        : state === 'edited'
          ? 'edited'
          : state === 'none'
            ? '—'
            : STATE_TITLE[state];
  const title =
    states.length === 1
      ? `${host}: ${STATE_TITLE[cellStateOf(states[0])]}`
      : `${host}: ${states.map((h) => `${harnessLabel(h.harness)} ${STATE_TITLE[cellStateOf(h)]}`).join(', ')}`;
  return { state, word, title };
}

const OUT_OF_SYNC: ReadonlySet<SkillCellState> = new Set(['behind', 'edited', 'drifted', 'missing']);

/** The catalog's skills (or another kind's assets) as rows of a host matrix. */
export function skillMatrix(listing: AssetListing | null, kind: AssetKind = 'skill'): SkillMatrix {
  const skills = (listing?.assets ?? [])
    .filter((a) => a.kind === kind)
    .sort((a, b) => a.name.localeCompare(b.name) || catalogOf(a).localeCompare(catalogOf(b)));
  const hosts = hostOrderOf(skills.flatMap((a) => a.hosts.map((h) => h.host_alias)));
  const rows = skills.map((a: AssetSummary): SkillRow => {
    const cells: Record<string, SkillCell> = {};
    for (const host of hosts) cells[host] = cellOf(host, a.version, a.hosts.filter((h) => h.host_alias === host));
    const harnesses = [...new Set(a.hosts.map((h) => h.harness))].sort();
    return {
      key: keyOf({ type: 'asset', catalog: catalogOf(a), kind: a.kind, name: a.name }),
      name: a.name,
      description: a.description,
      version: a.version,
      agents: harnesses.length ? harnesses.map(harnessLabel).join(', ') : 'any',
      cells,
      outOfSync: Object.values(cells).some((c) => OUT_OF_SYNC.has(c.state)),
    };
  });
  return { hosts, rows, outOfSync: rows.filter((r) => r.outOfSync).length };
}
