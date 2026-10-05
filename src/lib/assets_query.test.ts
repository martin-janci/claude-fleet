import { describe, it, expect } from 'vitest';
import { parseQuery, matchesQuery, keep, completions, applyCompletion, type QueryRow } from './assets_query';

const row = (over: Partial<QueryRow> = {}): QueryRow => ({
  kind: 'skill', name: 'infra-status', description: 'Health-check the infrastructure',
  catalog: 'personal', scope: 'shared', layers: ['core'],
  hosts: [
    { host_alias: 'local', state: 'drifted', drift_side: 'host' },
    { host_alias: 'oci', state: 'in_sync' },
    { host_alias: 'htz', state: 'missing' },
  ],
  ...over,
});

describe('parseQuery', () => {
  it('splits tokens from free text, case-insensitively, values by comma', () => {
    expect(parseQuery('Kind:Skill scope:shared,ORG infra')).toEqual({
      tokens: [{ key: 'kind', values: ['skill'] }, { key: 'scope', values: ['shared', 'org'] }],
      text: 'infra',
    });
  });
  it('an unknown key is free text; a key with no value yet matches everything', () => {
    expect(parseQuery('owner:me kind:')).toEqual({ tokens: [], text: 'owner:me' });
  });
});

describe('matchesQuery', () => {
  const m = (q: string, r: QueryRow = row()) => matchesQuery(parseQuery(q), r);
  it('host: is "present there", not missing', () => {
    expect(m('host:oci')).toBe(true);
    expect(m('host:local')).toBe(true);
    expect(m('host:htz')).toBe(false);
  });
  it('kind: takes aliases and dashes', () => {
    expect(m('kind:mcp', row({ kind: 'mcp_server' }))).toBe(true);
    expect(m('kind:mcp-server', row({ kind: 'mcp_server' }))).toBe(true);
    expect(m('kind:agent')).toBe(false);
  });
  it('state: reads any host, plus edited/behind by drift side', () => {
    expect(m('state:in-sync')).toBe(true);
    expect(m('state:edited')).toBe(true);
    expect(m('state:behind')).toBe(false);
    expect(m('state:orphan')).toBe(false);
  });
  it('layer:, catalog: and scope:', () => {
    expect(m('layer:CORE')).toBe(true);
    expect(m('catalog:papayapos')).toBe(false);
    expect(m('scope:org', row({ catalog: 'papayapos', scope: 'private' }))).toBe(true);
    expect(m('scope:private', row({ scope: undefined }))).toBe(true);
    expect(m('scope:managed', row({ managedElsewhere: true }))).toBe(true);
  });
  it('ORs values within a token and ANDs tokens and words', () => {
    expect(m('kind:agent,skill state:missing infra')).toBe(true);
    expect(m('kind:agent,skill state:orphan')).toBe(false);
    expect(m('health check')).toBe(true);
    expect(m('health nope')).toBe(false);
  });
});

describe('completions', () => {
  const vocab = { hosts: ['local', 'oci'], layers: ['core'], catalogs: ['personal', 'papayapos'] };
  it('offers keys, then that key’s values, continuing after a comma', () => {
    expect(completions('ho', vocab)).toEqual(['host:']);
    expect(completions('kind:sk', vocab)).toEqual(['kind:skill']);
    expect(completions('catalog:p', vocab)).toEqual(['catalog:personal', 'catalog:papayapos']);
    expect(completions('host:local,', vocab)).toEqual(['host:local,oci']);
    expect(completions('', vocab)).toEqual([]);
    expect(completions('nope:x', vocab)).toEqual([]);
    expect(completions('state:un', vocab)).toEqual(['state:unmanaged', 'state:unsupported']);
  });
  it('replaces only the fragment being typed', () => {
    expect(applyCompletion('kind:skill ho', 'host:')).toBe('kind:skill host:');
  });
});

describe('keep (the one search behaviour every view uses)', () => {
  it('takes the raw text or a parsed query, and an empty query keeps everything', () => {
    expect(keep('', row())).toBe(true);
    expect(keep('KIND:skill HEALTH', row())).toBe(true);
    expect(keep(parseQuery('state:edited oci'), row())).toBe(false);
    expect(keep('kind:agent', row())).toBe(false);
  });
  it('matches free words case-insensitively against the name and the description', () => {
    expect(keep('INFRA', row())).toBe(true);
    expect(keep('infrastructure', row())).toBe(true);
    expect(keep('infra nothing', row())).toBe(false);
    expect(keep('infra', row({ description: undefined }))).toBe(true);
  });
  it('a trailing key with no value yet matches everything while typing', () => {
    expect(keep('infra kind:', row())).toBe(true);
  });
  it('state:behind reads the catalog side and a copy of unknown side is neither edited nor behind', () => {
    const r = row({ hosts: [{ host_alias: 'oci', state: 'drifted', drift_side: 'catalog' }, { host_alias: 'trn', state: 'drifted' }] });
    expect(keep('state:behind', r)).toBe(true);
    expect(keep('state:edited', r)).toBe(false);
    expect(keep('state:drifted', r)).toBe(true);
  });
});

describe('completions limits', () => {
  it('offers at most eight values, and none already taken', () => {
    const hosts = Array.from({ length: 12 }, (_, i) => `h${i}`);
    expect(completions('host:h', { hosts, layers: [], catalogs: [] })).toHaveLength(8);
    expect(completions('host:h0,h', { hosts, layers: [], catalogs: [] })).not.toContain('host:h0,h0');
  });
  it('is case-insensitive on the fragment', () => {
    expect(completions('KIND:SK', { hosts: [], layers: [], catalogs: [] })).toEqual(['kind:skill']);
  });
});
