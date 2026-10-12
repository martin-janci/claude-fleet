import { readdirSync, readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import type { Page } from './pages/pages';
import { DECIDE_SECTION } from './decide_health';
import {
  SETTINGS_TREE,
  leafById,
  leafForPage,
  navGroups,
  reachablePages,
  resolveSection,
} from './settings_tree';

// Redesign step 7.1: one Settings tree. Its check is that every generated
// page is reachable from it, read from the page specs themselves so a new
// spec cannot be added without a place in Settings.
const DIR = 'crates/fleet-core/pages';
const pages: Page[] = readdirSync(DIR)
  .filter((f) => f.endsWith('.json'))
  .map((f) => JSON.parse(readFileSync(`${DIR}/${f}`, 'utf8')) as Page);
const navigable = pages.filter((p) => p.layout !== 'embed');

function sectionTitles(p: Page): string[] {
  return [...(p.sections ?? []), ...(p.tabs ?? []).flatMap((t) => t.sections)].map((s) => s.title);
}

describe('the Settings tree', () => {
  it('reaches every generated page', () => {
    const reach = reachablePages(pages);
    expect(navigable.map((p) => p.id).filter((id) => !reach.has(id))).toEqual([]);
  });

  it('names only pages and sections that exist', () => {
    const bad: string[] = [];
    for (const g of SETTINGS_TREE) {
      for (const l of g.items) {
        const page = l.page ? pages.find((p) => p.id === l.page) : undefined;
        if (l.page && !page) bad.push(`${l.id}: no page ${l.page}`);
        if (l.section && page && !sectionTitles(page).includes(l.section)) bad.push(`${l.id}: no section ${l.section}`);
        if (l.section && !l.page) bad.push(`${l.id}: a section with no page`);
        if (!l.page && !l.panel) bad.push(`${l.id}: shows nothing`);
      }
    }
    expect(bad).toEqual([]);
  });

  it('holds the sections the plan names, under unique ids', () => {
    const leaves = SETTINGS_TREE.flatMap((g) => g.items);
    const ids = leaves.map((l) => l.id);
    expect(ids.filter((id, i) => ids.indexOf(id) !== i)).toEqual([]);
    const labels = leaves.map((l) => l.label);
    for (const want of [
      'Appearance',
      'Notifications',
      'Shortcuts',
      'Voice',
      'Downloads',
      'Decisions',
      'Playbooks',
      'Clean-up & retention',
      'Error reports',
      'Repair workspace',
      'Projects',
      'Accounts & hosts',
    ]) {
      expect(labels).toContain(want);
    }
    expect(navigable.length).toBeGreaterThan(0);
    expect(leaves.length).toBeGreaterThanOrEqual(15);
  });

  it('nests an unnamed child page under its parent, and puts any other page in More', () => {
    const guide = { id: 'guides.deploy', title: 'Deploy', parent: 'guides', layout: 'guide', sections: [] } as unknown as Page;
    const stray = { id: 'lab', title: 'Lab', layout: 'category', sections: [] } as unknown as Page;
    const groups = navGroups([...pages, guide, stray]);
    const elsewhere = groups.find((g) => g.title === 'Elsewhere');
    const at = elsewhere?.rows.findIndex((r) => r.id === 'page:guides.deploy') ?? -1;
    expect(at).toBeGreaterThan(0);
    expect(elsewhere?.rows[at - 1].id).toBe('guides');
    expect(elsewhere?.rows[at].depth).toBe(1);
    expect(groups.at(-1)?.title).toBe('More');
    expect(groups.at(-1)?.rows.map((r) => r.id)).toEqual(['page:lab']);
    expect(navGroups(pages).some((g) => g.title === 'More')).toBe(false);
  });

  it('opens a page on the leaf that holds the setting, else on its whole-page leaf', () => {
    expect(leafForPage('settings.limits', pages, 'voice.max_capture_secs')?.id).toBe('voice');
    expect(leafForPage('settings.limits', pages, 'move.wait_max_mins')?.id).toBe('limits');
    expect(leafForPage('settings.limits', pages)?.id).toBe('limits');
    expect(leafForPage('settings.automation', pages, 'playbooks.press_enter')?.id).toBe('playbooks');
    expect(leafForPage('settings.work', pages, 'work.recent_days')?.id).toBe('work');
  });

  it('resolves the old section names, page ids and leaf ids', () => {
    expect(resolveSection('diagnostics', pages)).toBe('error-reports');
    expect(resolveSection('general', pages)).toBe('appearance');
    expect(resolveSection(DECIDE_SECTION, pages)).toBe('decisions');
    expect(resolveSection('settings.trackers', pages)).toBe('trackers');
    expect(resolveSection('settings', pages)).toBe('advanced');
    expect(resolveSection('hub', pages)).toBe('hub');
    expect(resolveSection('nope', pages)).toBeUndefined();
    expect(leafById('voice', pages)?.section).toBe('Voice');
  });
});
