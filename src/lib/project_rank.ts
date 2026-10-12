// The New session picker's ranking (project picker spec v2): pure functions
// from projects + picks + context + frecency to the switcher's sections.
// Pinned → Suggested (≤7) → every project in its group (dormant dimmed,
// last) → Hidden. With a query: one fuzzy list, hidden kept but last.
import { fuzzyMatchFields } from './fuzzy';
import type { ProjectTreeRow } from './projects';
import type { SessionRow } from './sessions';
import { pickKey, type ProjectPick } from './project_picks';
import { decayed, recencyTerm, type FrecencyMap } from './frecency';

export const SUGGESTED_CAP = 7;
export const CLUSTER_MIN = 3;
export const OWNER_GROUP_MIN = 3;
export const ACTIVE_DAYS = 30;
export const DORMANT_DAYS = 90;
export const FORKS = 'Forks & others';
const DAY = 86_400;
const THROWAWAY = [/^(test|tmp|example)-/i, /-analysis$/i, /-epic-\d+$/i];

export type HiddenReason = 'hidden by you' | 'throwaway name';

const daysSince = (at: number | null, now: number) => (at == null ? Infinity : (now - at) / DAY);

export function hiddenReason(p: ProjectTreeRow, pick: ProjectPick | undefined, now: number): HiddenReason | null {
  if (pick?.vis === 'hide') return 'hidden by you';
  if (pick?.vis === 'keep' || pick?.pinned || pick?.grp) return null;
  if (daysSince(p.project.last_session_at, now) <= ACTIVE_DAYS) return null;
  return THROWAWAY.some((re) => re.test(p.project.repo)) ? 'throwaway name' : null;
}

export function isDormant(p: ProjectTreeRow, now: number): boolean {
  return daysSince(p.project.last_session_at, now) > DORMANT_DAYS;
}

export interface GroupInfo {
  key: string;
  name: string;
  sub: string;
  /** Sort rank: person's groups 0, then owners by recency, forks last. */
  ownerOrder: number;
}

export function groupProjects(
  rows: readonly ProjectTreeRow[],
  picks: ReadonlyMap<string, ProjectPick>,
): Map<number, GroupInfo> {
  const byOwner = new Map<string, ProjectTreeRow[]>();
  for (const r of rows) byOwner.set(r.project.owner, [...(byOwner.get(r.project.owner) ?? []), r]);
  const owners = [...byOwner.entries()]
    .map(([owner, list]) => ({ owner, list, last: Math.max(0, ...list.map((r) => r.project.last_session_at ?? 0)) }))
    .sort((a, b) => b.last - a.last || a.owner.localeCompare(b.owner));
  const out = new Map<number, GroupInfo>();
  owners.forEach(({ owner, list }, i) => {
    const toks = (r: ProjectTreeRow) => r.project.repo.toLowerCase().split('-').filter(Boolean);
    const count = new Map<string, number>();
    for (const r of list) {
      const t = toks(r);
      for (let n = 1; n < t.length; n++) {
        const pre = t.slice(0, n).join('-');
        count.set(pre, (count.get(pre) ?? 0) + 1);
      }
    }
    const auto = new Map<number, GroupInfo>();
    for (const r of list) {
      const t = toks(r);
      let best: string | null = null;
      for (let n = t.length - 1; n >= 1; n--) {
        const pre = t.slice(0, n).join('-');
        if ((count.get(pre) ?? 0) >= CLUSTER_MIN) {
          best = pre;
          break;
        }
      }
      if (!best && t.length === 1 && (count.get(t[0]) ?? 0) >= CLUSTER_MIN) best = t[0];
      if (best) {
        auto.set(r.project.id, { key: `c:${owner}:${best}`, name: best, sub: `${best}-* · ${owner}`, ownerOrder: 1 + i });
      } else if (list.length >= OWNER_GROUP_MIN) {
        auto.set(r.project.id, { key: `o:${owner}`, name: `More from ${owner}`, sub: '', ownerOrder: 1 + i });
      } else {
        auto.set(r.project.id, { key: 'f', name: FORKS, sub: '', ownerOrder: 10_000 });
      }
    }
    // This owner's automatic clusters that someone is in, by lower-cased
    // name: a person's group of the same name joins it (one section).
    const clusters = new Map<string, GroupInfo>();
    for (const r of list) {
      if (picks.get(pickKey(owner, r.project.repo))?.grp) continue;
      const g = auto.get(r.project.id)!;
      if (g.key.startsWith('c:')) clusters.set(g.name.toLowerCase(), g);
    }
    for (const r of list) {
      const manual = picks.get(pickKey(owner, r.project.repo))?.grp;
      if (!manual) {
        out.set(r.project.id, auto.get(r.project.id)!);
        continue;
      }
      const lower = manual.toLowerCase();
      out.set(
        r.project.id,
        clusters.get(lower) ?? { key: `m:${lower}`, name: manual, sub: 'your group', ownerOrder: 0 },
      );
    }
  });
  return out;
}

export interface Entry {
  project: ProjectTreeRow;
  id: number;
  key: string;
  label: string;
  pinned: boolean;
  hidden: HiddenReason | null;
  dormant: boolean;
  group: GroupInfo;
  /** The person put it in a group (also when that joined an automatic one). */
  manualGroup: boolean;
}

export function entriesOf(
  projects: readonly ProjectTreeRow[],
  picks: ReadonlyMap<string, ProjectPick>,
  now: number,
): Entry[] {
  const visible = projects.filter((p) => !p.project.system);
  const groups = groupProjects(visible, picks);
  const repoCount = new Map<string, number>();
  for (const p of visible) repoCount.set(p.project.repo, (repoCount.get(p.project.repo) ?? 0) + 1);
  return visible.map((p) => {
    const key = pickKey(p.project.owner, p.project.repo);
    const pk = picks.get(key);
    return {
      project: p,
      id: p.project.id,
      key,
      label: (repoCount.get(p.project.repo) ?? 0) > 1 ? key : p.project.repo,
      pinned: !!pk?.pinned,
      hidden: hiddenReason(p, pk, now),
      dormant: isDormant(p, now),
      group: groups.get(p.project.id)!,
      manualGroup: !!pk?.grp,
    };
  });
}

export function ago(lastSessionAt: number | null, now: number): string {
  if (lastSessionAt == null) return 'no sessions yet';
  const s = Math.max(0, now - lastSessionAt);
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < DAY) return `${Math.round(s / 3600)}h`;
  if (s < 60 * DAY) return `${Math.round(s / DAY)}d`;
  return `${Math.round(s / (30 * DAY))}mo`;
}

export interface Ctx {
  selectedProjectId: number | null;
  preferredHost: string | null;
  sessions: readonly Pick<SessionRow, 'project_id' | 'host_alias' | 'last_activity_at'>[];
}

export interface ViewRow {
  entry: Entry;
  chip: string;
  kbd: string;
  meta: string;
}

export interface ViewSection {
  key: string;
  label: string;
  sub: string;
  foldable: boolean;
  openByDefault: boolean;
  rows: ViewRow[];
}

const byLabel = (a: Entry, b: Entry) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' });
const score = (e: Entry, f: FrecencyMap, now: number) =>
  decayed(f[e.key], now) * 10 + recencyTerm(e.project.project.last_session_at, now);

export function buildSections(entries: Entry[], ctx: Ctx, frecency: FrecencyMap, now: number): ViewSection[] {
  const out: ViewSection[] = [];
  let n = 0;
  const num = () => (n < 9 ? `⌘${++n}` : '');
  const meta = (e: Entry) => ago(e.project.project.last_session_at, now);

  const pinned = entries.filter((e) => e.pinned).sort(byLabel);
  if (pinned.length) {
    out.push({
      key: 'pinned', label: 'Pinned', sub: '', foldable: false, openByDefault: true,
      rows: pinned.map((e) => ({ entry: e, chip: '', kbd: num(), meta: meta(e) })),
    });
  }

  const taken = new Set<number>();
  const sugg: ViewRow[] = [];
  const add = (e: Entry | undefined, chip: string) => {
    if (!e || e.pinned || e.hidden || taken.has(e.id) || sugg.length >= SUGGESTED_CAP) return;
    taken.add(e.id);
    sugg.push({ entry: e, chip, kbd: '', meta: meta(e) });
  };
  const byId = new Map(entries.map((e) => [e.id, e]));
  if (ctx.selectedProjectId != null) add(byId.get(ctx.selectedProjectId), 'current session');
  if (ctx.preferredHost) {
    [...ctx.sessions]
      .filter((s) => s.host_alias === ctx.preferredHost && s.project_id != null)
      .sort((a, b) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0))
      .forEach((s) => add(byId.get(s.project_id!), `on ${ctx.preferredHost}`));
  }
  entries
    .filter((e) => score(e, frecency, now) > 0)
    .sort((a, b) => score(b, frecency, now) - score(a, frecency, now) || byLabel(a, b))
    .forEach((e) => add(e, ''));
  if (sugg.length) {
    sugg.forEach((r) => (r.kbd = num()));
    out.push({ key: 'suggested', label: 'Suggested', sub: 'what you open often, and lately', foldable: false, openByDefault: true, rows: sugg });
  }

  const groups = new Map<string, { info: GroupInfo; members: Entry[] }>();
  for (const e of entries) {
    if (e.hidden) continue;
    const g = groups.get(e.group.key) ?? { info: e.group, members: [] };
    g.members.push(e);
    groups.set(e.group.key, g);
  }
  [...groups.values()]
    .sort(
      (a, b) =>
        a.info.ownerOrder - b.info.ownerOrder ||
        Number(a.info.key.startsWith('o:')) - Number(b.info.key.startsWith('o:')) ||
        a.info.name.localeCompare(b.info.name, undefined, { sensitivity: 'base' }),
    )
    .forEach(({ info, members }) => {
      const live = members.filter((e) => !e.dormant).sort(byLabel);
      const dormant = members
        .filter((e) => e.dormant)
        .sort(
          (a, b) =>
            Number(a.project.project.last_session_at == null) - Number(b.project.project.last_session_at == null) ||
            byLabel(a, b),
        );
      const active = members.some((e) => daysSince(e.project.project.last_session_at, now) <= ACTIVE_DAYS);
      out.push({
        key: `g:${info.key}`,
        label: info.name,
        sub: info.sub || `${members.length} projects`,
        foldable: true,
        openByDefault: active,
        rows: [...live, ...dormant].map((e) => ({ entry: e, chip: '', kbd: '', meta: meta(e) })),
      });
    });

  const hidden = entries.filter((e) => e.hidden).sort(byLabel);
  if (hidden.length) {
    out.push({
      key: 'hidden', label: 'Hidden', sub: 'throwaway names and what you hid · search still finds them',
      foldable: true, openByDefault: false,
      rows: hidden.map((e) => ({ entry: e, chip: '', kbd: '', meta: e.hidden! })),
    });
  }
  return out;
}

export function searchEntries(entries: Entry[], query: string, ctx: Ctx, frecency: FrecencyMap, now: number): ViewRow[] {
  const q = query.trim();
  if (!q) return [];
  return entries
    .map((e) => ({ e, s: fuzzyMatchFields(q, [e.key, e.project.project.repo, e.group.name]) }))
    .filter((x): x is { e: Entry; s: number } => x.s !== null)
    .map(({ e, s }) => {
      let total = s;
      if (e.pinned) total += 40;
      total += Math.min(60, score(e, frecency, now));
      if (ctx.selectedProjectId === e.id) total += 80;
      if (e.hidden) total -= 400;
      return { e, total };
    })
    .sort((a, b) => b.total - a.total || byLabel(a.e, b.e))
    .map(({ e }) => ({
      entry: e,
      chip: ctx.selectedProjectId === e.id ? 'current session' : '',
      kbd: '',
      meta: e.hidden ? `hidden · ${e.hidden}` : ago(e.project.project.last_session_at, now),
    }));
}
