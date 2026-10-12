// Assets M5: the token query (spec, Query; Rulings R21). `key:v1,v2` tokens
// AND together, values within one token OR; unknown keys and bare words are
// free text matched against name and description. Case-insensitive.
import { KIND_ORDER, type AssetIdentity, type AssetInventoryRow, type AssetSummary } from './assets';

export const QUERY_KEYS = ['host', 'kind', 'state', 'layer', 'catalog', 'scope'] as const;
export type QueryKey = (typeof QUERY_KEYS)[number];
export interface QueryToken { key: QueryKey; values: string[] }
export interface ParsedQuery { tokens: QueryToken[]; text: string }

/** What a list row offers the query. `catalog: null` = in no catalog (an
 *  identity, an orphan). */
export interface QueryRow {
  kind: string;
  name: string;
  description?: string;
  catalog?: string | null;
  scope?: string | null;
  hosts: { host_alias: string; state: string; drift_side?: string | null }[];
  layers?: string[];
  /** R20: an asset this window can only look at. */
  managedElsewhere?: boolean;
}

export interface QueryVocab { hosts: string[]; layers: string[]; catalogs: string[] }

export const STATES = ['in_sync', 'drifted', 'edited', 'behind', 'missing', 'unmanaged', 'orphan', 'unsupported'];
export const SCOPES = ['private', 'shared', 'org', 'managed'];
const KIND_ALIASES: Record<string, string> = { mcp: 'mcp_server', plugin: 'plugin_ref', skills: 'skill', agents: 'agent', hooks: 'hook' };
const PRESENT = new Set(['in_sync', 'drifted', 'unmanaged', 'orphan']);
const isKey = (k: string): k is QueryKey => (QUERY_KEYS as readonly string[]).includes(k);
const snake = (v: string) => v.replace(/-/g, '_');

export function parseQuery(raw: string): ParsedQuery {
  const tokens: QueryToken[] = [];
  const text: string[] = [];
  for (const part of raw.split(/\s+/).filter(Boolean)) {
    const i = part.indexOf(':');
    const key = i > 0 ? part.slice(0, i).toLowerCase() : '';
    if (isKey(key)) {
      const values = part.slice(i + 1).split(',').map((v) => v.trim().toLowerCase()).filter(Boolean);
      if (values.length) tokens.push({ key, values });
      continue;
    }
    text.push(part.toLowerCase());
  }
  return { tokens, text: text.join(' ') };
}

function matchesToken(key: QueryKey, value: string, row: QueryRow): boolean {
  switch (key) {
    case 'host':
      return row.hosts.some((h) => h.host_alias.toLowerCase() === value && PRESENT.has(h.state));
    case 'kind':
      return row.kind === snake(KIND_ALIASES[value] ?? value);
    case 'state': {
      const s = snake(value);
      if (s === 'edited') return row.hosts.some((h) => h.state === 'drifted' && h.drift_side === 'host');
      if (s === 'behind') return row.hosts.some((h) => h.state === 'drifted' && h.drift_side === 'catalog');
      return row.hosts.some((h) => h.state === s);
    }
    case 'layer':
      return (row.layers ?? []).some((l) => l.toLowerCase() === value);
    case 'catalog':
      return (row.catalog ?? '').toLowerCase() === value;
    case 'scope':
      if (value === 'managed') return !!row.managedElsewhere;
      if (value === 'org') return !!row.catalog && row.catalog !== 'personal';
      return row.catalog === 'personal' && (row.scope ?? 'private') === value;
  }
}

export function matchesQuery(q: ParsedQuery, row: QueryRow): boolean {
  for (const t of q.tokens) if (!t.values.some((v) => matchesToken(t.key, v, row))) return false;
  if (!q.text) return true;
  const hay = `${row.name} ${row.description ?? ''}`.toLowerCase();
  return q.text.split(' ').every((w) => hay.includes(w));
}

/** The one search behaviour every view shares (PF11 / R21): the Inbox and
 *  the Library both ask this of each row, with the whole query, tokens and
 *  free words alike. Takes the raw text or an already parsed query. */
export function keep(query: string | ParsedQuery, row: QueryRow): boolean {
  return matchesQuery(typeof query === 'string' ? parseQuery(query) : query, row);
}

function valuesFor(key: QueryKey, vocab: QueryVocab): string[] {
  switch (key) {
    case 'host': return vocab.hosts;
    case 'kind': return [...KIND_ORDER];
    case 'state': return STATES;
    case 'layer': return vocab.layers;
    case 'catalog': return vocab.catalogs;
    case 'scope': return SCOPES;
  }
}

/** Completions for the fragment at the end of `raw` — whole replacement
 *  fragments, at most 8; none for an empty fragment. */
export function completions(raw: string, vocab: QueryVocab): string[] {
  const frag = (/(\S*)$/.exec(raw)?.[1] ?? '').toLowerCase();
  if (!frag) return [];
  const i = frag.indexOf(':');
  if (i < 0) return QUERY_KEYS.filter((k) => k.startsWith(frag)).map((k) => `${k}:`);
  const key = frag.slice(0, i);
  if (!isKey(key)) return [];
  const done = frag.slice(i + 1).split(',');
  const partial = done.pop() ?? '';
  const head = `${key}:${done.map((v) => `${v},`).join('')}`;
  return valuesFor(key, vocab)
    .filter((v) => v.toLowerCase().startsWith(partial) && !done.includes(v.toLowerCase()))
    .slice(0, 8)
    .map((v) => `${head}${v}`);
}

/** `raw` with its last fragment replaced by `completion`. */
export function applyCompletion(raw: string, completion: string): string {
  return raw.replace(/\S*$/, completion);
}

export function rowOfAsset(a: AssetSummary, layers: string[] = []): QueryRow {
  return { kind: a.kind, name: a.name, description: a.description, catalog: a.catalog ?? 'personal', scope: a.scope ?? 'private', hosts: a.hosts, layers };
}

export function rowOfIdentity(id: AssetIdentity): QueryRow {
  return { kind: id.kind, name: id.name, catalog: null, scope: null, hosts: id.hosts.map((h) => ({ host_alias: h.host_alias, state: 'unmanaged' })) };
}

export function rowOfOrphan(rows: AssetInventoryRow[]): QueryRow {
  return { kind: rows[0].kind, name: rows[0].name, catalog: null, scope: null, hosts: rows.map((r) => ({ host_alias: r.host_alias, state: 'orphan' })) };
}
