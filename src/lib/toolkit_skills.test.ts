import { describe, it, expect } from 'vitest';
import { skillMatrix } from './toolkit_skills';
import type { AssetListing } from './assets';

const listing = (assets: AssetListing['assets']): AssetListing => ({ head: 'h', loaded_at: 1, assets, unmanaged: [], problems: [] });

describe('skillMatrix (step 3.16)', () => {
  it('lays the skills out per host, local first, and shows a drifted host', () => {
    const m = skillMatrix(
      listing([
        { kind: 'skill', name: 'steward', version: '1.4', description: 'Drive a PR to green', tags: [], catalog: 'personal', hosts: [
          { host_alias: 'trn', harness: 'claude', state: 'drifted', drift_side: 'catalog' },
          { host_alias: 'local', harness: 'claude', state: 'in_sync' },
        ] },
        { kind: 'skill', name: 'jira-sync', version: '1.1', description: '', tags: [], hosts: [
          { host_alias: 'local', harness: 'claude', state: 'in_sync' },
          { host_alias: 'mercury', harness: 'claude', state: 'missing' },
        ] },
        { kind: 'hook', name: 'not-a-skill', version: '1', description: '', tags: [], hosts: [{ host_alias: 'oci', harness: 'claude', state: 'in_sync' }] },
      ]),
    );
    expect(m.hosts).toEqual(['local', 'mercury', 'trn']);
    expect(m.rows.map((r) => r.name)).toEqual(['jira-sync', 'steward']);
    const steward = m.rows[1];
    expect(steward.key).toBe('asset:personal:skill/steward');
    expect(steward.agents).toBe('Claude Code');
    expect(steward.cells.local).toMatchObject({ state: 'in_sync', word: '✓ 1.4' });
    expect(steward.cells.trn).toMatchObject({ state: 'behind', word: '1.4 ↑', title: 'trn: behind the catalog' });
    expect(steward.cells.mercury).toMatchObject({ state: 'none', word: '—' });
    expect(m.rows[0].cells.mercury).toMatchObject({ state: 'missing', word: 'missing' });
    expect(m.outOfSync).toBe(2);
  });

  it('takes the worst state when a host has several agents, and names each', () => {
    const m = skillMatrix(
      listing([
        { kind: 'skill', name: 'pr-review', version: '0.9', description: '', tags: [], hosts: [
          { host_alias: 'mac', harness: 'claude', state: 'in_sync' },
          { host_alias: 'mac', harness: 'codex', state: 'drifted', drift_side: 'host' },
        ] },
      ]),
    );
    const cell = m.rows[0].cells.mac;
    expect(cell).toMatchObject({ state: 'edited', word: 'edited' });
    expect(cell.title).toBe('mac: Claude Code in sync, Codex edited on host');
    expect(m.rows[0].agents).toBe('Claude Code, Codex');
  });

  it('is empty without a catalog', () => {
    expect(skillMatrix(null)).toEqual({ hosts: [], rows: [], outOfSync: 0 });
  });
});
