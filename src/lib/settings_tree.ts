// The one Settings tree (redesign step 7.1, boards Settings, SettingsMore,
// SettingsSections and SettingsHub). Every Settings screen is a leaf here: a
// hand-written panel, a generated page (`crates/fleet-core/pages/`), one
// section of a generated page, or a pointer to a rail view. The nav renders
// these groups in order; a page no leaf names still gets a place (nested
// under the page that is its parent, or in "More"), so nothing a page spec
// adds is ever unreachable.
import type { Page } from './pages/pages';

/** The hand-written panels `SettingsDialog.svelte` renders. */
export type PanelId =
  | 'appearance'
  | 'hosts'
  | 'notifications'
  | 'shortcuts'
  | 'composer'
  | 'projects'
  | 'work'
  | 'hub'
  | 'mcp'
  | 'diagnostics';

export interface SettingsLeaf {
  id: string;
  label: string;
  /** A hand-written panel, shown first. */
  panel?: PanelId;
  /** A generated page, shown after the panel. */
  page?: string;
  /** Only this section (by title) of `page`, with a link to the whole page. */
  section?: string;
  /** The panel edits the same settings as `page`: show the page only where
   *  the panel cannot work (a paired desktop, which does not own the fleet). */
  pageOnlyRemote?: boolean;
  /** The leaf points at a rail view (marked ↗): its panel says what is there
   *  and opens it. Settings holds preferences, not that view's content. */
  elsewhere?: boolean;
}

export interface SettingsGroup {
  title: string;
  items: SettingsLeaf[];
}

export const SETTINGS_TREE: readonly SettingsGroup[] = [
  {
    title: 'General',
    items: [
      { id: 'appearance', label: 'Appearance', panel: 'appearance' },
      { id: 'notifications', label: 'Notifications', panel: 'notifications', page: 'settings.notifications' },
      { id: 'shortcuts', label: 'Shortcuts', panel: 'shortcuts' },
      { id: 'voice', label: 'Voice', page: 'settings.limits', section: 'Voice' },
      { id: 'downloads', label: 'Downloads', page: 'settings.limits', section: 'Downloads' },
      { id: 'updates', label: 'Updates', page: 'settings.updates' },
    ],
  },
  {
    title: 'Sessions',
    items: [
      {
        id: 'sessions',
        label: 'Sessions & agents',
        panel: 'composer',
        page: 'settings.limits',
        section: 'Sessions and tasks',
      },
      {
        id: 'projects',
        label: 'Projects',
        panel: 'projects',
        page: 'settings.projects',
        pageOnlyRemote: true,
      },
      {
        id: 'restore',
        label: 'Restore lost sessions',
        page: 'settings.limits',
        section: 'Restoring lost sessions',
      },
      {
        id: 'retention',
        label: 'Clean-up & retention',
        page: 'settings.automation',
        section: 'Garbage collection',
      },
    ],
  },
  {
    title: 'Work',
    items: [
      { id: 'work', label: 'Work & trackers', panel: 'work', page: 'settings.work' },
      { id: 'trackers', label: 'Trackers', page: 'settings.trackers' },
      { id: 'decisions', label: 'Decisions', page: 'settings.decisions' },
      { id: 'playbooks', label: 'Playbooks', page: 'settings.automation', section: 'Playbooks' },
      { id: 'catalogs', label: 'Catalogs', page: 'settings.catalogs' },
    ],
  },
  {
    title: 'Organisations',
    items: [
      { id: 'orgs', label: 'Organisations', page: 'settings.orgs' },
      { id: 'people', label: 'People', page: 'settings.people' },
      { id: 'devices', label: 'Devices', page: 'settings.devices' },
      { id: 'federation', label: 'Federation', page: 'settings.federation' },
      { id: 'debug-devices', label: 'Debug devices', page: 'debug_devices' },
    ],
  },
  {
    title: 'System',
    items: [
      { id: 'hub', label: 'Hub & sync', panel: 'hub', page: 'settings.hub' },
      { id: 'control-api', label: 'Control API', panel: 'mcp', page: 'settings.control_api' },
      {
        id: 'error-reports',
        label: 'Error reports',
        panel: 'diagnostics',
        page: 'settings.limits',
        section: 'Error reports',
      },
      {
        id: 'repair',
        label: 'Repair workspace',
        page: 'settings.automation',
        section: 'Workspace repair',
      },
      { id: 'review', label: 'Proposed changes', page: 'settings.review' },
      { id: 'automation', label: 'Automation', page: 'settings.automation' },
      { id: 'limits', label: 'Limits', page: 'settings.limits' },
      { id: 'advanced', label: 'Advanced', page: 'settings' },
    ],
  },
  {
    title: 'Usage',
    items: [
      { id: 'usage', label: 'Usage', page: 'usage' },
      { id: 'usage-accounts', label: 'Claude accounts', page: 'usage.accounts' },
      { id: 'usage-work', label: 'Work graph usage', page: 'usage.work' },
    ],
  },
  {
    title: 'Elsewhere',
    items: [
      { id: 'accounts-hosts', label: 'Accounts & hosts', panel: 'hosts', elsewhere: true },
      { id: 'guides', label: 'Guides', page: 'guides' },
    ],
  },
];

export const DEFAULT_LEAF = 'appearance';

/** A nav row: a tree leaf, or a page the tree does not name. */
export interface NavRow {
  id: string;
  label: string;
  depth: 0 | 1;
  leaf: SettingsLeaf;
}

export interface NavGroup {
  title: string;
  rows: NavRow[];
}

/** Pages a person navigates (embeds sit inside other screens). */
function navigable(pages: Page[]): Page[] {
  return pages.filter((p) => p.layout !== 'embed');
}

/** A page with no leaf of its own gets one: id `page:<id>`. */
export function pageLeaf(page: Page): SettingsLeaf {
  return { id: `page:${page.id}`, label: page.title, page: page.id };
}

/**
 * The nav as rendered: the tree's groups, each whole-page leaf followed by
 * the child pages the tree does not name (a guide under Guides, a new
 * settings page under Advanced), and any other unnamed page in "More".
 */
export function navGroups(pages: Page[], tree: readonly SettingsGroup[] = SETTINGS_TREE): NavGroup[] {
  const nav = navigable(pages);
  const named = new Set(tree.flatMap((g) => g.items.map((l) => l.page)).filter(Boolean) as string[]);
  const placed = new Set<string>();
  const groups: NavGroup[] = tree.map((g) => ({
    title: g.title,
    rows: g.items.flatMap((leaf) => {
      const rows: NavRow[] = [{ id: leaf.id, label: leaf.label, depth: 0, leaf }];
      if (leaf.page && !leaf.section) {
        for (const child of nav.filter((p) => p.parent === leaf.page && !named.has(p.id))) {
          placed.add(child.id);
          rows.push({ id: `page:${child.id}`, label: child.title, depth: 1, leaf: pageLeaf(child) });
        }
      }
      return rows;
    }),
  }));
  const rest = nav.filter((p) => !named.has(p.id) && !placed.has(p.id));
  if (rest.length) {
    groups.push({
      title: 'More',
      rows: rest.map((p) => ({ id: `page:${p.id}`, label: p.title, depth: 0, leaf: pageLeaf(p) })),
    });
  }
  return groups;
}

/** Every leaf the nav shows, in order. */
export function navLeaves(pages: Page[]): SettingsLeaf[] {
  return navGroups(pages).flatMap((g) => g.rows.map((r) => r.leaf));
}

/** The leaf with `id`, among the tree's and the pages' own. */
export function leafById(id: string, pages: Page[]): SettingsLeaf | undefined {
  return navLeaves(pages).find((l) => l.id === id);
}

/**
 * The leaf that shows `pageId`, for a search hit, a link between pages or
 * `openSettingsAt`: the leaf holding the section `key` sits in when there is
 * one, else the leaf showing the whole page, else any leaf on that page.
 */
export function leafForPage(pageId: string, pages: Page[], key?: string | null): SettingsLeaf | undefined {
  const leaves = navLeaves(pages).filter((l) => l.page === pageId);
  if (key) {
    const page = pages.find((p) => p.id === pageId);
    const sec = [...(page?.sections ?? []), ...(page?.tabs ?? []).flatMap((t) => t.sections)].find((s) =>
      s.items.some((i) => i.type === 'field' && i.key === key),
    );
    const bySection = sec && leaves.find((l) => l.section === sec.title);
    if (bySection) return bySection;
  }
  return leaves.find((l) => !l.section) ?? leaves[0];
}

/** Names `openSettingsAt` took before the tree: the old General panels. */
const LEGACY_SECTIONS: Record<string, string> = {
  general: 'appearance',
  onboarding: 'appearance',
  hosts: 'accounts-hosts',
  composer: 'sessions',
  diagnostics: 'error-reports',
  mcp: 'control-api',
  // The "Jev degraded" pill (`decide_health.DECIDE_SECTION`).
  decide: 'decisions',
};

/** Resolve an `openSettingsAt` request (a leaf id, a page id or an old
 *  section name) to a leaf id. */
export function resolveSection(section: string, pages: Page[]): string | undefined {
  const id = LEGACY_SECTIONS[section] ?? section;
  if (leafById(id, pages)) return id;
  return leafForPage(section, pages)?.id;
}

/**
 * The pages reachable from the tree: a leaf's page, the children nested
 * under it, and every page a reachable page links to (the Advanced
 * overview's cards). The 7.1 check: every generated page is in here.
 */
export function reachablePages(pages: Page[]): Set<string> {
  const seen = new Set<string>();
  const queue = navLeaves(pages).flatMap((l) => (l.page ? [l.page] : []));
  while (queue.length) {
    const id = queue.pop() as string;
    if (seen.has(id)) continue;
    seen.add(id);
    const page = pages.find((p) => p.id === id);
    if (!page) continue;
    for (const s of [...(page.sections ?? []), ...(page.tabs ?? []).flatMap((t) => t.sections)]) {
      for (const i of s.items) if (i.type === 'link') queue.push(i.page);
    }
  }
  return seen;
}
