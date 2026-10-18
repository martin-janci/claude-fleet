import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  envAccessFrom,
  envReadonlyReason,
  envRecordWritable,
  envResourceFor,
  gitIdentityWrites,
  loadEnvAccess,
} from './env_access';
import { listRecords, runAction, type ResourceType } from './pages/resources';
import { bundle } from './pages/testing';

const envVar = bundle.resources.find((r) => r.id === 'env_var') as ResourceType;
const scopes = (r: ResourceType) => {
  const p = r.create?.params.find((x) => x.name === 'scope');
  return p && p.type === 'choice' ? p.options.map(([v]) => v) : [];
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('env access', () => {
  it('is the person scope for any device that speaks for a person, the shared ones for a trusted one', () => {
    expect(envAccessFrom(7, false)).toEqual({ person: true, shared: false });
    expect(envAccessFrom(null, true)).toEqual({ person: false, shared: true });
    expect(envAccessFrom(undefined, false)).toEqual({ person: false, shared: false });
  });

  it('narrows Add to the scopes the device may write', () => {
    expect(scopes(envResourceFor(envVar, { person: true, shared: true }))).toEqual(['person', 'global', 'host', 'project']);
    const mine = envResourceFor(envVar, { person: true, shared: false });
    expect(scopes(mine)).toEqual(['person']);
    expect(mine.create!.params.some((p) => p.name === 'scope_key')).toBe(false);
    expect(mine.create!.bind.some(([a]) => a === 'scope_key')).toBe(false);
    expect(mine.delete).toBeDefined();
    expect(scopes(envResourceFor(envVar, { person: false, shared: true }))).toEqual(['global', 'host', 'project']);
    const none = envResourceFor(envVar, { person: false, shared: false });
    expect(none.create).toBeUndefined();
    expect(none.delete).toBeUndefined();
    // The registry's own copy is untouched.
    expect(scopes(envVar)).toEqual(['person', 'global', 'host', 'project']);
  });

  it('gates removal per row', () => {
    const a = { person: true, shared: false };
    expect(envRecordWritable(a, { scope: 'person' })).toBe(true);
    expect(envRecordWritable(a, { scope: 'global' })).toBe(false);
  });

  it('says only what is held back', () => {
    expect(envReadonlyReason({ person: true, shared: true }, 'h', 'laptop')).toBeNull();
    expect(envReadonlyReason({ person: true, shared: false }, 'h', 'laptop')).toContain('fleet-hub client trust laptop');
    expect(envReadonlyReason({ person: false, shared: true }, 'h', null)).not.toContain('trust');
    expect(envReadonlyReason({ person: false, shared: false }, 'h', null)).toContain('fleet-hub client trust <name>');
  });

  it('reads an extra fleet’s access through `fleet`, and a refusal is no access', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'my_grants') return { person_id: 4, grants: [] };
      throw Object.assign(new Error('nope'), { code: 'E_FORBIDDEN', message: 'nope' });
    });
    expect(await loadEnvAccess('company')).toEqual({ person: true, shared: false });
    expect(inv).toHaveBeenCalledWith('my_grants', { fleet: 'company' });
    expect(inv).toHaveBeenCalledWith('setting_proposals', { fleet: 'company' });
    inv.mockClear();
    await loadEnvAccess('primary');
    expect(inv).toHaveBeenCalledWith('my_grants', {});
  });

  it('turns the Git & tokens form into person writes, skipping blanks', () => {
    expect(gitIdentityWrites({ name: ' Ada ', email: '', ghToken: '', jiraEmail: 'a@j', jiraToken: 'tok' })).toEqual([
      { name: 'GIT_AUTHOR_NAME', value: 'Ada', secret: false },
      { name: 'GIT_COMMITTER_NAME', value: 'Ada', secret: false },
      { name: 'JIRA_EMAIL', value: 'a@j', secret: false },
      { name: 'JIRA_API_TOKEN', value: 'tok', secret: true },
    ]);
    expect(gitIdentityWrites({ name: '', email: '', ghToken: ' ', jiraEmail: '', jiraToken: '' })).toEqual([]);
  });
});

describe('resource commands addressed to a fleet', () => {
  it('send `fleet` with a fleet-scoped command', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockResolvedValue([]);
    await listRecords(envVar, 'company');
    expect(inv).toHaveBeenCalledWith('env_vars_list', { fleet: 'company' });
    await runAction(envVar.create!, { name: 'X' }, 'company');
    expect(inv).toHaveBeenCalledWith('env_var_set', { args: { name: 'X' }, fleet: 'company' });
  });

  it('refuse any other command rather than let it reach the primary', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const org = bundle.resources.find((r) => r.id === 'org') as ResourceType;
    const r = await listRecords(org, 'company');
    expect(r.ok).toBe(false);
    const a = await runAction(org.create!, {}, 'company');
    expect(a.ok).toBe(false);
    expect(inv).not.toHaveBeenCalled();
  });

  it('address the primary exactly as before', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockResolvedValue([]);
    await listRecords(envVar, 'primary');
    await listRecords(envVar);
    for (const c of inv.mock.calls) expect(c).toEqual(['env_vars_list', undefined]);
  });
});
