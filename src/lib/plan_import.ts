// "Import plan" on a mission: a markdown plan read into step rows for
// `import_mission_plan` (`service::work::plan_import`).
//
// It reads every pipe table in the text. A table with a step column (`#`,
// `Id`, `Step id`) and a title column (`Step`, `Title`, `Task`) gives steps;
// its `Needs` / `Depends on` column gives the step ids each waits for, and
// its `Lane` / `Status` columns, when there, the lane and the status, and
// its `Repo` column the repository each step's task works in (G2.5). A
// table with `Lane` and `Steps in order` (the transition plan's Lanes table)
// gives each listed step its lane. Text a person wrote is passed on as
// text; the backend checks the rows again.

import { invokeCmd, type Result } from './result';
import { bumpWorkChanged } from './work';

/** One row (`plan_import::PlanRow`). */
export interface PlanRow {
  step: string;
  title: string;
  lane?: string;
  needs?: string[];
  status?: string;
  /** The repository a new step's task works in. */
  project_id?: number;
}

/** A parsed row: the wire row, with its `Repo` cell as written. */
export interface ParsedRow extends PlanRow {
  repo?: string;
}

/** What an import did (`plan_import::PlanImport`). */
export interface PlanImport {
  created: number;
  updated: number;
  unchanged: number;
  deps_added: number;
  deps_removed: number;
  unknown_needs?: string[];
}

/** `plan_import::PLAN_IMPORT_MAX_ROWS`. */
export const PLAN_IMPORT_MAX_ROWS = 200;
/** Longest title kept: the graph shows one line of it. */
export const PLAN_TITLE_MAX = 120;

const STEP_ID = /^[A-Za-z]{0,3}\d+(?:\.\d+)*[a-z]?$/;
const STEP_REF = /\b[A-Za-z]{0,3}\d+\.\d+[a-z]?\b/g;

/** The cells of one table line, or `null` for a line that is not one. */
function cells(line: string): string[] | null {
  const t = line.trim();
  if (!t.startsWith('|')) return null;
  // `\|` inside a cell is a pipe, not a border.
  const parts = t
    .replace(/\\\|/g, '\u0000')
    .split('|')
    .map((c) => c.replace(/\u0000/g, '|').trim());
  return parts.slice(1, t.endsWith('|') ? -1 : undefined);
}

const isRule = (cs: string[]) => cs.length > 0 && cs.every((c) => /^:?-{2,}:?$/.test(c.replace(/\s/g, '')));

/** Markdown emphasis and escapes out of a cell. */
function plain(cell: string): string {
  return cell
    .replace(/\\([_*`|\\])/g, '$1')
    .replace(/\*\*(.+?)\*\*/g, '$1')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/\s+/g, ' ')
    .trim();
}

/** A title short enough for one line: the step's first clause. */
export function shortTitle(text: string): string {
  const t = plain(text);
  const cut = t.split(/;|\s\(|:\s/)[0].trim() || t;
  return cut.length > PLAN_TITLE_MAX ? `${cut.slice(0, PLAN_TITLE_MAX - 1).trimEnd()}…` : cut;
}

function col(head: string[], ...names: string[]): number {
  return head.findIndex((h) => names.includes(h.toLowerCase().replace(/[*_`]/g, '').trim()));
}

const STATUS_WORDS: Record<string, string> = {
  done: 'done',
  merged: 'done',
  '✓': 'done',
  'in progress': 'in_progress',
  in_progress: 'in_progress',
  doing: 'in_progress',
  open: 'in_progress',
  todo: 'todo',
  'to do': 'todo',
  '': 'todo',
};

/** What `parsePlan` found. */
export interface ParsedPlan {
  rows: ParsedRow[];
  /** A step table had a Repo column: every row then needs a repository. */
  hasRepos: boolean;
  /** Lines worth telling the person: a step listed twice, a lane for a
   *  step no table names. */
  notes: string[];
}

/** Every step row in `text`, in order, with lanes from a Lanes table. */
export function parsePlan(text: string): ParsedPlan {
  const lines = text.split(/\r?\n/);
  const rows: ParsedRow[] = [];
  const at = new Map<string, ParsedRow>();
  let hasRepos = false;
  const laneOf = new Map<string, string>();
  const notes: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    const head = cells(lines[i]);
    const rule = i + 1 < lines.length ? cells(lines[i + 1]) : null;
    if (!head || !rule || !isRule(rule)) continue;
    const step = col(head, '#', 'id', 'step id', 'step #');
    const title = col(head, 'step', 'title', 'task', 'name');
    const needs = col(head, 'needs', 'depends on', 'after', 'dependencies');
    const lane = col(head, 'lane', 'owner');
    const status = col(head, 'status', 'state');
    const repo = col(head, 'repo', 'repository');
    const laneSteps = col(head, 'steps in order', 'steps');
    let j = i + 2;
    for (; j < lines.length; j++) {
      const cs = cells(lines[j]);
      if (!cs) break;
      if (step >= 0 && title >= 0 && step !== title) {
        const id = plain(cs[step] ?? '');
        if (!STEP_ID.test(id)) continue;
        if (at.has(id)) {
          notes.push(`Step ${id} is listed twice; the first one is kept.`);
          continue;
        }
        const r: ParsedRow = { step: id, title: shortTitle(cs[title] ?? '') || id };
        if (repo >= 0) {
          hasRepos = true;
          const named = plain(cs[repo] ?? '');
          if (named && !/^[-—–]$/.test(named)) r.repo = named;
        }
        const ns = needs >= 0 ? [...new Set(plain(cs[needs] ?? '').match(STEP_REF) ?? [])].filter((n) => n !== id) : [];
        if (ns.length) r.needs = ns;
        if (lane >= 0 && plain(cs[lane] ?? '')) r.lane = plain(cs[lane]).split(/\s+·\s+|\s+-\s+/)[0];
        if (status >= 0) {
          const s = STATUS_WORDS[plain(cs[status] ?? '').toLowerCase()];
          if (s) r.status = s;
        }
        rows.push(r);
        at.set(id, r);
      } else if (lane >= 0 && laneSteps >= 0) {
        // "A · Backend and contract" → "Lane A".
        const name = plain(cs[lane] ?? '').split(/\s+·\s+/)[0];
        if (!name) continue;
        const label = name.length <= 2 ? `Lane ${name}` : name;
        for (const s of plain(cs[laneSteps] ?? '').match(STEP_REF) ?? []) laneOf.set(s, label);
      }
    }
    i = j - 1;
  }
  for (const [s, l] of laneOf) {
    const r = at.get(s);
    if (r) r.lane ??= l;
  }
  return { rows, notes, hasRepos };
}

/** A project as the picker reads it. */
export interface RepoChoice {
  id: number;
  owner: string;
  repo: string;
}

/** The project a Repo cell names: `owner/repo` exactly, else a repository
 *  name only one project has (case ignored). `null` when none or several. */
export function matchRepo(text: string | undefined, projects: readonly RepoChoice[]): number | null {
  const t = (text ?? '').trim().toLowerCase().replace(/^https?:\/\/github\.com\//, '').replace(/\.git$/, '');
  if (!t) return null;
  const full = projects.find((p) => `${p.owner}/${p.repo}`.toLowerCase() === t);
  if (full) return full.id;
  const byName = projects.filter((p) => p.repo.toLowerCase() === t);
  return byName.length === 1 ? byName[0].id : null;
}

/** Each step's repository, as its cell names it (`null`: pick one). */
export function initialRepoPicks(rows: readonly ParsedRow[], projects: readonly RepoChoice[]): Record<string, number | null> {
  return Object.fromEntries(rows.map((r) => [r.step, matchRepo(r.repo, projects)]));
}

/** "Row 3 has no repo: pick one" for each row without a repository, when
 *  the table has a Repo column (rows numbered from 1). */
export function missingRepoLines(plan: ParsedPlan, picks: Readonly<Record<string, number | null>>): string[] {
  if (!plan.hasRepos) return [];
  return plan.rows.flatMap((r, i) => (picks[r.step] == null ? [`Row ${i + 1} has no repo: pick one`] : []));
}

/** The rows as the backend takes them, each with its picked repository. */
export function wireRows(rows: readonly ParsedRow[], picks: Readonly<Record<string, number | null>>): PlanRow[] {
  return rows.map(({ repo: _repo, ...r }) => {
    const pid = picks[r.step];
    return pid != null ? { ...r, project_id: pid } : r;
  });
}

/** What an import did, in words. */
export function importLine(r: PlanImport): string {
  const parts: string[] = [];
  if (r.created) parts.push(`${r.created} added`);
  if (r.updated) parts.push(`${r.updated} updated`);
  if (r.unchanged) parts.push(`${r.unchanged} unchanged`);
  const deps = r.deps_added + r.deps_removed;
  if (deps) {
    const links = r.deps_added === 1 ? 'link' : 'links';
    parts.push(`${r.deps_added} ${links} added${r.deps_removed ? `, ${r.deps_removed} removed` : ''}`);
  }
  return parts.length ? `Imported: ${parts.join(', ')}.` : 'Nothing to import.';
}

export function importMissionPlan(missionId: number, plan: PlanRow[]): Promise<Result<PlanImport>> {
  return invokeCmd<PlanImport>('import_mission_plan', { args: { mission_id: missionId, plan } }).then((r) => {
    if (r.ok) bumpWorkChanged();
    return r;
  });
}
