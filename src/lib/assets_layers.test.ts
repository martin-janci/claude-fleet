import { describe, it, expect } from 'vitest';
import { layerFootprint, whyChain, roleIn, acceptanceOf } from './assets_layers';
import type { LayerListing, CatalogStatus } from './assets_workspace';

const L: LayerListing = {
  layers: [
    { name: 'base', axis: 'role' },
    { name: 'server', axis: 'role', extends: 'base' },
    { name: 'core', axis: 'context', members: ['skill/w'] },
  ],
  hosts: [
    { host_alias: 'oci', catalog_id: 1, layer_name: 'server', axis: 'role', position: 0, active: true },
    { host_alias: 'oci', catalog_id: 1, layer_name: 'core', axis: 'context', position: 0, active: true },
    { host_alias: 'htz', catalog_id: 1, layer_name: 'core', axis: 'context', position: 0, active: false },
  ],
};

describe('layerFootprint', () => {
  it('counts active rows and roles that extend a layer', () => {
    const f = layerFootprint(L);
    expect([...f.get('server')!]).toEqual(['oci']);
    expect([...f.get('base')!]).toEqual(['oci']);
    expect([...f.get('core')!]).toEqual(['oci']);
  });
});

describe('whyChain', () => {
  it('walks role → extends to the layer', () => {
    expect(whyChain(L, 'oci', 'base')).toEqual(['role server', 'extends base']);
    expect(whyChain(L, 'oci', 'core')).toEqual(['context core']);
    expect(whyChain(L, 'htz', 'core')).toEqual([]);
  });
});

describe('roleIn', () => {
  it('names the host role in a catalog', () => {
    expect(roleIn(L, 'oci')).toBe('server');
    expect(roleIn(L, 'htz')).toBeNull();
  });
});

describe('acceptanceOf', () => {
  const cat = (over: Partial<CatalogStatus>): CatalogStatus => ({
    id: 2, name: 'acme', org_id: 7, repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: null,
    state: 'loaded', asset_count: 0, admitted: [], ...over,
  });
  it('personal: all for an org-less host, shared only for an org host, locked', () => {
    const p = cat({ id: 1, name: 'personal', org_id: null });
    expect(acceptanceOf({ alias: 'oci', org_id: null }, p)).toEqual({ state: 'all', locked: true, why: 'personal reaches every host' });
    expect(acceptanceOf({ alias: 'trn', org_id: 7 }, p)).toEqual({ state: 'shared', locked: true, why: 'an org host receives only shared assets' });
  });
  it('own org: all, locked; other org host: none, locked', () => {
    expect(acceptanceOf({ alias: 'trn', org_id: 7 }, cat({}))).toEqual({ state: 'all', locked: true, why: 'via its org' });
    expect(acceptanceOf({ alias: 'trn', org_id: 9 }, cat({}))).toEqual({ state: 'none', locked: true, why: 'a host of another org never receives it' });
  });
  it('org-less host: admitted or not, a toggle', () => {
    expect(acceptanceOf({ alias: 'mef', org_id: null }, cat({ admitted: ['mef'] }))).toEqual({ state: 'all', locked: false, why: 'admitted' });
    expect(acceptanceOf({ alias: 'mef', org_id: null }, cat({}))).toEqual({ state: 'none', locked: false, why: 'not admitted' });
  });
});
