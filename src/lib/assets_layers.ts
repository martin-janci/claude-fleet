// Assets M6 (R17, R18): layer footprints, "why is it on <host>?", a host's
// role per catalog and which catalogs a host accepts — mirrors of
// effective.rs `acceptance` and the resolver's role → context → extends chain.
import type { CatalogStatus, ChangesetSummary, LayerDef, LayerListing } from './assets_workspace';

function parentOf(l: LayerListing, name: string): string | null {
  return l.layers.find((x) => x.name === name)?.extends ?? null;
}

export function layerFootprint(l: LayerListing): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>(l.layers.map((x) => [x.name, new Set<string>()]));
  for (const r of l.hosts) {
    if (!r.active) continue;
    let name: string | null = r.layer_name;
    const seen = new Set<string>();
    while (name && !seen.has(name)) {
      seen.add(name);
      if (!out.has(name)) out.set(name, new Set());
      out.get(name)!.add(r.host_alias);
      name = parentOf(l, name);
    }
  }
  return out;
}

export function whyChain(l: LayerListing, host: string, layer: string): string[] {
  // The resolver follows `extends` from a role and from a context alike
  // (sync/layers.rs: "A context may itself extend another context").
  for (const r of l.hosts.filter((x) => x.host_alias === host && x.active)) {
    const chain = [`${r.axis} ${r.layer_name}`];
    let name: string | null = r.layer_name;
    const seen = new Set<string>();
    while (name && !seen.has(name)) {
      if (name === layer) return chain;
      seen.add(name);
      name = parentOf(l, name);
      if (name) chain.push(`extends ${name}`);
    }
  }
  return [];
}

export function roleIn(l: LayerListing, host: string): string | null {
  return l.hosts.find((r) => r.host_alias === host && r.axis === 'role' && r.active)?.layer_name ?? null;
}

export interface Acceptance { state: 'all' | 'shared' | 'none'; locked: boolean; why: string }

export function acceptanceOf(host: { alias: string; org_id?: number | null }, c: CatalogStatus): Acceptance {
  const org = host.org_id ?? null;
  if (c.org_id === null) {
    return org === null
      ? { state: 'all', locked: true, why: 'personal reaches every host' }
      : { state: 'shared', locked: true, why: 'an org host receives only shared assets' };
  }
  if (org !== null) {
    return org === c.org_id
      ? { state: 'all', locked: true, why: 'via its org' }
      : { state: 'none', locked: true, why: 'a host of another org never receives it' };
  }
  return (c.admitted ?? []).includes(host.alias)
    ? { state: 'all', locked: false, why: 'admitted' }
    : { state: 'none', locked: false, why: 'not admitted' };
}

/** The proposed cards that change layer `name` of `catalog` (M15 G7.13:
 *  the layer inspector's Changeset tab): a layer or rollout card committing
 *  to that catalog whose summary names the layer as a word. */
export function layerCards(
  cards: readonly ChangesetSummary[],
  catalog: string,
  name: string,
): ChangesetSummary[] {
  const word = new RegExp(`(^|[^\\w-])${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}($|[^\\w-])`);
  return cards.filter(
    (c) =>
      c.state === 'proposed' &&
      !c.withdrawn &&
      (c.kind === 'layer' || c.kind === 'rollout') &&
      (!c.catalogs || c.catalogs.length === 0 || c.catalogs.includes(catalog)) &&
      word.test(c.summary),
  );
}

/** A layer as the catalog's layer file defines it, for the Source tab. */
export function layerSource(layer: LayerDef): string {
  const lines = [`name: ${layer.name}`, `axis: ${layer.axis}`];
  if (layer.extends) lines.push(`extends: ${layer.extends}`);
  if (layer.description) lines.push(`description: ${JSON.stringify(layer.description)}`);
  const members = layer.members ?? [];
  lines.push(members.length === 0 ? 'members: []' : 'members:');
  for (const m of members) lines.push(`  - ${m}`);
  return lines.join('\n');
}
