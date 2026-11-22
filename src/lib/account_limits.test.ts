// Review w10-4: the accounts a switch may not land on count the org the
// session is filed under on its host, as the hub's `forbidden_accounts_on`
// does, not only the project's own org.
import { describe, it, expect, afterEach } from 'vitest';
import { accounts } from './accounts';
import { hosts } from './hosts';
import { forbiddenOf, freestLogin, headroomForAccount, type HostLogin } from './account_limits';

afterEach(() => {
  accounts.set([]);
  hosts.set([]);
});

describe('forbiddenOf', () => {
  it("counts the session's org, else its host's, beside the project's own rules", () => {
    accounts.set([
      { uuid: 'personal', email: 'me@x.com', nickname: null, forbidden_org_ids: [7] } as never,
      { uuid: 'blocked-here', email: 'b@x.com', nickname: null, forbidden_project_ids: [3] } as never,
      { uuid: 'free', email: 'f@x.com', nickname: null } as never,
    ]);
    hosts.set([{ alias: 'client-box', hidden: false, org_id: 7 } as never]);
    expect(forbiddenOf({ project_id: 3, host_alias: 'elsewhere' })).toEqual(['blocked-here']);
    expect(forbiddenOf({ project_id: 3, host_alias: 'client-box' }).sort()).toEqual(['blocked-here', 'personal']);
    expect(forbiddenOf({ project_id: 3, host_alias: 'elsewhere', org_id: 7 }).sort()).toEqual(['blocked-here', 'personal']);
    // No project, no rule applies (as the hub's).
    expect(forbiddenOf({ project_id: null, host_alias: 'client-box' })).toEqual([]);
  });
});

// Review w09-9: a login a conflict names is picked after every other, and
// still picked when it is all there is.
describe('conflicted logins rank last', () => {
  const l = (profile: string, used: number, conflicted = false): HostLogin => ({
    profile,
    account_uuid: profile,
    used_pct: used,
    conflicted,
  });
  it('in the New session default', () => {
    expect(freestLogin([l('copied', 5, true), l('b', 60)])?.profile).toBe('b');
    expect(freestLogin([l('copied', 5, true)])?.profile).toBe('copied');
  });
  it('in the switch suggestion', () => {
    const own = l('own', 95);
    const h = headroomForAccount(
      { pause_at_pct: 90, chosen: own, over: true, suggestion: null, logins: [own, l('copied', 5, true), l('b', 60)] },
      'own',
    );
    expect(h.suggestion?.profile).toBe('b');
  });
});
