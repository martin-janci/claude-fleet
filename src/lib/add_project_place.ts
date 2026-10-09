// Where Add project puts a project it just added (redesign 6.11, the
// AddProject board's Organisation and Tracker fields). Neither is a column
// on the project: an organisation is an `org_rules` row naming the
// repository, and a GitHub tracker covers a repository by listing it in its
// `settings.repos`. Both writes are best effort after the add — the project
// is in the fleet either way, and a refusal (a paired device that is not a
// trusted full device, an older hub) comes back as a sentence to show.
import type { OrgDetail, OrgRuleRow } from './orgs';
import type { ProjectRow } from './projects';
import type { TrackerRow } from './trackers';
import type { Result } from './result';

/** The org an `owner/repo` already belongs to by a rule naming it or its
 *  owner (the most specific wins, as `store::orgs` resolves it), or null. */
export function coveringOrgId(orgs: readonly OrgDetail[], owner: string, repo: string): number | null {
  const o = owner.toLowerCase();
  const r = repo.toLowerCase();
  let ownerOnly: number | null = null;
  for (const org of orgs) {
    for (const rule of org.rules ?? []) {
      if (rule.path_prefix || rule.host_alias || rule.owner?.toLowerCase() !== o) continue;
      if (rule.repo?.toLowerCase() === r) return org.id;
      if (!rule.repo && ownerOnly === null) ownerOnly = org.id;
    }
  }
  return ownerOnly;
}

/** The rule that puts this project in `orgId`: its repository, or for an
 *  adopted folder (owner `local`) its path. */
export function ruleFor(orgId: number, p: ProjectRow): Omit<OrgRuleRow, 'id'> {
  return p.owner === 'local'
    ? { org_id: orgId, path_prefix: p.base_path }
    : { org_id: orgId, owner: p.owner, repo: p.repo };
}

/** GitHub trackers: the only kind a repository can be pointed at today. */
export function githubTrackers(rows: readonly TrackerRow[]): TrackerRow[] {
  return rows.filter((t) => t.provider === 'github');
}

/** The repositories a GitHub tracker covers after adding `owner/repo`, or
 *  null when it covers it already. An empty list means "everything assigned
 *  to me across the site", which already covers it: adding one would
 *  narrow the tracker to that repository alone. */
export function reposWith(t: TrackerRow, owner: string, repo: string): string[] | null {
  const repos = t.settings?.repos ?? [];
  if (repos.length === 0) return null;
  const want = `${owner}/${repo}`.toLowerCase();
  if (repos.some((r) => r.toLowerCase() === want)) return null;
  return [...repos, want];
}

export interface PlaceDeps {
  addOrgRule: (rule: Omit<OrgRuleRow, 'id'>) => Promise<Result<unknown>>;
  updateTracker: (id: number, opts: { settings: NonNullable<TrackerRow['settings']> }) => Promise<Result<TrackerRow>>;
}

/** Puts `p` in the org and under the tracker asked for. Answers what could
 *  not be done, one sentence each; empty when everything went through. */
export async function placeProject(
  p: ProjectRow,
  want: { orgId: number | null; tracker: TrackerRow | null },
  orgs: readonly OrgDetail[],
  deps: PlaceDeps,
): Promise<string[]> {
  const problems: string[] = [];
  const name = p.owner === 'local' ? p.repo : `${p.owner}/${p.repo}`;
  if (want.orgId !== null && coveringOrgId(orgs, p.owner, p.repo) !== want.orgId) {
    const r = await deps.addOrgRule(ruleFor(want.orgId, p));
    if (!r.ok) problems.push(`${name} was added but not put in the organisation: ${r.error.message}`);
  }
  if (want.tracker && p.owner !== 'local') {
    const repos = reposWith(want.tracker, p.owner, p.repo);
    if (repos) {
      const r = await deps.updateTracker(want.tracker.id, { settings: { ...want.tracker.settings, repos } });
      if (!r.ok) problems.push(`${name} was added but ${want.tracker.name} does not cover it yet: ${r.error.message}`);
    }
  }
  return problems;
}
